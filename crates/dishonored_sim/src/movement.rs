//! Player locomotion (M7): a port of UE3 `PHYS_Walking` / `PHYS_Falling` semantics
//! driven by Dishonored's tuning values.
//!
//! Evidence levels (see research/dishonored/movement/MOVEMENT.md):
//! - constants: CONFIRMED (tweaks / class defaults / INI, see `GameplayConfig::provenance`)
//! - integration scheme, friction/braking model, step-up and floor snapping: LIKELY (stock
//!   UE3 behaviour; to be validated with traces from the original game, U-011)
//! - mantle timing and probe pattern: SPECULATIVE until measured (U-019)

use crate::events::SimEvent;
use crate::input::{PlayerInput, buttons};
use crate::state::{MoveMode, PlayerState, Traversal};
use dishonored_ir::gameplay::MovementConfig;
use dishonored_world::{Capsule, WorldQuery, channel};
use glam::{Vec2, Vec3};

/// Fixed tick rate of the simulation.
pub const TICK_HZ: u32 = 60;
pub const DT: f32 = 1.0 / TICK_HZ as f32;
/// World kill plane (`WorldInfo.KillZ` on L_Boyle_Ext_P).
pub const KILL_Z: f32 = -20000.0;
/// Camera lean offset at full lean (SPECULATIVE; not yet found in tweaks).
pub const LEAN_DISTANCE: f32 = 40.0;

pub fn half_height(p: &PlayerState, cfg: &MovementConfig) -> f32 {
    if p.crouched { cfg.crouch_half_height } else { cfg.collision_half_height }
}

pub fn capsule(p: &PlayerState, cfg: &MovementConfig) -> Capsule {
    Capsule { center: p.position, radius: cfg.collision_radius, half_height: half_height(p, cfg) }
}

/// Eye position: UE `BaseEyeHeight` above the cylinder centre when standing; the same
/// offset from the top of the cylinder when crouched.
pub fn eye_position(p: &PlayerState, cfg: &MovementConfig) -> Vec3 {
    let top_gap = cfg.collision_half_height - cfg.base_eye_height;
    let mut eye = p.position + Vec3::Z * (half_height(p, cfg) - top_gap);
    if p.lean != 0.0 {
        eye += right_vector(p.yaw) * p.lean * LEAN_DISTANCE;
    }
    eye
}

pub fn forward_vector(yaw: f32) -> Vec3 {
    let (s, c) = yaw.to_radians().sin_cos();
    Vec3::new(c, s, 0.0)
}

pub fn right_vector(yaw: f32) -> Vec3 {
    let (s, c) = yaw.to_radians().sin_cos();
    Vec3::new(-s, c, 0.0)
}

pub fn view_vector(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    Vec3::new(cp * cy, cp * sy, sp)
}

pub fn feet(p: &PlayerState, cfg: &MovementConfig) -> Vec3 {
    p.position - Vec3::Z * half_height(p, cfg)
}

/// Directional max speed from Dishonored's stance speeds and strafe/backward multipliers.
fn max_speed(p: &PlayerState, input: &PlayerInput, cfg: &MovementConfig, sword_out: bool) -> f32 {
    let (f, r) = input.axes();
    let sprinting = input.pressed(buttons::SPRINT) && f > 0.1 && !p.crouched;
    let (base, strafe, back) = if p.crouched {
        (cfg.crouch_speed, cfg.strafe_mult_sneak, cfg.back_mult_sneak)
    } else if sprinting {
        (cfg.sprint_speed, cfg.strafe_mult_sprint, cfg.back_mult_sprint)
    } else if input.pressed(buttons::WALK) {
        (cfg.walk_speed, cfg.strafe_mult_run, cfg.back_mult_run)
    } else {
        (cfg.run_speed, cfg.strafe_mult_run, cfg.back_mult_run)
    };
    let (af, ar) = (f.abs(), r.abs());
    let dir_mult = if af + ar < 1e-4 {
        1.0
    } else {
        let fm = if f >= 0.0 { 1.0 } else { back };
        (af * fm + ar * strafe) / (af + ar)
    };
    let sword = if sword_out && !sprinting { cfg.sword_unsheathed_speed_factor } else { 1.0 };
    base * dir_mult * sword
}

