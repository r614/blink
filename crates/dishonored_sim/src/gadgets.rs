//! Grenades, sticky grenades and Spring Razors, decoded from `Dishonored.exe`
//! (research/dishonored/PLAYER.md "Gadgets"):
//!
//! - grenades (`DisGrenadeComponent`, tick 0x00C5A2A0, delay 0x00C58210): a plain grenade is
//!   armed when thrown (measured with the engine hook: `m_bArmed` from its first tick, the
//!   timer counts 3.0 s down to the explosion whatever it hits, U-102); a sticky grenade by its
//!   first impact (not measured); the time to detonation (`m_fDetonationDelay` 3.0 +
//!   `Attribute_GrenadeExplosionDelay` for the player's grenades) cannot run below
//!   `m_fMinDetonationDelayAfterHit` (0.5 s) before it explodes; the timer runs at the
//!   grenade's own time dilation (Bend Time freezes it). Thrown at `m_fThrowSpeed`, raised by
//!   `m_fThrowAngle` (`DisTweaks_ThrowGrenade`), leaving the hand at `DishonoredNotify_FireProjectile`
//!   (0.300 s of `Gadgets_Grenade_Deploy`).
//! - explosion (`DisTweaks_Explosion`): `m_RadialDamage` inside `m_fFullDamageRadius`, falling
//!   to 0 at `m_fDamageRadius` (players: `m_fPlayerFullDamageRadius` / `m_fPlayerDamageRadius`),
//!   a line check to each pawn, `m_fAttachedDamage` to the pawn a sticky grenade is stuck to.
//! - Spring Razor (`DisGadget_SpringRazorPlaced`, tick 0x00C39C10): arms after `m_fArmingTime`;
//!   NPCs (the only `DisSpringRazorTargetInterface` pawns) inside the detection cylinder with a
//!   clear line trigger it; after `m_fTriggerReactionTime` it detonates (`m_fDamageReactionTime`
//!   when set off by damage), and `m_fDamageActorDelayTime` later damages everything in the
//!   detonation cylinder: NPCs are cut down, other pawns take `m_NonNPCDamage`.

use crate::events::{DamageCause, SimEvent, SoundCategory};
use crate::movement::{eye_position, half_height, view_vector};
use crate::state::{PawnKind, PlayerId, ProjectileKind, ProjectileState, TrapState, WorldState};
use crate::vitals::{apply_damage, kill};
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_ir::items::{ExplosionConfig, GrenadeConfig};
use dishonored_world::{WorldQuery, channel, geom::closest_segment_segment};
use glam::Vec3;

/// `Gadgets_Grenade_Deploy`: projectile at 0.300 s, back to idle at 0.680 s.
pub const GRENADE_FIRE: f32 = 0.300;
pub const GRENADE_EXIT: f32 = 0.680;
/// `Gadgets_SpringRazor_Deploy`: `DisNotify_PlaceSpringRazor` at 0.200 s, length 0.367 s.
pub const RAZOR_PLACE: f32 = 0.200;
pub const RAZOR_EXIT: f32 = 0.367;

pub fn grenade_cfg(cfg: &GameplayConfig, kind: ProjectileKind) -> &GrenadeConfig {
    if kind == ProjectileKind::StickyGrenade { &cfg.items.gadgets.sticky_grenade } else { &cfg.items.gadgets.grenade }
}

/// Throw velocity: the view direction pitched up by `m_fThrowAngle`, at `m_fThrowSpeed`.
pub fn throw_velocity(yaw: f32, pitch: f32, g: &GrenadeConfig) -> Vec3 {
    view_vector(yaw, (pitch + g.throw_angle).clamp(-89.0, 89.0)) * g.throw_speed
}

/// Damage of an explosion at distance `d` from a pawn (`player`: the player radii).
pub fn explosion_damage(e: &ExplosionConfig, d: f32, player: bool) -> f32 {
    let (full, outer) = if player { (e.player_full_damage_radius, e.player_damage_radius) } else { (e.full_damage_radius, e.damage_radius) };
    if d >= outer {
        0.0
    } else if d <= full {
        e.radial_damage
    } else {
        e.radial_damage * (1.0 - (d - full) / (outer - full).max(1.0))
    }
}

