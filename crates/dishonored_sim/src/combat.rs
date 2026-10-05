//! Sword combat (M9): swing phases, block, parry, back and drop assassinations.
//!
//! Hit geometry (range 210, sweep width 60, assassinate range 275, health-ratio damage)
//! is CONFIRMED tweak data. Phase timings are SPECULATIVE (see `CombatTuning`).
//! Simulation owns timing; animation only presents it.

use crate::events::{DamageCause, SimEvent, SoundCategory};
use crate::input::{PlayerInput, buttons};
use crate::movement::{eye_position, forward_vector, view_vector};
use crate::state::{MoveMode, PlayerId, PlayerState, SwordPhase, WorldState};
use crate::tuning::CombatTuning;
use crate::vitals::{apply_damage, kill};
use dishonored_ir::gameplay::{MovementConfig, VitalsConfig};
use dishonored_world::{WorldQuery, channel, geom::closest_segment_segment};
use glam::Vec3;

/// Advance the attacker-side state machine and collect swing requests.
pub fn update_sword(p: &mut PlayerState, input: &PlayerInput, t: &CombatTuning, dt: f32, events: &mut Vec<SimEvent>) {
    let s = &mut p.sword;
    s.timer += dt;
    match s.phase {
        SwordPhase::Idle => {
            if !p.alive || matches!(p.mode, MoveMode::Mantling | MoveMode::Blinking) {
                return;
            }
            if input.pressed(buttons::BLOCK) {
                s.phase = SwordPhase::Blocking;
                s.timer = 0.0;
                s.block_time = 0.0;
            } else if input.buttons & buttons::PRIMARY != 0 && p.prev_buttons & buttons::PRIMARY == 0 {
                s.phase = SwordPhase::Startup;
                s.timer = 0.0;
                s.hit_done = false;
                events.push(SimEvent::SwordSwing { id: p.id });
                events.push(SimEvent::Sound { position: p.position, radius: 300.0, category: SoundCategory::SwordSwing, source: p.id });
            }
        }
        SwordPhase::Blocking => {
            s.block_time += dt;
            s.force_block -= dt;
            if !input.pressed(buttons::BLOCK) && s.force_block <= 0.0 {
                s.phase = SwordPhase::Idle;
                s.timer = 0.0;
                s.force_block = 0.0;
            }
        }
        SwordPhase::Startup if s.timer >= t.swing_startup => {
            s.phase = SwordPhase::Active;
            s.timer = 0.0;
        }
        SwordPhase::Active if s.timer >= t.swing_active => {
            s.phase = SwordPhase::Recovery;
            s.timer = 0.0;
        }
        SwordPhase::Recovery if s.timer >= t.swing_recovery => {
            s.phase = SwordPhase::Idle;
            s.timer = 0.0;
        }
        SwordPhase::Staggered if s.timer >= t.stagger_time => {
            s.phase = SwordPhase::Idle;
            s.timer = 0.0;
        }
        SwordPhase::Assassinating if s.timer >= t.assassination_time => {
            s.phase = SwordPhase::Idle;
            s.timer = 0.0;
            s.assassination_target = None;
        }
        _ => {}
    }
}

pub fn sword_locks_movement(p: &PlayerState) -> bool {
    matches!(p.sword.phase, SwordPhase::Staggered | SwordPhase::Assassinating)
}

/// Melee reach (0x00C0A060 with 0x00AB2050): `m_fMaxContextRange` scaled by the forward-speed
/// factor `1 + clamp((v . view - m_fMinSpeedRayScale) * m_fRaySpeedScale, 0, m_fRaySpeedScale_Max)`
/// blended by `m_fRayScalePercent`: `range * ((factor - 1) * percent + 1)`. Sprinting forward
/// at 540 uu/s reaches 210 x 1.35 = 283.5 uu.
pub fn reach(a: &PlayerState, t: &CombatTuning) -> f32 {
    let along = a.velocity.dot(view_vector(a.yaw, a.pitch));
    if t.reach_speed_scale <= 0.0 || along <= t.reach_min_speed {
        return t.attack_range;
    }
    let factor = 1.0 + ((along - t.reach_min_speed) * t.reach_speed_scale).clamp(0.0, t.reach_speed_scale_max);
    t.attack_range * ((factor - 1.0) * t.reach_scale_percent + 1.0)
}

