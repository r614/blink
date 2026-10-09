//! Non-lethal takedowns and bodies, decoded from `Dishonored.exe`
//! (research/dishonored/PLAYER.md "Takedowns"):
//!
//! - choke (`StatePlayerMasterChoke`, enter 0x00A7AA00, tick 0x00A71100): the required time is
//!   `Attribute_ChokeTime` plus the choke animation length; the timer runs in the In and Loop
//!   states. In the Loop, releasing the choke button (`m_bChokeButton`) loses the choke (the
//!   victim breaks free); the timer reaching 0 wins it (knock-out). Whenever the victim is no
//!   longer in `StateNPCMasterBeingChoked` the choke is cancelled (0x00A71090). After a win,
//!   holding Use (`m_bUseButton`) for `m_fChokeHoldToAutoCarry` lifts the body.
//! - bodies (`StatePlayerCarryCorpseIdle`): lift with `DisNotify_AcquireCorpse` /
//!   `EndLiftCorpse` of `Empty_CarryCorpse_In` (0.056 / 0.967 s); drop with
//!   `ReleaseCorpse` / `EndDropCorpse` of `Empty_CarryCorpse_DropLow` (0.500 / 1.100 s);
//!   carrying uses `Attribute_GroundSpeedCarryingCorpse` and `Attribute_JumpZCarryingCorpse`.
//! - sleep darts (`DisDamageType_Arrow_Sleep`, DefaultWeapon.ini): `m_bPutToSleep`,
//!   `m_fTimeBeforeSleep` 1.5 s (instant for unaware NPCs: needs the AI's awareness, U-105).
//!
//! Unconscious pawns stay down for good (no wake-up exists in the original's NPC states for
//! choked or slept NPCs) and count as non-lethal; damaging one kills it (`m_bKillSleeper`).

use crate::events::{ChokeOutcome, KnockoutCause, SimEvent, SoundCategory};
use crate::input::{PlayerInput, buttons};
use crate::movement::forward_vector;
use crate::state::{ChokePhase, Condition, MoveMode, PawnKind, PlayerId, PlayerState, WorldState};
use crate::tuning::CombatTuning;
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;

/// `Ply_Sword_Choke_as` timings: `Sword_Choke_In_Master` length, `AnimStateExit` of the Win,
/// Lose and Cancel sequences, and `Sword_Choke_CarryCorpse_Master` exit.
pub const CHOKE_IN: f32 = 0.667;
pub const CHOKE_WIN_EXIT: f32 = 2.470;
pub const CHOKE_LOSE_EXIT: f32 = 2.229;
pub const CHOKE_CANCEL_EXIT: f32 = 1.457;
pub const CHOKE_CARRY_EXIT: f32 = 1.618;
/// `Ply_Empty_CarryCorpse_as`: lift end, low drop release / end.
pub const LIFT_END: f32 = 0.967;
pub const DROP_RELEASE: f32 = 0.500;
pub const DROP_END: f32 = 1.100;

/// Seconds a sleep dart takes to put an aware NPC to sleep (`m_fTimeBeforeSleep` of
/// `DisDamageType_Arrow_Sleep`, DefaultWeapon.ini).
pub const SLEEP_DELAY: f32 = 1.5;

