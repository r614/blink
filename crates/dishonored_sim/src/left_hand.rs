//! Left-hand items (M10–M12): pistol, crossbow, Windblast, Dark Vision, and the Blink
//! dispatch. Ranged weapons fire simulated projectiles (no hitscan) so client and server
//! resolve identically.

use crate::blink;
use crate::events::{DamageCause, SimEvent, SoundCategory};
use crate::input::{LeftHand, PlayerInput, buttons};
use crate::movement::{MoveCtx, eye_position, view_vector};
use crate::state::{LeftPhase, MoveMode, PawnKind, PlayerState, ProjectileKind, ProjectileState, WorldState, power};
use dishonored_ir::items::AmmoType;
use crate::tuning::CombatTuning;
use crate::vitals::{apply_damage, spend_mana};
use dishonored_ir::gameplay::{GameplayConfig, MovementConfig};
use dishonored_world::{WorldQuery, channel, geom::closest_segment_segment};
use glam::Vec3;

pub fn reset_loadout(p: &mut PlayerState, t: &CombatTuning) {
    p.left.pistol_loaded = 1;
    p.left.pistol_reserve = t.pistol_reserve;
    p.left.bolts_loaded = 1;
    p.left.bolts_reserve = t.crossbow_reserve;
    p.left.phase = LeftPhase::Idle;
    p.left.timer = 0.0;
    p.left.blink_target = None;
}

/// Reference view the dispersion offset is projected through: the default horizontal field of
/// view (`DefaultCamera.ini` m_fDefaultFOV 75) on a 16:9 screen. The original deprojects
/// through the player's actual view (0x00AA2B10), so a wider FOV or a narrower screen widens
/// the cone; the sim fixes the reference so every peer resolves the same shot.
pub const DISPERSION_REF_HFOV_DEG: f32 = 75.0;
pub const DISPERSION_REF_ASPECT: f32 = 16.0 / 9.0;

/// The held ranged weapon's dispersion attributes with its workshop upgrades applied
/// (`Attribute_DispersionMax` / `Min` / `InterpolationSpeed`, globals 0x0145DEA8 / 0x0145DEB0 /
/// 0x0145DEA0) and its `m_bMaxDispersionOn*` flags.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dispersion {
    pub max: f32,
    pub min: f32,
    pub speed: f32,
    pub max_on: u8,
}

impl Dispersion {
    /// None when the left hand holds no pistol or crossbow.
    pub fn of(p: &PlayerState, cfg: &GameplayConfig, t: &CombatTuning) -> Option<Self> {
        let (d, host): (_, fn(&str) -> bool) = if p.left.item == LeftHand::Pistol {
            (&t.pistol_dispersion, |n| n.starts_with("Pistol"))
        } else if p.left.item.is_crossbow() {
            (&t.crossbow_dispersion, |n| n.starts_with("Crossbow"))
        } else {
            return None;
        };
        let a = |name: &str, base: f32| crate::powers::attribute_for(p, cfg, name, base, host);
        Some(Dispersion {
            max: a("Attribute_DispersionMax", d.max),
            min: a("Attribute_DispersionMin", d.min),
            speed: a("Attribute_DispersionInterpolationSpeed", d.speed),
            max_on: d.max_on,
        })
    }
}

/// `FInterpConstantTo` (0x0041EBC0): move toward `target` by at most `speed * dt`.
fn interp_constant_to(current: f32, target: f32, dt: f32, speed: f32) -> f32 {
    let delta = target - current;
    if delta * delta < 1e-8 {
        return target;
    }
    let step = speed * dt;
    current + delta.clamp(-step, step)
}

/// Per-tick dispersion of the held ranged weapon (U-247):
/// - the `m_bMaxDispersionOn*` states hold it at the maximum: sprinting, in the air (the falling
///   state's tick 0x00A79150 tests the Jump bit), blocking, a sword attack;
/// - otherwise, while a round is loaded, it falls toward the minimum at the interpolation speed
///   per second (weapon tick, slot 94 0x00C1CB10; nothing happens while empty or reloading).
pub fn update_dispersion(p: &mut PlayerState, cfg: &GameplayConfig, t: &CombatTuning, dt: f32) {
    use crate::state::SwordPhase;
    use crate::tuning::DispersionTuning as D;
    let Some(d) = Dispersion::of(p, cfg, t) else { return };
    let held_max = (d.max_on & D::SPRINT != 0 && p.sprinting)
        || (d.max_on & D::JUMP != 0 && p.mode == MoveMode::Falling)
        || (d.max_on & D::BLOCK != 0 && p.sword.phase == SwordPhase::Blocking)
        || (d.max_on & D::MELEE != 0 && matches!(p.sword.phase, SwordPhase::Startup | SwordPhase::Active | SwordPhase::Recovery));
    if held_max {
        p.left.dispersion = d.max;
        return;
    }
    let loaded = match p.left.item {
        LeftHand::Pistol => p.left.pistol_loaded > 0,
        LeftHand::Crossbow => p.left.bolts_loaded > 0,
        // Sleep darts and incendiary bolts load from the inventory: a round is in while the
        // weapon is not reloading.
        _ => p.left.phase != LeftPhase::Reloading,
    };
    if loaded {
        p.left.dispersion = interp_constant_to(p.left.dispersion, d.min, dt, d.speed);
    }
}