/// Distance from the attacker's swing segment to the victim's body axis.
fn body_distance(eye: Vec3, dir: Vec3, range: f32, v: &PlayerState, mv: &MovementConfig) -> (f32, f32) {
    let hh = crate::movement::half_height(v, mv);
    let a0 = v.position - Vec3::Z * (hh - mv.collision_radius);
    let a1 = v.position + Vec3::Z * (hh - mv.collision_radius);
    let (on_swing, on_body) = closest_segment_segment(eye, eye + dir * range, a0, a1);
    (on_swing.distance(on_body) - mv.collision_radius, (on_swing - eye).length())
}

/// `UDisItemContext_MeleeBlockPlayer` blocked-attack handler (0x00C22B60): the hit is
/// parried when the block started less than `m_fTapForPerfectParryWindow` ago or the
/// block animation's `DisNotify_BlockParryZone` is active (t = 0.001..0.431 s of
/// Sword_Block, U-021), unless the post-tap forced block is still running.
pub fn parries(s: &crate::state::SwordState, t: &CombatTuning) -> bool {
    if s.force_block > 1e-8 {
        return false;
    }
    let tap = s.block_time < t.parry_tap_window;
    let zone = s.block_time >= 0.001 && s.block_time <= 0.001 + t.parry_window;
    tap || zone
}