/// UE3 `APawn::CalcVelocity` (LIKELY): braking when there is no acceleration,
/// otherwise turn velocity toward the acceleration direction with friction.
fn calc_velocity(vel: Vec3, accel: Vec3, max_speed: f32, friction: f32, dt: f32) -> Vec3 {
    let mut v = Vec2::new(vel.x, vel.y);
    let a = Vec2::new(accel.x, accel.y);
    if a.length_squared() < 1e-6 {
        let old = v;
        v -= v * (2.0 * friction * dt).min(1.0);
        if v.dot(old) <= 0.0 || v.length() < 1.0 {
            v = Vec2::ZERO;
        }
    } else {
        let dir = a.normalize();
        let speed = v.length();
        v -= (v - dir * speed) * (dt * friction).min(1.0);
        v += a * dt;
        if v.length() > max_speed {
            v = v.normalize() * max_speed;
        }
    }
    Vec3::new(v.x, v.y, vel.z)
}

fn input_accel(p: &PlayerState, input: &PlayerInput, cfg: &MovementConfig) -> Vec3 {
    let (f, r) = input.axes();
    let mut dir = forward_vector(p.yaw) * f + right_vector(p.yaw) * r;
    if dir.length() > 1.0 {
        dir = dir.normalize();
    }
    dir * cfg.accel_rate
}

pub struct MoveCtx<'a> {
    pub world: &'a dyn WorldQuery,
    pub cfg: &'a MovementConfig,
    pub dt: f32,
}

/// Step one player's locomotion by one tick.
pub fn step_movement(
    p: &mut PlayerState,
    input: &PlayerInput,
    sword_out: bool,
    can_move: bool,
    ctx: &MoveCtx,
    events: &mut Vec<SimEvent>,
) {
    p.yaw = input.yaw;
    p.pitch = input.pitch.clamp(-89.0, 89.0);
    if p.mode == MoveMode::Dead {
        return;
    }

    // Sneak toggle (keeps feet planted; standing up requires headroom).
    if p.just_pressed(input, buttons::CROUCH) && p.mode == MoveMode::Walking {
        set_crouch(p, !p.crouched, ctx);
    }

    // Lean only while standing still on the ground.
    let lean_target = if p.mode == MoveMode::Walking && p.velocity.truncate().length() < 10.0 {
        (input.pressed(buttons::LEAN_RIGHT) as i32 - input.pressed(buttons::LEAN_LEFT) as i32) as f32
    } else {
        0.0
    };
    p.lean += (lean_target - p.lean).clamp(-ctx.dt * 5.0, ctx.dt * 5.0);

    match p.mode {
        MoveMode::Walking => walking(p, input, sword_out, can_move, ctx, events),
        MoveMode::Falling => falling(p, input, can_move, ctx, events),
        MoveMode::Mantling => {
            if advance_traversal(p, ctx.dt) {
                p.mode = MoveMode::Walking;
                p.velocity = Vec3::ZERO;
            }
        }
        MoveMode::Blinking | MoveMode::Dead => {}
    }

    if p.position.z < KILL_Z && p.alive {
        events.push(SimEvent::FellOutOfWorld { id: p.id });
    }
}

/// Linear traversal used by mantle and Blink. Returns true when finished.
pub fn advance_traversal(p: &mut PlayerState, dt: f32) -> bool {
    let t = &mut p.traversal;
    t.elapsed += dt;
    let a = if t.duration <= 0.0 { 1.0 } else { (t.elapsed / t.duration).min(1.0) };
    p.position = t.from.lerp(t.to, a);
    a >= 1.0
}