/// The random screen offset of one shot (0x00BFFBA0): radius `rand() * d / 32767` (uniform in
/// the radius, so shots gather toward the centre), angle `rand() * 2 pi / 32767 - pi`, two
/// draws of the pawn's random stream in that order. In pixels of a 720-line screen.
pub fn dispersion_offset(p: &mut PlayerState, d: f32) -> (f32, f32) {
    let r = p.roll() * d;
    let a = p.roll() * std::f32::consts::TAU - std::f32::consts::PI;
    (a.cos() * r, a.sin() * r)
}

/// The view direction through screen point centre + `off` (0x00AA2B10: the offset is scaled by
/// `height / 720` and the point deprojected), for the reference view.
pub fn offset_direction(yaw: f32, pitch: f32, off: (f32, f32)) -> Vec3 {
    let f = view_vector(yaw, pitch);
    let (sy, cy) = yaw.to_radians().sin_cos();
    let right = Vec3::new(-sy, cy, 0.0);
    let mut up = right.cross(f);
    if up.z < 0.0 {
        up = -up;
    }
    // Tangent per 720-line pixel: 2 tan(hfov / 2) / (720 x aspect).
    let k = 2.0 * (DISPERSION_REF_HFOV_DEG.to_radians() * 0.5).tan() / (720.0 * DISPERSION_REF_ASPECT);
    // Screen y grows downward.
    (f + right * (off.0 * k) - up * (off.1 * k)).normalize()
}

/// Direction of a pistol or crossbow shot: the view through the dispersion offset; then the
/// dispersion jumps to the maximum (the projectile context fires, then calls the weapon's
/// slot 109 0x00C1CB80, 0x00C03A60).
fn fire_direction(p: &mut PlayerState, cfg: &GameplayConfig, t: &CombatTuning) -> Vec3 {
    let Some(d) = Dispersion::of(p, cfg, t) else { return view_vector(p.yaw, p.pitch) };
    let off = dispersion_offset(p, p.left.dispersion.max(0.0));
    p.left.dispersion = d.max;
    offset_direction(p.yaw, p.pitch, off)
}

/// The held gun's reload for this pawn: `Attribute_FastReload` and `Attribute_ReloadSpeed` of the
/// weapon (an upgrade modifies its host item only: Pistol Reload 2 gives the pistol 1.0, the
/// Crossbow Reload upgrade gives the crossbow 0.3), and when the magazine loads / the reload ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GunReload {
    pub fast: f32,
    pub load: f32,
    pub end: f32,
}

/// [`GunReload`] of the left-hand item (crossbow ammo types share the crossbow's).
pub fn gun_reload(p: &PlayerState, cfg: &GameplayConfig, t: &CombatTuning) -> GunReload {
    let bolt = p.left.item.is_crossbow();
    let (stages, host): (_, fn(&str) -> bool) = if bolt { (&t.crossbow_reload_stages, |n| n.starts_with("Crossbow")) } else { (&t.pistol_reload_stages, |n| n.starts_with("Pistol")) };
    let fast = crate::powers::attribute_for(p, cfg, "Attribute_FastReload", 0.0, host);
    let speed = crate::powers::attribute_for(p, cfg, "Attribute_ReloadSpeed", 0.0, host);
    let (load, end) = stages.stages(speed, fast);
    GunReload { fast, load, end }
}

/// Load the magazine from the reserve (0x00C28C50: `min(1 + MagazineBonus, held)` rounds in).
/// Returns whether a round moved.
fn load_magazine(p: &mut PlayerState, bolt: bool, mag: u8) -> bool {
    let (loaded, reserve) = if bolt { (&mut p.left.bolts_loaded, &mut p.left.bolts_reserve) } else { (&mut p.left.pistol_loaded, &mut p.left.pistol_reserve) };
    if *reserve > 0 && *loaded < mag {
        let n = (mag - *loaded).min((*reserve).min(255) as u8);
        *reserve -= n as u16;
        *loaded += n;
        return true;
    }
    false
}

/// World-level effects a left-hand item asks for this tick (resolved by `step_world`, which
/// can see the other pawns).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LeftAction {
    /// `DisNotify_BendTime`.
    BendTime,
    /// The power used again while active: end it.
    EndBendTime,
    /// `DisNotify_DevouringSwarm`.
    Swarm,
    /// Possession: release of the targeting (find the host, pay, travel in).
    PossessRelease,
    /// Possession used again while possessing: leave the host.
    PossessEnd,
    /// `DisNotify_PlaceSpringRazor`.
    PlaceTrap,
}