/// Explode at `pos`: radial damage with a line check, attached damage, momentum, and Spring
/// Razors in reach are set off by the damage.
pub fn explode(state: &mut WorldState, owner: PlayerId, pos: Vec3, stuck_to: Option<PlayerId>, kind: ProjectileKind, world: &dyn WorldQuery, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let g = grenade_cfg(cfg, kind);
    let e = &g.explosion;
    let cause = if kind == ProjectileKind::StickyGrenade { DamageCause::StickyGrenade } else { DamageCause::Grenade };
    events.push(SimEvent::Explosion { owner, position: pos, sticky: kind == ProjectileKind::StickyGrenade });
    // Port value: the explosion's AI noise caller was not decoded (stealth.md section 5.2).
    events.push(SimEvent::Sound { position: pos, radius: 4000.0, category: SoundCategory::Explosion, context: crate::events::NoiseContext::Danger, source: owner });
    let mv = &cfg.movement;
    for v in state.players.iter_mut() {
        if !v.alive && !v.is_body() {
            continue;
        }
        if crate::powers::is_hidden(v) {
            continue;
        }
        let d = v.position.distance(pos);
        let player = v.kind == PawnKind::Human;
        let mut dmg = explosion_damage(e, d, player);
        if dmg > 0.0 {
            let to = v.position - pos;
            if world.raycast(pos, to, to.length(), channel::PROJECTILE).is_some_and(|h| h.distance < to.length() - mv.collision_radius) {
                dmg = 0.0;
            }
        }
        if stuck_to == Some(v.id) {
            dmg += e.attached_damage;
        }
        if dmg <= 0.0 {
            continue;
        }
        let outer = if player { e.player_damage_radius } else { e.damage_radius };
        let k = (1.0 - d / outer.max(1.0)).clamp(0.0, 1.0);
        if v.alive && v.carried_by.is_none() && v.kind == PawnKind::Human {
            // Momentum as a velocity change on players (as for Windblast, U-051).
            let dir = (v.position - pos).normalize_or_zero();
            v.velocity += dir * e.radial_momentum * k + Vec3::Z * e.extra_up_force * k;
            if v.mode == crate::state::MoveMode::Walking && v.velocity.z > 0.0 {
                v.mode = crate::state::MoveMode::Falling;
            }
        }
        if v.alive {
            apply_damage(v, dmg.round(), Some(owner), cause, events, &cfg.vitals);
        }
    }
    let react = cfg.items.gadgets.springrazor.damage_reaction_time;
    for t in state.traps.iter_mut() {
        if !t.triggered && t.position.distance(pos) < e.damage_radius {
            t.triggered = true;
            t.detonation = react;
            events.push(SimEvent::TrapTriggered { trap: t.id });
        }
    }
}

