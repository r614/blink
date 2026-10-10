//! Locomotion: a port of the engine physics of `Dishonored.exe` (`APawn::physWalking`
//! 0x006BD150, `APawn::physFalling` 0x006BF0E0, `APawn::CalcVelocity` 0x006ACD50, braking
//! 0x006A2AC0, `APawn::stepUp` 0x006ADC10, `APawn::physSwimming` 0x006B7820,
//! `NewFallVelocity` 0x006A3860) driven by Dishonored's tuning values, see
//! research/dishonored/PLAYER.md "Engine physics".
//!
//! - The pawn sweeps an axis-aligned box `(radius, radius, half height)` against the world
//!   (UE3 extent traces, U-016), and walks keeping the floor check's distance between
//!   `MIN_FLOOR_DIST` and `MAX_FLOOR_DIST`. Sweeps report the engine's backed-off hit time
//!   (`hit_kind::engine_time`), as `MoveActor` and physWalking's floor check see it, so the box
//!   rests `FLOOR_DIST` + 4 uu above a per-polygon floor and + 1 uu above simplified collision
//!   (measured with the engine hook, research/dishonored/COLLISION.md "Movement and the back-off").
//! - The engine constants are UE3's scaled by 1.27 in this build (MAXSTEPHEIGHTFUDGE 2.54,
//!   MINFLOORDIST 2.413, MAXFLOORDIST 3.048, MAXSTEPSIDEZ 0.1016, MINSTEPSIZESQUARED 232.26,
//!   braking stop speed 12.7).
//! - Falling integrates `v1 = v0 + a dt`, moves by `v1 dt`, then sets the velocity to twice
//!   the measured average minus `v0` ("end velocity has twice the acceleration"): the pawn
//!   gains `2 a dt` per tick, so gravity and air control act twice as strongly as the raw
//!   values suggest (a JumpZ 800 jump rises 107 uu, not 213).
//! - Mantle timings remain SPECULATIVE (U-019).

use crate::events::SimEvent;
use crate::input::{PlayerInput, buttons};
use crate::state::{MoveMode, PawnKind, PlayerState, SlideState, Traversal};
use dishonored_ir::gameplay::MovementConfig;
use dishonored_world::{Capsule, PawnBox, WorldQuery, channel};
use glam::{Vec2, Vec3};

/// Fixed tick rate of the simulation.
pub const TICK_HZ: u32 = 60;
pub const DT: f32 = 1.0 / TICK_HZ as f32;
/// World kill plane (`WorldInfo.KillZ` on L_Boyle_Ext_P).
pub const KILL_Z: f32 = -20000.0;

/// `MAXSTEPHEIGHTFUDGE` (UE3 2.0, x1.27).
pub const STEP_FUDGE: f32 = 2.54;
/// `MINFLOORDIST` / `MAXFLOORDIST` / their mean: the walking pawn hovers this far above the floor.
pub const MIN_FLOOR_DIST: f32 = 2.413;
pub const MAX_FLOOR_DIST: f32 = 3.048;
pub const FLOOR_DIST: f32 = 2.7305;
/// `MAXSTEPSIDEZ`: walls with a normal Z below this are steps (0x006ADC10).
pub const MAX_STEP_SIDE_Z: f32 = 0.1016;
/// `MINSTEPSIZESQUARED`: a further step needs at least this much of the move left.
pub const MIN_STEP_SIZE_SQ: f32 = 232.2576;
/// Braking stops below this speed squared (`DAT_0103AB7C`, 12.7^2).
pub const BRAKE_STOP_SPEED_SQ: f32 = 161.29;
/// Braking sub-step (`DAT_0103C798`).
pub const BRAKE_STEP: f32 = 0.03;
/// Floors steeper than this (normal Z) are not "flat" for the uphill test of physWalking.
pub const FLAT_FLOOR_Z: f32 = 0.98;
/// Physics sub-step limit: moves longer than this are split (`DAT_00FBA454`).
pub const MAX_TIME_STEP: f32 = 0.05;

/// Rat pawns: `DisGameCrowdAgentSkeletalRat` (search extent 15, `GroundOffset` 10, `MaxSpeed` 350 of
/// `RatSwarm.CrowdAgent_SmallRat`).
pub const RAT_RADIUS: f32 = 15.0;
pub const RAT_HALF_HEIGHT: f32 = 10.0;

pub fn half_height(p: &PlayerState, cfg: &MovementConfig) -> f32 {
    if p.kind == PawnKind::Rat {
        RAT_HALF_HEIGHT
    } else if p.crouched {
        cfg.crouch_half_height
    } else {
        cfg.collision_half_height
    }
}

pub fn radius(p: &PlayerState, cfg: &MovementConfig) -> f32 {
    if p.kind == PawnKind::Rat { RAT_RADIUS } else { cfg.collision_radius }
}

/// The pawn's collision box (cylinder extent).
pub fn pawn_box(p: &PlayerState, cfg: &MovementConfig) -> PawnBox {
    PawnBox::new(p.position, radius(p, cfg), half_height(p, cfg))
}

/// The capsule of the same size (body hit tests of projectiles and swords).
pub fn capsule(p: &PlayerState, cfg: &MovementConfig) -> Capsule {
    Capsule { center: p.position, radius: radius(p, cfg), half_height: half_height(p, cfg) }
}

/// Eye position: UE `BaseEyeHeight` above the cylinder centre when standing; the same
/// offset from the top of the cylinder when crouched; swung sideways by the lean.
pub fn eye_position(p: &PlayerState, cfg: &MovementConfig) -> Vec3 {
    if p.kind == PawnKind::Rat {
        return p.position + Vec3::Z * RAT_HALF_HEIGHT * 0.5;
    }
    let top_gap = cfg.collision_half_height - cfg.base_eye_height;
    let mut eye = p.position + Vec3::Z * (half_height(p, cfg) - top_gap);
    if p.lean != 0.0 {
        eye += lean_offset(p, cfg);
    }
    eye
}

/// The player camera's location (`CameraCache.POV.Location`) at rest: the eye on the camera
/// rig's pitch arc ([`dishonored_ir::gameplay::CameraRig`], measured on the original), measured
/// from the top of the pawn's collision cylinder and forward along the view's yaw, swung
/// sideways by the lean as [`eye_position`] is. The crosshair's traces start here
/// (`ViewTarget.POV`, 0x00AD80A0 -> 0x00AB7B80).
pub fn camera_pov(p: &PlayerState, cfg: &MovementConfig) -> Vec3 {
    if p.kind == PawnKind::Rat {
        return eye_position(p, cfg);
    }
    let (fwd, up) = cfg.camera.offset(p.pitch.clamp(-89.0, 89.0));
    let (sy, cy) = p.yaw.to_radians().sin_cos();
    let mut pov = p.position + Vec3::new(cy * fwd, sy * fwd, half_height(p, cfg) + up);
    if p.lean != 0.0 {
        pov += lean_offset(p, cfg);
    }
    pov
}

/// `DishonoredCamera_Lean` (0x00AC5420 / 0x00AC5950): the eye is a spring point kept at
/// `m_fCurLeanHeight = m_fLeanHeightPct x CollisionHeight x 2` from a pivot that far below
/// it, so a lean of angle `a` moves it `L sin a` sideways and `L (1 - cos a)` down, `a`
/// limited to `m_fMaxLeanAngle` plus the soften angle. `p.lean` is the angle as a fraction of
/// that maximum.
pub fn lean_offset(p: &PlayerState, cfg: &MovementConfig) -> Vec3 {
    let l = cfg.lean.height_pct * half_height(p, cfg) * 2.0;
    let a = lean_angle(p, cfg).to_radians();
    right_vector(p.yaw) * (l * a.sin()) - Vec3::Z * (l * (1.0 - a.cos()))
}

/// Lean angle in degrees: `p.lean` x (the stance's `m_fMaxLeanAngle` + the soften angle).
pub fn lean_angle(p: &PlayerState, cfg: &MovementConfig) -> f32 {
    let max = if p.crouched { cfg.lean.max_angle_crouched } else { cfg.lean.max_angle };
    p.lean * (max + p.lean_soften)
}

/// Camera roll of the lean in degrees (`m_fCameraTiltPercent` x the lean angle; positive
/// rolls the view to the right, as the original's `CameraCache.POV` roll for a right lean).
pub fn lean_roll(p: &PlayerState, cfg: &MovementConfig) -> f32 {
    lean_angle(p, cfg) * cfg.lean.camera_tilt_percent
}

/// The lean master state (`StatePlayerMasterLeaning`) for one tick.
///
/// - The state starts on a lean key press (0x00A764C0) and its camera influence starts with an
///   empty fixed-step accumulator (the lean traces show no head motion on the press tick: the
///   first frame's time, 1/60 s, is short of the influence's 0.016667 step).
/// - Its tick (0x00A76590) writes the head point's velocity - `m_fLeanSpeed` along the lean
///   direction - on every tick a key is held (0x00AC0950, which also flags `m_bGotExternalForce`)
///   and re-arms `m_fReleaseTimer` with `m_fLeanReleaseTime`; without a key the timer runs down
///   by the frame time and the state ends when it is zero or less. Nothing is written then: the
///   head point keeps whatever velocity the spring left it.
fn lean_state_tick(p: &mut PlayerState, key: f32, ctx: &MoveCtx) {
    let lc = &ctx.cfg.lean;
    if !p.lean_state {
        if key == 0.0 {
            return;
        }
        p.lean_state = true;
        p.lean_release = 0.0;
        p.lean_acc = 0.0;
    }
    if key != 0.0 {
        p.lean_velocity = key.signum() * lc.lean_speed;
        p.lean_velocity_z = 0.0;
        p.lean_ext = true;
        p.lean_release = lc.release_time;
    } else {
        p.lean_release -= ctx.dt;
        if p.lean_release <= 0.0 {
            p.lean_state = false;
        }
    }
}