/// Per-player left-hand update. Spawned projectiles are pushed to `spawn`, world effects to
/// `actions`.
#[allow(clippy::too_many_arguments)]
pub fn update(
    p: &mut PlayerState,
    input: &PlayerInput,
    ctx: &MoveCtx,
    cfg: &GameplayConfig,
    t: &CombatTuning,
    spawn: &mut Vec<(ProjectileKind, Vec3, Vec3)>,
    actions: &mut Vec<LeftAction>,
    bend_active: bool,
    events: &mut Vec<SimEvent>,
) {
    let dt = ctx.dt;
    if !p.alive || p.incapacitated() {
        return;
    }
    // A Blink move in progress finishes whatever the left hand selects.
    if p.mode == MoveMode::Blinking {
        blink::update(p, false, false, ctx, &cfg.blink, &cfg.vitals, events);
        return;
    }
    let casting = p.left.phase == LeftPhase::Recovering && is_cast(p.left.item);
    // Item switch (not in the middle of a power cast or gadget throw).
    if input.left_hand != p.left.item && !casting {
        p.left.item = input.left_hand;
        p.left.blink_target = None;
        p.left.phase = if input.left_hand.is_crossbow() || input.left_hand == LeftHand::Pistol { LeftPhase::Equipping } else { LeftPhase::Idle };
        p.left.timer = 0.0;
        // Equipping a ranged weapon starts at the widest dispersion (slot 76, 0x00C2C1F0).
        if let Some(d) = Dispersion::of(p, cfg, t) {
            p.left.dispersion = d.max;
        }
    }
    update_dispersion(p, cfg, t, dt);
    let prev_timer = p.left.timer;
    p.left.timer += dt;
    let crossed = |at: f32| prev_timer < at && p.left.timer >= at;
    let held = input.pressed(buttons::SECONDARY);
    let pressed = p.just_pressed(input, buttons::SECONDARY);
    let released = p.just_released(input, buttons::SECONDARY);
    let busy = matches!(p.mode, MoveMode::Mantling | MoveMode::Dead) || crate::combat::sword_locks_movement(p) || p.choke.phase != crate::state::ChokePhase::None || p.action_lock > 0.0;
    // Carrying a body only leaves Blink usable.
    if p.carrying.is_some() && p.left.item != LeftHand::Blink {
        return;
    }
    let level = |i: usize| p.inv.power_levels[i];

    match p.left.item {
        LeftHand::BendTime | LeftHand::DevouringSwarm => {
            let swarm = p.left.item == LeftHand::DevouringSwarm;
            let (idx, cost, notify, exit) = if swarm {
                (power::DEVOURING_SWARM, cfg.items.swarm.mana_cost, crate::powers::SWARM_NOTIFY, crate::powers::SWARM_EXIT)
            } else {
                (power::BEND_TIME, cfg.items.bend_time.mana_cost, crate::powers::BEND_TIME_NOTIFY, crate::powers::BEND_TIME_EXIT)
            };
            match p.left.phase {
                LeftPhase::Recovering => {
                    if crossed(notify) {
                        actions.push(if swarm { LeftAction::Swarm } else { LeftAction::BendTime });
                    }
                    if p.left.timer >= exit {
                        p.left.phase = LeftPhase::Idle;
                    }
                }
                _ if pressed && !busy && level(idx) > 0 => {
                    if !swarm && bend_active {
                        actions.push(LeftAction::EndBendTime);
                    } else if spend_mana(p, cost, &cfg.vitals) {
                        p.left.phase = LeftPhase::Recovering;
                        p.left.timer = 0.0;
                    }
                }
                _ => {}
            }
        }
        LeftHand::Possession => {
            use crate::state::PossessStage;
            match p.possess.stage {
                PossessStage::Active | PossessStage::PrePossess => {
                    if pressed {
                        actions.push(LeftAction::PossessEnd);
                    }
                }
                PossessStage::Inactive | PossessStage::Targeting if level(power::POSSESSION) > 0 && !busy => {
                    if pressed {
                        p.left.phase = LeftPhase::Aiming;
                        p.possess.stage = PossessStage::Targeting;
                    } else if released && p.left.phase == LeftPhase::Aiming {
                        p.left.phase = LeftPhase::Idle;
                        p.possess.stage = PossessStage::Inactive;
                        actions.push(LeftAction::PossessRelease);
                    } else if !held && p.left.phase == LeftPhase::Aiming {
                        p.left.phase = LeftPhase::Idle;
                        p.possess.stage = PossessStage::Inactive;
                    }
                }
                _ => {}
            }
        }
        LeftHand::Grenade | LeftHand::StickyGrenade | LeftHand::SpringRazor => {
            let ammo = match p.left.item {
                LeftHand::Grenade => AmmoType::Grenade,
                LeftHand::StickyGrenade => AmmoType::StickyGrenade,
                _ => AmmoType::SpringRazor,
            };
            let razor = ammo == AmmoType::SpringRazor;
            let (fire, exit) = if razor { (crate::gadgets::RAZOR_PLACE, crate::gadgets::RAZOR_EXIT) } else { (crate::gadgets::GRENADE_FIRE, crate::gadgets::GRENADE_EXIT) };
            match p.left.phase {
                LeftPhase::Recovering => {
                    if crossed(fire) && crate::inventory::remove_ammo(p, ammo, 1) {
                        if razor {
                            actions.push(LeftAction::PlaceTrap);
                        } else {
                            let kind = if ammo == AmmoType::Grenade { ProjectileKind::Grenade } else { ProjectileKind::StickyGrenade };
                            let g = crate::gadgets::grenade_cfg(cfg, kind);
                            spawn.push((kind, eye_position(p, ctx.cfg), crate::gadgets::throw_velocity(p.yaw, p.pitch, g)));
                            events.push(SimEvent::Sound { position: p.position, radius: 300.0, category: SoundCategory::SwordSwing, source: p.id });
                        }
                    }
                    if p.left.timer >= exit {
                        p.left.phase = LeftPhase::Idle;
                    }
                }
                _ if pressed && !busy && crate::inventory::ammo(p, ammo) > 0 => {
                    p.left.phase = LeftPhase::Recovering;
                    p.left.timer = 0.0;
                }
                _ => {}
            }
        }
        LeftHand::SleepDart | LeftHand::IncendiaryBolt => {
            let (ammo, kind, speed) = if p.left.item == LeftHand::SleepDart {
                (AmmoType::SleepDart, ProjectileKind::SleepDart, t.sleep_dart_speed)
            } else {
                (AmmoType::IncendiaryBolt, ProjectileKind::IncendiaryBolt, t.incendiary_speed)
            };
            let g = gun_reload(p, cfg, t);
            let left = crate::inventory::ammo(p, ammo);
            match p.left.phase {
                LeftPhase::Equipping if p.left.timer >= t.crossbow_equip => p.left.phase = LeftPhase::Idle,
                // With Attribute_FastReload the shot that leaves a round plays the fast fire action,
                // which loads the next round when it ends (no reload stage).
                LeftPhase::Recovering if g.fast > 0.0 && left > 0 => {
                    if p.left.timer >= t.crossbow_fast_fire {
                        p.left.phase = LeftPhase::Idle;
                        events.push(SimEvent::Reloaded { id: p.id });
                    }
                }
                LeftPhase::Recovering if p.left.timer >= t.crossbow_fire_recovery => {
                    p.left.phase = if left > 0 { LeftPhase::Reloading } else { LeftPhase::Idle };
                    p.left.timer = 0.0;
                }
                LeftPhase::Reloading if p.left.timer >= g.end => {
                    p.left.phase = LeftPhase::Idle;
                    events.push(SimEvent::Reloaded { id: p.id });
                }
                LeftPhase::Idle if !busy && pressed && crate::inventory::ammo(p, ammo) > 0 => {
                    crate::inventory::remove_ammo(p, ammo, 1);
                    let eye = eye_position(p, ctx.cfg);
                    let dir = fire_direction(p, cfg, t);
                    spawn.push((kind, eye, dir * speed));
                    p.left.phase = LeftPhase::Recovering;
                    p.left.timer = 0.0;
                    events.push(SimEvent::Sound { position: eye, radius: 500.0, category: SoundCategory::Crossbow, source: p.id });
                }
                _ => {}
            }
        }
        LeftHand::Blink => {
            if p.mode == MoveMode::Blinking {
                blink::update(p, false, false, ctx, &cfg.blink, &cfg.vitals, events);
            } else if !busy {
                blink::update(p, held, released, ctx, &cfg.blink, &cfg.vitals, events);
            }
        }
        LeftHand::Windblast => {
            match p.left.phase {
                LeftPhase::Recovering if p.left.timer >= windblast_lockout(t) => p.left.phase = LeftPhase::Idle,
                LeftPhase::Idle if pressed && !busy => {
                    if spend_mana(p, t.windblast_cost, &cfg.vitals) {
                        p.left.phase = LeftPhase::Recovering;
                        p.left.timer = 0.0;
                        events.push(SimEvent::WindblastCast { id: p.id });
                        events.push(SimEvent::Sound { position: p.position, radius: 1500.0, category: SoundCategory::Windblast, source: p.id });
                    }
                }
                _ => {}
            }
        }
        LeftHand::DarkVision => {
            if pressed && !busy && level(power::DARK_VISION) > 0 {
                use_dark_vision(p, cfg, t, events);
            }
        }
        LeftHand::Pistol | LeftHand::Crossbow => {
            let bolt = p.left.item == LeftHand::Crossbow;
            // Pistol upgrades: the magazine holds 1 + (int)Attribute_MagazineBonus rounds (0x00C20DB0,
            // capped by the ammo held) and the reload time is divided by Attribute_ReloadSpeed when
            // set (0x00C23620).
            let mag = if bolt { 1 } else { 1 + crate::powers::attribute(p, cfg, "Attribute_MagazineBonus", 0.0).max(0.0) as u8 };
            let g = gun_reload(p, cfg, t);
            let (equip, recovery) = if bolt { (t.crossbow_equip, t.crossbow_fire_recovery) } else { (t.pistol_equip, t.pistol_fire_recovery) };
            let reserve_now = if bolt { p.left.bolts_reserve } else { p.left.pistol_reserve };
            // Crossbow with Attribute_FastReload: a shot that leaves a bolt plays the fast fire
            // action and loads the bolt at its end (0x00C2E570 / 0x00C2E730).
            let fast_fire = bolt && g.fast > 0.0 && reserve_now > 0;
            let now = p.left.timer;
            match p.left.phase {
                LeftPhase::Equipping if now >= equip => p.left.phase = LeftPhase::Idle,
                LeftPhase::Recovering if fast_fire => {
                    if now >= t.crossbow_fast_fire {
                        if load_magazine(p, bolt, mag) {
                            events.push(SimEvent::Reloaded { id: p.id });
                        }
                        p.left.phase = LeftPhase::Idle;
                    }
                }
                LeftPhase::Recovering if now >= recovery => {
                    // The fire action's end requests a reload when the weapon is empty and ammo
                    // is held (pistol 0x00C2EA00, crossbow 0x00C2E730).
                    let loaded = if bolt { p.left.bolts_loaded } else { p.left.pistol_loaded };
                    p.left.phase = if loaded == 0 && reserve_now > 0 { LeftPhase::Reloading } else { LeftPhase::Idle };
                    p.left.timer = 0.0;
                }
                LeftPhase::Reloading => {
                    // The timed stage's end loads the magazine; the ReloadOut action follows.
                    if prev_timer < g.load && now >= g.load && load_magazine(p, bolt, mag) {
                        events.push(SimEvent::Reloaded { id: p.id });
                    }
                    if now >= g.end {
                        p.left.phase = LeftPhase::Idle;
                    }
                }
                LeftPhase::Idle if !busy => {
                    let loaded = if bolt { p.left.bolts_loaded } else { p.left.pistol_loaded };
                    let reserve = if bolt { p.left.bolts_reserve } else { p.left.pistol_reserve };
                    if pressed && loaded > 0 {
                        if bolt { p.left.bolts_loaded -= 1 } else { p.left.pistol_loaded -= 1 }
                        let eye = eye_position(p, ctx.cfg);
                        let dir = fire_direction(p, cfg, t);
                        let speed = if bolt { t.bolt_speed } else { t.bullet_speed };
                        spawn.push((if bolt { ProjectileKind::Bolt } else { ProjectileKind::Bullet }, eye, dir * speed));
                        p.left.phase = LeftPhase::Recovering;
                        p.left.timer = 0.0;
                        let (radius, cat) = if bolt { (500.0, SoundCategory::Crossbow) } else { (4000.0, SoundCategory::Pistol) };
                        events.push(SimEvent::Sound { position: eye, radius, category: cat, source: p.id });
                    } else if reserve > 0 && ((loaded == 0 && pressed) || (loaded < mag && input.pressed(buttons::RELOAD))) {
                        p.left.phase = LeftPhase::Reloading;
                        p.left.timer = 0.0;
                    }
                }
                _ => {}
            }
        }
        LeftHand::Empty => {}
    }
}