/// Advance grenades (the other projectiles are stepped in `left_hand::step_projectiles`).
pub fn step_grenades(state: &mut WorldState, world: &dyn WorldQuery, cfg: &GameplayConfig, dt: f32, events: &mut Vec<SimEvent>) {
    let mv = &cfg.movement;
    let projectiles = std::mem::take(&mut state.projectiles);
    let mut keep = Vec::with_capacity(projectiles.len());
    let mut booms: Vec<(PlayerId, Vec3, Option<PlayerId>, ProjectileKind)> = Vec::new();
    for mut pr in projectiles {
        if !pr.kind.is_grenade() {
            keep.push(pr);
            continue;
        }
        let g = grenade_cfg(cfg, pr.kind).clone();
        pr.age += dt;
        if pr.kind == ProjectileKind::Grenade {
            arm(&mut pr, &g);
        }
        if let Some(t) = pr.stuck_to {
            match state.player(t) {
                Some(p) => pr.position = p.position + pr.stuck_offset,
                None => pr.stuck_to = None,
            }
        } else if !pr.stuck {
            let old_v = pr.velocity;
            pr.velocity.z += mv.gravity_z * g.gravity_mult * dt;
            let delta = (old_v + pr.velocity) * 0.5 * dt;
            let len = delta.length();
            let world_hit = if len > 1e-6 { world.raycast(pr.position, delta, len, channel::PROJECTILE) } else { None };
            // Pawn hit (sticky grenades stick to pawns; plain ones bounce off them).
            let mut pawn: Option<(PlayerId, f32, Vec3)> = None;
            for v in &state.players {
                if !v.alive || v.id == pr.owner || v.carried_by.is_some() || crate::powers::is_hidden(v) {
                    continue;
                }
                let hh = half_height(v, mv);
                let a0 = v.position - Vec3::Z * (hh - mv.collision_radius);
                let a1 = v.position + Vec3::Z * (hh - mv.collision_radius);
                let (on_seg, on_axis) = closest_segment_segment(pr.position, pr.position + delta, a0, a1);
                if on_seg.distance(on_axis) <= mv.collision_radius {
                    let along = (on_seg - pr.position).length();
                    if pawn.is_none_or(|(_, d, _)| along < d) {
                        pawn = Some((v.id, along, on_seg));
                    }
                }
            }
            match (pawn, world_hit) {
                (Some((vid, along, point)), wh) if wh.is_none_or(|w| along <= w.distance) => {
                    arm(&mut pr, &g);
                    if g.sticky {
                        pr.stuck_to = Some(vid);
                        pr.stuck_offset = point - state.player(vid).unwrap().position;
                        pr.position = point;
                        pr.velocity = Vec3::ZERO;
                    } else {
                        // `m_fBodyBounciness` (0.1 for the grenade).
                        pr.position = point;
                        pr.velocity = -old_v * 0.1;
                    }
                }
                (_, Some(h)) => {
                    arm(&mut pr, &g);
                    pr.position = h.point + h.normal * 2.0;
                    if g.sticky {
                        pr.stuck = true;
                        pr.velocity = Vec3::ZERO;
                    } else {
                        // Reflect, scaled by `m_fFloorBounciness` on floors, `m_fBounciness` on walls.
                        let b = if h.normal.z > mv.walkable_floor_z { g.floor_bounciness } else { g.bounciness };
                        let v = pr.velocity;
                        pr.velocity = (v - 2.0 * v.dot(h.normal) * h.normal) * b;
                        if pr.velocity.length() < 20.0 {
                            pr.velocity = Vec3::ZERO;
                            pr.stuck = true;
                        }
                    }
                }
                _ => pr.position += delta,
            }
        }
        if pr.armed {
            // 0x00C5A2A0: the delay runs down but is held at the post-impact minimum, which
            // runs down too (LIKELY reading of the two timers, U-102).
            pr.fuse -= dt;
            pr.min_fuse -= dt;
            if pr.fuse < pr.min_fuse {
                pr.fuse = pr.min_fuse;
            }
            if pr.fuse <= 0.0 {
                booms.push((pr.owner, pr.position, pr.stuck_to, pr.kind));
                continue;
            }
        }
        if pr.age < 30.0 && pr.position.z > crate::movement::KILL_Z {
            keep.push(pr);
        }
    }
    state.projectiles = keep;
    for (owner, pos, stuck, kind) in booms {
        explode(state, owner, pos, stuck, kind, world, cfg, events);
    }
}

fn arm(pr: &mut ProjectileState, g: &GrenadeConfig) {
    if !pr.armed {
        pr.armed = true;
        pr.fuse = g.fuse;
        pr.min_fuse = g.min_delay_after_hit;
    }
}

/// Place a Spring Razor where `p` aims within reach (or at its feet).
pub fn place_trap(state: &mut WorldState, owner: PlayerId, world: &dyn WorldQuery, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let Some(p) = state.player(owner).copied() else { return };
    let eye = eye_position(&p, &cfg.movement);
    let dir = view_vector(p.yaw, p.pitch);
    let reach = cfg.items.inventory.interact_distance;
    let (pos, normal) = match world.raycast(eye, dir, reach, channel::PAWN) {
        Some(h) => (h.point + h.normal * 2.0, h.normal),
        None => {
            let feet = p.position - Vec3::Z * half_height(&p, &cfg.movement);
            match world.raycast(eye + dir * reach, Vec3::NEG_Z, 1000.0, channel::PAWN) {
                Some(h) => (h.point + h.normal * 2.0, h.normal),
                None => (feet, Vec3::Z),
            }
        }
    };
    let bonus = crate::powers::attribute(&p, cfg, "Attribute_SpringRazorRadiusBonus", 0.0);
    let radius_scale = if bonus > 1.0 { bonus } else { 1.0 };
    let id = state.next_entity_id;
    state.next_entity_id = state.next_entity_id.wrapping_add(1);
    state.traps.push(TrapState { id, owner, position: pos, normal, arming: cfg.items.gadgets.springrazor.arming_time, detonation: 0.0, damage_timer: 0.0, triggered: false, detonated: false, emitted: false, radius_scale });
    events.push(SimEvent::TrapPlaced { id: owner, trap: id, position: pos });
}

