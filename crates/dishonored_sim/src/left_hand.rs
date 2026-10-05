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
    }
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
            match p.left.phase {
                LeftPhase::Equipping if p.left.timer >= t.crossbow_equip => p.left.phase = LeftPhase::Idle,
                LeftPhase::Recovering if p.left.timer >= 0.2 => {
                    p.left.phase = if crate::inventory::ammo(p, ammo) > 0 { LeftPhase::Reloading } else { LeftPhase::Idle };
                    p.left.timer = 0.0;
                }
                LeftPhase::Reloading if p.left.timer >= t.crossbow_reload => {
                    p.left.phase = LeftPhase::Idle;
                    events.push(SimEvent::Reloaded { id: p.id });
                }
                LeftPhase::Idle if !busy && pressed && crate::inventory::ammo(p, ammo) > 0 => {
                    crate::inventory::remove_ammo(p, ammo, 1);
                    let eye = eye_position(p, ctx.cfg);
                    spawn.push((kind, eye, view_vector(p.yaw, p.pitch) * speed));
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
            if pressed && !busy {
                if p.vitals.dark_vision_remaining > 0.0 {
                    p.vitals.dark_vision_remaining = 0.0;
                    events.push(SimEvent::DarkVision { id: p.id, active: false });
                } else if spend_mana(p, t.dark_vision_cost, &cfg.vitals) {
                    p.vitals.dark_vision_remaining = t.dark_vision_duration;
                    events.push(SimEvent::DarkVision { id: p.id, active: true });
                }
            }
        }
        LeftHand::Pistol | LeftHand::Crossbow => {
            let bolt = p.left.item == LeftHand::Crossbow;
            let (equip, reload, recovery) = if bolt {
                (t.crossbow_equip, t.crossbow_reload, 0.2)
            } else {
                (t.pistol_equip, t.pistol_reload, t.pistol_fire_recovery)
            };
            match p.left.phase {
                LeftPhase::Equipping if p.left.timer >= equip => p.left.phase = LeftPhase::Idle,
                LeftPhase::Recovering if p.left.timer >= recovery => {
                    // Auto-reload after firing when reserve ammo exists (LIKELY).
                    let (loaded, reserve) = if bolt { (p.left.bolts_loaded, p.left.bolts_reserve) } else { (p.left.pistol_loaded, p.left.pistol_reserve) };
                    p.left.phase = if loaded == 0 && reserve > 0 { LeftPhase::Reloading } else { LeftPhase::Idle };
                    p.left.timer = 0.0;
                }
                LeftPhase::Reloading if p.left.timer >= reload => {
                    let (loaded, reserve) = if bolt {
                        (&mut p.left.bolts_loaded, &mut p.left.bolts_reserve)
                    } else {
                        (&mut p.left.pistol_loaded, &mut p.left.pistol_reserve)
                    };
                    if *reserve > 0 && *loaded == 0 {
                        *reserve -= 1;
                        *loaded = 1;
                        events.push(SimEvent::Reloaded { id: p.id });
                    }
                    p.left.phase = LeftPhase::Idle;
                }
                LeftPhase::Idle if !busy => {
                    let loaded = if bolt { p.left.bolts_loaded } else { p.left.pistol_loaded };
                    let reserve = if bolt { p.left.bolts_reserve } else { p.left.pistol_reserve };
                    if pressed && loaded > 0 {
                        if bolt { p.left.bolts_loaded -= 1 } else { p.left.pistol_loaded -= 1 }
                        let eye = eye_position(p, ctx.cfg);
                        let dir = view_vector(p.yaw, p.pitch);
                        let speed = if bolt { t.bolt_speed } else { t.bullet_speed };
                        spawn.push((if bolt { ProjectileKind::Bolt } else { ProjectileKind::Bullet }, eye, dir * speed));
                        p.left.phase = LeftPhase::Recovering;
                        p.left.timer = 0.0;
                        let (radius, cat) = if bolt { (500.0, SoundCategory::Crossbow) } else { (4000.0, SoundCategory::Pistol) };
                        events.push(SimEvent::Sound { position: eye, radius, category: cat, source: p.id });
                    } else if loaded == 0 && reserve > 0 && (pressed || input.pressed(buttons::RELOAD)) {
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
/// momentum handed to `TakeDamage` is `(impulse.xy * k, 0.15 * |impulse| * k)` with `k` the
/// blast factor. Applying the momentum as a velocity change is an inference (the original
/// only ever blasts NPCs, which ragdoll).
pub fn windblast_momentum(dir: Vec3, force: f32, up_force: f32, d: f32, range: f32, k: f32) -> Vec3 {
    let mut imp = dir * force;
    let up = (1.0 - (d / range).min(1.0)) * up_force;
    if up > imp.z {
        imp.z += (up - imp.z).min(up);
    }
    let mag = imp.length();
    Vec3::new(imp.x * k, imp.y * k, 0.15 * mag * k)
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
            v.velocity += windblast_momentum(wb.direction, t.windblast_force, t.windblast_up_force, d, range, k);
            if matches!(v.mode, MoveMode::Walking) {
                v.mode = MoveMode::Falling;
                v.fall_time = 0.0;
            }
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
                        v.sleep_timer = sleep_delay(pr.owner).max(1e-3);
                        v.condition_by = Some(pr.owner);
                    }
                    continue;
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

/// Players revealed to `viewer` by Dark Vision (within range; occlusion ignored, as in
/// the original's x-ray presentation).
pub fn dark_vision_reveals(state: &WorldState, viewer: u32, t: &CombatTuning) -> Vec<u32> {
    let Some(v) = state.player(viewer) else { return Vec::new() };
    if v.vitals.dark_vision_remaining <= 0.0 {
        return Vec::new();
    }
    state
        .players
        .iter()
        .filter(|o| o.id != viewer && o.alive && o.position.distance(v.position) <= t.dark_vision_distance)
        .map(|o| o.id)
        .collect()
}