/// Dark Vision is in effect (`DisActivePowerComponent_DarkVision` slot +0x160: any of the fade-in,
/// effect and fade-out timers running).
pub fn dark_vision_active(p: &PlayerState) -> bool {
    p.dark.fade_in > 0.0 || p.dark.remaining > 0.0 || p.dark.fade_out > 0.0
}

/// Using Dark Vision (slot +0x168, 0x00BE89E0): while it is active and its
/// `m_fCooldownTimer` has run out, the effect ends (fade-in and duration zeroed, the fade-out
/// plays); otherwise, with the mana, it starts (0x00BF82A0): fade-in
/// `m_EyeLidClosingDuration`, duration `m_fDuration x (1 + Attribute_SpellStrength)`, fade-out
/// `m_PowerFadeOutDuration`, cooldown `m_fCooldownTimer`. There is no drain: the cost is paid once.
pub fn use_dark_vision(p: &mut PlayerState, cfg: &GameplayConfig, t: &CombatTuning, events: &mut Vec<SimEvent>) {
    if dark_vision_active(p) {
        if p.dark.cooldown < 0.0 {
            p.dark.fade_in = 0.0;
            p.dark.remaining = 0.0;
            events.push(SimEvent::DarkVision { id: p.id, active: false });
        }
        return;
    }
    if !spend_mana(p, t.dark_vision_cost, &cfg.vitals) {
        return;
    }
    let strength = crate::powers::attribute(p, cfg, "Attribute_SpellStrength", 0.0);
    p.dark.level = p.inv.power_levels[power::DARK_VISION].clamp(1, 2);
    p.dark.fade_in = t.dark_vision_fade_in;
    p.dark.remaining = t.dark_vision_duration * (1.0 + strength);
    p.dark.fade_out = t.dark_vision_fade_out;
    p.dark.cooldown = t.dark_vision_cooldown;
    p.vitals.dark_vision_remaining = p.dark.fade_in + p.dark.remaining + p.dark.fade_out;
    events.push(SimEvent::DarkVision { id: p.id, active: true });
}

