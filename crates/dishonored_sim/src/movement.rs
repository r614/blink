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

/// Lean spring (`m_LeanConstants`, integrator 0x00AC0A00: semi-implicit Euler,
/// `v += (-k (x - target) - c v) dt; x += v dt`), in units of the maximum angle.
fn step_lean(p: &mut PlayerState, target: f32, ctx: &MoveCtx) {
    let lc = &ctx.cfg.lean;
    p.lean_velocity += (lc.springiness * (target - p.lean) - lc.damping * p.lean_velocity) * ctx.dt;
    let pushed = p.lean + p.lean_velocity * ctx.dt;
    p.lean = pushed.clamp(-1.0, 1.0);
    // Soften (0x00AC5540): while the head is held at the limit the allowed angle grows toward
    // `m_fMaxLeanSoftenAngle` at `m_fMaxLeanSoftenSpeed`, otherwise it decays at that rate
    // (measured on the original: 15 -> 20 degrees, about 5 / s, U-048).
    let s = lc.soften_speed * ctx.dt;
    if target != 0.0 && pushed.abs() >= 1.0 {
        p.lean_soften = (p.lean_soften + (lc.soften_angle - p.lean_soften) * s).min(lc.soften_angle);
    } else {
        p.lean_soften = (p.lean_soften - p.lean_soften * s).max(0.0);
    }
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
fn walk_move(p: &mut PlayerState, delta: Vec3, ctx: &MoveCtx) {
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
    if can_move && p.just_pressed(input, buttons::JUMP) && p.kind != PawnKind::Rat {
        if try_mantle(p, input, ctx, events) {
            return;
        }
        if p.crouched {
            set_crouch(p, false, ctx);
        }
        if !p.crouched {
            // StatePlayerMasterJump enter (0x00A75EE0): `Velocity.Z = Attribute_JumpZ`
            // (`JumpZCarryingCorpse` while carrying), physics Falling.
            p.velocity.z = cfg.jump_z;
            p.mode = MoveMode::Falling;
            p.fall_time = 0.0;
            p.jump = crate::state::JumpState { active: true, held: 0.0, powered: false };
            events.push(SimEvent::Jumped { id: p.id });
            falling(p, input, can_move, ctx.dt, ctx, events);
            return;
        }
    }
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

    // Auto-mantle while airborne and pushing forward (Dishonored grabs ledges mid-jump).
    if can_move && input.forward > 0 && -p.velocity.z < cfg.mantle.max_fall_speed_for_mantle && p.kind != PawnKind::Rat {
        if try_mantle(p, input, ctx, events) {
            p.jump.active = false;
            return;
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
        // speeding down (after a wall the measured velocity is kept).
        let measured = (p.position - old_loc) / tick;
        p.velocity = measured;
        if !hit_wall && (measured.z < old_vel.z || old_vel.z >= 0.0) {
            p.velocity = measured * 2.0 - old_vel;
        }
        let terminal = cfg.terminal_velocity;
        if p.velocity.length_squared() > terminal * terminal {
            p.velocity = p.velocity.normalize_or_zero() * terminal;
        }
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
/// grid) are reduced to a box clearance test, standing then sneaking.
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
    let stand = PawnBox::new(dest_xy + Vec3::Z * (cfg.collision_half_height + FLOOR_DIST), r, cfg.collision_half_height);
    let (dest, crouch) = if box_fits(ctx.world, &stand) {
        (stand.center, false)
    } else {
        let low = PawnBox::new(dest_xy + Vec3::Z * (cfg.crouch_half_height + FLOOR_DIST), r, cfg.crouch_half_height);
        if !box_fits(ctx.world, &low) {
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
    p.jump.active = false;
    events.push(SimEvent::Mantle { id: p.id, edge_height: edge_h });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

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