/// One frame of the lean influence (`DishonoredCamera_Lean`): the lean state's tick, then as
/// many fixed steps (`m_fFixedTimeStep`, `m_bUseFixedTimeStep`) as the frame time fills in the
/// accumulator `m_fAccumulatedDelta`. A frame that ran steps clears `m_bGotExternalForce`
/// (0x00AC5950's last argument).
///
/// The loop that spends the accumulator is the camera's base update, not decoded; its shape is
/// read from the view (0x00ACAAB0 interpolates the head point by `m_fAccumulatedDelta /
/// m_fCurFixedTimeStep` between the last two steps) and from the lean traces, where the first
/// held tick moves nothing and every later tick runs exactly one step. The view interpolation
/// is not modelled: at 60 Hz the fraction is 0.99998 after every step.
fn step_lean(p: &mut PlayerState, key: f32, ctx: &MoveCtx) {
    lean_state_tick(p, key, ctx);
    let step = ctx.cfg.lean.fixed_time_step.max(1e-4);
    p.lean_acc += ctx.dt;
    let mut ran = false;
    while p.lean_acc >= step {
        lean_step(p, step, ctx);
        p.lean_acc -= step;
        ran = true;
    }
    if ran {
        p.lean_ext = false;
    }
}

/// One fixed step of the lean camera (`DishonoredCamera_Lean`, 0x00AC5950): the eye is a spring
/// point (`m_HeadPoint`, `DisSpringPoint`) in the plane of the lean, at rest at the upright eye,
/// kept on a circle of radius `L = m_fLeanHeightPct x CollisionHeight x 2` around a pivot `L`
/// below the eye, its angle limited to `m_fMaxLeanAngle` plus the soften angle.
///
/// - the integrator 0x00AC0A00 (`m_LeanConstants` k 80, c 12): `a = -k x - c v` (or `-v` once
///   nearly at rest), `v += a dt`, `x += v dt`;
/// - with `m_bGotExternalForce` the velocity is then put back to what the lean state wrote
///   (so each held tick starts at `m_fLeanSpeed` sideways);
/// - the point goes back onto the circle; an angle past the limit is clamped
///   (`m_bApplyingMaxAngle`) and the soften angle (0x00AC5540) grows toward
///   `m_fMaxLeanSoftenAngle` at `m_fMaxLeanSoftenSpeed` while the head stays at the limit,
///   otherwise decays at that rate.
///
/// Original at a fixed 1/60 s (the scenario trace, CameraCache.POV roll x 2): the angle is
/// 2.18, 4.31, 6.38, 8.40, 10.36, 12.27, 14.12 degrees on the first held steps, 15.007 on the
/// eighth (the limit, soften starting), 17.9 on the nineteenth and 20 held; this model gives the
/// same values to 0.01 degrees.
///
/// `p.lean` is the angle as a fraction of the current limit; `p.lean_velocity` and
/// `p.lean_velocity_z` the head point's velocity in the lean plane (sideways, up).
fn lean_step(p: &mut PlayerState, dt: f32, ctx: &MoveCtx) {
    let lc = &ctx.cfg.lean;
    let max = if p.crouched { lc.max_angle_crouched } else { lc.max_angle };
    let l = lc.height_pct * half_height(p, ctx.cfg) * 2.0;
    // The head point (sideways, up) relative to the upright eye, from the current angle.
    let a0 = (p.lean * (max + p.lean_soften)).to_radians();
    let mut pos = glam::Vec2::new(l * a0.sin(), l * (a0.cos() - 1.0));
    let mut vel = glam::Vec2::new(p.lean_velocity, p.lean_velocity_z);
    let saved = vel;
    // 0x00AC0A00: the spring toward rest (the upright eye).
    let acc = if pos.length_squared() > 1e-4 || vel.length_squared() > 1.0 { -lc.springiness * pos - lc.damping * vel } else { -vel };
    vel += acc * dt;
    pos += vel * dt;
    if p.lean_ext {
        vel = saved;
    }
    // Back onto the circle around the pivot.
    let pivot = glam::Vec2::new(0.0, -l);
    let dir = (pos - pivot).normalize_or(glam::Vec2::Y);
    let mut angle = dir.x.atan2(dir.y).to_degrees();
    // The limit and the soften angle (0x00AC5540).
    let limit = max + p.lean_soften;
    let at_limit = angle.abs() >= limit;
    if at_limit {
        angle = angle.clamp(-limit, limit);
    }
    // `m_bApplyLeanSoften`: set by the clamp, kept while the head stays within 5 uu of where it
    // was (0x00AC5540).
    let moved = (glam::Vec2::new(l * angle.to_radians().sin(), l * (angle.to_radians().cos() - 1.0)) - glam::Vec2::new(l * a0.sin(), l * (a0.cos() - 1.0))).length();
    let apply = at_limit || (p.lean.abs() >= 0.999 && moved <= 5.0);
    let s = lc.soften_speed * dt;
    if apply {
        p.lean_soften = (p.lean_soften + (lc.soften_angle - p.lean_soften) * s).min(lc.soften_angle);
    } else {
        p.lean_soften = (p.lean_soften - p.lean_soften * s).max(0.0);
    }
    let limit = (max + p.lean_soften).max(1e-3);
    p.lean = (angle / limit).clamp(-1.0, 1.0);
    p.lean_velocity = vel.x;
    p.lean_velocity_z = vel.y;
    if !p.lean_ext && p.lean.abs() * limit < 0.05 && vel.length() < 1.0 {
        p.lean = 0.0;
        p.lean_velocity = 0.0;
        p.lean_velocity_z = 0.0;
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
            p.lean_velocity_z = 0.0;
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
/// - times the sword factor (vslot 363, 0x00AA4700: `Attribute_SwordUnsheathedSpeedFactor` when
///   the right-hand item is the sword class, else `Attribute_SwordSheathedSpeedFactor`; see
///   [`sword_in_right_hand`]), in every stance including sprint;
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
    // An NPC brain's requested locomotion speed replaces the stance speeds (the authored loop
    // speed already includes the weapon stance).
    let npc = input.npc_speed > 0.0;
    let mut m = if npc {
        input.npc_speed / ground
    } else if p.sprinting {
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
    if !npc {
        m *= if sword_out { cfg.sword_unsheathed_speed_factor } else { cfg.sword_sheathed_speed_factor };
    }
    let last = p.speed_mod;
    if m < last && last - m > 1e-4 {
        let a = (dt * cfg.speed_blend_down_speed).clamp(0.0, 1.0);
        m = (m - last) * a + last;
    }
    p.speed_mod = m;
    m
}

/// The speed modifier's sword test (0x00AA4700): the item in the right hand (hand slot 1) is of
/// the sword class. A pawn carrying a body holds no sword (INVENTORY.md "Carrying": the sword is
/// sheathed while carrying).
pub fn sword_in_right_hand(p: &PlayerState) -> bool {
    p.right_hand == crate::state::RightHandItem::Sword && p.carrying.is_none() && p.holding.is_none()
}

/// Braking (`APawn::ApplyVelocityBraking`, 0x006A2AC0): in sub-steps of at most 0.03 s,
/// `v -= 2 v dt friction`, summing each sub-step's velocity weighted by its share of the tick
/// while it still points the old way; the result is that average, zeroed when it reversed or
/// fell below 12.7 uu/s.
pub fn apply_braking(vel: Vec3, dt: f32, friction: f32) -> Vec3 {
    let old = vel;
    let mut v = vel;
    let mut sum = Vec3::ZERO;
    let mut t = dt;
    while t > 0.0 {
        let s = t.min(BRAKE_STEP);
        v -= v * 2.0 * s * friction;
        t -= s;
        if v.dot(old) > 0.0 {
            sum += v * s / dt;
        }
    }
    if sum.dot(old) < 0.0 || sum.length_squared() < BRAKE_STOP_SPEED_SQ {
        Vec3::ZERO
    } else {
        sum
    }
}

/// `APawn::CalcVelocity` (0x006ACD50, no root motion): the acceleration (the controller's,
/// `bForceMaxAccel` is false) is limited to `AccelRate * modifier` (`GetMaxAccel`, 0x006A2EE0);
/// without acceleration and with `brake`, the velocity brakes; otherwise it turns toward the
/// acceleration direction, `v -= (v - dir |v|) dt friction`; then
/// `v = v (1 - fluid dt friction) + a dt`, plus `(1 - Buoyancy) g dt` when buoyant; the speed is
/// capped at `max_speed * modifier`.
#[allow(clippy::too_many_arguments)]
pub fn calc_velocity(vel: Vec3, accel: Vec3, accel_dir: Vec3, dt: f32, max_speed: f32, friction: f32, fluid: bool, brake: bool, buoyant: Option<(f32, f32)>, modifier: f32, accel_rate: f32) -> (Vec3, Vec3) {
    let max_accel = accel_rate * modifier;
    let max_speed = max_speed * modifier;
    let mut a = accel;
    let mut v = vel;
    if !brake || a != Vec3::ZERO {
        if a.length_squared() > max_accel * max_accel {
            a = a.normalize_or_zero() * max_accel;
        }
        let size = v.length();
        v -= (v - accel_dir * size) * dt * friction;
    } else {
        v = apply_braking(v, dt, friction);
    }
    let f = if fluid { 1.0 } else { 0.0 };
    v = v * (1.0 - f * dt * friction) + a * dt;
    if let Some((buoyancy, gravity_z)) = buoyant {
        v.z += (1.0 - buoyancy) * gravity_z * dt;
    }
    if v.length_squared() > max_speed * max_speed {
        v = v.normalize_or_zero() * max_speed;
    }
    (v, a)
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
    let rat = p.kind == PawnKind::Rat;
    // The player pawn's tick (0x00ADBD00): `m_bMantleAllowed` is set while walking or swimming.
    if matches!(p.mode, MoveMode::Walking | MoveMode::Swimming) {
        p.mantle_allowed = true;
    }

    // `Dis_SneakOrSlide`: sneak toggle (keeps feet planted; standing up requires
    // headroom), or a slide when sprinting at `m_fMinSpeedToSlide` or faster.
    if !rat && p.just_pressed(input, buttons::CROUCH) && p.mode == MoveMode::Walking && !p.slide.active {
        if p.sprinting && can_move && p.velocity.truncate().length() >= ctx.cfg.input.min_speed_to_slide {
            start_slide(p, sword_out, ctx);
        } else {
            set_crouch(p, !p.crouched, ctx);
        }
    }

    // Lean only while standing still on the ground (moving breaks the lean).
    let lean_target = if !rat && p.mode == MoveMode::Walking && p.velocity.truncate().length() < 10.0 {
        (input.pressed(buttons::LEAN_RIGHT) as i32 - input.pressed(buttons::LEAN_LEFT) as i32) as f32
    } else {
        0.0
    };
    step_lean(p, lean_target, ctx);

    let can_sprint = !rat && can_move && p.mode == MoveMode::Walking && p.sword.phase != crate::state::SwordPhase::Blocking;
    update_locomotion_flags(p, input, ctx.cfg, can_sprint);

    // Entering deep water switches to swimming (`PhysicsVolumeChange`: `bWaterVolume`).
    if matches!(p.mode, MoveMode::Walking | MoveMode::Falling) && !rat {
        if let Some(w) = ctx.world.water_at(p.position) {
            if !w.shallow {
                start_swimming(p, events);
            }
        }
    }

    if !rat {
        crate::climb::tick_off_chain(p, ctx);
    }
    match p.mode {
        MoveMode::Walking => walking(p, input, sword_out, can_move, ctx, events),
        MoveMode::Falling if p.thrown.active => flying(p, ctx),
        MoveMode::Falling => {
            falling(p, input, can_move, ctx.dt, ctx, events);
            // A chain touched while airborne is grabbed (0x00A3FA70).
            if !rat && p.mode == MoveMode::Falling {
                crate::climb::try_touch_grab(p, ctx, events);
            }
        }
        MoveMode::Climbing => crate::climb::climbing(p, input, can_move, ctx, events),
        MoveMode::Swimming => {
            crate::swim::swimming(p, input, can_move, sword_out, ctx, events);
            if !rat && p.mode == MoveMode::Swimming {
                crate::climb::try_touch_grab(p, ctx, events);
            }
        }
        MoveMode::Mantling => {
            if advance_traversal(p, ctx.cfg, ctx.dt) {
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

pub(crate) fn start_swimming(p: &mut PlayerState, events: &mut Vec<SimEvent>) {
    if p.mode == MoveMode::Swimming {
        return;
    }
    // `startSwimming` (0x006B6360): entering slowly from above dips: a downward speed below
    // 203.2 becomes `-101.6 - 0.7 |v.xy|`.
    if p.velocity.z > -203.2 && p.velocity.z < 0.0 {
        p.velocity.z = -101.6 - p.velocity.truncate().length() * 0.7;
    }
    p.mode = MoveMode::Swimming;
    p.slide.active = false;
    p.jump.active = false;
    p.fall_time = 0.0;
    if p.crouched {
        p.crouched = false;
    }
    events.push(SimEvent::Swim { id: p.id, entered: true });
}

/// The mantle's move (`StatePlayerMasterMantle`, `RMM_Translate`): the pawn is placed at the
/// finder's location and the mantle sequence's root motion moves it from there, its forward
/// part as authored and its vertical part scaled by the mantle height over the sequence's
/// `m_fMantleTravelDistanceZ` (`DishonoredAnimNodeSeqMantle`, 0x00A8F9B0: the transform
/// handed to the root motion is the component rotation with scale (1, 1, height / travel)),
/// played at `Attribute_MantleAnimRate`. The state's tick (0x00A6C590) does not move the pawn
/// itself; its intro (0x00A636C0, a linear slide over the previous action's time to go) only
/// runs after another action state (0x00A6EC20), not from walking or falling.
/// Returns true when finished.
pub fn advance_traversal(p: &mut PlayerState, cfg: &MovementConfig, dt: f32) -> bool {
    let t = &mut p.traversal;
    t.elapsed += dt;
    let a = if t.duration <= 0.0 { 1.0 } else { (t.elapsed / t.duration).min(1.0) };
    p.position = match t.anim.and_then(|i| cfg.mantle.anims.get(i as usize)) {
        Some(anim) => mantle_root_position(t.from, t.to, anim, t.elapsed * if t.rate > 0.0 { t.rate } else { 1.0 }),
        None => t.from.lerp(t.to, a),
    };
    a >= 1.0
}

/// Position `seq_time` seconds into a mantle sequence started at `from` (the finder's location)
/// whose root motion ends at `to`: forward along the horizontal direction to `to` by the root's
/// forward offset, up by the root's vertical offset times (`to.z - from.z`) / `travel_z`.
pub fn mantle_root_position(from: Vec3, to: Vec3, anim: &dishonored_ir::gameplay::MantleAnim, seq_time: f32) -> Vec3 {
    let [fwd, up] = anim.root_at(seq_time);
    let dir = (to - from).truncate().normalize_or_zero();
    let scale = if anim.travel_z.abs() > 1e-3 { (to.z - from.z) / anim.travel_z } else { 1.0 };
    from + Vec3::new(dir.x, dir.y, 0.0) * fwd + Vec3::Z * (up * scale)
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
    p.slide = SlideState { active: true, elapsed: 0.0, start_velocity: v0, target_velocity: target, stop_interpolating: false, hit: false };
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
    walk_move(p, delta, ctx);
    let s = &mut p.slide;
    let cur = p.velocity.truncate();
    let start_dir = s.start_velocity.truncate().normalize_or_zero();
    if cur.normalize_or_zero().dot(start_dir) < cfg.slide_deactivate_angle.to_radians().cos() {
        s.active = false;
    } else if cur.length() < s.target_velocity.truncate().length() {
        s.stop_interpolating = true;
    }
}

fn box_fits(world: &dyn WorldQuery, b: &PawnBox) -> bool {
    let mut contacts = Vec::new();
    world.box_contacts(b, channel::PAWN, &mut contacts);
    !contacts.iter().any(|c| c.depth > 0.5)
}

/// 0x00ACD960, from the enter of the action states (Mantle, Choke, Swim, Soiree, InDialog,
/// Versus, Minigame, the melee attack): `m_bSneakModeToggled` is cleared, so the pawn stands up
/// from a toggled sneak (headroom permitting; the original then plays its stand-up camera).
pub fn cancel_sneak_toggle(p: &mut PlayerState, ctx: &MoveCtx) {
    if p.crouched {
        set_crouch(p, false, ctx);
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
    if !crouch && !box_fits(ctx.world, &PawnBox::new(new_center, radius(p, cfg), new_hh)) {
        return; // no headroom
    }
    p.crouched = crouch;
    p.position = new_center;
}

/// One UE3 `MoveActor` of the pawn box: moves as far as it can along `delta`; returns the
/// blocking hit (normal, time).
pub(crate) fn move_actor(p: &mut PlayerState, delta: Vec3, ctx: &MoveCtx) -> Option<(Vec3, f32)> {
    if delta.length_squared() < 1e-10 {
        return None;
    }
    let b = pawn_box(p, ctx.cfg);
    match ctx.world.sweep_box(&b, delta, channel::PAWN) {
        None => {
            p.position += delta;
            None
        }
        Some(h) if h.start_penetrating => {
            p.position = depenetrate(p, ctx);
            Some((h.normal, 0.0))
        }
        Some(h) => {
            p.position += delta * h.fraction;
            Some((h.normal, h.fraction))
        }
    }
}

fn depenetrate(p: &PlayerState, ctx: &MoveCtx) -> Vec3 {
    let mut b = pawn_box(p, ctx.cfg);
    let mut contacts = Vec::new();
    for _ in 0..6 {
        contacts.clear();
        ctx.world.box_contacts(&b, channel::PAWN, &mut contacts);
        let Some(k) = contacts.iter().max_by(|a, b| a.depth.total_cmp(&b.depth)) else { break };
        if k.depth < 0.05 {
            break;
        }
        b.center += k.normal * (k.depth + 0.05);
    }
    b.center
}

/// `AActor::TwoWallAdjust`: in a corner of 90 degrees or less move along the crease, else slide
/// along the new wall (zeroed when that turns against the desired direction).
pub fn two_wall_adjust(desired: Vec3, delta: Vec3, n: Vec3, old_n: Vec3, time: f32) -> Vec3 {
    if old_n.dot(n) <= 0.0 {
        let dir = n.cross(old_n).normalize_or_zero();
        let mut d = dir * (delta.dot(dir) * (1.0 - time));
        if desired.dot(d) < 0.0 {
            d = -d;
        }
        d
    } else {
        let mut d = (delta - n * delta.dot(n)) * (1.0 - time);
        if d.dot(desired) <= 0.0 {
            d = Vec3::ZERO;
        } else if (n.dot(old_n) - 1.0).abs() < 1e-4 {
            d += n * 0.1;
        }
        d
    }
}

/// `APawn::stepUp` (0x006ADC10): a near-vertical barrier (normal Z below `MAXSTEPSIDEZ`) or a
/// walkable slope is climbed by lifting `MaxStepHeight + MAXSTEPHEIGHTFUDGE`, moving, and
/// dropping back; another near-vertical barrier with enough move left repeats the step;
/// otherwise the move slides along the barrier treated as vertical (with the two-wall adjust).
fn step_up(p: &mut PlayerState, desired: Vec3, delta: Vec3, hit: (Vec3, f32), ctx: &MoveCtx, depth: u32) {
    let cfg = ctx.cfg;
    let down = Vec3::Z * -(cfg.max_step_height + STEP_FUDGE);
    let mut step_down = true;
    let (mut n, mut t) = hit;
    if n.z < MAX_STEP_SIDE_Z || n.z >= cfg.walkable_floor_z {
        move_actor(p, -down, ctx);
        match move_actor(p, delta, ctx) {
            Some(h) => (n, t) = h,
            None => t = 1.0,
        }
    } else if p.mode != MoveMode::Walking {
        let dist = delta.length();
        match move_actor(p, delta + Vec3::Z * (dist * n.z), ctx) {
            Some(h) => (n, t) = h,
            None => t = 1.0,
        }
        step_down = false;
    }
    if t < 1.0 {
        if n.z.abs() < MAX_STEP_SIDE_Z && t * delta.length_squared() > MIN_STEP_SIZE_SQ && depth < 4 {
            if step_down {
                move_actor(p, down, ctx);
            }
            step_up(p, desired, delta * (1.0 - t), (n, t), ctx, depth + 1);
            return;
        }
        let old_n = n;
        let flat = Vec3::new(n.x, n.y, 0.0).normalize_or_zero();
        let nd = (delta - flat * delta.dot(flat)) * (1.0 - t);
        if nd.dot(delta) >= 0.0 {
            if let Some((n2, t2)) = move_actor(p, nd, ctx) {
                let adj = two_wall_adjust(desired, nd, n2, old_n, t2);
                move_actor(p, adj, ctx);
            }
        }
    }
    if step_down {
        move_actor(p, down, ctx);
    }
}

/// NPC navmesh walking (`DishonoredGameInfo.m_bNewNPCPhysWalking`, default true; the NPC pawn's
/// physics 0x00B4E9F0 -> the locomotion component 0x00943D00 -> 0x00940D80, AI.md "Go-to
/// completion"): while the locomotion component follows a navmesh path, the pawn is not moved by
/// physWalking. The move `Velocity x dt` is kept when its end lies on the navmesh reached from
/// the pawn's poly (0x0068F660); the height comes from the poly's plane, with the cylinder's
/// bottom on it; world geometry is not swept. Otherwise the original falls back to its walking
/// physics (0x0093FB00), here `walk_move`. Returns false for that fallback.
/// APPROXIMATION: the poly is found by position within the step height instead of walking the
/// poly edges from the current poly, and the original's actor box check (0x0064A6B0) is not
/// made.
fn nav_walk_move(p: &mut PlayerState, delta: Vec3, ctx: &MoveCtx) -> bool {
    let Some(nav) = ctx.world.nav() else { return false };
    if delta.truncate().length_squared() < 1e-8 {
        // Not moving: the component leaves the pawn where it is (no physWalking either).
        p.velocity = Vec3::ZERO;
        return true;
    }
    let hh = half_height(p, ctx.cfg);
    let step = ctx.cfg.max_step_height;
    let foot = p.position - Vec3::Z * hh;
    if nav.locate(foot, step, step).is_none() {
        return false;
    }
    let dest = foot + Vec3::new(delta.x, delta.y, 0.0);
    let Some(poly) = nav.locate(dest, step, step) else { return false };
    let z = nav.poly_z(poly, dest.x, dest.y);
    p.position = Vec3::new(dest.x, dest.y, z + hh);
    p.velocity = Vec3::new(delta.x, delta.y, 0.0) / ctx.dt;
    p.floor_normal = Vec3::Z;
    true
}

/// The horizontal part of physWalking's move: the move, then stepUp on a barrier.
pub(crate) fn walk_move(p: &mut PlayerState, delta: Vec3, ctx: &MoveCtx) {
    if delta.abs().max_element() < 1e-4 {
        return;
    }
    // Moving into a sloped floor counts as hitting it (physWalking: `Floor.Z < 0.98 &&
    // (Delta | Floor) < 0`).
    let hit = if p.floor_normal.z < FLAT_FLOOR_Z && delta.dot(p.floor_normal) < 0.0 {
        Some((p.floor_normal, 0.0))
    } else {
        move_actor(p, delta, ctx)
    };
    if let Some((n, t)) = hit {
        if t < 1.0 {
            let desired = delta.normalize_or_zero();
            step_up(p, desired, delta * (1.0 - t), (n, t), ctx, 0);
        }
    }
}

/// Floor under the pawn: a box sweep down `MaxStepHeight + MAXSTEPHEIGHTFUDGE`. Returns the
/// distance to the floor and its normal (`face_up`, the most upward face the box rests on).
fn find_floor(p: &PlayerState, ctx: &MoveCtx) -> Option<(f32, Vec3)> {
    let probe = ctx.cfg.max_step_height + STEP_FUDGE;
    let b = pawn_box(p, ctx.cfg);
    let h = ctx.world.sweep_box(&b, Vec3::Z * -probe, channel::PAWN)?;
    let n = if h.face_up.z > 0.0 { h.face_up } else { h.normal };
    Some((if h.start_penetrating { 0.0 } else { probe * h.fraction }, n))
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
    let mut jump = false;
    if can_move && p.just_pressed(input, buttons::JUMP) && p.kind != PawnKind::Rat {
        // `DoJump` (0x00AA3820): the mantle request first (0x00ADBE70: `m_bMantleAllowed` set, the
        // Jump button is down), the Jump state only when it did not mantle. A step-up leaves the
        // pawn walking and its physics runs this tick; the mantle state takes over.
        if jump_press_mantle(p, ctx, events) {
            if p.mode != MoveMode::Walking {
                return;
            }
        } else {
            if p.crouched {
                set_crouch(p, false, ctx);
            }
            jump = !p.crouched;
        }
    }
    walking_move(p, input, sword_out, can_move, ctx);
    if jump && p.mode == MoveMode::Walking {
        // StatePlayerMasterJump enter (0x00A75EE0): `Velocity.Z = Attribute_JumpZ`
        // (`JumpZCarryingCorpse` while carrying), physics Falling. The jump is taken with the
        // tick's input, after the pawn's walking physics ran this tick: the original's tick
        // of the press ends with Velocity.Z = JumpZ and the pawn not yet risen; the climb
        // starts with the next tick's physFalling (engine-hook trace of a standing jump at a
        // fixed 1/60 s step, 2026-10-08).
        p.velocity.z = cfg.jump_z;
        p.mode = MoveMode::Falling;
        p.fall_time = 0.0;
        p.jump = crate::state::JumpState { active: true, held: 0.0, powered: false };
        events.push(SimEvent::Jumped { id: p.id });
    }
}

/// physWalking (or the slide) for one tick, then the floor check.
fn walking_move(p: &mut PlayerState, input: &PlayerInput, sword_out: bool, can_move: bool, ctx: &MoveCtx) {
    let cfg = ctx.cfg;
    // Slide cancel (0x00A71450): once `m_fSlideTime * m_fSlidePercentNotCancelable` has passed,
    // releasing the stick ends the slide (the cancel request's source is LIKELY the stick).
    if p.slide.active && p.slide.elapsed > cfg.slide_time * cfg.slide_percent_not_cancelable && input.forward == 0 && input.right == 0 {
        p.slide.active = false;
    }
    let old = p.position;
    if p.slide.active {
        slide(p, ctx);
        // The speed modifier keeps tracking so the exit blends down from the slide.
        p.speed_mod = p.velocity.truncate().length() / cfg.run_speed;
    } else {
        // physWalking: Velocity.Z = Acceleration.Z = 0; CalcVelocity(AccelDir, dt, GroundSpeed,
        // PhysicsVolume.GroundFriction, 0, brake, 0).
        let accel = if can_move { input_accel(p, input, cfg) } else { Vec3::ZERO };
        let modifier = speed_modifier(p, input, cfg, sword_out, ctx.dt);
        let mut v = p.velocity;
        v.z = 0.0;
        let (nv, _) = calc_velocity(v, accel, accel.normalize_or_zero(), ctx.dt, cfg.run_speed, cfg.ground_friction, false, true, None, modifier, cfg.accel_rate);
        p.velocity = nv;
        p.velocity.z = 0.0;
        // physWalking: DesiredMove = Velocity + ZoneVelocity x 25 x dt (`bVelocityAffectsWalking`),
        // Z dropped; the move is DesiredMove x dt.
        let mut desired = p.velocity;
        if let Some(v) = dishonored_world::volume_at(ctx.world.physics_volumes(), p.position).filter(|v| v.velocity_affects_walking) {
            desired += Vec3::new(v.zone_velocity.x, v.zone_velocity.y, 0.0) * 25.0 * ctx.dt;
        }
        let mut delta = desired * ctx.dt;
        // An accurate stop: the step that would reach the path's end lands on it.
        let mut landed = false;
        if let (true, Some(end)) = (p.nav_walk, p.nav_end) {
            let to = (end - p.position).truncate();
            if delta.truncate().length() >= to.length() {
                delta = Vec3::new(to.x, to.y, 0.0);
                landed = true;
            }
        }
        if p.nav_walk && nav_walk_move(p, delta, ctx) {
            if landed {
                p.velocity = Vec3::ZERO;
            }
            return;
        }
        walk_move(p, delta, ctx);
        if landed {
            p.velocity = Vec3::ZERO;
        }
    }
    if p.mode != MoveMode::Walking {
        return;
    }

    // Floor: keep between MINFLOORDIST and MAXFLOORDIST above a walkable floor, else fall.
    match find_floor(p, ctx) {
        Some((dist, n)) if n.z >= cfg.walkable_floor_z => {
            if dist > MAX_FLOOR_DIST || dist < MIN_FLOOR_DIST {
                move_actor(p, Vec3::Z * (FLOOR_DIST - dist), ctx);
            }
            p.floor_normal = n;
        }
        _ => {
            // Off a ledge or onto a slope too steep: fall with the walking velocity.
            p.velocity = (p.position - old) / ctx.dt;
            p.velocity.z = 0.0;
            p.mode = MoveMode::Falling;
            p.fall_time = 0.0;
            return;
        }
    }
    // The velocity is what the move achieved (horizontal).
    if !p.slide.active {
        p.velocity = (p.position - old) / ctx.dt;
    }
    p.velocity.z = 0.0;
}

/// `StatePlayerMasterJump` style 0 (`eDisJumpStyle_FixedNormalJump_FixedPowerJump`, tick
/// 0x00A76110): while rising, the time the jump button has been held accumulates (reset when
/// released); at the top of the jump (`Velocity.Z <= 1e-4`) a jump held for more than 0.05 s
/// becomes a power jump when `Attribute_JumpZ_PowerJump` is positive (Agility):
/// `Velocity.Z = JumpZ_PowerJump` (0x00A71370); otherwise the jump hands over to falling.
fn jump_tick(p: &mut PlayerState, input: &PlayerInput, power_jump_z: f32, dt: f32, events: &mut Vec<SimEvent>) {
    if !p.jump.active {
        return;
    }
    if p.velocity.z <= 1e-4 {
        if !p.jump.powered && p.jump.held > MAX_TIME_STEP && power_jump_z > 1e-4 {
            p.velocity.z = power_jump_z;
            p.jump.powered = true;
            events.push(SimEvent::PowerJump { id: p.id });
        } else {
            p.jump.active = false;
            return;
        }
    }
    if !p.jump.powered {
        p.jump.held = if input.pressed(buttons::JUMP) { p.jump.held + dt } else { 0.0 };
    }
}

/// `APawn::physFalling` (0x006BF0E0) with `bLimitFallAccel` (Engine `Default__Pawn`):
/// - air control: `AirControl`, zeroed when the air-control move would run into something
///   within the tick; the acceleration is limited to `AccelRate * AirControl` (plus
///   `(10 - speed2d) / dt` below 10 uu/s); at or above `GroundSpeed` the horizontal speed may not
///   grow (`MaxAirSpeed`), or with `AirControl <= 0.05` the acceleration is 1;
/// - `v1 = NewFallVelocity(v0, a + g, dt)`, move `v1 dt`;
/// - landing on a walkable surface; walls slide (`ComputeSlideVector`, two-wall adjust);
/// - after the move, `v = 2 (moved / dt) - v0` when rising or still accelerating down, capped
///   at the volume's terminal velocity.
pub(crate) fn falling(p: &mut PlayerState, input: &PlayerInput, can_move: bool, dt: f32, ctx: &MoveCtx, events: &mut Vec<SimEvent>) {
    let cfg = ctx.cfg;
    p.fall_time += dt;
    // Leaving the ground ends a slide (the slide physics exits on a fall, 0x00A6F440).
    p.slide.active = false;
    let power = if p.kind == PawnKind::Rat { 0.0 } else { cfg.power_jump_z };
    jump_tick(p, input, power, dt, events);

    // The mantle while airborne (0x00ADBD00, falling branch): every tick with `m_bMantleAllowed`
    // and the Jump button down; a Jump press re-allows it (`DoJump` -> 0x00ADBE70). The stick
    // plays no part (the finder searches along the facing). A step-up leaves the pawn falling.
    if can_move && p.kind != PawnKind::Rat {
        let mantled = if p.just_pressed(input, buttons::JUMP) {
            jump_press_mantle(p, ctx, events)
        } else {
            p.mantle_allowed && input.pressed(buttons::JUMP) && try_mantle(p, ctx, events)
        };
        if mantled {
            p.jump.active = false;
            if p.mode != MoveMode::Falling {
                return;
            }
        }
    }

    let mut accel = if can_move { input_accel(p, input, cfg) } else { Vec3::ZERO };
    accel.z = 0.0;
    let mut tick_air = cfg.air_control;
    if tick_air > 0.05 {
        let dir = Vec2::new(accel.x, accel.y).normalize_or_zero();
        let test = (Vec2::new(p.velocity.x, p.velocity.y) + dir * cfg.accel_rate * tick_air) * dt;
        if test != Vec2::ZERO {
            let b = pawn_box(p, cfg);
            if ctx.world.sweep_box(&b, Vec3::new(test.x, test.y, 0.0), channel::PAWN).is_some() {
                tick_air = 0.0;
            }
        }
    }
    let speed2d = p.velocity.truncate().length();
    let mut max_accel = cfg.accel_rate * tick_air;
    let mut max_air_speed = 0.0;
    if speed2d < 10.0 && tick_air > 0.0 {
        max_accel += (10.0 - speed2d) / dt;
    } else if speed2d >= cfg.run_speed {
        if tick_air <= 0.05 {
            max_accel = 1.0;
        } else {
            max_air_speed = speed2d;
        }
    }
    if accel.length_squared() > max_accel * max_accel {
        accel = accel.normalize_or_zero() * max_accel;
    }

    let mut remaining = dt;
    let mut iterations = 0;
    while remaining > 0.0 && iterations < 8 {
        iterations += 1;
        let tick = if remaining > MAX_TIME_STEP { (remaining * 0.5).min(MAX_TIME_STEP) } else { remaining };
        remaining -= tick;
        let old_loc = p.position;
        let old_vel = p.velocity;
        // NewFallVelocity (0x006A3860): v (1 - fluid dt) + a (1 - buoyancy) dt; out of water both are 0.
        let mut v = old_vel + (accel + Vec3::Z * cfg.gravity_z) * tick;
        if max_air_speed > 0.0 && v.truncate().length_squared() > max_air_speed * max_air_speed {
            let xy = v.truncate().normalize_or_zero() * max_air_speed;
            v.x = xy.x;
            v.y = xy.y;
        }
        p.velocity = v;
        // physFalling: the move is (Velocity + ZoneVelocity) x dt.
        let zone = dishonored_world::volume_at(ctx.world.physics_volumes(), p.position).map(|z| z.zone_velocity).unwrap_or(Vec3::ZERO);
        let adjusted = (v + zone) * tick;
        let impact_speed = -v.z;
        let mut hit_wall = false;
        if let Some((n, t)) = move_actor(p, adjusted, ctx) {
            if n.z >= cfg.walkable_floor_z {
                // processLanded: the velocity is the measured one when enough of the tick moved.
                if t > 0.1 && t * tick > 0.003 {
                    p.velocity = (p.position - old_loc) / (tick * t);
                }
                let speed = -p.velocity.z;
                land(p, speed, n, events);
                return;
            }
            hit_wall = true;
            // A blasted NPC's wall contact (normal, speed before it) for the Windblast wall rule
            // (`left_hand::windblast_wall_hits`).
            if p.windblasted_by.is_some() && p.wall_hit.is_none() {
                p.wall_hit = Some((n, v.length()));
            }
            // ComputeSlideVector (slot 290): slide, never rising faster than the original move.
            let mut delta = (adjusted - n * adjusted.dot(n)) * (1.0 - t);
            if delta.z > 0.0 {
                delta.z = delta.z.min(adjusted.z * (1.0 - t));
            }
            if delta.dot(adjusted) >= 0.0 {
                if let Some((n2, t2)) = move_actor(p, delta, ctx) {
                    if n2.z >= cfg.walkable_floor_z {
                        land(p, impact_speed.max(0.0), n2, events);
                        return;
                    }
                    let desired = adjusted.normalize_or_zero();
                    let adj = two_wall_adjust(desired, delta, n2, n, t2);
                    let ditch = n.z > 0.0 && n2.z > 0.0 && adj.z == 0.0 && n2.dot(n) < 0.0;
                    let third = move_actor(p, adj, ctx);
                    if ditch || third.is_some_and(|(n3, _)| n3.z >= cfg.walkable_floor_z) {
                        let nn = third.map_or(n2, |h| h.0);
                        land(p, impact_speed.max(0.0), nn, events);
                        return;
                    }
                }
            }
        }
        // Entered deep water during the move: startSwimming.
        if let Some(w) = ctx.world.water_at(p.position) {
            if !w.shallow {
                p.velocity = (p.position - old_loc) / tick;
                start_swimming(p, events);
                return;
            }
        }
        // Velocity from the move; twice the average less the old velocity while rising or
        // speeding down (physFalling tail, 0x006BF0E0). After a wall the saved old horizontal
        // velocity is overwritten by the measured one before the formula, the old vz is kept:
        // the horizontal part stays measured, only z is doubled.
        let measured = (p.position - old_loc) / tick;
        p.velocity = measured;
        if measured.z < old_vel.z || old_vel.z >= 0.0 {
            let old = if hit_wall { Vec3::new(measured.x, measured.y, old_vel.z) } else { old_vel };
            p.velocity = measured * 2.0 - old;
        }
        let terminal = cfg.terminal_velocity;
        if p.velocity.length_squared() > terminal * terminal {
            p.velocity = p.velocity.normalize_or_zero() * terminal;
        }
    }
}

/// The minimum momentum of a throw (0x00BE8710, run by the throw state's entry 0x00A86B90): a
/// horizontal part shorter than `min_2d` is scaled up to it (kept zero when it is zero), and
/// `z` is raised to at least `min_z`.
pub fn clamp_throw_momentum(m: Vec3, min_2d: f32, min_z: f32) -> Vec3 {
    let mut out = m;
    let h2 = m.x * m.x + m.y * m.y;
    if h2.sqrt() < min_2d {
        let n = if h2 >= 1e-8 { Vec2::new(m.x, m.y) / h2.sqrt() } else { Vec2::ZERO };
        out.x = n.x * min_2d;
        out.y = n.y * min_2d;
    }
    out.z = out.z.max(min_z);
    out
}

/// `StateNPCMasterThrown.m_ThrowDirection` for a throw: the throw start (0x00A81860) turns the
/// pawn to face against the momentum, plus 90 (1), -90 (2) or 180 degrees (3) of yaw; the
/// direction is the one that keeps the pawn's current facing (LIKELY: the code choosing it was
/// not found). 0 / 1 / 2 / 3 play `Generic_WindBlast_Front` / `_Left` / `_Right` / `_Back` (the
/// `PushToRagdoll` chain's Windblast group, indexed by the direction).
pub fn throw_direction(yaw: f32, momentum: Vec3) -> usize {
    let back = Vec2::new(-momentum.x, -momentum.y);
    if back.length_squared() < 1e-8 {
        return 0;
    }
    let face = back.y.atan2(back.x).to_degrees();
    let rel = (yaw - face + 540.0).rem_euclid(360.0) - 180.0;
    if rel.abs() <= 45.0 {
        0
    } else if rel > 45.0 && rel <= 135.0 {
        1
    } else if rel < -45.0 && rel >= -135.0 {
        2
    } else {
        3
    }
}

/// A thrown NPC (the Windblast's `TakeDamage` on an NPC pawn, `StateNPCMasterThrown`): the
/// momentum is clamped (`clamp_throw_momentum`), the velocity is `momentum /
/// m_fEffectiveMassForThrowing` (0x00BEB6E0) and, on foot or in the air, the pawn flies
/// (0x00A81860: `PHYS_Flying`, `AirSpeed = |v|`) until the Windblast throw animation of its
/// direction reaches its `DishonoredNotify_Ragdoll`. `anim_dt` is the animation's first update,
/// which runs in the hit's tick (the original trace: the ragdoll at the 28th update).
pub fn start_throw(p: &mut PlayerState, momentum: Vec3, th: &dishonored_ir::gameplay::ThrowConfig, anim_dt: f32) {
    let m = clamp_throw_momentum(momentum, th.min_momentum_2d, th.min_momentum_z);
    let v = m / th.effective_mass;
    p.velocity = v;
    if matches!(p.mode, MoveMode::Walking | MoveMode::Falling) {
        p.mode = MoveMode::Falling;
        p.fall_time = 0.0;
        p.jump.active = false;
        p.slide.active = false;
        p.thrown = crate::state::ThrownState {
            active: true,
            time: anim_dt,
            ragdoll_at: th.windblast_ragdoll_notify[throw_direction(p.yaw, m)],
            air_speed: v.length(),
            friction: th.fluid_friction * 0.5,
            ragdoll: false,
            rest_timer: THROW_REST_TEST_DELAY,
            prev_scale: 0.0,
        };
    }
}

/// `m_fBodyAtRestTestTimer` as the throw state's enter sets it (0x00A9FAA0: 0.5, a constant of
/// the code).
pub const THROW_REST_TEST_DELAY: f32 = 0.5;

/// The thrown pawn's rest test and get-up (the throw state's tick 0x00A98DD0, after the move):
/// while `m_fBodyAtRestTestTimer` runs it only counts down; then, when the pawn's time dilation
/// was and is above zero, the pawn is at rest once both horizontal Velocity components are below
/// `m_fMaxRagdollSpeedToRecover x CustomTimeDilation` (`FVector::IsNearlyZero` of (vx, vy, 0),
/// 0x004019A0; the original also takes a sleeping PhysX body as at rest, which the sim has
/// not). At rest the pawn walks again (`SetPhysics(PHYS_Walking)`) and goes to
/// `StateNPCMasterStunned` with `eStunEnterAnim_RagdollFront` (0x00A814B0): the `Stun` chain's
/// `RagdollToFront`, its loop step for `m_fStunDuration` (0 here: one tick) and one recovery
/// sequence, the pawn held all along (`action_lock`). Until then the thrown pawn is held too.
/// The ragdoll itself is the sim's falling and walking moves (P-5: its PhysX dynamics, hence
/// when the original's body comes to rest, are not modelled).
pub fn step_thrown(p: &mut PlayerState, th: &dishonored_ir::gameplay::ThrowConfig, scale: f32, dt: f32) {
    if !p.alive || !(p.thrown.active || p.thrown.ragdoll) {
        return;
    }
    let t = &mut p.thrown;
    let mut rest = false;
    if t.rest_timer > 0.0 {
        t.rest_timer -= dt;
    } else if t.prev_scale > 0.0 && scale > 0.0 {
        let thr = th.max_ragdoll_speed_to_recover * scale;
        rest = p.velocity.x.abs() < thr && p.velocity.y.abs() < thr;
    }
    t.prev_scale = scale;
    if !rest {
        p.action_lock = p.action_lock.max(dt);
        return;
    }
    p.thrown = crate::state::ThrownState::default();
    if p.mode == MoveMode::Falling {
        p.mode = MoveMode::Walking;
    }
    let n = th.getup_recover.len();
    let recover = if n == 0 { 0.0 } else { th.getup_recover[((p.roll() * n as f32) as usize).min(n - 1)] };
    p.action_lock = th.getup_anim + DT + recover;
}

/// The Flying phase of a throw (`physFlying`, then the state's tick 0x00A98DD0 with `m_bRagdoll`
/// clear): `CalcVelocity` with no acceleration, friction `FluidFriction / 2`, fluid, no braking
/// (0x006ACD50: `v (1 - f dt)` twice per tick, measured 0.99500 / tick on the original trace),
/// speed capped at `AirSpeed`; the move slides along what it hits (no landing in `PHYS_Flying`)
/// and the velocity is the move's; then the state adds `GravityZ dt` to `Velocity.z` (the
/// trace's first flight step has no gravity in it: 406.6 uu/s up against the 412 thrown). The
/// throw animation advances; at its ragdoll notify the pawn goes to the ragdoll, which the sim
/// models with the falling move from the flight's velocity (the ragdoll impulse is `|v|` along
/// `v`, 0x00A98DD0 with `m_bUseMomemtumAsForce` clear; its per-body scale is not decoded, P-5).
pub(crate) fn flying(p: &mut PlayerState, ctx: &MoveCtx) {
    let dt = ctx.dt;
    let cfg = ctx.cfg;
    let (v, _) = calc_velocity(p.velocity, Vec3::ZERO, Vec3::ZERO, dt, p.thrown.air_speed, p.thrown.friction, true, false, None, 1.0, cfg.accel_rate);
    p.velocity = v;
    let zone = dishonored_world::volume_at(ctx.world.physics_volumes(), p.position).map(|z| z.zone_velocity).unwrap_or(Vec3::ZERO);
    let adjusted = (v + zone) * dt;
    let old = p.position;
    if let Some((n, t)) = move_actor(p, adjusted, ctx) {
        let delta = (adjusted - n * adjusted.dot(n)) * (1.0 - t);
        if delta.dot(adjusted) > 0.0 {
            move_actor(p, delta, ctx);
        }
    }
    p.velocity = (p.position - old) / dt;
    p.velocity.z += cfg.gravity_z * dt;
    p.thrown.time += dt;
    if p.thrown.time >= p.thrown.ragdoll_at {
        p.thrown.active = false;
        p.thrown.ragdoll = true;
    }
}

fn land(p: &mut PlayerState, impact_speed: f32, normal: Vec3, events: &mut Vec<SimEvent>) {
    p.mode = MoveMode::Walking;
    p.velocity.z = 0.0;
    p.floor_normal = normal;
    p.jump.active = false;
    events.push(SimEvent::Landed { id: p.id, speed: impact_speed, fall_time: p.fall_time });
    p.fall_time = 0.0;
}

/// The spot the mantle edge finder publishes (`DisMantleSpot`, 0x00AC6D40 from `CheckCanStand`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MantleSpot {
    /// `m_StartLocation`: where the last forward box trace stopped against the face, at the
    /// standing half height above the pawn's feet.
    pub location: Vec3,
    /// `m_StartYaw` as a horizontal unit vector: the direction of the clear trace over the edge.
    pub dir: Vec3,
    /// `m_fEdgeHeight`: above the feet, the bottom of the first crouched box that passed over the
    /// edge (on the search's step grid, not the top surface's own height).
    pub edge_height: f32,
    /// `m_bRequiresImpactAnim`: the search started at the ledge-grab height (falling fast).
    pub requires_impact_anim: bool,
    /// `m_bForcesCrouch`: a standing box does not pass over the edge.
    pub forces_crouch: bool,
}

/// The mantle edge finder (`DishonoredMantleEdgeFinderComponent`, a native state machine run in
/// 0.25 ms slices; here run to completion when asked, so the original's lag of a few frames is
/// not reproduced). All traces are box traces of the owner, `r` its radius, `c` =
/// `Pawn.CrouchHeight` (`m_fCrouchHeight`, the crawl half height), `H` the standing half height,
/// heights measured from the cylinder's bottom (the feet):
/// - start height (0x00AD69B0): nothing when falling faster than `m_fMaxFallSpeedForMantle`;
///   faster than `m_fFallSpeedForLedgeGrab` `m_fLedgeGrabMantleMinEdgeHeight` (and the impact
///   flag), else the tweak's `m_MantleMinEdgeHeight`, less one search step, not below 0;
/// - `FindClearDistanceAboveOwner` (0x00AD6410, setup 0x00AC6B80): a (r, r, c) box with its
///   bottom at the feet swept up `m_MantleMaxEdgeHeight - 2c + 2H`; the clear height is the box
///   top where it stopped;
/// - `ForwardSearches` (0x00ADA6F0 setup, 0x00AD6600 step): (r, r, c) boxes centred from the
///   start height + c up to `min(clear - c, m_MantleMaxEdgeHeight + c)` in steps of
///   `m_LineCheckStepSize`, the grid placed so that its last height is 0.1 below the end; each
///   is swept `m_MantleEdgeSearchDist` along the facing. A blocking face within
///   `m_MaxVerticalAngleForEdgeFace` of vertical and `m_MaxHorizontalAngleForEdgeFace` of the
///   facing moves the search start to where the box stopped and re-aims it along the face
///   normal, `m_fMantleForwardMoveAmount` long after the first face, then the unused part of the
///   last one; other faces are passed over. The first clear trace after a face is the edge;
/// - `CheckDestinationValid` (0x00ADA4C0): the box at the end of that trace swept down one
///   step plus 1; a floor steeper than `m_MaxSlopeAngleForEdgeTop` rejects the spot (an actor
///   with `DisMantleableInterface` may still pass with its 6 x 6 grid, 0x00AD5F40: the port's
///   world has no such actors);
/// - `CheckCanStand` (0x00ACCAD0): a standing (r, r, H) box with its bottom at the edge height,
///   swept along the same trace, must be clear and fit under the clear height, else the spot
///   forces a crouch.
pub fn find_mantle_spot(p: &PlayerState, facing: Vec3, min_edge: f32, ctx: &MoveCtx) -> Option<MantleSpot> {
    let cfg = ctx.cfg;
    let m = &cfg.mantle;
    let step = m.line_check_step_size;
    if step <= 0.0 {
        return None;
    }
    // 0x00AD69B0: the owner's vertical velocity picks the start height. (The step it subtracts is
    // the state's previous one, the tweak's after the first search.)
    let vz = p.velocity.z;
    if vz < -m.max_fall_speed_for_mantle {
        return None;
    }
    let impact = vz < -m.fall_speed_for_ledge_grab;
    let start_h = ((if impact { m.ledge_grab_min_edge_height } else { min_edge }) - step).max(0.0);
    let r = radius(p, cfg);
    let c = cfg.crawl_half_height;
    let stand = cfg.collision_half_height;
    let foot = feet(p, cfg);
    let swept = |center: Vec3, hh: f32, delta: Vec3| ctx.world.sweep_box(&PawnBox::new(center, r, hh), delta, channel::PAWN);

    // FindClearDistanceAboveOwner.
    let up = m.max_edge_height - 2.0 * c + 2.0 * stand;
    let t = match swept(foot + Vec3::Z * c, c, Vec3::Z * up) {
        Some(h) if h.start_penetrating => 0.0,
        Some(h) => h.fraction,
        None => 1.0,
    };
    let clear = t * up + 2.0 * c;

    // ForwardSearches.
    let first_h = start_h + c;
    let end = (clear - c).min(m.max_edge_height + c);
    let range = end - first_h;
    let mut h = range - (range / step).floor() * step - 0.1 + first_h;
    let facing = Vec3::new(facing.x, facing.y, 0.0).normalize_or_zero();
    let max_horizontal = m.max_horizontal_angle_edge_face.to_radians();
    let mut start = foot;
    let mut dir = facing;
    let mut dist = m.edge_search_dist;
    let mut found = false;
    let mut first = true;
    loop {
        let center = Vec3::new(start.x, start.y, foot.z + h);
        let delta = dir * dist;
        match swept(center, c, delta) {
            Some(hit) => {
                let n = hit.normal;
                let t = if hit.start_penetrating { 0.0 } else { hit.fraction };
                let vertical = 90.0 - n.z.clamp(-1.0, 1.0).acos().to_degrees() <= m.max_vertical_angle_edge_face;
                let horizontal = (-(facing.x * n.x + facing.y * n.y)).clamp(-1.0, 1.0).acos() <= max_horizontal;
                if vertical && horizontal {
                    found = true;
                    let at = center + delta * t;
                    start = Vec3::new(at.x, at.y, start.z);
                    dir = Vec3::new(-n.x, -n.y, 0.0);
                    dist = if first { m.forward_move_amount } else { dist * (1.0 - t) };
                    first = false;
                }
            }
            None if found => {
                // CheckDestinationValid.
                if let Some(down) = swept(center + delta, c, -Vec3::Z * (step + 1.0)) {
                    if down.normal.z.clamp(-1.0, 1.0).acos() > m.max_slope_angle_edge_top.to_radians() {
                        return None;
                    }
                }
                // CheckCanStand.
                let edge = h - c;
                let s = Vec3::new(start.x, start.y, foot.z + edge + stand);
                let forces_crouch = if s.z + stand <= foot.z + clear { swept(s, stand, delta).is_some() } else { true };
                return Some(MantleSpot {
                    location: Vec3::new(start.x, start.y, foot.z + stand),
                    dir: Vec3::new(delta.x, delta.y, 0.0).normalize_or(facing),
                    edge_height: edge,
                    requires_impact_anim: impact,
                    forces_crouch,
                });
            }
            None => {}
        }
        h += step;
        if h > end {
            return None;
        }
    }
}

/// The mantle request (0x00AD78D0) with the edge finder's spot along the pawn's facing; the
/// stick plays no part. Returns true when the pawn mantled or stepped up.
pub fn try_mantle(p: &mut PlayerState, ctx: &MoveCtx, events: &mut Vec<SimEvent>) -> bool {
    let fwd = forward_vector(p.yaw);
    match find_mantle_spot(p, fwd, ctx.cfg.mantle.min_edge_height, ctx) {
        Some(spot) => {
            start_mantle(p, spot, ctx, events);
            true
        }
        None => false,
    }
}

/// `DoJump`'s mantle attempt (0x00ADBE70): `m_bMantleAllowed` is set, then the request is made
/// (the Jump button is down on its press).
fn jump_press_mantle(p: &mut PlayerState, ctx: &MoveCtx, events: &mut Vec<SimEvent>) -> bool {
    p.mantle_allowed = true;
    try_mantle(p, ctx, events)
}

/// The mantle the original tries when a Blink ends (`ADishonoredPlayerPawn` 0x00AD78D0
/// called from 0x00BE7420): the Blink edge finder uses `pMantleBlinkTweaks`, which leave
/// `m_MantleMinEdgeHeight` at its class default 0 and match the normal tweaks otherwise.
pub fn try_blink_mantle(p: &mut PlayerState, ctx: &MoveCtx, events: &mut Vec<SimEvent>) -> bool {
    let fwd = forward_vector(p.yaw);
    match find_mantle_spot(p, fwd, 0.0, ctx) {
        Some(spot) => {
            start_mantle(p, spot, ctx, events);
            true
        }
        None => false,
    }
}

/// 0x00AD78D0 with a valid spot: an edge no higher than `m_LowMantleMaxEdgeHeight` with
/// `m_bLowMantleUseStepUp` and no crouch needed is the step-up (0x00AD35E0); any other enters
/// `StatePlayerMasterMantle` (0x00ACD820, which clears `m_bMantleAllowed`).
fn start_mantle(p: &mut PlayerState, spot: MantleSpot, ctx: &MoveCtx, events: &mut Vec<SimEvent>) {
    let m = &ctx.cfg.mantle;
    events.push(SimEvent::Mantle { id: p.id, edge_height: spot.edge_height });
    if m.low_use_step_up && spot.edge_height <= m.low_max_edge_height && !spot.forces_crouch {
        // 0x00AD35E0: three moves of the pawn (MoveActor, blocked by the world): to the spot's
        // location, up by the edge height, forward along the spot's yaw by
        // `m_fMantleForwardMoveAmount`. The velocity and the physics are left as they were (the
        // camera is smoothed by `StepUpMantle`, `ADD_Head_Mantle_Low`, over
        // `m_fLowMantleStepUpBlendTime`).
        move_actor(p, spot.location - p.position, ctx);
        move_actor(p, Vec3::Z * spot.edge_height, ctx);
        move_actor(p, spot.dir * m.forward_move_amount, ctx);
        p.traversal.step_ups = p.traversal.step_ups.wrapping_add(1);
        return;
    }
    p.mantle_allowed = false;
    if spot.forces_crouch && !p.crouched {
        p.crouched = true;
    }
    p.velocity = Vec3::ZERO;
    p.jump.active = false;
    // The finder's classes: low up to `m_LowMantleMaxEdgeHeight`, medium up to
    // `m_MediumMantleMaxEdgeHeight`, else high.
    let kind = if spot.edge_height <= m.low_max_edge_height {
        0
    } else if spot.edge_height <= m.medium_max_edge_height {
        1
    } else {
        2
    };
    let anim = m.anims.get(kind);
    // The mantle state sets the pawn at the spot's location at once (0x00A6EC20, no intro after
    // walking or falling); the sequence's root motion carries it from there, its forward part
    // as authored and its rise scaled to the edge height (0x00A8F9B0).
    let fwd_end = anim.and_then(|a| a.root.last()).map(|e| e[0]).unwrap_or(m.forward_move_amount);
    let from = spot.location;
    let to = from + spot.dir * fwd_end + Vec3::Z * spot.edge_height;
    let rate = m.anim_rate.max(0.01);
    let (anim_i, duration) = match anim {
        Some(a) => (Some(kind as u8), a.duration / rate),
        None => (None, 0.0),
    };
    p.position = from;
    p.traversal = Traversal { from, to, elapsed: 0.0, duration, edge_height: spot.edge_height, anim: anim_i, rate, step_ups: p.traversal.step_ups };
    p.mode = MoveMode::Mantling;
}
#[cfg(test)]
mod tests {
    use super::*;

    /// `Sword_Ready_MantleHigh` root bone (forward, up) per frame, decoded from Startup
    /// `Ply_Sword_Locomotion_as` (Edge, 31 frames at 29.03 Hz; `anim_root`, rounded to 1 uu).
    const HIGH_ROOT: [[f32; 2]; 31] = [
        [0.0, 0.0], [0.0, 0.0], [0.0, 2.0], [0.0, 3.0], [0.0, 6.0], [0.0, 9.0], [0.0, 12.0], [0.0, 16.0], [0.0, 19.0], [0.0, 23.0], [0.0, 27.0],
        [0.0, 29.0], [1.0, 32.0], [2.0, 37.0], [3.0, 43.0], [5.0, 50.0], [7.0, 58.0], [9.0, 65.0], [11.0, 71.0], [14.0, 77.0], [16.0, 80.0],
        [19.0, 82.0], [22.0, 83.0], [25.0, 83.0], [27.0, 82.0], [30.0, 80.0], [33.0, 79.0], [35.0, 78.0], [38.0, 79.0], [40.0, 80.0], [40.0, 80.0],
    ];

    fn high() -> dishonored_ir::gameplay::MantleAnim {
        dishonored_ir::gameplay::MantleAnim { sequence: "Sword_Ready_MantleHigh".into(), travel_z: 80.0, sample_rate: 29.032259, duration: 1.0333333, root: HIGH_ROOT.to_vec() }
    }

    /// The sim's mantle path follows the decoded root-motion curve frame by frame: forward as
    /// authored, the rise scaled by height / `m_fMantleTravelDistanceZ` (0x00A8F9B0).
    #[test]
    fn mantle_path_is_the_sequence_root_motion() {
        let a = high();
        let from = Vec3::new(100.0, 50.0, 10.0);
        // A 200 uu rise straight along +Y, ending 40 uu on (the sequence's forward travel).
        let to = from + Vec3::new(0.0, 40.0, 200.0);
        let k = 200.0 / 80.0;
        for (f, r) in HIGH_ROOT.iter().enumerate() {
            let p = mantle_root_position(from, to, &a, f as f32 / a.sample_rate);
            let want = from + Vec3::new(0.0, r[0], r[1] * k);
            assert!((p - want).length() < 1e-3, "frame {f}: {p} vs {want}");
        }
        assert!((mantle_root_position(from, to, &a, a.duration) - to).length() < 1e-3);
        // Between frames: linear.
        let mid = mantle_root_position(from, to, &a, 15.5 / a.sample_rate);
        assert!((mid.z - (from.z + 54.0 * k)).abs() < 1e-3, "{mid}");
    }

    /// `advance_traversal` plays the sequence at `Attribute_MantleAnimRate` and ends on `to`.
    #[test]
    fn mantle_traversal_runs_the_sequence() {
        let cfg = crate::SimConfig::from_json(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json")).unwrap()).unwrap();
        let mut mv = cfg.gameplay.movement.clone();
        mv.mantle.anims = vec![high(), high(), high()];
        mv.mantle.anim_rate = 1.5;
        let mut p = PlayerState::new(1, Vec3::ZERO, 0.0);
        let (from, to) = (Vec3::ZERO, Vec3::new(40.0, 0.0, 160.0));
        p.traversal = Traversal { from, to, elapsed: 0.0, duration: high().duration / 1.5, edge_height: 160.0, anim: Some(2), rate: 1.5, step_ups: 0 };
        let mut ticks = 0;
        while !advance_traversal(&mut p, &mv, 1.0 / 60.0) {
            ticks += 1;
            // 14 ticks in: 0.35 s of the sequence at rate 1.5.
            if ticks == 14 {
                let want = mantle_root_position(from, to, &high(), 14.0 / 60.0 * 1.5);
                assert!((p.position - want).length() < 1e-3);
            }
            assert!(ticks < 60);
        }
        assert!((p.position - to).length() < 1e-3, "{}", p.position);
        assert_eq!(ticks, (high().duration / 1.5 * 60.0).ceil() as i32 - 1);
    }
    #[test]
    fn braking_matches_the_substep_rule() {
        // One 1/60 s sub-step at friction 8: v (1 - 2 x 8 / 60).
        let v = apply_braking(Vec3::new(360.0, 0.0, 0.0), DT, 8.0);
        assert!((v.x - 360.0 * (1.0 - 16.0 / 60.0)).abs() < 1e-3, "{v}");
        // Below 12.7 uu/s it stops.
        assert_eq!(apply_braking(Vec3::new(15.0, 0.0, 0.0), DT, 8.0), Vec3::ZERO);
        // A 0.1 s tick brakes in 0.03 s sub-steps and returns their average.
        let w = apply_braking(Vec3::new(400.0, 0.0, 0.0), 0.1, 8.0);
        let mut x = 400.0f32;
        let mut sum = 0.0;
        for s in [0.03f32, 0.03, 0.03, 0.01] {
            x -= x * 2.0 * s * 8.0;
            sum += x * s / 0.1;
        }
        assert!((w.x - sum).abs() < 1e-2, "{} vs {sum}", w.x);
    }

    #[test]
    fn calc_velocity_limits_acceleration_by_the_speed_modifier() {
        // Running with the sword out (modifier 0.9): accel 2000 is limited to 1800 and the
        // speed to 360.
        let (v, a) = calc_velocity(Vec3::ZERO, Vec3::X * 2000.0, Vec3::X, DT, 400.0, 8.0, false, true, None, 0.9, 2000.0);
        assert!((a.x - 1800.0).abs() < 1e-3);
        assert!((v.x - 1800.0 * DT).abs() < 1e-3);
        let mut v = Vec3::ZERO;
        for _ in 0..120 {
            v = calc_velocity(v, Vec3::X * 2000.0, Vec3::X, DT, 400.0, 8.0, false, true, None, 0.9, 2000.0).0;
        }
        assert!((v.x - 360.0).abs() < 1e-3, "{v}");
        // Turning: a velocity across the acceleration is pulled toward it by dt * friction.
        let (t, _) = calc_velocity(Vec3::Y * 300.0, Vec3::X * 2000.0, Vec3::X, DT, 400.0, 8.0, false, true, None, 1.0, 2000.0);
        assert!((t.y - 300.0 * (1.0 - 8.0 * DT)).abs() < 1e-3, "{t}");
    }

    #[test]
    fn two_wall_adjust_follows_the_crease() {
        let d = two_wall_adjust(Vec3::new(1.0, 1.0, 0.0).normalize(), Vec3::new(10.0, 10.0, 0.0), -Vec3::X, -Vec3::Y, 0.0);
        // Perpendicular walls: the crease is vertical; nothing horizontal remains.
        assert!(d.truncate().length() < 1e-4, "{d}");
    }
}

/// Slide impacts (`FUN_00a79bf0`, run by the slide tick 0x00A7B220 until the slide has hit
/// something): a line trace of `m_fSlideDamageTestDistance` from the pawn along its velocity;
/// the first pawn it meets (not immune, not asleep when `m_bSlideDamageIgnoresSleepers`) takes
/// the `m_SlideDamageInfo` damage of its class (`DishonoredNPCPawn` 0, rats 0, other actors 5)
/// with momentum `velocity x m_fSlideImpactDamageVelRatio`. NPC pawns take the momentum as a
/// velocity change; the player pawn's damage path drops it (U-051).
pub fn slide_impacts(state: &mut crate::state::WorldState, cfg: &MovementConfig, vitals: &dishonored_ir::gameplay::VitalsConfig, events: &mut Vec<SimEvent>) {
    for i in 0..state.players.len() {
        let p = state.players[i];
        if !p.alive || !p.slide.active || p.slide.hit {
            continue;
        }
        let dir = p.velocity.normalize_or_zero();
        if dir == Vec3::ZERO {
            continue;
        }
        let len = cfg.slide_damage_test_distance;
        // Nearest pawn whose collision cylinder the segment enters.
        let mut best: Option<(usize, f32)> = None;
        for (j, v) in state.players.iter().enumerate() {
            if j == i || !v.alive {
                continue;
            }
            if cfg.slide_damage_ignores_sleepers && v.condition == crate::state::Condition::Unconscious {
                continue;
            }
            let c = capsule(v, cfg);
            let Some(t) = segment_cylinder(p.position, dir * len, c.center, c.radius, c.half_height) else { continue };
            if best.is_none_or(|b| t < b.1) {
                best = Some((j, t));
            }
        }
        let Some((j, _)) = best else { continue };
        state.players[i].slide.hit = true;
        let amount = match state.players[j].kind {
            PawnKind::Civilian | PawnKind::Guard => cfg.slide_damage_npc,
            PawnKind::Rat => cfg.slide_damage_rat,
            PawnKind::Human => cfg.slide_damage_other,
        };
        let v = &mut state.players[j];
        if v.kind != PawnKind::Human {
            v.velocity += p.velocity * cfg.slide_impact_damage_vel_ratio;
        }
        if amount > 0.0 {
            crate::vitals::apply_damage(v, amount, Some(p.id), crate::events::DamageCause::Slide, events, vitals);
        }
    }
}

/// Entry parameter (0..1 along `d`) of the segment `o + t d` into a vertical cylinder.
fn segment_cylinder(o: Vec3, d: Vec3, c: Vec3, r: f32, hh: f32) -> Option<f32> {
    let (o2, d2) = (Vec2::new(o.x - c.x, o.y - c.y), Vec2::new(d.x, d.y));
    let a = d2.length_squared();
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    if a < 1e-8 {
        if o2.length_squared() > r * r {
            return None;
        }
    } else {
        let b = o2.dot(d2);
        let k = b * b - a * (o2.length_squared() - r * r);
        if k < 0.0 {
            return None;
        }
        let s = k.sqrt();
        t0 = t0.max((-b - s) / a);
        t1 = t1.min((-b + s) / a);
    }
    // Height slab.
    let (oz, dz) = (o.z - c.z, d.z);
    if dz.abs() < 1e-8 {
        if oz.abs() > hh {
            return None;
        }
    } else {
        let (za, zb) = ((-hh - oz) / dz, (hh - oz) / dz);
        t0 = t0.max(za.min(zb));
        t1 = t1.min(za.max(zb));
    }
    (t0 <= t1).then_some(t0)
}
