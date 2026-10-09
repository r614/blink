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
    /// Nominal seconds (presentation).
    pub load: f32,
    pub end: f32,
    /// Ticks from the reload's start to the load and to its end ([`ReloadTiming::ticks`]).
    pub load_ticks: u32,
    pub end_ticks: u32,
}

/// [`GunReload`] of the left-hand item (crossbow ammo types share the crossbow's).
pub fn gun_reload(p: &PlayerState, cfg: &GameplayConfig, t: &CombatTuning) -> GunReload {
    let bolt = p.left.item.is_crossbow();
    let (stages, host): (_, fn(&str) -> bool) = if bolt { (&t.crossbow_reload_stages, |n| n.starts_with("Crossbow")) } else { (&t.pistol_reload_stages, |n| n.starts_with("Pistol")) };
    let fast = crate::powers::attribute_for(p, cfg, "Attribute_FastReload", 0.0, host);
    let speed = crate::powers::attribute_for(p, cfg, "Attribute_ReloadSpeed", 0.0, host);
    let (load, end) = stages.stages(speed, fast);
    let (load_ticks, end_ticks) = stages.ticks(speed, fast, crate::movement::DT);
    GunReload { fast, load, end, load_ticks, end_ticks }
}

/// Whole ticks a left-hand timer (advanced by `dt` from 0) has run.
fn ticks_of(timer: f32, dt: f32) -> u32 {
    (timer / dt).round().max(0.0) as u32
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
    /// Blink asks its time-dilation layer (`BTC_BlinkPower`) for a world rate over a blend
    /// time (0x009FA0D0 from 0x00BFC990 / 0x00BE7380).
    BlinkLayer { target: f32, blend: f32 },
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
    // The Blink component ticks whatever the left hand holds (its cooldown, 0x00BFC5D0).
    blink::tick_cooldown(p, dt, &cfg.blink, actions);
    if !p.alive || p.incapacitated() {
        return;
    }
    // A Blink move in progress finishes whatever the left hand selects.
    if p.mode == MoveMode::Blinking {
        blink::update(p, false, false, false, ctx, &cfg.blink, &cfg.vitals, actions, events);
        return;
    }
    let casting = p.left.phase == LeftPhase::Recovering && is_cast(p.left.item);
    // Item switch (not in the middle of a power cast or gadget throw).
    if input.left_hand != p.left.item && !casting {
        // Deselecting Blink while aiming cancels it (the cooldown follows).
        blink::cancel(p, &cfg.blink, actions);
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
                    // The cost is taken with the cast notify (MEASURED 2026-10-08: Bend Time's 60
                    // mana 9 ticks after the press; the swarm actor 39 ticks after its cost).
                    if crossed(notify) && spend_mana(p, cost, &cfg.vitals) {
                        actions.push(if swarm { LeftAction::Swarm } else { LeftAction::BendTime });
                    }
                    if p.left.timer >= exit {
                        p.left.phase = LeftPhase::Idle;
                    }
                }
                _ if pressed && !busy && level(idx) > 0 => {
                    if !swarm && bend_active {
                        actions.push(LeftAction::EndBendTime);
                    } else if p.vitals.mana >= cost {
                        // The cast starts only when the power is usable (mana >= cost).
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
                            // From the eye: the original starts it at the hand
                            // (`m_AlignmentBoneName`, after a line check from the eye,
                            // 0x00C38F70); the hand's offset needs a capture (ranged.md F7).
                            // `DisTweaks_ThrowGrenade` has no AI loudness: the throw is silent.
                            spawn.push((kind, eye_position(p, ctx.cfg), crate::gadgets::throw_velocity(p.yaw, p.pitch, g)));
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
            // The bolt leaves when the fire clip's `DishonoredNotify_FireProjectile` passes
            // (`bolt_shot_tick`), as the crossbow's own bolts do.
            let shot_tick = bolt_shot_tick(t, g.fast > 0.0 && left > 0, ctx.dt);
            if p.left.phase == LeftPhase::Recovering && shot_tick > 0 && ticks_of(prev_timer, ctx.dt) < shot_tick && ticks_of(p.left.timer, ctx.dt) >= shot_tick {
                fire_special_bolt(p, kind, speed, ctx, cfg, t, spawn);
            }
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
                LeftPhase::Recovering if ticks_of(p.left.timer, ctx.dt) >= crate::tuning::action_ticks(t.crossbow_fire_recovery, ctx.dt) => {
                    p.left.phase = if left > 0 { LeftPhase::Reloading } else { LeftPhase::Idle };
                    p.left.timer = 0.0;
                }
                LeftPhase::Reloading if ticks_of(p.left.timer, ctx.dt) >= g.end_ticks => {
                    p.left.phase = LeftPhase::Idle;
                    events.push(SimEvent::Reloaded { id: p.id });
                }
                LeftPhase::Idle if !busy && pressed && crate::inventory::ammo(p, ammo) > 0 => {
                    crate::inventory::remove_ammo(p, ammo, 1);
                    // The crossbow's shot (`fire_gun`): `m_fArrowSpawnDistance` along the
                    // dispersed direction, speed scaled by `max(1, Attribute_Range)`, no AI noise.
                    p.left.phase = LeftPhase::Recovering;
                    p.left.timer = 0.0;
                    if bolt_shot_tick(t, g.fast > 0.0 && left > 1, ctx.dt) == 0 {
                        fire_special_bolt(p, kind, speed, ctx, cfg, t, spawn);
                    }
                }
                _ => {}
            }
        }
        LeftHand::Blink => {
            if p.mode == MoveMode::Blinking {
                blink::update(p, false, false, false, ctx, &cfg.blink, &cfg.vitals, actions, events);
            } else if !busy {
                blink::update(p, pressed, held, released, ctx, &cfg.blink, &cfg.vitals, actions, events);
            }
        }
        LeftHand::Windblast => {
            match p.left.phase {
                LeftPhase::Recovering if p.left.timer >= windblast_lockout(p, cfg, t) => p.left.phase = LeftPhase::Idle,
                LeftPhase::Idle if pressed && !busy => {
                    if spend_mana(p, t.windblast_cost, &cfg.vitals) {
                        p.left.phase = LeftPhase::Recovering;
                        p.left.timer = 0.0;
                        events.push(SimEvent::WindblastCast { id: p.id });
                        events.push(SimEvent::PowerUsed { id: p.id, power: power::WINDBLAST as u8, level: p.inv.power_levels[power::WINDBLAST] });
                        // Port value: the Windblast's AI noise caller was not decoded (stealth.md section 5.4).
                        events.push(SimEvent::Sound { position: p.position, radius: 1500.0, category: SoundCategory::Windblast, context: crate::events::NoiseContext::Danger, source: p.id });
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
            // The player's fire context starts the fire action on the press and the shot leaves
            // on the next tick (measured at 60 Hz: shot -> reload start 0.283 s for the pistol's
            // 0.300 s fire action, HOOK_TRIAGE.md); NPC guns are their own context (not
            // measured): they fire on the press.
            let delayed = p.kind == crate::state::PawnKind::Human;
            // Ticks after the press on which the shot leaves: the crossbow's fire clip carries a
            // `DishonoredNotify_FireProjectile` (`bolt_shot_tick`); the pistol's has none.
            let shot_tick = if bolt { bolt_shot_tick(t, fast_fire, ctx.dt) } else { 1 };
            if delayed && p.left.phase == LeftPhase::Recovering && shot_tick > 0 && ticks_of(prev_timer, ctx.dt) < shot_tick && ticks_of(now, ctx.dt) >= shot_tick {
                fire_gun(p, bolt, ctx, cfg, t, spawn, events);
            }
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
                // The fire action ends on the tick its animation reaches the AnimStateExit
                // (`tuning::action_ticks`, counted from the press).
                LeftPhase::Recovering if ticks_of(now, ctx.dt) >= crate::tuning::action_ticks(recovery, ctx.dt) => {
                    // The fire action's end requests a reload when the weapon is empty and ammo
                    // is held (pistol 0x00C2EA00, crossbow 0x00C2E730).
                    let loaded = if bolt { p.left.bolts_loaded } else { p.left.pistol_loaded };
                    p.left.phase = if loaded == 0 && reserve_now > 0 { LeftPhase::Reloading } else { LeftPhase::Idle };
                    p.left.timer = 0.0;
                }
                LeftPhase::Reloading => {
                    // ReloadIn, then the timed countdown whose end loads the magazine
                    // (0x00C2AC00), then ReloadOut, in whole ticks (`ReloadTiming::ticks`).
                    let (t0, t1) = (ticks_of(prev_timer, ctx.dt), ticks_of(now, ctx.dt));
                    if t0 < g.load_ticks && t1 >= g.load_ticks && load_magazine(p, bolt, mag) {
                        events.push(SimEvent::Reloaded { id: p.id });
                    }
                    if t1 >= g.end_ticks {
                        p.left.phase = LeftPhase::Idle;
                    }
                }
                LeftPhase::Idle if !busy => {
                    let loaded = if bolt { p.left.bolts_loaded } else { p.left.pistol_loaded };
                    let reserve = if bolt { p.left.bolts_reserve } else { p.left.pistol_reserve };
                    if pressed && loaded > 0 {
                        if bolt { p.left.bolts_loaded -= 1 } else { p.left.pistol_loaded -= 1 }
                        p.left.phase = LeftPhase::Recovering;
                        p.left.timer = 0.0;
                        if !delayed || shot_tick == 0 {
                            fire_gun(p, bolt, ctx, cfg, t, spawn, events);
                        }
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

/// Ticks after the press on which a crossbow bolt leaves (0 = on the press tick). The fire
/// action's animation advances by the frame time from the press tick, so the notify
/// (`CombatTuning::crossbow_fire_notify`, 0.0336 s of `Crossbow_Fire`; 0 s of
/// `Crossbow_Fire_FastReload`, used with `Attribute_FastReload` while bolts remain) fires on the
/// tick whose animation time first reaches it ([`crate::tuning::action_ticks`] counts the
/// advances including the press tick's). Measured in the original (60 Hz): fire sound on the
/// press tick, bolt spawned two ticks later.
pub fn bolt_shot_tick(t: &CombatTuning, fast: bool, dt: f32) -> u32 {
    let at = if fast { t.crossbow_fast_fire_notify } else { t.crossbow_fire_notify };
    crate::tuning::action_ticks(at, dt).saturating_sub(1)
}

/// A sleep dart or incendiary bolt leaves the crossbow (the shot of `fire_gun` for the other
/// ammunition): `m_fArrowSpawnDistance` along the dispersed direction, speed scaled by
/// `max(1, Attribute_Range)`, no AI noise.
fn fire_special_bolt(p: &mut PlayerState, kind: ProjectileKind, speed: f32, ctx: &MoveCtx, cfg: &GameplayConfig, t: &CombatTuning, spawn: &mut Vec<(ProjectileKind, Vec3, Vec3)>) {
    let eye = eye_position(p, ctx.cfg);
    let dir = fire_direction(p, cfg, t);
    spawn.push((kind, eye + dir * t.bolt_spawn_distance, dir * speed * crossbow_range_scale(p, cfg)));
}

/// The crossbow's `max(1, Attribute_Range)` (0x00C38230: the launch speed `m_fArrowFireSpeed`
/// is scaled by it; the weapon's range too, 0x00C072D0, which the sim does not model). No
/// attribute tweak carries a Range base, so it is 0 until the `Crossbow_Range` upgrade (+2).
pub fn crossbow_range_scale(p: &PlayerState, cfg: &GameplayConfig) -> f32 {
    crate::powers::attribute_for(p, cfg, "Attribute_Range", 0.0, |n| n.starts_with("Crossbow")).max(1.0)
}

/// The shot of a pistol or crossbow (0x00C2A3E0 / 0x00C38230): the projectile starts
/// `m_fBulletSpawnDistance` / `m_fArrowSpawnDistance` in front of the camera (the pistol along
/// the undispersed view, the crossbow along the dispersed direction it flies) and flies through
/// the dispersion offset (`fire_direction`); the pistol makes its AI noise
/// (`m_AINoiseLoudnessWhenFiring`). The crossbow's fire tweak has no loudness. Its muzzle-socket
/// spawn (when the muzzle and the camera rays reach the same actor) is not modelled: the socket
/// offset needs a capture (research/dishonored/systems/ranged.md F2).
#[allow(clippy::too_many_arguments)]
fn fire_gun(p: &mut PlayerState, bolt: bool, ctx: &MoveCtx, cfg: &GameplayConfig, t: &CombatTuning, spawn: &mut Vec<(ProjectileKind, Vec3, Vec3)>, events: &mut Vec<SimEvent>) {
    let eye = eye_position(p, ctx.cfg);
    let view = view_vector(p.yaw, p.pitch);
    let dir = fire_direction(p, cfg, t);
    if bolt {
        let speed = t.bolt_speed * crossbow_range_scale(p, cfg);
        spawn.push((ProjectileKind::Bolt, eye + dir * t.bolt_spawn_distance, dir * speed));
    } else {
        spawn.push((ProjectileKind::Bullet, eye + view * t.bullet_spawn_distance, dir * t.bullet_speed));
        // `m_AINoiseLoudnessWhenFiring` (`AiConfig::noise.pistol_fire`, Danger) at the shooter's
        // Location (0x00C2A3E0 passes the pawn's +0xC4).
        crate::noise::emit(events, &cfg.ai.noise.pistol_fire, p.position, SoundCategory::Pistol, p.id, 0.0);
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
    // The component's use notification (0x00BE89E0 calls slot 94 when it starts the effect).
    events.push(SimEvent::PowerUsed { id: p.id, power: power::DARK_VISION as u8, level: p.inv.power_levels[power::DARK_VISION] });
}

/// Dark Vision tick (slot +0x13C, 0x00BFBC90): the cooldown runs down; the step is spent on the
/// fade-in first, the rest on the duration, then on the fade-out.
///
/// The duration step (0x00BF9AB0) first runs the combat check (0x00BF84C0): while the pawn has
/// combat opponents (its skirmish's combatant count, 0x00C5AF60, at least 1) `m_fCombatTimer`
/// is set to `m_fDesactivateInCombatTimer` on the first such step and counted down on the next
/// ones; when it reaches zero the remaining duration is zeroed. Out of combat it is reset to 0.
pub fn tick_dark_vision(p: &mut PlayerState, dt: f32, combat_timer: f32, events: &mut Vec<SimEvent>) {
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
        if p.combat_opponents < 1 {
            p.dark.combat_timer = 0.0;
        } else if p.dark.combat_timer <= 0.0 {
            p.dark.combat_timer = combat_timer;
        } else {
            p.dark.combat_timer -= left;
            if p.dark.combat_timer <= 0.0 {
                p.dark.remaining = 0.0;
                events.push(SimEvent::DarkVision { id: p.id, active: false });
            }
        }
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

/// Windblast cooldown after the front reached its range (`m_fCooldownTime`, 1.0 s): class
/// default of `DishonoredActivePowerComponent_WindBlast`. Its `m_fWarmupTime` (0.1 s) is not
/// when the blast starts: the fire mode waits for the cast animation's notify
/// (`CombatTuning::windblast_start_delay`).
pub const WINDBLAST_COOLDOWN: f32 = 1.0;

/// The caster's Windblast level index (`m_CurrentLevel - 1`, 0x00BF6780 / 0x00BF7090 read
/// `m_Levels[m_CurrentLevel - 1]`) and its range `m_Distance x (1 + Attribute_SpellStrength)`
/// (the gather radius and the front's cap).
pub fn windblast_params(p: &PlayerState, cfg: &GameplayConfig, t: &CombatTuning) -> (u8, f32) {
    let level = p.inv.power_levels[power::WINDBLAST].clamp(1, 2) - 1;
    let strength = crate::powers::attribute(p, cfg, "Attribute_SpellStrength", 0.0);
    (level, (strength + 1.0) * t.windblast_distance[level as usize])
}

/// Seconds from the cast until Windblast can be cast again (start delay, front travel, cooldown).
pub fn windblast_lockout(p: &PlayerState, cfg: &GameplayConfig, t: &CombatTuning) -> f32 {
    let (level, range) = windblast_params(p, cfg, t);
    t.windblast_start_delay + range / t.windblast_speed[level as usize].max(1.0) + WINDBLAST_COOLDOWN
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
/// decode read the z as `0.15 x |impulse| x k` (407 here). An NPC is thrown with it
/// (`movement::start_throw`: velocity = momentum / `m_fEffectiveMassForThrowing`).
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
    th: &dishonored_ir::gameplay::ThrowConfig,
    t: &CombatTuning,
    dt: f32,
    events: &mut Vec<SimEvent>,
) {
    let mut blasts = std::mem::take(&mut state.windblasts);
    for wb in blasts.iter_mut() {
        let lv = (wb.level as usize).min(1);
        let range = if wb.range > 0.0 { wb.range } else { t.windblast_distance[lv] };
        let cos_half = (t.windblast_angle[lv] * 0.5).to_radians().cos();
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
            // The colliding walls of light are gathered the same way (their bounds centre).
            wb.walls = state.wall_blockers.iter().filter(|b| b.center().distance(wb.origin) < range && windblast_factor(dir, b.center(), wb.origin, range, cos_half) > 0.0).map(|b| b.wall).collect();
            // Projectiles are gathered the same way: the front reaches them below (0x00C4F290).
            let (o, d) = (wb.origin, dir);
            wb.projectiles = state.projectiles.iter().filter(|pr| pr.position.distance(o) < range && windblast_factor(d, pr.position, o, range, cos_half) > 0.0).map(|pr| pr.id).collect();
            wb.suggested = None;
        }
        // `m_pSuggestedTarget` (0x00BEE2A0, at the reach before this step): kept while it exists,
        // else the gathered living pawn beyond the front whose direction from the origin is
        // nearest the blast's (a positive dot product).
        let prev_reach = (wb.elapsed * t.windblast_speed[lv]).min(range);
        if wb.suggested.is_some_and(|s| state.player(s).is_none()) {
            wb.suggested = None;
        }
        if wb.suggested.is_none() {
            let mut best = (0.0f32, None);
            for &vid in &wb.targets {
                let Some(v) = state.player(vid).filter(|v| v.alive) else { continue };
                let to = v.position - wb.origin;
                if to.length_squared() > prev_reach * prev_reach {
                    let dot = wb.direction.dot(to.normalize_or_zero());
                    if best.0 < dot {
                        best = (dot, Some(vid));
                    }
                }
            }
            wb.suggested = best.1;
        }
        // Front advance (0x00BF7090): reach = (elapsed + dt) * speed, capped at the range.
        let reach = ((wb.elapsed + dt) * t.windblast_speed[lv]).min(range);
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
                // The throw (`StateNPCMasterThrown`): velocity = clamped momentum / effective
                // mass, then the Flying phase (physics.md P-1..P-3).
                let m = windblast_momentum(wb.direction, t.windblast_force[lv], t.windblast_up_force[lv], d, range, k);
                crate::movement::start_throw(v, m, th, dt);
                // The thrown reaction (0x00C4DE90 ends in `DisSeqEvent_Windblasted` output 1).
                events.push(SimEvent::Windblasted { victim: v.id, by: wb.caster });
                // Last hit by this caster's Windblast (instigator and damage type, read by the
                // NPC wall collision 0x00B529C0).
                v.windblasted_by = Some(wb.caster);
            }
            // `Twk_Inv_EliteHelmet`: Windblast knocks a helmet off whatever it hits.
            v.helmet = false;
            if v.sword.phase == crate::state::SwordPhase::Blocking {
                v.sword.phase = crate::state::SwordPhase::Idle;
            }
            let dmg = (t.windblast_damage[lv] * k + 0.5).floor();
            apply_damage(v, dmg, Some(wb.caster), DamageCause::Windblast, events, vitals);
        }
        wb.targets = remaining;
        // A wall of light the front passed (0x00BF6AF0, after the sight check to its bounds
        // centre, the wall itself excluded) takes the blast in its vslot 223 instead of a push
        // (`crate::walls::apply_hits`); one no longer colliding was left out of the world.
        let mut walls_left = Vec::new();
        for &w in &wb.walls {
            let Some(b) = state.wall_blockers.iter().find(|b| b.wall == w) else { continue };
            let c = b.center();
            let d = c.distance(wb.origin);
            if d > reach {
                walls_left.push(w);
                continue;
            }
            let to = c - wb.origin;
            if let Some(h) = world.raycast(wb.origin, to, d, channel::PROJECTILE) {
                let own = h.point.cmpge(b.lo - Vec3::ONE).all() && h.point.cmple(b.hi + Vec3::ONE).all();
                if !own && h.distance < d - mv.collision_radius {
                    continue;
                }
            }
            let k = windblast_factor(wb.direction, c, wb.origin, range, cos_half);
            events.push(SimEvent::WallOfLightBlast { wall: w, force: k, by: wb.caster });
        }
        wb.walls = walls_left;
        // A projectile the front passed (the same sight check to its location) reacts by its
        // tweak's `m_WindblastBehavior` (0x00C4F290, k = the blast factor at it).
        let mut projectiles_left = Vec::new();
        let suggested = wb.suggested.and_then(|s| state.player(s).map(|p| (s, p.position)));
        for &pid in &wb.projectiles {
            let Some(pi) = state.projectiles.iter().position(|p| p.id == pid) else { continue };
            let at = state.projectiles[pi].position;
            let d = at.distance(wb.origin);
            if d > reach {
                projectiles_left.push(pid);
                continue;
            }
            if d > 1e-3 {
                if let Some(h) = world.raycast(wb.origin, at - wb.origin, d, channel::PROJECTILE) {
                    if h.distance < d - 1.0 {
                        continue;
                    }
                }
            }
            let k = windblast_factor(wb.direction, at, wb.origin, range, cos_half);
            if k <= 0.0 {
                continue;
            }
            let force = t.windblast_force[lv] * k;
            let pr = &mut state.projectiles[pi];
            match t.projectile_windblast_behavior {
                1 => windblast_push(pr, wb.direction, force, None, t),
                2 => windblast_push(pr, wb.direction, force, suggested.map(|s| s.1), t),
                3 => windblast_homing(pr, wb.caster, wb.origin, wb.direction, suggested, world, t),
                _ => {}
            }
        }
        wb.projectiles = projectiles_left;
        wb.elapsed += dt;
        if reach >= range {
            wb.targets.clear();
            wb.walls.clear();
            wb.projectiles.clear();
        }
    }
    blasts.retain(|wb| !wb.started || !wb.targets.is_empty() || !wb.walls.is_empty() || !wb.projectiles.is_empty());
    state.windblasts = blasts;
}

/// The pistol bullet's damage on a pawn (`DisBullet` slot 239, 0x00C29820):
/// `m_Damage + m_fDamage`, times `m_fDamageMultiplier_Headshot` on the head region and
/// `m_fDamageMultiplier_Stealth` unless the pawn is `InCombat`; then the distance from the
/// spawn point picks `m_fDamageMultiplier_CloseRange` (strictly below `m_fCloseRange`) or
/// `m_fDamageMultiplier_LongRange` (strictly above `m_fLongRange`); a bullet that already went
/// through a pawn deals `m_fPostPiercingDamageRatio` of it.
pub fn bullet_damage(t: &CombatTuning, base: f32, head: bool, in_combat: bool, dist: f32, pierced: bool) -> f32 {
    let mut d = base;
    if head {
        d *= t.bullet_headshot_mult;
    }
    if !in_combat {
        d *= t.bullet_stealth_mult;
    }
    if dist < t.bullet_close_range {
        d *= t.bullet_close_mult;
    } else if dist > t.bullet_long_range {
        d *= t.bullet_long_mult;
    }
    if pierced {
        d *= t.bullet_post_pierce_ratio;
    }
    d
}

/// A crossbow projectile's damage on a pawn (`DisProjectile_Arrow` slot 239, 0x00C503D0 over
/// the base 0x00C4A310): `None` for a kill (`m_bKillOnHeadshot` on the head region, through the
/// pawn's instakill interface); otherwise the base (`m_Damage x m_fDamageMultiplier`), times the
/// headshot factor on the head; head and torso hits on a pawn that is not aware of the shooter
/// (Unaware, Surprised, Suspicious, Fearful) times `m_fDamageMultiplier_Stealth`; any other
/// region capped at `m_fLegsAndArmsDamageCap`.
pub fn arrow_damage(t: &CombatTuning, a: &crate::tuning::ArrowDamage, base: f32, region: dishonored_ir::items::HitRegion, stealth: bool) -> Option<f32> {
    use dishonored_ir::items::HitRegion;
    let mut d = base;
    if region == HitRegion::Head {
        if a.kill_on_headshot {
            return None;
        }
        d *= t.bolt_headshot_mult.max(1.0);
    }
    if crate::hit_regions::is_core(region) {
        if stealth {
            d *= a.stealth_mult;
        }
    } else {
        d = d.min(a.limb_cap);
    }
    Some(d)
}

/// A blasted NPC hitting a wall (the NPC pawn's rigid-body collision, 0x00B529C0): when the
/// pawn's last hit was the player's Windblast, the other side is world geometry, the contact
/// normal is wall-like (`n.z^2 < n.x^2 + n.y^2`), the blaster's Windblast is at level 2
/// (`m_CurrentLevel > 1`) and the pawn moves at `m_fMinSpeedToExplodeNPCs` or faster, it takes
/// `m_HitWallAdditionalDamage` from the blaster; if that kills it, a roll at or below
/// `m_fChanceToExplodeNPCs` explodes the body (0x00B52530, no corpse left). The original
/// collides the ragdoll; the sim collides the falling capsule (the hit is recorded by the
/// falling move in `PlayerState::wall_hit`). A pawn that landed is no longer "blasted".
pub fn windblast_wall_hits(state: &mut WorldState, t: &CombatTuning, vitals: &dishonored_ir::gameplay::VitalsConfig, events: &mut Vec<SimEvent>) {
    for i in 0..state.players.len() {
        let hit = state.players[i].wall_hit.take();
        let p = state.players[i];
        let Some(by) = p.windblasted_by else { continue };
        if p.mode != MoveMode::Falling {
            state.players[i].windblasted_by = None;
        }
        let Some((n, speed)) = hit else { continue };
        if p.kind == PawnKind::Human || !p.alive || n.z * n.z >= n.x * n.x + n.y * n.y || speed < t.windblast_wall_min_speed {
            continue;
        }
        if state.player(by).is_none_or(|c| c.inv.power_levels[power::WINDBLAST] <= 1) {
            continue;
        }
        let v = &mut state.players[i];
        apply_damage(v, t.windblast_wall_damage, Some(by), DamageCause::Windblast, events, vitals);
        if !v.alive && t.windblast_explode_chance > 0.0 && v.roll() <= t.windblast_explode_chance {
            v.no_body = true;
            events.push(SimEvent::BodyRemoved { body: v.id, ash: false });
        }
    }
}

/// A projectile pushed by a Windblast (0x00C4A430, behaviours Normal and PushTowardNPC): the
/// velocity keeps only its part along the push (none when it points away), never downward, then
/// gains `force / m_fMass` along it and `0.15 force / m_fMass` up. The push goes along the blast,
/// or toward `target`. `m_pSourceActor` is cleared.
pub fn windblast_push(pr: &mut ProjectileState, dir: Vec3, force: f32, target: Option<Vec3>, t: &CombatTuning) {
    if t.projectile_mass <= 0.0 {
        return;
    }
    pr.source_cleared = true;
    pr.homing = None;
    let d = match target {
        Some(p) => (p - pr.position).try_normalize().unwrap_or(Vec3::ZERO),
        None => dir,
    };
    let along = pr.velocity.dot(d);
    let mut v = if along > 0.0 { d * along } else { Vec3::ZERO };
    v.z = v.z.max(0.0);
    let s = 1.0 / t.projectile_mass;
    v.x += d.x * force * s;
    v.y += d.y * force * s;
    v.z += force * 0.15 * s;
    pr.velocity = v;
}

/// A projectile a Windblast sends homing (0x00C4A650, behaviour Homing): it becomes the
/// caster's (instigator) and its source is cleared; when the suggested target lies within a
/// 0.8 dot product of the projectile's direction from the blast origin it homes on that pawn,
/// else on what the blast's line hits within 10000 uu (its end without a hit); at
/// `m_fWindblastVelocityMultiplier x` its speed for `m_fWindblastHomingTime`, turning up to the
/// default 360 degrees.
pub fn windblast_homing(pr: &mut ProjectileState, caster: crate::state::PlayerId, origin: Vec3, dir: Vec3, suggested: Option<(crate::state::PlayerId, Vec3)>, world: &dyn WorldQuery, t: &CombatTuning) {
    pr.source_cleared = true;
    pr.owner = caster;
    let speed = t.projectile_windblast_velocity_mult * pr.velocity.length();
    let from_origin = (pr.position - origin).normalize_or_zero();
    let homing = |actor, position| crate::state::ProjectileHoming { actor, position, speed, time_left: t.projectile_windblast_homing_time, max_angle: 360.0 };
    if let Some((id, at)) = suggested {
        let to = (at - pr.position).normalize_or_zero();
        if to.dot(from_origin) > 0.8 {
            pr.homing = Some(homing(Some(id), Vec3::ZERO));
            return;
        }
    }
    let end = origin + dir * 10000.0;
    let point = world.raycast(origin, dir, 10000.0, channel::PROJECTILE).map_or(end, |h| origin + dir * h.distance);
    pr.homing = Some(homing(None, point));
}

/// `DisTweaks_Projectile.m_fSpeedMultiplier` of a projectile kind (the bolts'; 1 for the rest).
fn speed_mult(kind: ProjectileKind, t: &CombatTuning) -> f32 {
    match kind {
        ProjectileKind::Bolt => t.bolt_speed_mult,
        ProjectileKind::SleepDart => t.sleep_dart_speed_mult,
        ProjectileKind::IncendiaryBolt => t.incendiary_speed_mult,
        _ => 1.0,
    }
}

/// The homing step (0x00C49EE0), before the move: the time runs down; the wanted velocity is
/// `m_fSpeed` toward the position, or along the low ballistic arc (0x00BEC130 at `m_fSpeed x
/// m_fSpeedMultiplier`, gravity x `m_fGravityMultiplier`) toward the pawn (its location: the
/// hit regions' aim focus 0x00BC7190 is not decoded); a velocity turned further than
/// `m_fMaxCorrectionAngle_Deg` from it ends the homing, within 400 uu of the target or 30 degrees
/// of it the velocity takes it, else it moves 20 % of the way (0x00A1A7C0, per step); the homing
/// ends when the time runs out, the pawn is gone, or the step passes within 50 uu of the target.
pub fn step_homing(pr: &mut ProjectileState, players: &[PlayerState], gravity_z: f32, t: &CombatTuning, dt: f32) {
    let Some(mut h) = pr.homing else { return };
    if dt <= 0.0 {
        return;
    }
    h.time_left -= dt;
    let mut done = h.time_left < 0.0;
    let (target, want) = match h.actor {
        None => (h.position, (h.position - pr.position).try_normalize().unwrap_or(Vec3::ZERO)),
        Some(id) => match players.iter().find(|p| p.id == id) {
            Some(p) => {
                let g = -gravity_z * pr.gravity_mult;
                (p.position, crate::launchers::ballistic(pr.position, p.position, h.speed * speed_mult(pr.kind, t), g).0)
            }
            None => {
                pr.homing = None;
                return;
            }
        },
    };
    let wanted = want * h.speed;
    if !done {
        let c = pr.velocity.length();
        if c > 1e-8 {
            let angle = (pr.velocity.dot(want) / c).clamp(-1.0, 1.0).acos().to_degrees();
            if h.max_angle < angle {
                done = true;
            } else if (target - pr.position).length_squared() < 160_000.0 || angle < 30.0 {
                pr.velocity = wanted;
            } else {
                pr.velocity += (wanted - pr.velocity) * 0.2;
            }
        } else {
            pr.velocity += (wanted - pr.velocity) * 0.2;
        }
    }
    // 0x0040F270: the target's distance to this step's move.
    let next = pr.position + pr.velocity * dt;
    let seg = next - pr.position;
    let u = if seg.length_squared() > 1e-12 { ((target - pr.position).dot(seg) / seg.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    let passed = (pr.position + seg * u).distance(target) < 50.0;
    pr.homing = if done || passed { None } else { Some(h) };
}

/// Advance projectiles: sub-stepped segment tests against world and player bodies.
#[allow(clippy::too_many_arguments)]
pub fn step_projectiles(
    state: &mut WorldState,
    world: &dyn WorldQuery,
    mv: &MovementConfig,
    t: &CombatTuning,
    hb: &dishonored_ir::items::HitBodyConfig,
    vitals: &dishonored_ir::gameplay::VitalsConfig,
    gravity_z: f32,
    dt: f32,
    sleep_delay: &dyn Fn(u32) -> f32,
    events: &mut Vec<SimEvent>,
) -> Vec<(crate::state::PlayerId, Vec3)> {
    use dishonored_ir::items::HitRegion;
    // Tallboy arrows that hit something: (shooter, explosion point).
    let mut booms = Vec::new();
    let mut keep = Vec::with_capacity(state.projectiles.len());
    let projectiles = std::mem::take(&mut state.projectiles);
    'shots: for mut pr in projectiles {
        // Grenades have their own step; a launcher's shots are flown by `launchers::step_shots`.
        if pr.kind.is_grenade() || pr.kind.is_launcher_shot() {
            keep.push(pr);
            continue;
        }
        pr.age += dt;
        // A homing explosive arrow within 115 uu of its target explodes there
        // (`DisProjectile_Arrow_Explosive` vslot 103, 0x00C58310 -> 0x00C550D0).
        if pr.kind == ProjectileKind::TallboyArrow {
            if let Some(h) = pr.homing {
                let target = match h.actor {
                    Some(id) => state.player(id).map(|p| p.position),
                    None => Some(h.position),
                };
                if target.is_some_and(|at| at.distance_squared(pr.position) < 13_225.0) {
                    booms.push((pr.owner, pr.position));
                    continue 'shots;
                }
            }
        }
        // A Windblast's homing steers it first (0x00C49EE0).
        step_homing(&mut pr, &state.players, gravity_z, t, dt);
        let old_v = pr.velocity;
        pr.velocity.z += gravity_z * pr.gravity_mult * dt;
        let delta = (old_v + pr.velocity) * 0.5 * dt;
        let mut from = pr.position;
        let to = from + delta;
        let dir = delta.normalize_or_zero();
        // The sweep goes on past a pierced pawn (0x00C4F3A0 re-traces from the hit).
        loop {
            let seg = to - from;
            let len = seg.length();
            // World hit.
            let world_hit = if len > 1e-6 { world.raycast(from, seg, len, channel::PROJECTILE).map(|h| h.distance) } else { None };
            // Body hits (capsule vs segment), owner and the pierced pawn excluded.
            let mut body: Option<(usize, f32, Vec3)> = None;
            for (i, v) in state.players.iter().enumerate() {
                if !v.alive || (v.id == pr.owner && !pr.source_cleared) || Some(v.id) == pr.pierced || v.carried_by.is_some() || crate::powers::is_hidden(v) {
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
            // A wall of light that collides takes the hit and destroys the projectile (0x00C4F3A0).
            if let Some((wall, point)) = crate::walls::projectile_hit(state, from, to, world_hit, body.map(|b| b.1)) {
                events.push(SimEvent::WallOfLightHit { wall, point, projectile: pr.id, owner: pr.owner });
                continue 'shots;
            }
            match (body, world_hit) {
                (Some((i, along, point)), wh) if wh.is_none_or(|w| along <= w) => {
                    // `rand()` for the pierce, from the shooter's stream (0x00C3A660 draws it
                    // after the damage, on a bullet that has not pierced yet).
                    let pierce_roll = if pr.kind == ProjectileKind::Bullet && pr.pierced.is_none() { state.player_mut(pr.owner).map(|o| o.roll()) } else { None };
                    if pr.kind == ProjectileKind::TallboyArrow {
                        // An explosive arrow explodes at its first impact, 10 uu off the hit
                        // (0x00c550d0 / 0x00c5a4c0); the bow deals no damage of its own.
                        events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: Some(state.players[i].id) });
                        booms.push((pr.owner, point - dir * 10.0));
                        continue 'shots;
                    }
                    let v = &mut state.players[i];
                    let region = crate::hit_regions::hit_region(v, mv, hb, point, dir);
                    let head = region == HitRegion::Head;
                    // Sleep darts (`m_bPutToSleep`, `m_fTimeBeforeSleep`) and incendiary bolts
                    // (`m_bPutToFire`, `m_bCannotKillNPC`) act on NPCs; on players they only hit.
                    if pr.kind == ProjectileKind::SleepDart {
                        events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: Some(v.id) });
                        if v.kind != PawnKind::Human && v.condition == crate::state::Condition::Normal && v.sleep_timer <= 0.0 {
                            // `m_bInstantSleepForUnawareNPCs`: an NPC the AI reports unaware sleeps at once.
                            v.sleep_timer = if v.ai_unaware { 1e-3 } else { sleep_delay(pr.owner).max(1e-3) };
                            v.condition_by = Some(pr.owner);
                        }
                        continue 'shots;
                    }
                    // `Twk_Inv_EliteHelmet.m_DamageTypesThatKnockOff`: bullets and bolts on the head,
                    // flare bolts anywhere. The hit itself is applied unchanged: the shipped helmets'
                    // `m_DamageTypesToAbsorb` is empty (U-246).
                    if v.helmet && (pr.kind == ProjectileKind::IncendiaryBolt || head) {
                        v.helmet = false;
                    }
                    // Awareness of the shooter: the AI's `ai_unaware` (any state but combat) stands
                    // for the original's Unaware / Surprised / Suspicious / Fearful set (players
                    // are never unaware).
                    let stealth = v.ai_unaware;
                    if pr.kind == ProjectileKind::IncendiaryBolt {
                        events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: Some(v.id) });
                        let mut dmg = arrow_damage(t, &t.arrows[2], pr.damage, region, stealth).unwrap_or(v.vitals.health.max(pr.damage));
                        if v.kind != PawnKind::Human {
                            dmg = dmg.min(v.vitals.health - 1.0).max(0.0);
                            if v.burning <= 0.0 {
                                v.burning = 1e-4;
                                v.condition_by = Some(pr.owner);
                                events.push(SimEvent::Ignited { victim: v.id, by: pr.owner });
                            }
                        }
                        apply_damage(v, dmg, Some(pr.owner), DamageCause::Crossbow, events, vitals);
                        continue 'shots;
                    }
                    let (dmg, cause) = if pr.kind == ProjectileKind::Bullet {
                        let dist = point.distance(pr.spawn);
                        let cause = if head { DamageCause::PistolHeadshot } else { DamageCause::Pistol };
                        (bullet_damage(t, pr.damage, head, !stealth, dist, pr.pierced.is_some()), cause)
                    } else {
                        match arrow_damage(t, &t.arrows[0], pr.damage, region, stealth) {
                            None => (v.vitals.health.max(pr.damage), DamageCause::CrossbowHeadshot),
                            Some(d) => (d, if head { DamageCause::CrossbowHeadshot } else { DamageCause::Crossbow }),
                        }
                    };
                    let victim = v.id;
                    events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: Some(victim) });
                    // `TakeDamage` takes the int of the damage (slot 239's float, truncated).
                    apply_damage(v, dmg.trunc(), Some(pr.owner), cause, events, vitals);
                    // Piercing (0x00C3A660): `rand() / 32767 <= m_fPiercingChance` keeps the bullet
                    // going through its first pawn.
                    if let Some(r) = pierce_roll {
                        if r <= t.bullet_pierce_chance {
                            pr.pierced = Some(victim);
                            from = point;
                            continue;
                        }
                    }
                    continue 'shots;
                }
                (_, Some(w)) => {
                    let point = from + seg.normalize_or_zero() * w;
                    events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: None });
                    if pr.kind == ProjectileKind::TallboyArrow {
                        booms.push((pr.owner, point - dir * 10.0));
                    }
                    continue 'shots;
                }
                _ => break,
            }
        }
        pr.position = to;
        if pr.age < 5.0 && pr.position.z > crate::movement::KILL_Z {
            keep.push(pr);
        }
    }
    state.projectiles = keep;
    booms
}

