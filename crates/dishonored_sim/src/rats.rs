//! Rat pawns ([`PawnKind::Rat`]) and the rat attack rule, decoded from `Dishonored.exe`
//! (research/dishonored/INVENTORY.md "Rats").
//!
//! The original's rats are crowd agents (`DisGameCrowdAgentSkeletalRat`, archetype
//! `RatSwarm.CrowdAgent_SmallRat`: `MaxSpeed` 350, `Health` 1, tweak `Twk_RatSwarm.Twk_Rat` with
//! `m_fAttackDistance` 120) grouped in swarms (`DisRatSwarm`). The sim keeps a Devouring Swarm as
//! one aggregate ([`crate::state::SwarmState`]) and models individual rats as small pawns that a
//! brain (`dishonored_ai`) or a possessor drives through [`PlayerInput`]s.
//!
//! Attack groups (0x00C891E0): the rats attacking one pawn deal, every `m_fDamageTimeInterval`,
//! `clamp(count x m_fDamagePerBite, m_fMinDamage, m_fMaxDamage)` as an int with the bite damage
//! type, once the group has been on the target for `m_fInitialDelayBeforeDealingDamage`
//! (immediately when the target is the player); each bite also slows the target
//! (`Modifier_RatAttackSpeedModifier`: `-m_fPawnSlowdownPercent` % of `Attribute_GroundSpeed`
//! and `Attribute_GroundSpeedSprint` for `m_fPawnSlowdownDuration`). Rats attack the player within
//! `m_fStandUpRatSwarmAttackDistance` (200) or, sneaking, `m_fCrouchedRatSwarmAttackDistance` (60).
//!
//! Hooks for the AI (`dishonored_ai`):
//! - spawn rats with [`spawn_rat`] (emits `SimEvent::RatSpawned`); a rat pawn attacks while its
//!   input holds `buttons::PRIMARY` and a target is within reach in front of it;
//! - `SimEvent::RatBite` per bite; rats die from any damage (`Health` 1);
//! - possessed rats (Possession level 1, `Twk_Possessable_Rat`) never attack.

use crate::events::{DamageCause, SimEvent};
use crate::input::{LeftHand, PlayerInput, buttons};
use crate::state::{Condition, PawnKind, PlayerId, PlayerState, WorldState};
use dishonored_ir::gameplay::{MovementConfig, VitalsConfig};
use glam::Vec3;

/// `RatSwarm.CrowdAgent_SmallRat.MaxSpeed`.
pub const RAT_SPEED: f32 = 350.0;
/// `Twk_RatSwarm.Twk_Rat.m_fAttackDistance`.
pub const RAT_ATTACK_DISTANCE: f32 = 120.0;
/// `DisTweaks_PlayerPawn.m_fStandUpRatSwarmAttackDistance` / `m_fCrouchedRatSwarmAttackDistance`
/// (`Twk_Pawn_Corvo_Release`).
pub const PLAYER_ATTACK_DISTANCE_STANDING: f32 = 200.0;
pub const PLAYER_ATTACK_DISTANCE_CROUCHED: f32 = 60.0;
/// `RatSwarm.CrowdAgent_SmallRat.Health`.
pub const RAT_HEALTH: f32 = 1.0;

/// Attack group values of an ambient swarm (`DisTweaks_RatSwarm` class defaults; Devouring
/// Swarm levels use their own, see `ItemsConfig::swarm`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatAttackTweak {
    pub initial_delay: f32,
    pub damage_per_bite: f32,
    pub min_damage: f32,
    pub max_damage: f32,
    pub interval: f32,
    pub slowdown_percent: f32,
    pub slowdown_duration: f32,
}

impl Default for RatAttackTweak {
    fn default() -> Self {
        Self { initial_delay: 1.0, damage_per_bite: 0.2, min_damage: 1.0, max_damage: 10.0, interval: 0.5, slowdown_percent: 40.0, slowdown_duration: 1.0 }
    }
}

