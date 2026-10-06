//! Climbing `DisClimbable` chains, decoded from `Dishonored.exe` (research/dishonored/PLAYER.md
//! "Climbing"):
//!
//! - Grab: Use on the chain (its interactable `Interact`, 0x00A23130, asks for
//!   `StatePlayerMasterClimb` and sets `m_pClimbable`), touching it while not walking (the
//!   chain's collision notify 0x00A3FA70: the player pawn, no `m_pClimbable`, not
//!   `m_bLettingGoOfClimbable`, physics neither Walking nor None), or Blinking into it (the
//!   Blink step's probe, 0x00BECDF0).
//! - Climb state enter (0x00A72B00): physics Ladder, the chain's collision off (0x00A1A170),
//!   `m_fStartTime`, the chain's `DisSeqEvent_Climb` Start.
//! - `physLadder` (0x00AA82C0): the velocity is zero; with any movement input, the target
//!   height is the current one `+- m_fClimbingSpeed x 16383.5` (8.19 uu), up when the input
//!   points within `acos 0.9` of the facing, down otherwise (inverted when
//!   `m_bCameraRelativeClimbing` and the camera looks down, `m_bClimbingUp` switching at
//!   `+-m_ClimbingAimThreshold` of camera pitch), clamped between `anchor.z - 2 x
//!   Bounds.SphereRadius` and `anchor.z - CollisionHeight`. The horizontal target is the
//!   chain's bone closest to a point 2/3 of `CollisionHeight` above the pawn's centre, 30 uu
//!   behind it along the facing and 10 uu to its right. The pawn moves `VInterpTo(Location,
//!   target, dt, 20)` with `MoveActor`, sliding along what blocks (two-wall adjust). So the
//!   climbing speed is 8.19 x 20 = 163.8 uu/s while `20 dt < 1`.
//! - Steps make the chain sound every `min + (max - min) x |pitch| / 16383.5` seconds.
//! - Leave (exit 0x00A6E2C0, every exit to another state): `Velocity = facing x
//!   m_fJumpImpulse` (doubled when a trace 200 uu straight up hits something), the chain's
//!   Finish output. Jump (after `m_fClimbReuseDelay` with `m_bClimbReuseDelayJump`, transition
//!   test 0x00A6E570) enters `StatePlayerMasterJump`, which sets `Velocity.Z = JumpZ`; the
//!   chain stays held until that jump ends (0x00A5FFA0), then its collision returns after
//!   `m_fCollisionRestorationDelay`. Use (0x00AD5BA0 -> 0x00AAA030) lets go: Falling (also
//!   after the reuse delay), `m_bLettingGoOfClimbable`, the chain released at once.
//! - There is no automatic exit at the ends: the pawn stops at the limits; at the top a jump
//!   (with the mantle that runs while airborne) gets off onto a ledge.

use crate::events::SimEvent;
use crate::input::{PlayerInput, buttons};
use crate::movement::{MoveCtx, forward_vector, half_height, pawn_box, two_wall_adjust};
use crate::state::{MoveMode, PawnKind, PlayerState};
use dishonored_world::{Climbable, WorldQuery, channel};
use glam::{Vec2, Vec3};

/// `DAT_0111870C`: the climbing speed tweak is scaled by this (the int16 stick range).
pub const SPEED_SCALE: f32 = 16383.5;
/// `DAT_00FBA450`: `VInterpTo` speed toward the climb target.
pub const INTERP_SPEED: f32 = 20.0;
/// `DAT_00FB9D1C` / `DAT_00FC5E4C`: the pawn hangs this far behind / to the right of the bone.
pub const BEHIND: f32 = 30.0;
pub const RIGHT: f32 = 10.0;
/// `DAT_00FCF9E4`: input within `acos(0.9)` of the facing climbs up.
pub const FORWARD_DOT: f32 = 0.9;
/// `DAT_01118708`: the bone search point is this share of `CollisionHeight` above the centre.
pub const BONE_SEARCH_HEIGHT: f32 = 2.0 / 3.0;
/// `DAT_00FB96C4`: the jump-off impulse doubles when a trace this far up hits something.
pub const CEILING_TRACE: f32 = 200.0;

