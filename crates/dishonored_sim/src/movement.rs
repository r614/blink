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
use crate::state::{MoveMode, PlayerState, SlideState, Traversal};
use dishonored_ir::gameplay::MovementConfig;
use dishonored_world::{Capsule, WorldQuery, channel};
use glam::{Vec2, Vec3};

/// Fixed tick rate of the simulation.
pub const TICK_HZ: u32 = 60;
pub const DT: f32 = 1.0 / TICK_HZ as f32;
/// World kill plane (`WorldInfo.KillZ` on L_Boyle_Ext_P).
pub const KILL_Z: f32 = -20000.0;

pub fn half_height(p: &PlayerState, cfg: &MovementConfig) -> f32 {
    if p.crouched { cfg.crouch_half_height } else { cfg.collision_half_height }
}

pub fn capsule(p: &PlayerState, cfg: &MovementConfig) -> Capsule {
    Capsule { center: p.position, radius: cfg.collision_radius, half_height: half_height(p, cfg) }
}

/// Eye position: UE `BaseEyeHeight` above the cylinder centre when standing; the same
/// offset from the top of the cylinder when crouched; swung sideways by the lean.
pub fn eye_position(p: &PlayerState, cfg: &MovementConfig) -> Vec3 {
    let top_gap = cfg.collision_half_height - cfg.base_eye_height;
    let mut eye = p.position + Vec3::Z * (half_height(p, cfg) - top_gap);
    if p.lean != 0.0 {
        eye += lean_offset(p, cfg);
    }
    eye
}

/// `DishonoredCamera_Lean` (0x00AC5420 / 0x00AC5950): the eye is a spring point kept at
/// `m_fCurLeanHeight = m_fLeanHeightPct x CollisionHeight x 2` from a pivot that far below
/// it, so a lean of angle `a` moves it `L sin a` sideways and `L (1 - cos a)` down, `a`
/// limited to `m_fMaxLeanAngle`. `p.lean` is the angle as a fraction of the maximum.
pub fn lean_offset(p: &PlayerState, cfg: &MovementConfig) -> Vec3 {
    let l = cfg.lean.height_pct * half_height(p, cfg) * 2.0;
    let max = if p.crouched { cfg.lean.max_angle_crouched } else { cfg.lean.max_angle };
    let a = (p.lean * max).to_radians();
    right_vector(p.yaw) * (l * a.sin()) - Vec3::Z * (l * (1.0 - a.cos()))
}

/// Lean spring (`m_LeanConstants`): `a'' = k (target - a) - c a'`, semi-implicit, in
/// units of the maximum angle (the camera's spring acts on the head point; LIKELY).
fn step_lean(p: &mut PlayerState, target: f32, ctx: &MoveCtx) {
    let lc = &ctx.cfg.lean;
    p.lean_velocity += (lc.springiness * (target - p.lean) - lc.damping * p.lean_velocity) * ctx.dt;
    p.lean = (p.lean + p.lean_velocity * ctx.dt).clamp(-1.0, 1.0);
    if target == 0.0 && p.lean.abs() < 1e-3 && p.lean_velocity.abs() < 1e-2 {
        p.lean = 0.0;
        p.lean_velocity = 0.0;
    }
    // Camera collision (`m_bHandleCameraCollision`): the eye stops short of geometry.
    // Simplified to a ray from the upright eye (SPECULATIVE margin, U-048).
    if p.lean != 0.0 {
        let off = lean_offset(p, ctx.cfg);
        let len = off.length();
        let lean = p.lean;
        p.lean = 0.0;
        let base = eye_position(p, ctx.cfg);
        p.lean = lean;
        if let Some(h) = ctx.world.raycast(base, off, len + LEAN_CAMERA_MARGIN, channel::PAWN) {
            let allowed = ((h.distance - LEAN_CAMERA_MARGIN).max(0.0) / len).min(1.0);
            p.lean *= allowed;
            p.lean_velocity = 0.0;
        }
    }
}

/// Distance kept between the leaning eye and walls (SPECULATIVE).
pub const LEAN_CAMERA_MARGIN: f32 = 10.0;

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