fn set_crouch(p: &mut PlayerState, crouch: bool, ctx: &MoveCtx) {
    let cfg = ctx.cfg;
    if crouch == p.crouched {
        return;
    }
    let old_hh = half_height(p, cfg);
    let new_hh = if crouch { cfg.crouch_half_height } else { cfg.collision_half_height };
    let new_center = p.position + Vec3::Z * (new_hh - old_hh);
    if !crouch {
        let cap = Capsule { center: new_center, radius: cfg.collision_radius, half_height: new_hh };
        let mut contacts = Vec::new();
        ctx.world.capsule_contacts(&cap, channel::PAWN, &mut contacts);
        if contacts.iter().any(|c| c.depth > 0.5) {
            return; // no headroom
        }
    }
    p.crouched = crouch;
    p.position = new_center;
}

fn walking(
    p: &mut PlayerState,
    input: &PlayerInput,
    sword_out: bool,
    can_move: bool,
    ctx: &MoveCtx,
    events: &mut Vec<SimEvent>,
) {
    let cfg = ctx.cfg;
    if can_move && p.just_pressed(input, buttons::JUMP) {
        if try_mantle(p, input, ctx, events) {
            return;
        }
        if p.crouched {
            set_crouch(p, false, ctx);
        }
        if !p.crouched {
            p.velocity.z = cfg.jump_z;
            p.mode = MoveMode::Falling;
            p.fall_time = 0.0;
            events.push(SimEvent::Jumped { id: p.id });
            falling(p, input, can_move, ctx, events);
            return;
        }
    }
    let accel = if can_move { input_accel(p, input, cfg) } else { Vec3::ZERO };
    let vmax = max_speed(p, input, cfg, sword_out);
    p.velocity = calc_velocity(p.velocity, accel, vmax, cfg.ground_friction, ctx.dt);
    p.velocity.z = 0.0;
    let delta = Vec3::new(p.velocity.x, p.velocity.y, 0.0) * ctx.dt;
    move_and_slide(p, delta, true, ctx);

    // Stay glued to the floor within step height, otherwise start falling.
    let cap = capsule(p, cfg);
    let probe = cfg.max_step_height + 2.0;
    match ctx.world.sweep_capsule(&cap, Vec3::Z * -probe, channel::PAWN) {
        Some(h) if !h.start_penetrating && h.face_up.z >= cfg.walkable_floor_z => {
            p.position.z -= probe * h.fraction;
            p.floor_normal = h.face_up;
        }
        Some(h) if h.start_penetrating && h.face_up.z >= cfg.walkable_floor_z => {
            p.floor_normal = h.face_up;
        }
        _ => {
            p.mode = MoveMode::Falling;
            p.fall_time = 0.0;
        }
    }
}

fn falling(p: &mut PlayerState, input: &PlayerInput, can_move: bool, ctx: &MoveCtx, events: &mut Vec<SimEvent>) {
    let cfg = ctx.cfg;
    let dt = ctx.dt;
    p.fall_time += dt;

    // Auto-mantle while airborne and pushing forward (Dishonored grabs ledges mid-jump).
    if can_move && input.forward > 0 && -p.velocity.z < cfg.mantle.max_fall_speed_for_mantle {
        if try_mantle(p, input, ctx, events) {
            return;
        }
    }

    // Air control: a fraction of ground acceleration, never exceeding the larger of the
    // current horizontal speed and run speed.
    let old = p.velocity;
    let accel = if can_move { input_accel(p, input, cfg) * cfg.air_control } else { Vec3::ZERO };
    let mut vh = Vec2::new(old.x, old.y) + Vec2::new(accel.x, accel.y) * dt;
    let cap_speed = Vec2::new(old.x, old.y).length().max(cfg.run_speed);
    if vh.length() > cap_speed {
        vh = vh.normalize() * cap_speed;
    }
    let vz = (old.z + cfg.gravity_z * dt).max(-cfg.terminal_velocity);
    p.velocity = Vec3::new(vh.x, vh.y, vz);
    // Trapezoidal integration (exact apex for constant gravity).
    let delta = (old + p.velocity) * 0.5 * dt;
    let impact_speed = -p.velocity.z;
    if let Some((_, normal)) = move_and_slide(p, delta, false, ctx) {
        if normal.z >= cfg.walkable_floor_z && p.velocity.z <= 0.0 {
            land(p, impact_speed, normal, events);
        }
    }
}

