//! Blink (M8). Sustained cast: hold to aim (target updates every tick), release to travel.
//!
//! CONFIRMED data (Twk_Blink / DishonoredActivePowerComponent_Blink): per-level
//! horizontal and vertical ranges, 20 mana, target test extent 20^3, close-collision
//! distance 300 with 30 uu offset steps, ledge check scale 1.2.
//! LIKELY algorithm (pending original traces, U-010):
//! 1. Sweep a small box (modelled as a sphere of the test extent) from the eye along the
//!    view, clamped to the horizontal/vertical range envelope.
//! 2. Floor hit -> stand there. Wall hit -> look for a ledge top within mantle reach
//!    above the impact (scaled by the ledge-check factor) and land on it; otherwise land
//!    in front of the wall (and fall).
//! 3. Destination must fit the standing (or sneaking) capsule; if not, step back along
//!    the aim in `close_collision_offset_step` increments.
//! Travel is a fast linear move (`BLINK_TRAVEL_SPEED`, SPECULATIVE) that keeps momentum
//! cleared and ends in Falling so gravity resolves short drops.

use crate::events::{SimEvent, SoundCategory};
use crate::movement::{MoveCtx, eye_position, view_vector};
use crate::state::{MoveMode, PlayerState, Traversal};
use crate::vitals::spend_mana;
use dishonored_ir::gameplay::{BlinkConfig, MovementConfig, VitalsConfig};
use dishonored_world::{Capsule, WorldQuery, channel};
use glam::Vec3;

/// Uu/s during Blink travel (SPECULATIVE; the original is a near-instant dash).
pub const BLINK_TRAVEL_SPEED: f32 = 9000.0;
pub const BLINK_LEVEL: usize = 1; // Contracts/FFA give level 2 Blink (index 1).

/// Compute the Blink destination (capsule centre) for the current view, or None.
pub fn compute_target(p: &PlayerState, world: &dyn WorldQuery, mv: &MovementConfig, cfg: &BlinkConfig) -> Option<Vec3> {
    let level = BLINK_LEVEL.min(cfg.horiz_distance.len() - 1);
    let horiz = cfg.horiz_distance[level];
    let vert = cfg.vert_distance[level];
    let eye = eye_position(p, mv);
    let dir = view_vector(p.yaw, p.pitch);
    // Range envelope: limited separately in the horizontal plane and vertically.
    let h_len = dir.truncate().length();
    let mut range = f32::MAX;
    if h_len > 1e-4 {
        range = range.min(horiz / h_len);
    }
    if dir.z.abs() > 1e-4 {
        range = range.min(vert / dir.z.abs());
    }
    let probe = Capsule { center: eye, radius: cfg.target_test_extent[0], half_height: cfg.target_test_extent[2] };
    let hit = world.sweep_capsule(&probe, dir * range, channel::PAWN);
    let r = mv.collision_radius;
    let hh = mv.collision_half_height;

    let (impact, normal) = match hit {
        Some(h) if !h.start_penetrating => (eye + dir * range * h.fraction, Some(h.normal)),
        Some(_) => return None,
        None => (eye + dir * range, None),
    };

    let mut candidates: Vec<Vec3> = Vec::new();
    match normal {
        Some(n) if n.z >= mv.walkable_floor_z => {
            // Floor: stand on it.
            candidates.push(impact + Vec3::Z * (hh - probe.half_height + 1.0));
        }
        Some(n) => {
            // Wall or ceiling: try the ledge above first (Blink auto-mantle).
            let into = -Vec3::new(n.x, n.y, 0.0).normalize_or_zero();
            if into != Vec3::ZERO {
                let reach = mv.mantle.max_edge_height * cfg.ledge_check_distance_scale;
                let probe_xy = impact + into * (r + 2.0);
                if let Some(top) = world.raycast(probe_xy + Vec3::Z * reach, -Vec3::Z, reach + hh, channel::PAWN) {
                    if top.normal.z >= mv.walkable_floor_z && top.point.z > impact.z - hh {
                        candidates.push(top.point + into * 10.0 + Vec3::Z * (hh + 1.0));
                    }
                }
            }
            // Otherwise in front of the wall.
            let out = Vec3::new(n.x, n.y, n.z.min(0.0)).normalize_or(-dir);
            candidates.push(impact + out * (r + 1.0));
        }
        None => {
            // Open air: end of range, drop to the floor below if near.
            candidates.push(impact);
        }
    }

    for c in candidates {
        if let Some(ok) = fit_with_backoff(c, eye, dir, world, mv, cfg) {
            return Some(ok);
        }
    }
    None
}

