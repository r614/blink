//! Left-hand items (M10–M12): pistol, crossbow, Windblast, Dark Vision, and the Blink
//! dispatch. Ranged weapons fire simulated projectiles (no hitscan) so client and server
//! resolve identically.

use crate::blink;
use crate::events::{DamageCause, SimEvent, SoundCategory};
use crate::input::{LeftHand, PlayerInput, buttons};
use crate::movement::{MoveCtx, eye_position, view_vector};
use crate::state::{LeftPhase, MoveMode, PlayerState, ProjectileKind, ProjectileState, WorldState};
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

/// Per-player left-hand update. Spawned projectiles are pushed to `spawn`.
pub fn update(
    p: &mut PlayerState,
    input: &PlayerInput,
    ctx: &MoveCtx,
    cfg: &GameplayConfig,
    t: &CombatTuning,
    spawn: &mut Vec<(ProjectileKind, Vec3, Vec3)>,
    events: &mut Vec<SimEvent>,
) {
    let dt = ctx.dt;
    if !p.alive {
        return;
    }
    // Item switch.
    if input.left_hand != p.left.item {
        p.left.item = input.left_hand;
        p.left.blink_target = None;
        p.left.phase = match p.left.item {
            LeftHand::Pistol | LeftHand::Crossbow => LeftPhase::Equipping,
            _ => LeftPhase::Idle,
        };
        p.left.timer = 0.0;
    }
    p.left.timer += dt;
    let held = input.pressed(buttons::SECONDARY);
    let pressed = p.just_pressed(input, buttons::SECONDARY);
    let released = p.just_released(input, buttons::SECONDARY);
    let busy = matches!(p.mode, MoveMode::Mantling | MoveMode::Dead) || crate::combat::sword_locks_movement(p);

    match p.left.item {
        LeftHand::Blink => {
            if p.mode == MoveMode::Blinking {
                blink::update(p, false, false, ctx, &cfg.blink, &cfg.vitals, events);
            } else if !busy {
                blink::update(p, held, released, ctx, &cfg.blink, &cfg.vitals, events);
            }
        }
        LeftHand::Windblast => {
            match p.left.phase {
                LeftPhase::Recovering if p.left.timer >= t.windblast_recovery => p.left.phase = LeftPhase::Idle,
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

/// Windblast effect, applied after all players updated (reads caster state).
pub fn resolve_windblasts(
    state: &mut WorldState,
    world: &dyn WorldQuery,
    mv: &MovementConfig,
    t: &CombatTuning,
    cast_by: &[u32],
    events: &mut Vec<SimEvent>,
) {
    for &cid in cast_by {
        let Some(caster) = state.player(cid).copied() else { continue };
        let eye = eye_position(&caster, mv);
        let dir = view_vector(caster.yaw, caster.pitch);
        let half = (t.windblast_angle * 0.5).to_radians().cos();
        for v in state.players.iter_mut() {
            if v.id == cid || !v.alive {
                continue;
            }
            let to = v.position - eye;
            let d = to.length();
            if d > t.windblast_distance || d < 1e-3 || to.normalize().dot(dir) < half {
                continue;
            }
            if let Some(h) = world.raycast(eye, to, d, channel::PROJECTILE) {
                if h.distance < d - mv.collision_radius {
                    continue;
                }
            }
            let falloff = 1.0 - d / t.windblast_distance;
            let push = to.normalize() * t.windblast_force * falloff + Vec3::Z * t.windblast_up_force * falloff;
            v.velocity += push;
            if matches!(v.mode, MoveMode::Walking) {
                v.mode = MoveMode::Falling;
                v.fall_time = 0.0;
            }
            if v.sword.phase == crate::state::SwordPhase::Blocking {
                v.sword.phase = crate::state::SwordPhase::Idle;
            }
            apply_damage(v, t.windblast_damage * falloff.max(0.25), Some(cid), DamageCause::Windblast, events);
        }
        // Windblast destroys projectiles in the cone.
        state.projectiles.retain(|pr| {
            let to = pr.position - eye;
            !(to.length() < t.windblast_distance && to.normalize_or_zero().dot(dir) >= half)
        });
    }
}

/// Advance projectiles: sub-stepped segment tests against world and player bodies.
pub fn step_projectiles(
    state: &mut WorldState,
    world: &dyn WorldQuery,
    mv: &MovementConfig,
    t: &CombatTuning,
    gravity_z: f32,
    dt: f32,
    events: &mut Vec<SimEvent>,
) {
    let mut keep = Vec::with_capacity(state.projectiles.len());
    let projectiles = std::mem::take(&mut state.projectiles);
    for mut pr in projectiles {
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
            if !v.alive || v.id == pr.owner {
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
                let (mut dmg, cause) = match (pr.kind, head) {
                    (ProjectileKind::Bullet, true) => (pr.damage * t.bullet_headshot_mult, DamageCause::PistolHeadshot),
                    (ProjectileKind::Bullet, false) => (pr.damage, DamageCause::Pistol),
                    (ProjectileKind::Bolt, true) => (v.vitals.health.max(pr.damage), DamageCause::CrossbowHeadshot),
                    (ProjectileKind::Bolt, false) => (pr.damage, DamageCause::Crossbow),
                };
                if pr.kind == ProjectileKind::Bullet && dist <= t.bullet_close_range {
                    dmg *= t.bullet_close_mult;
                }
                events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: Some(v.id) });
                apply_damage(v, dmg, Some(pr.owner), cause, events);
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
    };
    ProjectileState { id, kind, owner, position: pos, velocity: vel, age: 0.0, damage, gravity_mult }
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