pub fn make_projectile(id: u32, owner: u32, kind: ProjectileKind, pos: Vec3, vel: Vec3, t: &CombatTuning) -> ProjectileState {
    let (damage, gravity_mult) = match kind {
        ProjectileKind::Bullet => (t.bullet_damage, 0.0),
        ProjectileKind::Bolt => (t.bolt_damage, t.bolt_gravity_mult),
        ProjectileKind::SleepDart => (t.sleep_dart_damage, t.sleep_dart_gravity_mult),
        ProjectileKind::IncendiaryBolt => (t.incendiary_damage, t.incendiary_gravity_mult),
        // Grenades carry their own gravity (`step_grenades`).
        ProjectileKind::Grenade | ProjectileKind::StickyGrenade => (0.0, 0.0),
        // Its gravity is the arrow tweak's (set by `step_world`); all its damage is the explosion's.
        ProjectileKind::TallboyArrow => (0.0, 0.0),
        // A launcher's shots are made by `launchers::LaunchersState::fire`.
        ProjectileKind::TrapArrow | ProjectileKind::TrapBolt => (0.0, 0.0),
    };
    ProjectileState { id, kind, owner, position: pos, velocity: vel, age: 0.0, damage, gravity_mult, armed: false, fuse: 0.0, min_fuse: 0.0, stuck_to: None, stuck: false, stuck_offset: Vec3::ZERO, spawn: pos, pierced: None, homing: None, source_cleared: false, npc_throw: false }
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


#[cfg(test)]
mod windblast_projectile_tests {
    use super::*;
    use crate::state::{PawnKind, ProjectileHoming};

    fn tuning() -> CombatTuning {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
        CombatTuning::from_config(&crate::SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap().gameplay)
    }

    fn bolt(pos: Vec3, vel: Vec3, t: &CombatTuning) -> ProjectileState {
        make_projectile(1, 7, ProjectileKind::Bolt, pos, vel, t)
    }

    #[test]
    fn the_class_defaults_make_every_projectile_home() {
        let t = tuning();
        assert_eq!(t.projectile_windblast_behavior, 3);
        assert_eq!((t.projectile_windblast_homing_time, t.projectile_windblast_velocity_mult, t.projectile_mass), (1.6, 1.2, 1.0));
    }

    #[test]
    fn a_push_keeps_the_velocity_along_it_never_down_and_adds_the_impulse() {
        let t = tuning();
        // Flying back at the caster (against the blast): only the impulse is left.
        let mut pr = bolt(Vec3::ZERO, Vec3::new(-3000.0, 0.0, -100.0), &t);
        windblast_push(&mut pr, Vec3::X, 1000.0, None, &t);
        assert_eq!(pr.velocity, Vec3::new(1000.0, 0.0, 150.0));
        assert!(pr.source_cleared && pr.owner == 7, "the instigator stays the shooter");
        // Along the blast, partly down: the part along it is kept, its z clamped at 0.
        let mut pr = bolt(Vec3::ZERO, Vec3::new(2000.0, 500.0, -300.0), &t);
        let d = Vec3::new(0.8, 0.0, -0.6);
        windblast_push(&mut pr, d, 100.0, None, &t);
        let along = Vec3::new(2000.0, 500.0, -300.0).dot(d);
        assert!((pr.velocity - Vec3::new(d.x * along + 80.0, 0.0, 15.0)).length() < 1e-2, "{:?}", pr.velocity);
    }

    #[test]
    fn homing_turns_by_a_fifth_far_away_and_takes_the_velocity_near_the_target() {
        let t = tuning();
        let h = ProjectileHoming { actor: None, position: Vec3::new(0.0, 2000.0, 0.0), speed: 1000.0, time_left: 1.6, max_angle: 360.0 };
        // Far (> 400 uu) and turned 90 degrees: 20 % of the way to the wanted velocity.
        let mut pr = bolt(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0), &t);
        pr.homing = Some(h);
        step_homing(&mut pr, &[], -1500.0, &t, 1.0 / 60.0);
        assert!((pr.velocity - Vec3::new(800.0, 200.0, 0.0)).length() < 1e-2, "{:?}", pr.velocity);
        assert!(pr.homing.is_some_and(|x| (x.time_left - (1.6 - 1.0 / 60.0)).abs() < 1e-5));
        // Within 400 uu: the wanted velocity at once.
        let mut pr = bolt(Vec3::new(0.0, 1700.0, 0.0), Vec3::new(1000.0, 0.0, 0.0), &t);
        pr.homing = Some(h);
        step_homing(&mut pr, &[], -1500.0, &t, 1.0 / 60.0);
        assert!((pr.velocity - Vec3::new(0.0, 1000.0, 0.0)).length() < 1e-2);
        // A step passing within 50 uu ends it; so does the time running out.
        let mut pr = bolt(Vec3::new(0.0, 1990.0, 0.0), Vec3::new(0.0, 1000.0, 0.0), &t);
        pr.homing = Some(h);
        step_homing(&mut pr, &[], -1500.0, &t, 1.0 / 60.0);
        assert!(pr.homing.is_none());
        let mut pr = bolt(Vec3::ZERO, Vec3::new(0.0, 1000.0, 0.0), &t);
        pr.homing = Some(ProjectileHoming { time_left: 0.01, ..h });
        step_homing(&mut pr, &[], -1500.0, &t, 1.0 / 60.0);
        assert!(pr.homing.is_none());
    }

    #[test]
    fn homing_on_a_pawn_that_is_gone_stops() {
        let t = tuning();
        let mut pr = bolt(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0), &t);
        pr.homing = Some(ProjectileHoming { actor: Some(42), position: Vec3::ZERO, speed: 1200.0, time_left: 1.6, max_angle: 360.0 });
        let mut p = PlayerState::new(9, Vec3::new(3000.0, 0.0, 0.0), 0.0);
        p.kind = PawnKind::Guard;
        step_homing(&mut pr, &[p], -1500.0, &t, 1.0 / 60.0);
        assert!(pr.homing.is_none());
        assert_eq!(pr.velocity, Vec3::new(1000.0, 0.0, 0.0));
    }
}