fn fits(center: Vec3, world: &dyn WorldQuery, mv: &MovementConfig) -> bool {
    let cap = Capsule { center, radius: mv.collision_radius, half_height: mv.collision_half_height };
    let mut contacts = Vec::new();
    world.capsule_contacts(&cap, channel::PAWN, &mut contacts);
    contacts.iter().all(|c| c.depth < 1.0)
}

fn fit_with_backoff(
    mut c: Vec3,
    eye: Vec3,
    dir: Vec3,
    world: &dyn WorldQuery,
    mv: &MovementConfig,
    cfg: &BlinkConfig,
) -> Option<Vec3> {
    let steps = (cfg.close_collision_distance / cfg.close_collision_offset_step).ceil() as usize;
    for _ in 0..=steps {
        if fits(c, world, mv) {
            // Line of travel must be clear for the player's body (not just the probe).
            return Some(c);
        }
        // Try lifting slightly (uneven floors) before backing off.
        for lift in [10.0, 25.0] {
            if fits(c + Vec3::Z * lift, world, mv) {
                return Some(c + Vec3::Z * lift);
            }
        }
        c -= dir * cfg.close_collision_offset_step;
        if (c - eye).dot(dir) < 0.0 {
            break;
        }
    }
    None
}

/// Per-tick Blink handling from the left hand. `aiming` = button held this tick.
pub fn update(
    p: &mut PlayerState,
    aiming: bool,
    released: bool,
    ctx: &MoveCtx,
    cfg: &BlinkConfig,
    vitals: &VitalsConfig,
    events: &mut Vec<SimEvent>,
) {
    if p.mode == MoveMode::Blinking {
        if crate::movement::advance_traversal(p, ctx.dt) {
            p.mode = MoveMode::Falling;
            p.velocity = Vec3::ZERO;
            events.push(SimEvent::BlinkEnd { id: p.id });
        }
        return;
    }
    if aiming {
        p.left.blink_target = compute_target(p, ctx.world, ctx.cfg, cfg);
    }
    if released {
        let target = p.left.blink_target.take();
        let can = !matches!(p.mode, MoveMode::Mantling | MoveMode::Dead);
        match target {
            Some(to) if can && spend_mana(p, cfg.mana_cost, vitals) => {
                let from = p.position;
                // Travel the full capsule path; stop early if the body would collide.
                let cap = Capsule { center: from, radius: ctx.cfg.collision_radius, half_height: crate::movement::half_height(p, ctx.cfg) };
                let to = match ctx.world.sweep_capsule(&cap, to - from, channel::PAWN) {
                    Some(h) if !h.start_penetrating && h.fraction < 1.0 => {
                        // The probe found the target; let the body stop at first contact
                        // only if that is not the destination itself (ledge case).
                        if from.distance(to) * (1.0 - h.fraction) < ctx.cfg.collision_radius * 2.0 { to } else { from + (to - from) * h.fraction }
                    }
                    _ => to,
                };
                if p.crouched {
                    p.crouched = false;
                }
                let duration = (from.distance(to) / BLINK_TRAVEL_SPEED).max(ctx.dt);
                p.traversal = Traversal { from, to, elapsed: 0.0, duration };
                p.mode = MoveMode::Blinking;
                p.velocity = Vec3::ZERO;
                events.push(SimEvent::BlinkStart { id: p.id, from, to });
                events.push(SimEvent::Sound { position: from, radius: 400.0, category: SoundCategory::Blink, source: p.id });
            }
            _ => events.push(SimEvent::BlinkFizzle { id: p.id }),
        }
    }
}