fn angle_deg(a: Vec3, b: Vec3) -> f32 {
    a.normalize_or_zero().dot(b.normalize_or_zero()).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Whether `a` can choke `v` from where it stands: a living pawn that is not a player, not
/// already in a takedown or carried, within the back assassination's reach and behind it
/// (the `CanInteract` of the choke's interactable; the view cone is the crosshair's trace).
pub fn choke_eligible(a: &PlayerState, v: &PlayerState, t: &CombatTuning, _mv: &dishonored_ir::gameplay::MovementConfig) -> bool {
    if v.id == a.id || !v.alive || v.kind == PawnKind::Human || v.condition != Condition::Normal || v.carried_by.is_some() {
        return false;
    }
    let to_v = v.position - a.position;
    let behind = angle_deg(-forward_vector(v.yaw), -to_v) <= t.back_half_angle;
    to_v.length() <= t.assassinate_range * 0.6 && behind
}

/// Start a choke (0x00A7AA00): `m_fRequiredTime = m_fTimer = ChokeTime + anim length`.
pub fn start_choke(state: &mut WorldState, ai: usize, vi: usize, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let a = state.players[ai];
    let required = crate::powers::attribute(&a, cfg, "Attribute_ChokeTime", cfg.items.inventory.choke_time) + cfg.items.inventory.choke_anim_time;
    let v = &mut state.players[vi];
    v.condition = Condition::BeingChoked;
    v.condition_by = Some(a.id);
    v.velocity = Vec3::ZERO;
    v.sword.phase = crate::state::SwordPhase::Idle;
    let vid = v.id;
    let p = &mut state.players[ai];
    p.choke.phase = ChokePhase::In;
    p.choke.target = Some(vid);
    p.choke.required = required;
    p.choke.timer = required;
    p.choke.phase_time = 0.0;
    p.choke.hold = cfg.items.inventory.choke_hold_to_auto_carry;
    p.velocity = Vec3::ZERO;
    events.push(SimEvent::ChokeStart { attacker: a.id, victim: vid });
}

fn set_phase(p: &mut PlayerState, ph: ChokePhase) {
    p.choke.phase = ph;
    p.choke.phase_time = 0.0;
}

/// Free a pawn held in a choke.
fn release_victim(state: &mut WorldState, vid: Option<PlayerId>) {
    if let Some(v) = vid.and_then(|v| state.player_mut(v)) {
        if v.condition == Condition::BeingChoked {
            v.condition = Condition::Normal;
            v.condition_by = None;
        }
    }
}

/// Knock a pawn out (non-lethal): `DisSeqEvent_KnockedOut`. The AI hook is the event.
pub fn knock_out(v: &mut PlayerState, by: Option<PlayerId>, cause: KnockoutCause, events: &mut Vec<SimEvent>) {
    if !v.alive || v.condition == Condition::Unconscious {
        return;
    }
    v.condition = Condition::Unconscious;
    v.condition_by = by;
    v.sleep_timer = 0.0;
    v.velocity = Vec3::ZERO;
    v.sword.phase = crate::state::SwordPhase::Idle;
    v.left.phase = crate::state::LeftPhase::Idle;
    v.vitals.dark_vision_remaining = 0.0;
    v.dark = Default::default();
    events.push(SimEvent::KnockedOut { victim: v.id, by, cause });
}

/// Choke state machine for the attacker (tick 0x00A71100). Returns true while the choke locks
/// the attacker's movement.
pub fn step_choke(state: &mut WorldState, ai: usize, input: &PlayerInput, dt: f32, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) -> bool {
    let a = state.players[ai];
    if a.choke.phase == ChokePhase::None {
        return false;
    }
    let vid = a.choke.target;
    let victim_held = vid.and_then(|v| state.player(v)).is_some_and(|v| v.alive && v.condition == Condition::BeingChoked && v.condition_by == Some(a.id));
    let p = &mut state.players[ai];
    p.choke.phase_time += dt;
    p.velocity = Vec3::ZERO;
    let held = input.pressed(buttons::USE);
    if !a.alive {
        set_phase(p, ChokePhase::None);
        release_victim(state, vid);
        return false;
    }
    match a.choke.phase {
        ChokePhase::In | ChokePhase::Loop => {
            if !victim_held {
                set_phase(p, ChokePhase::Cancel);
                p.choke.target = None;
                release_victim(state, vid);
                if let Some(v) = vid {
                    events.push(SimEvent::ChokeEnd { attacker: a.id, victim: v, outcome: ChokeOutcome::Cancel });
                }
                return true;
            }
            if a.choke.phase == ChokePhase::In && p.choke.phase_time >= CHOKE_IN {
                set_phase(p, ChokePhase::Loop);
            }
            if p.choke.phase == ChokePhase::Loop && !held {
                set_phase(p, ChokePhase::Lose);
                release_victim(state, vid);
                if let Some(v) = vid {
                    events.push(SimEvent::ChokeEnd { attacker: a.id, victim: v, outcome: ChokeOutcome::Lose });
                }
                return true;
            }
            p.choke.timer -= dt;
            if p.choke.phase == ChokePhase::Loop && p.choke.timer <= 0.0 {
                set_phase(p, ChokePhase::Win);
                if let Some(v) = vid.and_then(|v| state.player_mut(v)) {
                    knock_out(v, Some(a.id), KnockoutCause::Choke, events);
                }
                if let Some(v) = vid {
                    events.push(SimEvent::ChokeEnd { attacker: a.id, victim: v, outcome: ChokeOutcome::Win });
                }
            }
            true
        }
        ChokePhase::Win => {
            // Holding Use counts down `m_fHoldTimeForAutoCarry`, else it is reset.
            if held {
                p.choke.hold -= dt;
            } else {
                p.choke.hold = cfg.items.inventory.choke_hold_to_auto_carry;
            }
            if p.choke.hold <= 0.0 && p.carrying.is_none() {
                if let Some(v) = vid {
                    let pid = p.id;
                    set_phase(&mut state.players[ai], ChokePhase::None);
                    state.players[ai].choke.target = None;
                    lift(state, pid, v, events);
                    state.players[ai].carry_anim = CHOKE_CARRY_EXIT;
                    return true;
                }
            }
            if p.choke.phase_time >= CHOKE_WIN_EXIT {
                set_phase(p, ChokePhase::None);
                p.choke.target = None;
            }
            true
        }
        ChokePhase::Lose | ChokePhase::Cancel => {
            let exit = if a.choke.phase == ChokePhase::Lose { CHOKE_LOSE_EXIT } else { CHOKE_CANCEL_EXIT };
            if p.choke.phase_time >= exit {
                set_phase(p, ChokePhase::None);
                p.choke.target = None;
            }
            true
        }
        ChokePhase::None => false,
    }
}

/// Lift a body (`StatePlayerGrabCorpse`).
pub fn lift(state: &mut WorldState, carrier: PlayerId, body: PlayerId, events: &mut Vec<SimEvent>) {
    if let Some(b) = state.player_mut(body) {
        b.carried_by = Some(carrier);
        b.velocity = Vec3::ZERO;
    }
    if let Some(c) = state.player_mut(carrier) {
        c.carrying = Some(body);
        c.carry_anim = LIFT_END;
        c.crouched = c.crouched && c.mode == MoveMode::Walking;
    }
    events.push(SimEvent::BodyLifted { carrier, body });
}

/// Release a carried body in front of the carrier (`ReleaseCorpse`), with `velocity` (zero
/// for every drop of the original), and let it fall to the floor.
pub fn release(state: &mut WorldState, carrier: PlayerId, velocity: Vec3, world: &dyn WorldQuery, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let Some(c) = state.player(carrier).copied() else { return };
    let Some(bid) = c.carrying else { return };
    let r = cfg.movement.collision_radius;
    let fwd = forward_vector(c.yaw);
    let mut pos = c.position + fwd * (2.0 * r);
    if world.raycast(c.position, fwd, 2.0 * r, channel::PAWN).is_some() {
        pos = c.position;
    }
    if let Some(b) = state.player_mut(bid) {
        b.carried_by = None;
        b.position = pos;
        b.velocity = velocity;
        b.yaw = c.yaw;
    }
    if let Some(cp) = state.player_mut(carrier) {
        cp.carrying = None;
        cp.carry_anim = 0.0;
    }
    events.push(SimEvent::BodyDropped { carrier, body: bid });
}

/// Bodies on the ground fall to the floor (they do not run the locomotion of living pawns).
/// A body hitting a surface is a `Corpse` contact: its AI noise (`Corpse` -> `Environment`,
/// only from `m_fMinSpeed` up) at the contact, made by the body (`noise::emit`).
pub fn step_body(b: &mut PlayerState, world: &dyn WorldQuery, cfg: &GameplayConfig, dt: f32, events: &mut Vec<SimEvent>) {
    let mv = &cfg.movement;
    let hh = crate::movement::half_height(b, mv);
    if b.velocity.length_squared() < 1e-4 {
        // Settle: is there floor under the body?
        if world.raycast(b.position, Vec3::NEG_Z, hh + 4.0, channel::PAWN).is_some() {
            return;
        }
    }
    b.velocity.z += mv.gravity_z * dt;
    let delta = b.velocity * dt;
    let len = delta.length();
    if len < 1e-6 {
        return;
    }
    // Stop on the first surface the body's centre would cross (feet for downward moves).
    let probe = delta + delta.normalize_or_zero() * if delta.z < 0.0 { hh } else { mv.collision_radius };
    if let Some(h) = world.raycast(b.position, probe, probe.length(), channel::PAWN) {
        let back = if delta.z < 0.0 { hh } else { mv.collision_radius };
        b.position += delta.normalize_or_zero() * (h.distance - back).max(0.0);
        crate::noise::emit(events, crate::noise::body_noise(b, cfg), h.point, SoundCategory::BodyFall, b.id, b.velocity.length());
        b.velocity = Vec3::ZERO;
    } else {
        b.position += delta;
    }
    if b.position.z < crate::movement::KILL_Z {
        b.velocity = Vec3::ZERO;
    }
}

/// Carrier side each tick: the body follows the carrier (on its shoulder); dropping on Use or
/// attack, dropping when the carrier is hurt badly
/// enough to fall (dead, choked, unconscious).
pub fn step_carry(state: &mut WorldState, ci: usize, input: &PlayerInput, dt: f32, world: &dyn WorldQuery, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let c = state.players[ci];
    let Some(bid) = c.carrying else { return };
    if !c.alive || c.incapacitated() || state.player(bid).is_none_or(|b| b.no_body) {
        release(state, c.id, Vec3::ZERO, world, cfg, events);
        return;
    }
    state.players[ci].carry_anim = (c.carry_anim - dt).max(0.0);
    let ready = c.carry_anim <= 0.0;
    // Low drop: the body leaves the shoulder at `ReleaseCorpse` (0.5 s). The corpse-carry state
    // has no throw: leaving it (0x00A69A50) releases the body without an impulse, and
    // `Attribute_ThrowStrength` is only read by the carried-object release (0x00C45340, U-106).
    // Attack leaving the state drops the body the same way (LIKELY).
    if ready && (c.just_pressed(input, buttons::USE) || c.just_pressed(input, buttons::PRIMARY)) {
        state.players[ci].action_lock = DROP_END;
        release(state, c.id, Vec3::ZERO, world, cfg, events);
        return;
    }
    let shoulder = c.position + Vec3::Z * (crate::movement::half_height(&c, &cfg.movement) * 0.5);
    if let Some(b) = state.player_mut(bid) {
        b.position = shoulder;
        b.velocity = c.velocity;
        b.yaw = c.yaw;
    }
}

/// Sleep timers and burning for one pawn. Returns true when the pawn died of fire.
pub fn step_afflictions(state: &mut WorldState, vi: usize, dt: f32, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let v = &mut state.players[vi];
    if v.sleep_timer > 0.0 && v.alive {
        v.sleep_timer -= dt;
        if v.sleep_timer <= 0.0 {
            let by = v.condition_by;
            knock_out(v, by, KnockoutCause::SleepDart, events);
        }
    }
    if v.burning > 0.0 && v.alive {
        v.burning += dt;
        // `m_fNPCImmolationTime`: an immolated NPC burns to death (LIKELY, U-107).
        if v.burning >= cfg.items.gadgets.immolation_time {
            if v.kind != PawnKind::Human {
                let by = v.condition_by;
                crate::vitals::kill(v, by, crate::events::DamageCause::Fire, events);
            }
            v.burning = 0.0;
        }
    }
}