/// Limbs rats eat off a corpse: the NPC's `DisSeveredLimbInfo.m_SeveredLimbInfo` entries not yet
/// severed. Every NPC tweak of the base game uses `SLInfo_Default` or `SLInfo_Elite` (one uses
/// `SLInfo_wolfhoundeaten`): six entries each (both lower legs, both lower arms, the neck, the
/// chest bone). The NPC pawn's rat-target slot 20 (0x00B79830) returns them from a random start in
/// circular order, the chest (`m_ChestBoneBreakName`) last.
pub const CORPSE_LIMBS: u32 = 6;

/// Seconds a group needs to eat a target (0x00C6D760), counted by its eating clock
/// (`m_fEatStageDuration`, running only while at least `m_EatRequiredRatCount` rats are in reach):
/// a dead pawn first takes `m_fEatStartupDuration` (then its mesh turns to the eaten one and the
/// clock restarts), then one limb per `m_fEatPerLimbDuration` (the clock restarts after each),
/// and when no limb is left one more interval consumes it (`eDisEatenState_CorpseConsumed`). A
/// target that is not dead (health at or above 1, e.g. knocked out; LIKELY that the tested
/// object is the target) is consumed after `limbs x m_fEatPerLimbDuration` in one go (the NPC
/// pawn's rat-target `GetLimbCount`, 0x00B79960, returns its `m_SeveredLimbInfo` count, 1 without
/// limb info).
pub fn eat_duration(dead: bool, limbs: u32, startup: f32, per_limb: f32) -> f32 {
    if dead {
        startup + (limbs + 1) as f32 * per_limb
    } else {
        limbs as f32 * per_limb
    }
}

/// Where the eating clock starts (0x00C89EE0): a group that was attacking its target when it
/// died starts at `-m_fEatStartupDelayAfterKill`; a group that finds a corpse starts at 0.
pub fn eat_clock_start(was_attacking: bool, delay_after_kill: f32) -> f32 {
    if was_attacking { -delay_after_kill } else { 0.0 }
}

/// Group damage per interval (0x00C891E0): `clamp(count x per_bite, min, max)`, whole points.
pub fn group_damage(count: usize, t: &RatAttackTweak) -> f32 {
    let d = count as f32 * t.damage_per_bite;
    let d = if t.min_damage <= d { if d < t.max_damage { d } else { t.max_damage } } else { t.min_damage };
    d.trunc()
}

/// Movement of a rat pawn: `MaxSpeed` in every direction, no sprint, no jump, no sword factor.
pub fn rat_movement(m: &mut MovementConfig) {
    m.run_speed = RAT_SPEED;
    m.sprint_speed = RAT_SPEED;
    m.walk_speed = RAT_SPEED;
    m.slow_walk_speed = RAT_SPEED;
    m.crouch_speed = RAT_SPEED;
    m.strafe_mult_run = 1.0;
    m.strafe_mult_sprint = 1.0;
    m.strafe_mult_sneak = 1.0;
    m.back_mult_run = 1.0;
    m.back_mult_sprint = 1.0;
    m.back_mult_sneak = 1.0;
    m.sword_unsheathed_speed_factor = 1.0;
    m.sword_sheathed_speed_factor = 1.0;
    m.jump_z = 0.0;
    m.power_jump_z = 0.0;
}

/// Spawn a rat pawn (hook for the AI and for swarms). Replaces an existing pawn with that id.
pub fn spawn_rat(state: &mut WorldState, id: PlayerId, position: Vec3, yaw: f32, swarm: Option<u32>, cfg: &crate::SimConfig, events: &mut Vec<SimEvent>) {
    crate::spawn_player(state, id, position, yaw, cfg);
    if let Some(p) = state.player_mut(id) {
        p.kind = PawnKind::Rat;
        p.vitals.health = RAT_HEALTH;
        p.vitals.mana = 0.0;
        p.inv = crate::state::Inventory { power_levels: [0; crate::state::power::COUNT], ..Default::default() };
        p.left.item = LeftHand::Empty;
        p.last_input.left_hand = LeftHand::Empty;
    }
    events.push(SimEvent::RatSpawned { rat: id, swarm });
}