/// Per-tick locomotion flags, as the player controller sets them before the pawn moves
/// (`Dishonored.exe` 0x00AAE610, called from the controller tick at 0x00AAEB10):
/// - stick magnitude `m = |(forward, strafe)|` (raw axes, keyboard = 1);
/// - sprinting: sprint held, `m > m_fStopSprint_Threshold`, not sneaking (any direction);
/// - `bIsWalking`: not sprinting and `0 < m <= m_fWalk_Trigger_Threshold`
///   (`WalkingPct` = SlowWalk or Walk speed over GroundSpeed by `m_fSlowWalk_Trigger_Threshold`);
///   the WALK button stands for a half-deflected stick (NPCs and the walk binding);
/// - strafing / moving backwards (exclusive, only when not walking): the stick angle from
///   the pure sideways axis is below `m_fStrafeAngle_Forward` (stick forward) or
///   `m_fStrafeAngle_Back` (stick back); else, stick back within `m_fBackwardsAngle` of
///   straight back.
pub fn update_locomotion_flags(p: &mut PlayerState, input: &PlayerInput, cfg: &MovementConfig, can_sprint: bool) {
    let (f, r) = input.axes();
    let m = (f * f + r * r).sqrt();
    let ic = &cfg.input;
    p.sprinting = can_sprint && input.pressed(buttons::SPRINT) && m > ic.stop_sprint_threshold && !p.crouched;
    p.walking = !p.sprinting && m > 1e-4 && (input.pressed(buttons::WALK) || m <= ic.walk_threshold);
    p.strafing = false;
    p.moving_backwards = false;
    if !p.walking && m > 1e-4 {
        let inv = 1.0 / m;
        let side_angle = (r.abs() * inv).clamp(-1.0, 1.0).acos().to_degrees();
        let axis_angle = (f.abs() * inv).clamp(-1.0, 1.0).acos().to_degrees();
        let forward = f * inv >= 0.0;
        let strafe_limit = if forward { ic.strafe_angle_forward } else { ic.strafe_angle_back };
        if side_angle < strafe_limit {
            p.strafing = true;
        } else if !forward && axis_angle < ic.backwards_angle {
            p.moving_backwards = true;
        }
    }
}

/// `ADishonoredPawn::MaxSpeedModifier` (0x00B56C40): the multiplier of `GroundSpeed`
/// (`Attribute_GroundSpeed`) used by walking physics.
/// - base (UE3 `APawn::MaxSpeedModifier`): `WalkingPct` when walking, else 1;
/// - not sprinting, not sneaking: strafing -> `StrafeMultiplierRun`, backwards ->
///   `BackwardMultiplierRun`, else the base (the multipliers replace it);
/// - sneaking: strafing -> `StrafeMultiplierSneak`, else `CrouchedPct * base` with
///   `CrouchedPct = GroundSpeedCrouch / GroundSpeed` (the backward sneak multiplier is
///   unreachable: the controller never sets both flags);
/// - sprinting: `GroundSpeedSprint * (StrafeMultiplierSprint | BackwardMultiplierSprint | 1)
///   / GroundSpeed`;
/// - times the sword factor (`Attribute_SwordUnsheathedSpeedFactor` with a weapon drawn,
///   in every stance including sprint);
/// - a drop of more than 1e-4 below last tick's value is blended:
///   `last + (new - last) * clamp(dt * m_fSpeedBlendDownSpeed, 0, 1)`; rises are instant.
pub fn speed_modifier(p: &mut PlayerState, input: &PlayerInput, cfg: &MovementConfig, sword_out: bool, dt: f32) -> f32 {
    let ground = cfg.run_speed;
    let base = if p.walking {
        let (f, r) = input.axes();
        let m = (f * f + r * r).sqrt();
        let slow = !input.pressed(buttons::WALK) && m <= cfg.input.slow_walk_threshold;
        (if slow { cfg.slow_walk_speed } else { cfg.walk_speed }) / ground
    } else {
        1.0
    };
    let mut m = if p.sprinting {
        let dir = if p.strafing {
            cfg.strafe_mult_sprint
        } else if p.moving_backwards {
            cfg.back_mult_sprint
        } else {
            1.0
        };
        cfg.sprint_speed * dir / ground
    } else if p.crouched {
        if p.strafing { cfg.strafe_mult_sneak } else { cfg.crouch_speed / ground * base }
    } else if p.strafing {
        cfg.strafe_mult_run
    } else if p.moving_backwards {
        cfg.back_mult_run
    } else {
        base
    };
    m *= if sword_out { cfg.sword_unsheathed_speed_factor } else { cfg.sword_sheathed_speed_factor };
    let last = p.speed_mod;
    if m < last && last - m > 1e-4 {
        let a = (dt * cfg.speed_blend_down_speed).clamp(0.0, 1.0);
        m = (m - last) * a + last;
    }
    p.speed_mod = m;
    m
}

