//! Swimming and breath, decoded from `Dishonored.exe` (research/dishonored/PLAYER.md
//! "Swimming"):
//!
//! - `APawn::physSwimming` (0x006B7820): net buoyancy and fluid friction scale with the
//!   submerged part of the cylinder (`GetNetBuoyancy`, 0x006A41F0); rising faster than 127 uu/s
//!   is scaled by `NetBuoyancy / Buoyancy`; `CalcVelocity(dir, dt, WaterSpeed, FluidFriction / 2,
//!   fluid, no brake, buoyant)`; leaving the water volume switches to falling, and a slow rise
//!   out of it becomes `0.4 |v.xy| + 40` (0x006B7820 tail).
//! - `StatePlayerMasterSwim`: the swim acceleration (0x00A60690) is `m_fMaxSwimAccel` during a
//!   stroke (`m_fCurStrokeTime`), else `m_fMinSwimAccel` below `m_fMaxSpeedNoStroke`, else 0.
//!   At the surface (head out, `m_pActiveWaterVolume` empty) a stroke is due every
//!   `m_fSurfaceSwimDelayBetweenStrokes`, but `m_bConstantSpeedAtSurface` suppresses it
//!   (0x00A76710). Underwater strokes come from the swim animation (`DisNotify_SwimStroke`), not
//!   decoded: the sim starts one every `m_fSurfaceSwimDelayBetweenStrokes` while swimming
//!   forward underwater (SPECULATIVE, U-243).
//! - Breath (0x00AD7BE0): while the head is under water the air capacity
//!   (`m_fApneaMaxDuration x (1 + Attribute_BreathCapacityIncrease)`, 0x00ACF570) runs down in
//!   real seconds; out of the water it refills at `max / m_fApneaRecoveryDuration` per second.
//!   With no air left (0x00ABB1E0), every `Attribute_DrowningDamageStep` seconds health loses
//!   `Attribute_DrowningDamageAmount` directly (not through the damage path), and the pawn dies
//!   below 1.

use crate::events::{DamageCause, SimEvent};
use crate::input::{PlayerInput, buttons};
use crate::movement::{MoveCtx, calc_velocity, eye_position, half_height, pawn_box, right_vector, speed_modifier, view_vector};
use crate::state::{MoveMode, PlayerState};
use dishonored_ir::gameplay::{MovementConfig, WaterMoveConfig};
use dishonored_world::channel;
use glam::Vec3;

/// `0x0103AB80`: rising faster than this in water is limited by the net buoyancy.
pub const BUOYANT_RISE_LIMIT: f32 = 127.0;

/// Air capacity: `m_fApneaMaxDuration x (1 + Attribute_BreathCapacityIncrease)`.
pub fn air_capacity(wc: &WaterMoveConfig, capacity_increase: f32) -> f32 {
    wc.apnea_max * (1.0 + capacity_increase)
}

/// Swim acceleration of `StatePlayerMasterSwim` (0x00A60690).
pub fn swim_accel(p: &PlayerState, wc: &WaterMoveConfig) -> f32 {
    if p.water.stroke > 0.0 {
        wc.max_swim_accel
    } else if p.velocity.length_squared() < wc.max_speed_no_stroke * wc.max_speed_no_stroke {
        wc.min_swim_accel
    } else {
        0.0
    }
}

fn leave_water(p: &mut PlayerState, events: &mut Vec<SimEvent>) {
    p.mode = MoveMode::Falling;
    p.fall_time = 0.0;
    if p.velocity.z > 0.0 && p.velocity.z < 160.0 {
        p.velocity.z = p.velocity.truncate().length() * 0.4 + 40.0;
    }
    p.water.stroke = 0.0;
    events.push(SimEvent::Swim { id: p.id, entered: false });
}