fn angle_deg(a: Vec3, b: Vec3) -> f32 {
    a.normalize_or_zero().dot(b.normalize_or_zero()).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Resolve sword hits for this tick. Runs after every player's state machine advanced.
pub fn resolve_sword_hits(
    state: &mut WorldState,
    world: &dyn WorldQuery,
    mv: &MovementConfig,
    vitals: &VitalsConfig,
    t: &CombatTuning,
    events: &mut Vec<SimEvent>,
) {
    let n = state.players.len();
    // Deterministic order: players are kept sorted by id.
    for ai in 0..n {
        let attacker = state.players[ai];
        if !attacker.alive || attacker.incapacitated() || attacker.carrying.is_some() {
            continue;
        }
        let wants_attack = attacker.sword.phase == SwordPhase::Startup && attacker.sword.timer == 0.0;
        let active = attacker.sword.phase == SwordPhase::Active && !attacker.sword.hit_done;

        // Assassinations are decided at swing start, like the original's context action.
        if wants_attack {
            if let Some(vi) = find_assassination(&attacker, state, world, mv, t) {
                let vid = state.players[vi].id;
                let drop = attacker.mode == MoveMode::Falling;
                let cause = if drop { DamageCause::DropAssassination } else { DamageCause::Assassination };
                {
                    let a = &mut state.players[ai];
                    a.sword.phase = SwordPhase::Assassinating;
                    a.sword.timer = 0.0;
                    a.sword.assassination_target = Some(vid);
                    a.velocity = Vec3::ZERO;
                }
                let v = &mut state.players[vi];
                kill(v, Some(attacker.id), cause, events);
                events.push(SimEvent::Sound { position: v.position, radius: 250.0, category: SoundCategory::BodyFall, source: attacker.id });
                continue;
            }
        }
        if !active {
            continue;
        }
        let eye = eye_position(&attacker, mv);
        let dir = view_vector(attacker.yaw, attacker.pitch);
        let mut best: Option<(usize, f32)> = None;
        for vi in 0..n {
            if vi == ai || !state.players[vi].alive || state.players[vi].carried_by.is_some() || crate::powers::is_hidden(&state.players[vi]) {
                continue;
            }
            let (gap, along) = body_distance(eye, dir, reach(&attacker, t), &state.players[vi], mv);
            if gap > t.attack_sweep_size * 0.5 {
                continue;
            }
            // Melee trace channel must be clear between the eye and the target.
            let tgt = state.players[vi].position;
            let to = tgt - eye;
            if let Some(h) = world.raycast(eye, to, to.length(), channel::MELEE) {
                if h.distance < to.length() - mv.collision_radius {
                    continue;
                }
            }
            if best.is_none_or(|(_, d)| along < d) {
                best = Some((vi, along));
            }
        }
        let Some((vi, _)) = best else { continue };
        state.players[ai].sword.hit_done = true;
        let victim = state.players[vi];
        let to_attacker = attacker.position - victim.position;
        let facing = angle_deg(forward_vector(victim.yaw), to_attacker) <= t.block_half_angle;
        if victim.sword.phase == SwordPhase::Blocking && facing {
            if parries(&victim.sword, t) {
                let a = &mut state.players[ai];
                a.sword.phase = SwordPhase::Staggered;
                a.sword.timer = 0.0;
                if victim.sword.block_time < t.parry_tap_window {
                    state.players[vi].sword.force_block = t.force_block_after_tap;
                }
                events.push(SimEvent::SwordParried { attacker: attacker.id, defender: victim.id });
            } else {
                let a = &mut state.players[ai];
                a.sword.phase = SwordPhase::Recovery;
                a.sword.timer = -t.blocked_recoil;
                events.push(SimEvent::SwordBlocked { attacker: attacker.id, defender: victim.id });
            }
            events.push(SimEvent::Sound { position: victim.position, radius: 600.0, category: SoundCategory::SwordBlock, source: attacker.id });
            continue;
        }
        let (ratio, cause) = if victim.sword.phase == SwordPhase::Staggered {
            (t.big_damage_ratio, DamageCause::SwordCounter)
        } else {
            (t.normal_damage_ratio, DamageCause::Sword)
        };
        let dmg = ratio * vitals.health_max;
        let v = &mut state.players[vi];
        events.push(SimEvent::Sound { position: v.position, radius: 600.0, category: SoundCategory::SwordImpact, source: attacker.id });
        apply_damage(v, dmg, Some(attacker.id), cause, events, vitals);
        // Interrupt the victim's own swing.
        if v.alive && matches!(v.sword.phase, SwordPhase::Startup) {
            v.sword.phase = SwordPhase::Idle;
            v.sword.timer = 0.0;
        }
    }
}

fn find_assassination(
    a: &PlayerState,
    state: &WorldState,
    world: &dyn WorldQuery,
    mv: &MovementConfig,
    t: &CombatTuning,
) -> Option<usize> {
    let eye = eye_position(a, mv);
    let dir = view_vector(a.yaw, a.pitch);
    let mut best: Option<(usize, f32)> = None;
    for (vi, v) in state.players.iter().enumerate() {
        if v.id == a.id || !v.alive || v.sword.phase == SwordPhase::Assassinating {
            continue;
        }
        let to_v = v.position - a.position;
        let horiz = to_v.truncate().length();
        // Drop assassination: falling onto a target below.
        let drop = a.mode == MoveMode::Falling
            && -a.velocity.z >= t.drop_assassination_min_fall_speed
            && to_v.z < 0.0
            && horiz <= t.assassinate_range * 0.6
            && -to_v.z <= t.assassinate_range * 2.0;
        // Back assassination: close, behind the victim, victim not blocking.
        let behind = angle_deg(-forward_vector(v.yaw), -to_v) <= t.back_half_angle;
        let back = a.mode == MoveMode::Walking
            && to_v.length() <= t.assassinate_range * 0.6
            && behind
            && v.sword.phase != SwordPhase::Blocking
            && angle_deg(dir, to_v) <= 45.0;
        if !(drop || back) {
            continue;
        }
        if let Some(h) = world.raycast(eye, v.position - eye, (v.position - eye).length(), channel::MELEE) {
            if h.distance < (v.position - eye).length() - mv.collision_radius {
                continue;
            }
        }
        let d = to_v.length();
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((vi, d));
        }
    }
    best.map(|(i, _)| i)
}

pub fn id_of(state: &WorldState, i: usize) -> PlayerId {
    state.players[i].id
}