/// Rotator units of a pitch in degrees (the camera's `CameraCache.POV.Rotation.Pitch`).
pub fn pitch_units(pitch_deg: f32) -> i32 {
    (pitch_deg * (65536.0 / 360.0)) as i32
}

/// Vertical limits of the pawn's centre on a chain: `(anchor.z - 2 SphereRadius, anchor.z -
/// CollisionHeight)`.
pub fn limits(c: &Climbable, collision_height: f32) -> (f32, f32) {
    (c.anchor.z - 2.0 * c.sphere_radius, c.anchor.z - collision_height)
}

/// The horizontal target: the closest bone, 30 uu behind it and 10 uu to its right.
pub fn bone_target(c: &Climbable, p: &PlayerState, collision_height: f32) -> Option<Vec3> {
    let search = p.position + Vec3::Z * (collision_height * BONE_SEARCH_HEIGHT);
    let b = c.bones[c.closest_bone(search)?];
    let fwd = forward_vector(p.yaw);
    let right = forward_vector(p.yaw + 90.0);
    Some(b - fwd * BEHIND + right * RIGHT)
}

/// `VInterpTo` (0x0041E860).
pub fn vinterp_to(cur: Vec3, target: Vec3, dt: f32, speed: f32) -> Vec3 {
    if speed <= 0.0 {
        return target;
    }
    let d = target - cur;
    if d.length_squared() < 1e-4 {
        return target;
    }
    cur + d * (dt * speed).clamp(0.0, 1.0)
}

/// Whether the pawn may grab chains now (touch and Use).
fn can_grab(p: &PlayerState) -> bool {
    p.kind == PawnKind::Human && p.alive && p.climb.on.is_none() && p.climb.no_touch <= 0.0 && p.carrying.is_none()
}

/// Start climbing chain `i` (`StatePlayerMasterClimb` enter).
pub fn start_climb(p: &mut PlayerState, i: u16, events: &mut Vec<SimEvent>) {
    p.mode = MoveMode::Climbing;
    p.velocity = Vec3::ZERO;
    p.crouched = false;
    p.slide.active = false;
    p.jump.active = false;
    p.fall_time = 0.0;
    p.lean = 0.0;
    p.lean_velocity = 0.0;
    p.climb.on = Some(i);
    p.climb.time = 0.0;
    p.climb.jumped = false;
    events.push(SimEvent::Climb { id: p.id, climbable: i, started: true });
}

/// The chain whose bodies the pawn's box overlaps.
pub fn touching(p: &PlayerState, world: &dyn WorldQuery, cfg: &dishonored_ir::gameplay::MovementConfig) -> Option<u16> {
    let b = pawn_box(p, cfg);
    let (lo, hi) = (b.center - b.half, b.center + b.half);
    for (i, c) in world.climbables().iter().enumerate() {
        // Closest points of the body line and the box (alternating projections; exact for the
        // vertical chains), then the bodies' half-width.
        let mut q = c.closest_point(b.center);
        for _ in 0..4 {
            q = c.closest_point(q.clamp(lo, hi));
        }
        let near = q.clamp(lo, hi);
        if (q - near).abs().cmple(Vec3::splat(c.body_half_width)).all() {
            return Some(i as u16);
        }
    }
    None
}

/// The collision notify of a chain (0x00A3FA70): a pawn touching it while not walking grabs it.
pub fn try_touch_grab(p: &mut PlayerState, ctx: &MoveCtx, events: &mut Vec<SimEvent>) -> bool {
    if !can_grab(p) || p.climb.letting_go || matches!(p.mode, MoveMode::Walking | MoveMode::Dead | MoveMode::Climbing | MoveMode::Mantling) {
        return false;
    }
    match touching(p, ctx.world, ctx.cfg) {
        Some(i) => {
            start_climb(p, i, events);
            true
        }
        None => false,
    }
}