fn land(p: &mut PlayerState, impact_speed: f32, normal: Vec3, events: &mut Vec<SimEvent>) {
    p.mode = MoveMode::Walking;
    p.velocity.z = 0.0;
    p.floor_normal = normal;
    events.push(SimEvent::Landed { id: p.id, speed: impact_speed, fall_time: p.fall_time });
    p.fall_time = 0.0;
}

/// Sweep-and-slide up to 4 iterations. Returns the last blocking (contact normal, face_up).
fn move_and_slide(p: &mut PlayerState, delta: Vec3, walking: bool, ctx: &MoveCtx) -> Option<(Vec3, Vec3)> {
    let cfg = ctx.cfg;
    let mut remaining = delta;
    let mut last_normal = None;
    for _ in 0..4 {
        if remaining.length_squared() < 1e-8 {
            break;
        }
        let cap = capsule(p, cfg);
        let Some(hit) = ctx.world.sweep_capsule(&cap, remaining, channel::PAWN) else {
            p.position += remaining;
            break;
        };
        if hit.start_penetrating {
            p.position = depenetrate(p, ctx);
            last_normal = Some((hit.normal, hit.face_up));
            continue;
        }
        p.position += remaining * hit.fraction;
        let left = remaining * (1.0 - hit.fraction);
        let mut n = hit.normal;
        last_normal = Some((n, hit.face_up));
        if walking && n.z < cfg.walkable_floor_z {
            if try_step_up(p, left, ctx) {
                remaining = Vec3::ZERO;
                continue;
            }
            // Walls are treated as vertical while walking so slides never climb.
            n = Vec3::new(n.x, n.y, 0.0).normalize_or(n);
        }
        remaining = left - n * left.dot(n);
        let vn = p.velocity.dot(n);
        if vn < 0.0 {
            p.velocity -= n * vn;
        }
    }
    last_normal
}

fn depenetrate(p: &PlayerState, ctx: &MoveCtx) -> Vec3 {
    let cap = capsule(p, ctx.cfg);
    // CollisionWorld implements depenetration natively; other WorldQuery impls get a
    // contact-based push.
    let mut c = cap;
    let mut contacts = Vec::new();
    for _ in 0..6 {
        contacts.clear();
        ctx.world.capsule_contacts(&c, channel::PAWN, &mut contacts);
        let Some(k) = contacts.iter().max_by(|a, b| a.depth.total_cmp(&b.depth)) else { break };
        if k.depth < 0.05 {
            break;
        }
        c.center += k.normal * (k.depth + 0.05);
    }
    c.center
}

/// UE3-style step up: lift by MaxStepHeight, move, drop back onto a walkable floor.
fn try_step_up(p: &mut PlayerState, delta: Vec3, ctx: &MoveCtx) -> bool {
    let cfg = ctx.cfg;
    let horiz = Vec3::new(delta.x, delta.y, 0.0);
    if horiz.length() < 0.01 {
        return false;
    }
    let start = p.position;
    let cap = capsule(p, cfg);
    let up = Vec3::Z * cfg.max_step_height;
    let up_frac = ctx.world.sweep_capsule(&cap, up, channel::PAWN).map_or(1.0, |h| h.fraction);
    let lifted = start + up * up_frac;
    let fwd = ctx.world.sweep_capsule(&cap.at(lifted), horiz, channel::PAWN);
    let fwd_frac = fwd.map_or(1.0, |h| h.fraction);
    if fwd_frac < 0.05 {
        return false;
    }
    let moved = lifted + horiz * fwd_frac;
    let down = Vec3::Z * -(cfg.max_step_height * up_frac + 2.0);
    match ctx.world.sweep_capsule(&cap.at(moved), down, channel::PAWN) {
        Some(h) if !h.start_penetrating && h.face_up.z >= cfg.walkable_floor_z => {
            let landed = moved + down * h.fraction;
            let start_feet = start.z - half_height(p, cfg);
            // Cylinder semantics: the supporting point may not be higher than the step.
            if landed.z - start.z > cfg.max_step_height + 0.5 || h.point.z - start_feet > cfg.max_step_height + 0.5 {
                return false;
            }
            p.position = landed;
            true
        }
        _ => false,
    }
}