/// Ground max speed this tick: `GroundSpeed * MaxSpeedModifier()`.
fn max_speed(p: &mut PlayerState, input: &PlayerInput, cfg: &MovementConfig, sword_out: bool, dt: f32) -> f32 {
    cfg.run_speed * speed_modifier(p, input, cfg, sword_out, dt)
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

/// UE3 `PlayerController` `PlayerWalking.PlayerMove`: `Acceleration = AccelRate *
/// Normal(aForward * X + aStrafe * Y)`; stick deflection only selects the walk speed.
fn input_accel(p: &PlayerState, input: &PlayerInput, cfg: &MovementConfig) -> Vec3 {
    let (f, r) = input.axes();
    let dir = forward_vector(p.yaw) * f + right_vector(p.yaw) * r;
    dir.normalize_or_zero() * cfg.accel_rate
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

    // `Dis_SneakOrSlide`: sneak toggle (keeps feet planted; standing up requires
    // headroom), or a slide when sprinting at `m_fMinSpeedToSlide` or faster.
    if p.just_pressed(input, buttons::CROUCH) && p.mode == MoveMode::Walking && !p.slide.active {
        if p.sprinting && can_move && p.velocity.truncate().length() >= ctx.cfg.input.min_speed_to_slide {
            start_slide(p, sword_out, ctx);
        } else {
            set_crouch(p, !p.crouched, ctx);
        }
    }

    // Lean only while standing still on the ground (moving breaks the lean).
    let lean_target = if p.mode == MoveMode::Walking && p.velocity.truncate().length() < 10.0 {
        (input.pressed(buttons::LEAN_RIGHT) as i32 - input.pressed(buttons::LEAN_LEFT) as i32) as f32
    } else {
        0.0
    };
    step_lean(p, lean_target, ctx);

    let can_sprint = can_move && p.mode == MoveMode::Walking && p.sword.phase != crate::state::SwordPhase::Blocking;
    update_locomotion_flags(p, input, ctx.cfg, can_sprint);

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

/// Slide start (`StatePlayerMasterSlide` enter, 0x00A67FA0): the slide eases the
/// horizontal velocity from its value at the start to the same direction at
/// `Attribute_GroundSpeedCrouch` (times the sword factor) over `m_fSlideTime`, sneaking.
fn start_slide(p: &mut PlayerState, sword_out: bool, ctx: &MoveCtx) {
    let cfg = ctx.cfg;
    let v0 = Vec3::new(p.velocity.x, p.velocity.y, 0.0);
    let sword = if sword_out { cfg.sword_unsheathed_speed_factor } else { cfg.sword_sheathed_speed_factor };
    let target = v0.normalize_or_zero() * cfg.crouch_speed * sword;
    set_crouch(p, true, ctx);
    p.sprinting = false;
    p.slide = SlideState { active: true, elapsed: 0.0, start_velocity: v0, target_velocity: target, stop_interpolating: false };
}

/// Slide physics (0x00A6F440): `v = v0 + (target - v0) * (1 - cos(pi * t / SlideTime)) / 2`
/// until the speed drops below the target speed (then the velocity is left alone); the
/// slide ends when the velocity turns more than `m_fSlideVelocityDeactivateAngle` from
/// its start direction, or after `m_fSlideTime` (0x00A7B220).
fn slide(p: &mut PlayerState, ctx: &MoveCtx) {
    let cfg = ctx.cfg;
    let s = &mut p.slide;
    s.elapsed += ctx.dt;
    if s.elapsed > cfg.slide_time {
        s.active = false;
        return;
    }
    if !s.stop_interpolating {
        let t = (s.elapsed / cfg.slide_time).clamp(0.0, 1.0);
        let e = (1.0 - (t * std::f32::consts::PI).cos()) * 0.5;
        let v = s.start_velocity + (s.target_velocity - s.start_velocity) * e;
        p.velocity.x = v.x;
        p.velocity.y = v.y;
    }
    p.velocity.z = 0.0;
    let delta = Vec3::new(p.velocity.x, p.velocity.y, 0.0) * ctx.dt;
    move_and_slide(p, delta, true, ctx);
    let s = &mut p.slide;
    let cur = p.velocity.truncate();
    let start_dir = s.start_velocity.truncate().normalize_or_zero();
    if cur.normalize_or_zero().dot(start_dir) < cfg.slide_deactivate_angle.to_radians().cos() {
        s.active = false;
    } else if cur.length() < s.target_velocity.truncate().length() {
        s.stop_interpolating = true;
    }
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
    if p.slide.active {
        slide(p, ctx);
        // The speed modifier keeps tracking so the exit blends down from the slide.
        p.speed_mod = p.velocity.truncate().length() / cfg.run_speed;
    } else {
        let accel = if can_move { input_accel(p, input, cfg) } else { Vec3::ZERO };
        let vmax = max_speed(p, input, cfg, sword_out, ctx.dt);
        p.velocity = calc_velocity(p.velocity, accel, vmax, cfg.ground_friction, ctx.dt);
        p.velocity.z = 0.0;
        let delta = Vec3::new(p.velocity.x, p.velocity.y, 0.0) * ctx.dt;
        move_and_slide(p, delta, true, ctx);
    }

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
    // Leaving the ground ends a slide (the slide physics exits on a fall, 0x00A6F440).
    p.slide.active = false;

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

/// Ledge detection and mantle start for the player's own mantle (jump into a ledge, or
/// grab one while airborne). Thresholds are the `pMantleTweaks` values; falling faster
/// than `m_fFallSpeedForLedgeGrab` only grabs edges at least
/// `m_fLedgeGrabMantleMinEdgeHeight` high, and nothing faster than
/// `m_fMaxFallSpeedForMantle`.
pub fn try_mantle(p: &mut PlayerState, input: &PlayerInput, ctx: &MoveCtx, events: &mut Vec<SimEvent>) -> bool {
    let m = &ctx.cfg.mantle;
    if input.forward <= 0 {
        return false;
    }
    let fall = -p.velocity.z;
    if p.mode == MoveMode::Falling && fall >= m.max_fall_speed_for_mantle {
        return false;
    }
    let min_edge = if p.mode == MoveMode::Falling && fall > m.fall_speed_for_ledge_grab { m.ledge_grab_min_edge_height } else { m.min_edge_height };
    try_mantle_from(p, forward_vector(p.yaw), min_edge, ctx, events)
}

/// The mantle the original tries when a Blink ends (`ADishonoredPlayerPawn` 0x00AD78D0
/// called from 0x00BE7420): the Blink edge finder uses `pMantleBlinkTweaks`, which leave
/// `m_MantleMinEdgeHeight` at its class default 0 and match the normal tweaks otherwise.
pub fn try_blink_mantle(p: &mut PlayerState, ctx: &MoveCtx, events: &mut Vec<SimEvent>) -> bool {
    let dir = forward_vector(p.yaw);
    try_mantle_from(p, dir, 0.0, ctx, events)
}

/// Edge search in the spirit of `DisMantleFinderState_ForwardSearches` (0x00ADA6F0 /
/// 0x00AD6600): horizontal traces `m_MantleEdgeSearchDist` beyond the cylinder, every
/// `m_LineCheckStepSize` from the lowest edge height up to `m_MantleMaxEdgeHeight`; the
/// wall must be within `m_MaxVerticalAngleForEdgeFace` of vertical and
/// `m_MaxHorizontalAngleForEdgeFace` of the facing; the first clear trace above it marks
/// the edge, whose top (slope within `m_MaxSlopeAngleForEdgeTop`) is found straight down.
/// The finder's later states (destination valid, can stand, `m_FinalVerifyLocationGridSize`
/// grid) are reduced to a capsule clearance test, standing then sneaking.
pub fn try_mantle_from(p: &mut PlayerState, fwd: Vec3, min_edge: f32, ctx: &MoveCtx, events: &mut Vec<SimEvent>) -> bool {
    let cfg = ctx.cfg;
    let m = &cfg.mantle;
    let base = feet(p, cfg);
    let r = cfg.collision_radius;
    let reach = r + m.edge_search_dist;
    let step = m.line_check_step_size.max(1.0);
    let max_vert = m.max_vertical_angle_edge_face.to_radians().sin();
    let min_facing = m.max_horizontal_angle_edge_face.to_radians().cos();
    let mut wall_dist = None;
    let mut edge_clear_h = None;
    let mut h = min_edge.max(step);
    while h <= m.max_edge_height + step {
        match ctx.world.raycast(base + Vec3::Z * h, fwd, reach, channel::PAWN) {
            Some(hit) => {
                let face_ok = hit.normal.z.abs() <= max_vert && (-hit.normal.truncate().normalize_or_zero()).dot(fwd.truncate()) >= min_facing;
                if !face_ok {
                    return false;
                }
                wall_dist = Some(hit.distance);
            }
            None if wall_dist.is_some() => {
                edge_clear_h = Some(h);
                break;
            }
            None => {}
        }
        h += step;
    }
    let (Some(wall_dist), Some(clear_h)) = (wall_dist, edge_clear_h) else {
        return false;
    };
    // Top of the edge, just beyond the wall face.
    let probe_xy = base + fwd * (wall_dist + r * 0.5 + 4.0);
    let top_start = probe_xy + Vec3::Z * clear_h;
    let Some(top) = ctx.world.raycast(top_start, -Vec3::Z, step + 1.0, channel::PAWN) else {
        return false;
    };
    let max_slope_z = m.max_slope_angle_edge_top.to_radians().cos();
    if top.normal.z < max_slope_z {
        return false;
    }
    let edge_h = top.point.z - base.z;
    if edge_h < min_edge || edge_h > m.max_edge_height {
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