/// One tick of swimming physics.
pub fn swimming(p: &mut PlayerState, input: &PlayerInput, can_move: bool, sword_out: bool, ctx: &MoveCtx, events: &mut Vec<SimEvent>) {
    let cfg: &MovementConfig = ctx.cfg;
    let wc = &cfg.water;
    let dt = ctx.dt;
    let Some(w) = ctx.world.water_at(p.position).filter(|w| !w.shallow) else {
        leave_water(p, events);
        return;
    };
    // The mantle while swimming (0x00ADBD00, swimming branch): requested every tick the Jump
    // button is down (with `m_bAutomaticallyMantleInWater`, which no shipped tweak sets, the
    // forward input would do instead); the finder runs in the water as on land, along the
    // facing. A step-up leaves the pawn swimming.
    if can_move && p.kind != crate::state::PawnKind::Rat && input.pressed(buttons::JUMP) && crate::movement::try_mantle(p, ctx, events) && p.mode != MoveMode::Swimming {
        return;
    }
    let head = eye_position(p, cfg);
    let head_under = head.z < w.surface_z;

    // GetNetBuoyancy: the share of the cylinder below the surface.
    let hh = half_height(p, cfg);
    let frac = ((w.surface_z - (p.position.z - hh)) / (2.0 * hh)).clamp(0.0, 1.0);
    let net_buoyancy = wc.buoyancy * frac;
    if p.velocity.z > BUOYANT_RISE_LIMIT && wc.buoyancy != 0.0 {
        p.velocity.z = p.velocity.z / wc.buoyancy * net_buoyancy;
    }

    // Strokes.
    p.water.stroke = (p.water.stroke - dt).max(0.0);
    p.water.stroke_delay -= dt;
    let swimming_forward = can_move && input.forward > 0;
    if p.water.stroke_delay <= 0.0 {
        let underwater_stroke = head_under && swimming_forward;
        let surface_stroke = !head_under && !wc.constant_speed_at_surface;
        if underwater_stroke || surface_stroke {
            p.water.stroke = wc.stroke_time;
        }
        p.water.stroke_delay = wc.surface_stroke_delay;
    }

    // PlayerSwimming: acceleration along the view (pitch included) and strafe, up/down with
    // jump/crouch; magnitude from the swim state.
    let (f, r) = input.axes();
    let up = input.pressed(buttons::JUMP) as i32 as f32 - input.pressed(buttons::CROUCH) as i32 as f32;
    let dir = if can_move { (view_vector(p.yaw, p.pitch) * f + right_vector(p.yaw) * r + Vec3::Z * up).normalize_or_zero() } else { Vec3::ZERO };
    let accel = dir * swim_accel(p, wc);
    let modifier = speed_modifier(p, input, cfg, sword_out, dt);
    let (v, _) = calc_velocity(p.velocity, accel, dir, dt, wc.water_speed, w.fluid_friction * 0.5, true, false, Some((wc.buoyancy, cfg.gravity_z)), modifier, cfg.accel_rate);
    p.velocity = v;

    // swimMove: move, sliding along what blocks.
    let old = p.position;
    let delta = p.velocity * dt;
    let b = pawn_box(p, cfg);
    match ctx.world.sweep_box(&b, delta, channel::PAWN) {
        None => p.position += delta,
        Some(h) if h.start_penetrating => {}
        Some(h) => {
            p.position += delta * h.fraction;
            let rest = delta * (1.0 - h.fraction);
            let slide = rest - h.normal * rest.dot(h.normal);
            let b2 = pawn_box(p, cfg);
            match ctx.world.sweep_box(&b2, slide, channel::PAWN) {
                None => p.position += slide,
                Some(h2) if !h2.start_penetrating => p.position += slide * h2.fraction,
                _ => {}
            }
            p.velocity = (p.position - old) / dt;
        }
    }
    p.floor_normal = Vec3::Z;
    if ctx.world.water_at(p.position).is_none_or(|w| w.shallow) {
        leave_water(p, events);
    }
}

/// Breath and drowning for one pawn (real seconds). Returns true when the pawn drowned.
pub fn step_breath(p: &mut PlayerState, world: &dyn dishonored_world::WorldQuery, cfg: &MovementConfig, capacity_increase: f32, min_health: f32, dt: f32, events: &mut Vec<SimEvent>) -> bool {
    // The breath and drowning code is the player pawn's (DishonoredPlayerPawn tick).
    if !p.alive || p.kind != crate::state::PawnKind::Human {
        return false;
    }
    let wc = &cfg.water;
    let max = air_capacity(wc, capacity_increase);
    let head = eye_position(p, cfg);
    let under = world.water_at(head).is_some_and(|w| !w.shallow);
    p.water.under = under;
    if under {
        p.water.air_used = (p.water.air_used + dt).min(max);
    } else if p.water.air_used > 0.0 {
        let rate = if wc.apnea_recovery > 0.0 { max / wc.apnea_recovery } else { max };
        p.water.air_used = (p.water.air_used - rate * dt).max(0.0);
    }
    if p.water.air_used >= max - 1e-6 && max > 0.0 {
        p.water.drown_timer -= dt;
        if p.water.drown_timer < 0.0 {
            p.vitals.health = (p.vitals.health - wc.drowning_damage).max(min_health);
            if p.vitals.health < 1.0 {
                crate::vitals::kill(p, None, DamageCause::Drowning, events);
                return true;
            }
            p.water.drown_timer = wc.drowning_step;
        }
    } else {
        p.water.drown_timer = 0.0;
    }
    false
}

/// Air left (seconds), for the HUD.
pub fn air_left(p: &PlayerState, cfg: &MovementConfig, capacity_increase: f32) -> f32 {
    (air_capacity(&cfg.water, capacity_increase) - p.water.air_used).max(0.0)
}