/// Ledge detection and mantle start (SPECULATIVE probe pattern, CONFIRMED thresholds).
pub fn try_mantle(p: &mut PlayerState, input: &PlayerInput, ctx: &MoveCtx, events: &mut Vec<SimEvent>) -> bool {
    let cfg = ctx.cfg;
    let m = &cfg.mantle;
    if input.forward <= 0 {
        return false;
    }
    let fwd = forward_vector(p.yaw);
    let base = feet(p, cfg);
    let r = cfg.collision_radius;
    // There must be something in front of us at chest height.
    let chest = base + Vec3::Z * (m.min_edge_height * 0.5 + 10.0);
    let reach = r + m.edge_search_dist;
    let wall = ctx.world.raycast(chest, fwd, reach, channel::PAWN);
    let wall_dist = match wall {
        Some(h) if h.normal.z.abs() < 0.7 => h.distance,
        _ => {
            // Also allow when the wall only starts above knee height (e.g. a ledge while airborne).
            match ctx.world.raycast(base + Vec3::Z * m.min_edge_height, fwd, reach, channel::PAWN) {
                Some(h) if h.normal.z.abs() < 0.7 => h.distance,
                _ => return false,
            }
        }
    };
    // Probe downward just beyond the wall face for a top surface.
    let probe_xy = base + fwd * (wall_dist + r * 0.5 + 4.0);
    let top_start = probe_xy + Vec3::Z * (m.max_edge_height + 5.0);
    let Some(top) = ctx.world.raycast(top_start, -Vec3::Z, m.max_edge_height - m.min_edge_height + 5.0, channel::PAWN)
    else {
        return false;
    };
    let max_slope_z = m.max_slope_angle_edge_top.to_radians().cos();
    if top.normal.z < max_slope_z {
        return false;
    }
    let edge_h = top.point.z - base.z;
    if edge_h < m.min_edge_height || edge_h > m.max_edge_height {
        return false;
    }
    // Standing clearance on top (crouched clearance is accepted too).
    let dest_xy = top.point + fwd * m.forward_move_amount;
    let stand = Capsule { center: dest_xy + Vec3::Z * (cfg.collision_half_height + 1.0), radius: r, half_height: cfg.collision_half_height };
    let mut contacts = Vec::new();
    ctx.world.capsule_contacts(&stand, channel::PAWN, &mut contacts);
    let (dest, crouch) = if contacts.iter().all(|c| c.depth < 0.5) {
        (stand.center, false)
    } else {
        let low = Capsule { center: dest_xy + Vec3::Z * (cfg.crouch_half_height + 1.0), half_height: cfg.crouch_half_height, ..stand };
        contacts.clear();
        ctx.world.capsule_contacts(&low, channel::PAWN, &mut contacts);
        if contacts.iter().any(|c| c.depth >= 0.5) {
            return false;
        }
        (low.center, true)
    };
    if crouch && !p.crouched {
        p.crouched = true;
    }
    // Duration grows with edge height; anim-driven in the original (U-019).
    let duration = if edge_h <= m.low_max_edge_height { 0.35 } else if edge_h <= m.medium_max_edge_height { 0.55 } else { 0.75 };
    p.traversal = Traversal { from: p.position, to: dest, elapsed: 0.0, duration };
    p.mode = MoveMode::Mantling;
    p.velocity = Vec3::ZERO;
    events.push(SimEvent::Mantle { id: p.id, edge_height: edge_h });
    true
}