/// The reach at which rats attack `v`.
pub fn attack_distance(v: &PlayerState) -> f32 {
    if v.kind == PawnKind::Human {
        if v.crouched { PLAYER_ATTACK_DISTANCE_CROUCHED } else { PLAYER_ATTACK_DISTANCE_STANDING }
    } else {
        RAT_ATTACK_DISTANCE
    }
}

/// The pawn a rat attacks this tick: holding attack, not possessed, the nearest living non-rat
/// pawn within the attack distance and in front of it (within 90 degrees).
pub fn rat_target(state: &WorldState, rat: &PlayerState, input: &PlayerInput) -> Option<usize> {
    if rat.kind != PawnKind::Rat || !rat.alive || rat.condition != Condition::Normal || !input.pressed(buttons::PRIMARY) {
        return None;
    }
    let fwd = crate::movement::forward_vector(rat.yaw);
    let mut best: Option<(usize, f32)> = None;
    for (i, v) in state.players.iter().enumerate() {
        if v.id == rat.id || v.kind == PawnKind::Rat || !v.alive || v.carried_by.is_some() || crate::powers::is_hidden(v) {
            continue;
        }
        let to = v.position - rat.position;
        let d = to.truncate().length();
        if d > attack_distance(v) + crate::movement::RAT_RADIUS || to.truncate().normalize_or_zero().dot(fwd.truncate()) < 0.0 {
            continue;
        }
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|b| b.0)
}

/// Resolve rat attacks for one tick. `inputs[i]` is the input of `state.players[i]`.
pub fn step_rat_attacks(state: &mut WorldState, inputs: &[PlayerInput], dt: f32, vitals: &VitalsConfig, t: &RatAttackTweak, events: &mut Vec<SimEvent>) {
    let n = state.players.len();
    let mut groups: Vec<Vec<PlayerId>> = vec![Vec::new(); n];
    for (i, inp) in inputs.iter().enumerate().take(n) {
        let rat = state.players[i];
        if let Some(vi) = rat_target(state, &rat, inp) {
            groups[vi].push(rat.id);
        }
    }
    for (vi, group) in groups.iter().enumerate() {
        let v = &mut state.players[vi];
        if group.is_empty() {
            v.rat_attack = Default::default();
            continue;
        }
        v.rat_attack.time += dt;
        if v.rat_attack.time <= t.initial_delay && v.kind != PawnKind::Human {
            continue;
        }
        v.rat_attack.acc += dt;
        while v.rat_attack.acc > t.interval && v.alive {
            v.rat_attack.acc -= t.interval;
            let dmg = group_damage(group.len(), t);
            let biter = group[0];
            v.rat_slow = t.slowdown_duration;
            v.rat_slow_percent = t.slowdown_percent;
            events.push(SimEvent::RatBite { rat: biter, victim: v.id });
            crate::vitals::apply_damage(v, dmg, Some(biter), DamageCause::RatBite, events, vitals);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_damage_clamps() {
        let t = RatAttackTweak::default();
        assert_eq!(group_damage(1, &t), 1.0);
        assert_eq!(group_damage(20, &t), 4.0);
        assert_eq!(group_damage(80, &t), 10.0);
        // Devouring Swarm level 1: 0.4 per rat clamped to 2..5.
        let s = RatAttackTweak { damage_per_bite: 0.4, min_damage: 2.0, max_damage: 5.0, ..t };
        assert_eq!(group_damage(30, &s), 5.0);
        assert_eq!(group_damage(3, &s), 2.0);
    }
}