/// Dark Vision tick (slot +0x13C, 0x00BFBC90): the cooldown runs down; the step is spent on the
/// fade-in first, the rest on the duration, then on the fade-out.
pub fn tick_dark_vision(p: &mut PlayerState, dt: f32, events: &mut Vec<SimEvent>) {
    p.dark.cooldown -= dt;
    if !dark_vision_active(p) {
        p.vitals.dark_vision_remaining = 0.0;
        return;
    }
    let mut left = dt;
    if p.dark.fade_in > 0.0 {
        let used = left.min(p.dark.fade_in);
        p.dark.fade_in -= used;
        left -= used;
    }
    if left > 0.0 && p.dark.remaining > 0.0 {
        let used = left.min(p.dark.remaining);
        p.dark.remaining -= used;
        left -= used;
        if p.dark.remaining <= 0.0 {
            events.push(SimEvent::DarkVision { id: p.id, active: false });
        }
    }
    if left > 0.0 && p.dark.fade_in <= 0.0 && p.dark.remaining <= 0.0 && p.dark.fade_out > 0.0 {
        p.dark.fade_out = (p.dark.fade_out - left).max(0.0);
    }
    p.vitals.dark_vision_remaining = p.dark.fade_in + p.dark.remaining + p.dark.fade_out;
}