/// Use on a chain in view (its interactable, 0x00A23130): grabs from walking or falling.
/// The chain is the one whose body line passes nearest the crosshair within `reach` of the eye
/// and `cos_limit` of the view (the pickups' rule), with a clear line to it.
pub fn try_use_grab(p: &mut PlayerState, world: &dyn WorldQuery, cfg: &dishonored_ir::gameplay::MovementConfig, reach: f32, cos_limit: f32, events: &mut Vec<SimEvent>) -> bool {
    if !can_grab(p) || !matches!(p.mode, MoveMode::Walking | MoveMode::Falling) {
        return false;
    }
    let eye = crate::movement::eye_position(p, cfg);
    let dir = crate::movement::view_vector(p.yaw, p.pitch);
    let mut best: Option<(u16, f32)> = None;
    for (i, c) in world.climbables().iter().enumerate() {
        // Closest point of the chain to the view ray.
        let ab = c.bottom - c.top;
        let mut q = c.closest_point(eye + dir * reach * 0.5);
        for _ in 0..8 {
            let along = (q - eye).dot(dir).max(0.0);
            let on_ray = eye + dir * along;
            let l2 = ab.length_squared();
            let t = if l2 > 1e-6 { ((on_ray - c.top).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            q = c.top + ab * t;
        }
        let to = q - eye;
        let d = to.length();
        if d > reach || d < 1e-3 {
            continue;
        }
        let cos = to.dot(dir) / d;
        if cos < cos_limit {
            continue;
        }
        if world.raycast(eye, to, d, channel::PAWN).is_some_and(|h| h.distance < d - c.body_half_width - 1.0) {
            continue;
        }
        if best.is_none_or(|(_, bc)| cos > bc) {
            best = Some((i as u16, cos));
        }
    }
    match best {
        Some((i, _)) => {
            start_climb(p, i, events);
            true
        }
        None => false,
    }
}

/// Leave the chain (climb state exit 0x00A6E2C0 into Jump or Falling).
fn leave(p: &mut PlayerState, jump: bool, ctx: &MoveCtx, events: &mut Vec<SimEvent>) {
    let cc = &ctx.cfg.climb;
    let i = p.climb.on.unwrap_or(0);
    events.push(SimEvent::Climb { id: p.id, climbable: i, started: false });
    let mut impulse = cc.jump_impulse;
    if ctx.world.raycast(p.position, Vec3::Z, CEILING_TRACE, channel::PAWN).is_some() {
        impulse *= 2.0;
    }
    p.velocity = forward_vector(p.yaw) * impulse;
    p.mode = MoveMode::Falling;
    p.fall_time = 0.0;
    if jump {
        // StatePlayerMasterJump enter (0x00A75EE0): `Velocity.Z = JumpZ`; the chain stays held.
        p.velocity.z = ctx.cfg.jump_z;
        p.jump = crate::state::JumpState { active: true, held: 0.0, powered: false };
        p.climb.jumped = true;
        events.push(SimEvent::Jumped { id: p.id });
    } else {
        // Let go (0x00AAA030): released at once, no grabbing by touch until landed.
        p.climb.on = None;
        p.climb.letting_go = true;
    }
}

/// Bookkeeping of the climb state outside the climb (every tick, before the physics).
pub fn tick_off_chain(p: &mut PlayerState, ctx: &MoveCtx) {
    if p.mode == MoveMode::Climbing {
        return;
    }
    // Jump exit (0x00A5FFA0) after a jump off a chain: the chain is released and its collision
    // comes back after `m_fCollisionRestorationDelay`.
    if p.climb.jumped && !p.jump.active {
        p.climb.jumped = false;
        p.climb.on = None;
        p.climb.no_touch = ctx.cfg.climb.collision_restoration_delay;
    } else if !p.climb.jumped && p.climb.on.is_some() {
        p.climb.on = None;
    }
    if p.climb.no_touch > 0.0 {
        p.climb.no_touch = (p.climb.no_touch - ctx.dt).max(0.0);
    }
    // `StatePlayerMasterFalling` exit (0x00A63110) clears `m_bLettingGoOfClimbable`.
    if p.climb.letting_go && !matches!(p.mode, MoveMode::Falling | MoveMode::Blinking) {
        p.climb.letting_go = false;
    }
}

/// One tick on a chain: the leave requests, then `physLadder`.
pub fn climbing(p: &mut PlayerState, input: &PlayerInput, can_move: bool, ctx: &MoveCtx, events: &mut Vec<SimEvent>) {
    let cc = &ctx.cfg.climb;
    let dt = ctx.dt;
    let Some(c) = p.climb.on.and_then(|i| ctx.world.climbables().get(i as usize)).cloned() else {
        p.climb.on = None;
        p.mode = MoveMode::Falling;
        p.fall_time = 0.0;
        return;
    };
    p.climb.time += dt;
    p.climb.since_sound += dt;
    // Transition test (0x00A6E570): Falling always, Jump with `m_bClimbReuseDelayJump`, only
    // after `m_fClimbReuseDelay`.
    let reused = p.climb.time > cc.reuse_delay;
    if can_move && p.just_pressed(input, buttons::JUMP) && (reused || !cc.reuse_delay_jump) {
        leave(p, true, ctx, events);
        return;
    }
    if can_move && p.just_pressed(input, buttons::USE) && reused {
        leave(p, false, ctx, events);
        return;
    }
    physladder(p, input, can_move, &c, ctx, events);
}

/// `ADishonoredPlayerPawn::physLadder` (0x00AA82C0).
fn physladder(p: &mut PlayerState, input: &PlayerInput, can_move: bool, c: &Climbable, ctx: &MoveCtx, events: &mut Vec<SimEvent>) {
    let cc = &ctx.cfg.climb;
    let dt = ctx.dt;
    p.velocity = Vec3::ZERO;
    let height = half_height(p, ctx.cfg);
    let (f, r) = input.axes();
    let mut z = p.position.z;
    if can_move && (f != 0.0 || r != 0.0) {
        let pitch = pitch_units(p.pitch);
        let threshold = if p.climb.down { cc.aim_threshold } else { -cc.aim_threshold };
        p.climb.down = threshold >= pitch;
        let mut step = cc.speed * SPEED_SCALE;
        // The acceleration's 2D direction against the facing: `f / |(f, r)|`.
        let accel = Vec2::new(f, r).normalize_or_zero();
        if cc.camera_relative && p.climb.down {
            step = -step;
        }
        if accel.x <= FORWARD_DOT {
            step = -step;
        }
        let target = z + step;
        let (low, high) = limits(c, height);
        let new_z = if low <= target { if target < high { target } else { high } } else { low };
        let tol = 1e-4f32;
        let moved = (new_z - z).abs() >= (new_z.abs() * tol).max(z.abs() * tol).max(tol);
        z = new_z;
        if moved {
            let delay = (cc.sound_max_delay - cc.sound_min_delay) * pitch.unsigned_abs() as f32 / SPEED_SCALE + cc.sound_min_delay;
            if delay < p.climb.since_sound {
                events.push(SimEvent::ClimbSound { id: p.id, climbable: p.climb.on.unwrap_or(0), volume: step.abs() });
                p.climb.since_sound = 0.0;
            }
        }
    }
    let Some(mut target) = bone_target(c, p, height) else { return };
    target.z = z;
    let goal = vinterp_to(p.position, target, dt, INTERP_SPEED);
    let delta = goal - p.position;
    ladder_move(p, delta, ctx);
}

/// The `MoveActor` of `physLadder`: one slide along a blocking wall, then the two-wall adjust.
fn ladder_move(p: &mut PlayerState, delta: Vec3, ctx: &MoveCtx) {
    if delta.length_squared() < 1e-10 {
        return;
    }
    let Some((n, t)) = crate::movement::move_actor(p, delta, ctx) else { return };
    if t >= 1.0 {
        return;
    }
    let desired = delta.normalize_or_zero();
    let slide = (delta - n * n.dot(delta)) * (1.0 - t);
    if slide.dot(delta) < 0.0 {
        return;
    }
    let Some((n2, t2)) = crate::movement::move_actor(p, slide, ctx) else { return };
    if t2 >= 1.0 {
        return;
    }
    let adj = two_wall_adjust(desired, slide, n2, n, t2);
    crate::movement::move_actor(p, adj, ctx);
}

/// Index of the chain this pawn holds, if climbing.
pub fn holding(p: &PlayerState) -> Option<u16> {
    (p.mode == MoveMode::Climbing).then_some(p.climb.on).flatten()
}