/// Point inside the trap's cylinder (radius about its axis, height along its normal).
fn in_cylinder(axis_origin: Vec3, normal: Vec3, radius: f32, height: f32, p: Vec3) -> bool {
    let rel = p - axis_origin;
    let h = rel.dot(normal);
    if !(-height * 0.5..=height).contains(&h) {
        return false;
    }
    (rel - normal * h).length() <= radius
}

/// Spring Razor tick (0x00C39C10 with 0x00C2D650 detection and 0x00C36350 damage).
pub fn step_traps(state: &mut WorldState, world: &dyn WorldQuery, cfg: &GameplayConfig, dt: f32, events: &mut Vec<SimEvent>) {
    let rc = cfg.items.gadgets.springrazor.clone();
    let mv = &cfg.movement;
    let mut traps = std::mem::take(&mut state.traps);
    for t in traps.iter_mut() {
        if t.arming > 0.0 {
            t.arming -= dt;
            if t.arming <= 0.0 {
                t.arming = 0.0;
            }
            continue;
        }
        if !t.triggered {
            // Detection: NPC pawns with a sample point in the cylinder and a clear line.
            let sensor = t.position + t.normal * rc.sensor_offset;
            for v in &state.players {
                if !v.alive || v.kind == PawnKind::Human || v.carried_by.is_some() || v.incapacitated() {
                    continue;
                }
                let hh = half_height(v, mv) * 0.5;
                let pts = [v.position, v.position + Vec3::Z * hh, v.position - Vec3::Z * hh];
                let hit = pts.iter().any(|p| in_cylinder(t.position, t.normal, rc.detection_radius * t.radius_scale, rc.detection_height, *p) && world.raycast(sensor, *p - sensor, (*p - sensor).length(), channel::PROJECTILE).is_none_or(|h| h.distance >= (*p - sensor).length() - mv.collision_radius));
                if hit {
                    t.triggered = true;
                    t.detonation = rc.trigger_reaction_time;
                    events.push(SimEvent::TrapTriggered { trap: t.id });
                    break;
                }
            }
            continue;
        }
        if !t.detonated {
            t.detonation -= dt;
            if t.detonation > 0.0 {
                continue;
            }
            t.detonated = true;
            t.damage_timer = rc.damage_delay;
            // Port value (the razor's detonation noise caller was not decoded).
            events.push(SimEvent::Sound { position: t.position, radius: 1500.0, category: SoundCategory::Explosion, context: crate::events::NoiseContext::Danger, source: t.owner });
            continue;
        }
        if !t.emitted {
            t.damage_timer -= dt;
            if t.damage_timer > 0.0 {
                continue;
            }
            t.emitted = true;
            let sensor = t.position + t.normal * rc.sensor_offset;
            for v in state.players.iter_mut() {
                if !v.alive || crate::powers::is_hidden(v) {
                    continue;
                }
                let hh = half_height(v, mv) * 0.5;
                let pts = [v.position, v.position + Vec3::Z * hh, v.position - Vec3::Z * hh];
                let inside = pts.iter().any(|p| in_cylinder(t.position, t.normal, rc.detonation_radius * t.radius_scale, rc.detonation_height, *p) && p.distance(t.position) <= rc.max_damage_distance && world.raycast(sensor, *p - sensor, (*p - sensor).length(), channel::PROJECTILE).is_none_or(|h| h.distance >= (*p - sensor).length() - mv.collision_radius));
                if !inside {
                    continue;
                }
                if v.kind == PawnKind::Human {
                    apply_damage(v, rc.non_npc_damage, Some(t.owner), DamageCause::SpringRazor, events, &cfg.vitals);
                } else {
                    kill(v, Some(t.owner), DamageCause::SpringRazor, events);
                }
            }
        }
    }
    traps.retain(|t| !t.emitted);
    state.traps = traps;
}