/// Items whose use plays an uninterruptible cast/throw (no item switch until it ends).
fn is_cast(item: LeftHand) -> bool {
    matches!(item, LeftHand::BendTime | LeftHand::DevouringSwarm | LeftHand::Grenade | LeftHand::StickyGrenade | LeftHand::SpringRazor)
}

/// Windblast warmup before the blast starts (`m_fWarmupTime` default, 0.1 s) and cooldown
/// after the front reached its range (`m_fCooldownTime`, 1.0 s): class defaults of
/// `DishonoredActivePowerComponent_WindBlast`.
pub const WINDBLAST_WARMUP: f32 = 0.1;
pub const WINDBLAST_COOLDOWN: f32 = 1.0;

/// Seconds from the cast until Windblast can be cast again (warmup, front travel, cooldown).
pub fn windblast_lockout(t: &CombatTuning) -> f32 {
    WINDBLAST_WARMUP + t.windblast_distance / t.windblast_speed.max(1.0) + WINDBLAST_COOLDOWN
}

/// Strength of the blast on a target (0x00BE8570): zero outside the cone (tested in the
/// horizontal plane against the unnormalized direction's x/y); inside,
/// `(1 - min(d / range, 1))^2`, times `((dot - cos) / (1 - cos))^2` beyond 100 uu.
pub fn windblast_factor(dir: Vec3, center: Vec3, origin: Vec3, range: f32, cos_half: f32) -> f32 {
    let d = center.distance(origin);
    let flat = (center - origin).truncate().normalize_or_zero();
    let dot = flat.x * dir.x + flat.y * dir.y;
    if dot <= cos_half {
        return 0.0;
    }
    let r = 1.0 - (d / range).min(1.0);
    let range_f = r * r;
    if d > 100.0 {
        let a = (dot - cos_half) / (1.0 - cos_half);
        a * a * range_f
    } else {
        range_f
    }
}

/// Velocity given to a pawn hit by the blast (0x00BF6AF0 then 0x00BE9770): the impulse is
/// `direction * m_Force` with its z raised to the extra up force
/// `(1 - min(d / range, 1)) * m_ExtraUpForce` (added when the aim points down), and the
/// momentum handed to `TakeDamage` is `impulse * k` with `k` the blast factor. MEASURED
/// 2026-10-06 (engine hook, level aim, the damage path's momentum argument): a hit at 183 uu
/// with k = 0.879 received (2362, -227, 1317) = (2700 dir.xy k, 0.939 x 1600 x k); an earlier
/// decode read the z as `0.15 x |impulse| x k` (407 here). Applying the momentum as a velocity
/// change is an inference (the original only ever blasts NPCs, which ragdoll).|impulse| * k)` with `k` the
/// blast factor. Applying the momentum as a velocity change is an inference (the original
/// only ever blasts NPCs, which ragdoll).
pub fn windblast_momentum(dir: Vec3, force: f32, up_force: f32, d: f32, range: f32, k: f32) -> Vec3 {
    let mut imp = dir * force;
    let up = (1.0 - (d / range).min(1.0)) * up_force;
    if up > imp.z {
        imp.z += (up - imp.z).min(up);
    }
    imp * k
}

/// Start Windblasts cast this tick (after the warmup) and advance blast fronts.
pub fn step_windblasts(
    state: &mut WorldState,
    world: &dyn WorldQuery,
    mv: &MovementConfig,
    vitals: &dishonored_ir::gameplay::VitalsConfig,
    t: &CombatTuning,
    dt: f32,
    events: &mut Vec<SimEvent>,
) {
    let range = t.windblast_distance;
    let cos_half = (t.windblast_angle * 0.5).to_radians().cos();
    let mut blasts = std::mem::take(&mut state.windblasts);
    for wb in blasts.iter_mut() {
        if !wb.started {
            wb.warmup -= dt;
            if wb.warmup > 0.0 {
                continue;
            }
            // Blast start (0x00BF9570): origin and direction from the caster's view.
            let Some(c) = state.player(wb.caster).copied().filter(|c| c.alive) else {
                wb.started = true;
                wb.targets.clear();
                continue;
            };
            wb.started = true;
            wb.origin = eye_position(&c, mv);
            let mut dir = view_vector(c.yaw, c.pitch);
            if !t.windblast_directional {
                dir.z = 0.0;
                dir = dir.normalize_or_zero();
            }
            wb.direction = dir;
            // Targets (0x00BF6780): within range and with a positive blast factor.
            wb.targets = state
                .players
                .iter()
                .filter(|v| v.id != wb.caster && v.alive)
                .filter(|v| v.position.distance(wb.origin) < range && windblast_factor(dir, v.position, wb.origin, range, cos_half) > 0.0)
                .map(|v| v.id)
                .collect();
            // Projectiles in the cone are knocked away (the original redirects them).
            let (o, d) = (wb.origin, dir);
            state.projectiles.retain(|pr| windblast_factor(d, pr.position, o, range, cos_half) <= 0.0 || pr.position.distance(o) >= range);
        }
        // Front advance (0x00BF7090): reach = (elapsed + dt) * speed, capped at the range.
        let reach = ((wb.elapsed + dt) * t.windblast_speed).min(range);
        let mut remaining = Vec::new();
        for &vid in &wb.targets {
            let Some(v) = state.player_mut(vid) else { continue };
            if !v.alive {
                continue;
            }
            let d = v.position.distance(wb.origin);
            if d > reach {
                remaining.push(vid);
                continue;
            }
            let to = v.position - wb.origin;
            if let Some(h) = world.raycast(wb.origin, to, d, channel::PROJECTILE) {
                if h.distance < d - mv.collision_radius {
                    continue;
                }
            }
            let k = windblast_factor(wb.direction, v.position, wb.origin, range, cos_half);
            if k <= 0.0 {
                continue;
            }
            // The momentum goes to TakeDamage (0x00BE9770). NPCs are thrown (ragdoll); the player
            // pawn's damage path (0x00B5CCD0) hands it only to the hit reactions and clears it,
            // so players are not pushed (U-051).
            if v.kind != PawnKind::Human {
                v.velocity += windblast_momentum(wb.direction, t.windblast_force, t.windblast_up_force, d, range, k);
                if matches!(v.mode, MoveMode::Walking) {
                    v.mode = MoveMode::Falling;
                    v.fall_time = 0.0;
                }
            }
            // `Twk_Inv_EliteHelmet`: Windblast knocks a helmet off whatever it hits.
            v.helmet = false;
            if v.sword.phase == crate::state::SwordPhase::Blocking {
                v.sword.phase = crate::state::SwordPhase::Idle;
            }
            let dmg = (t.windblast_damage * k + 0.5).floor();
            apply_damage(v, dmg, Some(wb.caster), DamageCause::Windblast, events, vitals);
        }
        wb.targets = remaining;
        wb.elapsed += dt;
        if reach >= range {
            wb.targets.clear();
        }
    }
    blasts.retain(|wb| !wb.started || !wb.targets.is_empty());
    state.windblasts = blasts;
}

/// Advance projectiles: sub-stepped segment tests against world and player bodies.
pub fn step_projectiles(
    state: &mut WorldState,
    world: &dyn WorldQuery,
    mv: &MovementConfig,
    t: &CombatTuning,
    vitals: &dishonored_ir::gameplay::VitalsConfig,
    gravity_z: f32,
    dt: f32,
    sleep_delay: &dyn Fn(u32) -> f32,
    events: &mut Vec<SimEvent>,
) {
    let mut keep = Vec::with_capacity(state.projectiles.len());
    let projectiles = std::mem::take(&mut state.projectiles);
    for mut pr in projectiles {
        if pr.kind.is_grenade() {
            keep.push(pr);
            continue;
        }
        pr.age += dt;
        let old_v = pr.velocity;
        pr.velocity.z += gravity_z * pr.gravity_mult * dt;
        let delta = (old_v + pr.velocity) * 0.5 * dt;
        let from = pr.position;
        let to = from + delta;
        let len = delta.length();
        // World hit.
        let world_hit = world.raycast(from, delta, len, channel::PROJECTILE).map(|h| h.distance);
        // Body hits (capsule vs segment), owner excluded.
        let mut body: Option<(usize, f32, Vec3)> = None;
        for (i, v) in state.players.iter().enumerate() {
            if !v.alive || v.id == pr.owner || v.carried_by.is_some() || crate::powers::is_hidden(v) {
                continue;
            }
            let hh = crate::movement::half_height(v, mv);
            let a0 = v.position - Vec3::Z * (hh - mv.collision_radius);
            let a1 = v.position + Vec3::Z * (hh - mv.collision_radius);
            let (on_seg, on_axis) = closest_segment_segment(from, to, a0, a1);
            if on_seg.distance(on_axis) <= mv.collision_radius {
                let along = (on_seg - from).length();
                if body.is_none_or(|(_, d, _)| along < d) {
                    body = Some((i, along, on_seg));
                }
            }
        }
        match (body, world_hit) {
            (Some((i, along, point)), wh) if wh.is_none_or(|w| along <= w) => {
                let v = &mut state.players[i];
                let hh = crate::movement::half_height(v, mv);
                let head = point.z > v.position.z + hh * 0.6;
                let dist = (pr.age - dt).max(0.0) * old_v.length() + along;
                // Sleep darts (`m_bPutToSleep`, `m_fTimeBeforeSleep`) and incendiary bolts
                // (`m_bPutToFire`, `m_bCannotKillNPC`) act on NPCs; on players they only hit.
                if pr.kind == ProjectileKind::SleepDart {
                    events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: Some(v.id) });
                    if v.kind != PawnKind::Human && v.condition == crate::state::Condition::Normal && v.sleep_timer <= 0.0 {
                        // `m_bInstantSleepForUnawareNPCs`: an NPC the AI reports unaware sleeps at once.
                        v.sleep_timer = if v.ai_unaware { 1e-3 } else { sleep_delay(pr.owner).max(1e-3) };
                        v.condition_by = Some(pr.owner);
                    }
                    continue;
                }
                // `Twk_Inv_EliteHelmet.m_DamageTypesThatKnockOff`: bullets and bolts on the head,
                // flare bolts anywhere. The hit itself is applied unchanged: the shipped helmets'
                // `m_DamageTypesToAbsorb` is empty (U-246).
                if v.helmet && (pr.kind == ProjectileKind::IncendiaryBolt || head) {
                    v.helmet = false;
                }
                if pr.kind == ProjectileKind::IncendiaryBolt {
                    events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: Some(v.id) });
                    let mut dmg = pr.damage;
                    if v.kind != PawnKind::Human {
                        dmg = dmg.min(v.vitals.health - 1.0).max(0.0);
                        if v.burning <= 0.0 {
                            v.burning = 1e-4;
                            v.condition_by = Some(pr.owner);
                            events.push(SimEvent::Ignited { victim: v.id, by: pr.owner });
                        }
                    }
                    apply_damage(v, dmg, Some(pr.owner), DamageCause::Crossbow, events, vitals);
                    continue;
                }
                let (mut dmg, cause) = match (pr.kind, head) {
                    (ProjectileKind::Bullet, true) => (pr.damage * t.bullet_headshot_mult, DamageCause::PistolHeadshot),
                    (ProjectileKind::Bullet, false) => (pr.damage, DamageCause::Pistol),
                    (_, true) => (v.vitals.health.max(pr.damage), DamageCause::CrossbowHeadshot),
                    (_, false) => (pr.damage, DamageCause::Crossbow),
                };
                if pr.kind == ProjectileKind::Bullet && dist <= t.bullet_close_range {
                    dmg *= t.bullet_close_mult;
                }
                events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: Some(v.id) });
                apply_damage(v, dmg, Some(pr.owner), cause, events, vitals);
            }
            (_, Some(w)) => {
                events.push(SimEvent::ProjectileImpact { projectile: pr.id, point: from + delta.normalize_or_zero() * w, victim: None });
            }
            _ => {
                pr.position = to;
                if pr.age < 5.0 && pr.position.z > crate::movement::KILL_Z {
                    keep.push(pr);
                }
            }
        }
    }
    state.projectiles = keep;
}

pub fn make_projectile(id: u32, owner: u32, kind: ProjectileKind, pos: Vec3, vel: Vec3, t: &CombatTuning) -> ProjectileState {
    let (damage, gravity_mult) = match kind {
        ProjectileKind::Bullet => (t.bullet_damage, 0.0),
        ProjectileKind::Bolt => (t.bolt_damage, t.bolt_gravity_mult),
        ProjectileKind::SleepDart => (t.sleep_dart_damage, t.sleep_dart_gravity_mult),
        ProjectileKind::IncendiaryBolt => (t.incendiary_damage, t.incendiary_gravity_mult),
        // Grenades carry their own gravity (`step_grenades`).
        ProjectileKind::Grenade | ProjectileKind::StickyGrenade => (0.0, 0.0),
    };
    ProjectileState { id, kind, owner, position: pos, velocity: vel, age: 0.0, damage, gravity_mult, armed: false, fuse: 0.0, min_fuse: 0.0, stuck_to: None, stuck: false, stuck_offset: Vec3::ZERO }
}

/// Pawns revealed to `viewer` by Dark Vision: the "souls" of living pawns within `m_fDistance`
/// while the power is in effect (occlusion ignored, as in the original's x-ray presentation;
/// `Twk_Rat` leaves `m_bOnlyVisibleAtLevel2` false, so rats show at both levels).
pub fn dark_vision_reveals(state: &WorldState, viewer: u32, t: &CombatTuning) -> Vec<u32> {
    let Some(v) = state.player(viewer) else { return Vec::new() };
    if !dark_vision_active(v) {
        return Vec::new();
    }
    state
        .players
        .iter()
        .filter(|o| o.id != viewer && o.alive && !crate::powers::is_hidden(o) && o.position.distance(v.position) <= t.dark_vision_distance)
        .map(|o| o.id)
        .collect()
}
