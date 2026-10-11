//! Player- and world-side Kismet events the engine fires through `FireGeneratedEvents`
//! (0x00BC7B30: every enabled event of the actor's `GeneratedEvents` whose class is the given one
//! checks with an originator, an instigator and the output indices). Each firing function was
//! read in `Dishonored.exe` (`oracle index fn`); the table is in KISMET.md ("Player and world
//! events").
//!
//! Events of the `DisSeqEvent_PlayerEvent` family attach to the player pawn: their registration
//! (0x00B8A140) gives an event without an `Originator` the player (`DAT_0145F628`), so the host
//! finds them as the events of the class whose originator is unset.

use super::*;
use dishonored_mission::runtime::TaskNote;
use dishonored_sim::events::ChokeOutcome;
use dishonored_sim::state::awareness;

/// What the event feeders keep between steps (saved with the Kismet run).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerEventState {
    /// `DishonoredPlayerPawn.m_OldCrouchState` (+0xB6C) per player: the last `PlayerCrouch` output.
    crouch: BTreeMap<PlayerId, u8>,
    /// `m_bIsInCombat` (+0x8A0 bit 3) per player, as `PlayerCombat` (0x00AB9FA0) keeps it.
    pub(super) combat: BTreeMap<PlayerId, bool>,
    /// Lock / unlock outputs of usables and doors the Kismet changed, fired by the next feed:
    /// (actor, `DisSeqEvent_Lock` output, last user).
    pub(super) locks: Vec<(u32, usize, Option<PlayerId>)>,
    /// Kismet usables a player used (not pickups, not talks): `Interact` fires for each in the
    /// next feed (0x00A3DF60 after the interactable's own use): (user, usable actor).
    pub(super) interacts: Vec<(PlayerId, u32)>,
    /// The pawn that last used a usable (`DishonoredUsableObject` +0x2C8: the instigator of its
    /// lock events).
    pub(super) usable_user: BTreeMap<u32, PlayerId>,
    /// Names of the active map's climbable chains (the index of `SimEvent::Climb`), set by the
    /// match each step.
    pub(super) climbables: Vec<String>,
    /// The player's whole mana as `SetMana` (vslot 456, 0x00AB1A30) last stored it (+0xA60).
    mana: BTreeMap<PlayerId, i32>,
    /// The power in the player's left hand at the last step (`PowerEquipped`).
    equipped: BTreeMap<PlayerId, u8>,
    /// The journal was opened this step (`JournalViewed` fires next feed).
    pub(super) journal_viewed: bool,
}

/// Output of `DisSeqEvent_PlayerManaThreshold` (`EDisPlayerManaThresholdOutput`).
pub const MANA_ABOVE: usize = 0;
pub const MANA_BELOW: usize = 1;

/// The power index of a left-hand item (`power::*`), for the power ones.
fn left_power(item: dishonored_sim::input::LeftHand) -> Option<usize> {
    use dishonored_sim::input::LeftHand as L;
    use dishonored_sim::state::power;
    Some(match item {
        L::Blink => power::BLINK,
        L::Windblast => power::WINDBLAST,
        L::DarkVision => power::DARK_VISION,
        L::Possession => power::POSSESSION,
        L::BendTime => power::BEND_TIME,
        L::DevouringSwarm => power::DEVOURING_SWARM,
        _ => return None,
    })
}

/// Output of `DisSeqEvent_PlayerCrouch` (`EDisPlayerCrouchEventOutput`).
#[allow(dead_code)]
pub const CROUCH_STAND: usize = 0;
#[allow(dead_code)]
pub const CROUCH_SNEAK: usize = 1;
pub const CROUCH_CROUCH: usize = 2;
/// Outputs of `DisSeqEvt_Choke` (`DisEventChoke_Stage`).
pub const CHOKE_STARTED: usize = 0;
pub const CHOKE_WIN: usize = 1;
pub const CHOKE_LOSE: usize = 2;
/// Outputs of `DisSeqEvent_Windblasted` (`EDisWindblastedKismetEvent`).
#[allow(dead_code)]
pub const WINDBLASTED_PUSHED: usize = 0;
pub const WINDBLASTED_THROWN: usize = 1;
/// Outputs of `DisSeqEvent_Possessed` (`EDisPossessedKismetEvent`).
pub const POSSESSED_STARTED: usize = 0;
pub const POSSESSED_FINISHED: usize = 1;
/// Outputs of `DisSeqEvent_Lock` (`EDisLockKismetEvent`).
pub const LOCK_LOCKED: usize = 0;
pub const LOCK_UNLOCKED: usize = 1;
/// Outputs of `DisSeqEvent_KeyHoleUsed`.
pub const KEYHOLE_USED: usize = 0;
pub const KEYHOLE_UNUSED: usize = 1;
/// Outputs of `DisSeqEvent_Interact` (`EDisInteractKismetEvent`).
pub const INTERACT_USE: usize = 0;
pub const INTERACT_ALT: usize = 1;
/// Outputs of `DisSeqEvent_Climb` (`EDisClimbEventOutput`).
pub const CLIMB_START: usize = 0;
pub const CLIMB_FINISH: usize = 1;
/// Outputs of `DisSeqEvent_PlayerCombat` (`EDisPlayerCombatOutput`).
pub const COMBAT_ENTERED: usize = 0;
pub const COMBAT_EXITED: usize = 1;
/// The attention level a hostile needs toward the player to count for `m_bIsInCombat`
/// (0x00B0B780 is called with 4: `DAL_Busted`).
const COMBAT_ATTENTION: u8 = 4;

/// `Kismet` names of the powers in the `DisSeqEvent_PowerUsed` output links ("Blink 1" ..
/// "Windblast 2": `<name> <level>`), by [`power`] index.
const POWER_NAMES: [&str; 6] = ["Blink", "Windblast", "DarkVision", "Possess", "BendTime", "DevouringSwarm"];

/// The events of a `PlayerEvent`-family class attached to the player (no `Originator`).
fn player_events(k: &Kismet, class: &str) -> Vec<OpId> {
    k.events_for(class, &[Obj::None])
}

/// `FireGeneratedEvents` over events the caller found: each one checks with the originator, the
/// instigator and the output indices; the events that activated are returned.
fn fire_events(k: &mut Kismet, h: &mut H, evs: Vec<OpId>, originator: Obj, instigator: Obj, outputs: Option<Vec<usize>>) -> Vec<OpId> {
    let mut out = Vec::new();
    for e in evs {
        let before = k.ops[e as usize].trigger_count;
        k.check_activate(h, e, originator.clone(), instigator.clone(), false, outputs.clone(), false);
        if k.ops[e as usize].trigger_count > before {
            out.push(e);
        }
    }
    out
}

/// The originators an NPC's events are found under: the pawn and the spawner actor that spawned it.
fn npc_origins(h: &H, pawn: PlayerId) -> Vec<Obj> {
    let mut origs = vec![Obj::Pawn(pawn)];
    if let Some((&a, _)) = h.st.spawner.iter().find(|(_, i)| h.m.setup.original[**i].spawn.id == pawn) {
        origs.push(Obj::Actor(a));
    }
    origs
}

/// `DishonoredPlayerPawn.m_bIsInCombat` as the per-tick check `PlayerCombat` 0x00AB9FA0 computes
/// it: a hostile that tracks the player at attention `DAL_Busted` or above and whose awareness is
/// `EAIAwareness_InCombat` (the list of 0x00AB6960 filtered by 0x00B0B780 with relation 2 and
/// attention 4, then 0x00B0C1E0 == 5). The registry lookup (0x00C6CE00) and the DLC check
/// (0x00CF7B40) of the same function have no port system.
pub(super) fn in_combat_now(h: &H, player: PlayerId) -> bool {
    h.m.crowd.brains.iter().any(|b| {
        h.m.world.player(b.id).is_some_and(|p| p.alive) && b.awareness() == awareness::IN_COMBAT && b.attn.iter().any(|a| a.who == player && a.level >= COMBAT_ATTENTION)
    })
}

/// Whether a task event's `m_pObjective` / `m_pTask` name the task `path` ("Objective.Task"):
/// 0x00AD0F80 compares the task's name and its objective's name.
fn task_matches(k: &Kismet, e: OpId, path: &str) -> bool {
    let Some(Obj::Path(task)) = k.prop(e, "m_pTask").and_then(|v| v.as_obj().cloned()) else { return false };
    let objective = match k.prop(e, "m_pObjective").and_then(|v| v.as_obj().cloned()) {
        Some(Obj::Path(o)) => obj_name(&Obj::Path(o)),
        // The task's path names its objective one level up.
        _ => task.rsplit(['.', ':']).nth(1).unwrap_or_default().to_string(),
    };
    format!("{objective}.{}", obj_name(&Obj::Path(task))).eq_ignore_ascii_case(path)
}

/// The output of `DisSeqEvent_PowerUsed` for `power` at `level` (0x00BF30C0): the first output
/// link named for the power, plus the level less one.
pub fn power_used_output(k: &Kismet, e: OpId, power: usize, level: u8) -> Option<usize> {
    let name = POWER_NAMES.get(power)?;
    let first = k.op(e).outputs.iter().position(|o| o.name.rsplit_once(' ').map_or(o.name.as_str(), |x| x.0).eq_ignore_ascii_case(name))?;
    Some(first + level.max(1) as usize - 1)
}

/// The Kismet actor of a placed chain: the actor named like the world's climbable, in the active map.
fn climbable_actor(h: &H, name: &str) -> Option<u32> {
    let map = h.m.current_map.clone();
    let in_map = |a: &dishonored_kismet::Actor| h.db.packages.iter().position(|p| p.eq_ignore_ascii_case(&a.package)).and_then(|p| h.db.map_of_package(p as u16)).is_some_and(|m| m.eq_ignore_ascii_case(&map));
    h.db.actors.iter().position(|a| a.name.eq_ignore_ascii_case(name) && in_map(a)).map(|i| i as u32)
}

/// Fire the events of this step (called from [`super::feed`]).
pub(super) fn feed(k: &mut Kismet, h: &mut H, sim_events: &[SimEvent]) {
    player_state_events(k, h);
    task_events(k, h);
    lock_events(k, h);
    interact_events(k, h);
    for e in sim_events {
        match *e {
            // `DisSeqEvent_Possessed` of a possessed NPC pawn (0x00B7D110 / 0x00B7D2F0 are the
            // `DishonoredNPCPawn` possession start / end): the pawn's events, the possessor the
            // instigator, output 0 Started / 1 Finished. Rats have their own event
            // (`RatPossess`, not fed).
            SimEvent::PossessStart { id, host } | SimEvent::PossessEnd { id, host } => {
                if h.m.world.player(host).is_none_or(|p| p.kind == PawnKind::Rat || h.m.humans.contains(&host)) {
                    continue;
                }
                let started = matches!(e, SimEvent::PossessStart { .. });
                let origs = npc_origins(h, host);
                let evs = k.events_for("DisSeqEvent_Possessed", &origs);
                let fired = fire_events(k, h, evs, Obj::Pawn(host), Obj::Pawn(id), Some(vec![if started { POSSESSED_STARTED } else { POSSESSED_FINISHED }]));
                if !fired.is_empty() {
                    h.note(format!("pawn {host} {} by {id}: {} Possessed event(s)", if started { "possessed" } else { "released" }, fired.len()));
                }
            }
            // `DisSeqEvt_Choke` on the choked NPC (the `StateNPCMasterBeingChoked` state:
            // enter 0x00A7C270 output 0, the win 0x00A6D380 output 1, the loss 0x00A74720 and
            // the exit 0x00A7C4D0 output 2), the player the instigator.
            SimEvent::ChokeStart { attacker, victim } | SimEvent::ChokeEnd { attacker, victim, .. } => {
                if h.m.humans.contains(&victim) {
                    continue;
                }
                let out = match *e {
                    SimEvent::ChokeEnd { outcome: ChokeOutcome::Win, .. } => CHOKE_WIN,
                    SimEvent::ChokeEnd { .. } => CHOKE_LOSE,
                    _ => CHOKE_STARTED,
                };
                let origs = npc_origins(h, victim);
                let evs = k.events_for("DisSeqEvt_Choke", &origs);
                let fired = fire_events(k, h, evs, Obj::Pawn(victim), Obj::Pawn(attacker), Some(vec![out]));
                if !fired.is_empty() {
                    h.note(format!("choke of pawn {victim} by {attacker}: output {out}, {} event(s)", fired.len()));
                }
            }
            // `SeqEvent_RigidBodyCollision` (UE3 `CheckRBCollisionEvents`): the prop's events whose
            // `MinCollisionVelocity` the contact reaches, the prop the originator, every output; the
            // impact velocity and location go to the event's variables.
            SimEvent::MovableContact { movable, speed, point } => {
                let Some((package, name)) = h.m.world.movables.iter().find(|m| m.id == movable).map(|m| (m.package.clone(), m.name.clone())) else { continue };
                let Some(a) = h.kismet_actor(&package, &name) else { continue };
                let evs: Vec<OpId> = k.events_for("SeqEvent_RigidBodyCollision", &[Obj::Actor(a)]).into_iter().filter(|e| speed >= k.prop_float(*e, "MinCollisionVelocity")).collect();
                for e in &evs {
                    k.set_link(*e, "ImpactVelocity", Value::Float(speed));
                    k.set_link(*e, "ImpactLocation", Value::Vector(point.to_array()));
                }
                let fired = fire_events(k, h, evs, Obj::Actor(a), Obj::None, None);
                if !fired.is_empty() {
                    h.note(format!("{name} collided at {speed:.0}: {} RigidBodyCollision event(s)", fired.len()));
                }
            }
            // A body gone (eaten by rats, burnt to ash): the pawn is destroyed.
            SimEvent::BodyRemoved { body, .. } => {
                if !h.m.humans.contains(&body) {
                    h.st.destroyed.push(Obj::Pawn(body));
                }
            }
            // `DisSeqEvent_AttackedByRats` (the enter of `StateNPCMasterActionMeleeSwarm`,
            // 0x00A70450): the NPC's events, every output.
            SimEvent::RatsAttackStart { victim, rat } => {
                let origs = npc_origins(h, victim);
                let evs = k.events_for("DisSeqEvent_AttackedByRats", &origs);
                let fired = fire_events(k, h, evs, Obj::Pawn(victim), Obj::Pawn(rat), None);
                if !fired.is_empty() {
                    h.note(format!("pawn {victim} attacked by rats: {} event(s)", fired.len()));
                }
            }
            // `DisSeqEvent_Windblasted` (0x00C4DE90 ends with it after the thrown reaction was
            // started): output 1 Thrown. The "Pushed" output (0x00C53790, an NPC that resists:
            // 0x00B4B4C0) needs the armour state the port lacks.
            SimEvent::Windblasted { victim, by } => {
                let origs = npc_origins(h, victim);
                let evs = k.events_for("DisSeqEvent_Windblasted", &origs);
                let fired = fire_events(k, h, evs, Obj::Pawn(victim), Obj::Pawn(by), Some(vec![WINDBLASTED_THROWN]));
                if !fired.is_empty() {
                    h.note(format!("pawn {victim} thrown by {by}'s Windblast: {} event(s)", fired.len()));
                }
            }
            // `DisSeqEvent_PowerUsed` (0x00BF32F0 -> 0x00BF30C0): the player's events of exactly
            // that class; the output named for the power, at its level.
            SimEvent::PowerUsed { id, power, level } => {
                let evs: Vec<OpId> = player_events(k, "DisSeqEvent_PowerUsed").into_iter().filter(|e| k.class(*e) == "DisSeqEvent_PowerUsed").collect();
                for ev in evs {
                    let Some(out) = power_used_output(k, ev, power as usize, level) else { continue };
                    if fire_events(k, h, vec![ev], Obj::Pawn(id), Obj::Pawn(id), Some(vec![out])).len() == 1 {
                        h.note(format!("power {} used at level {level}: {}", POWER_NAMES.get(power as usize).copied().unwrap_or("?"), k.describe(ev)));
                    }
                }
            }
            // `DisSeqEvent_Climb` on the chain: Start from the climb state's enter (0x00A72B00),
            // Finish from its exit (0x00A6E2C0), both `bcfac0(chain, class, chain, pawn, exact
            // 1, output)`.
            SimEvent::Climb { id, climbable, started } => {
                let Some(name) = h.st.player_ev.climbables.get(climbable as usize).cloned() else { continue };
                let Some(a) = climbable_actor(h, &name) else { continue };
                let evs = k.events_for("DisSeqEvent_Climb", &[Obj::Actor(a)]);
                let fired = fire_events(k, h, evs, Obj::Actor(a), Obj::Pawn(id), Some(vec![if started { CLIMB_START } else { CLIMB_FINISH }]));
                if !fired.is_empty() {
                    h.note(format!("climb of {name} {}: {} event(s)", if started { "started" } else { "finished" }, fired.len()));
                }
            }
            // `DisSeqEvent_MovablePickedUp` (0x00A45FD0, from the grab 0x00C3D030) and
            // `DisSeqEvent_MovableDropped` (0x00A46380, from the release 0x00C45340, a drop or a
            // throw): `FireGeneratedEvents(prop, class, prop, pawn, 0, 0)`: the prop's events,
            // the prop the originator, the pawn the instigator, every output.
            SimEvent::MovablePickedUp { id, movable } | SimEvent::MovableDropped { id, movable, .. } => {
                let Some((package, name)) = h.m.world.movables.iter().find(|m| m.id == movable).map(|m| (m.package.clone(), m.name.clone())) else { continue };
                let Some(a) = h.kismet_actor(&package, &name) else { continue };
                let (class, what) = if matches!(e, SimEvent::MovablePickedUp { .. }) { ("DisSeqEvent_MovablePickedUp", "picked up") } else { ("DisSeqEvent_MovableDropped", "dropped") };
                let evs = k.events_for(class, &[Obj::Actor(a)]);
                let fired = fire_events(k, h, evs, Obj::Actor(a), Obj::Pawn(id), None);
                if !fired.is_empty() {
                    h.note(format!("{name} {what} by {id}: {} {class} event(s)", fired.len()));
                }
            }
            _ => {}
        }
    }
    destroyed_events(k, h);
    // `DisSeqEvent_JournalViewed`: the journal menu opened (0x00BD11E0), the player's events.
    if std::mem::take(&mut h.st.player_ev.journal_viewed) {
        if let Some(p) = h.m.humans.first().copied() {
            let evs = player_events(k, "DisSeqEvent_JournalViewed");
            let fired = fire_events(k, h, evs, Obj::Pawn(p), Obj::Pawn(p), None);
            if !fired.is_empty() {
                h.note(format!("journal viewed: {} event(s)", fired.len()));
            }
        }
    }
}

/// `SeqEvent_Destroyed` (the engine's `Actor::Destroyed` -> `TriggerEventClass`): the destroyed
/// actor's events (a pawn's under the pawn and its spawner), the actor the originator, every
/// output.
fn destroyed_events(k: &mut Kismet, h: &mut H) {
    for o in std::mem::take(&mut h.st.destroyed) {
        let origs = match &o {
            Obj::Pawn(p) => npc_origins(h, *p),
            other => match h.pawn_of(other) {
                Some(p) => npc_origins(h, p),
                None => vec![other.clone()],
            },
        };
        let evs = k.events_for("SeqEvent_Destroyed", &origs);
        let fired = fire_events(k, h, evs, o.clone(), Obj::None, None);
        if !fired.is_empty() {
            h.note(format!("{} destroyed: {} Destroyed event(s)", obj_name(&o), fired.len()));
        }
    }
}

/// The per-tick player pawn checks: `PlayerCrouch` (0x00AB0960, from the pawn's tick 0x00ABB1E0)
/// and `PlayerCombat` (0x00AB9FA0, from 0x00ABBDE0).
fn player_state_events(k: &mut Kismet, h: &mut H) {
    for p in h.m.humans.clone() {
        let Some(pl) = h.m.world.player(p).filter(|x| x.alive).copied() else { continue };
        // PlayerCrouch: 2 when `bIsCrouched`, else 1 when sneak is toggled (0x00AC8280), else 0;
        // an output fires when it differs from `m_OldCrouchState`. The port applies the sneak
        // toggle to the crouch at once and has no separate toggled-but-standing state, so output
        // 1 does not occur.
        let state = if pl.crouched { CROUCH_CROUCH } else { CROUCH_STAND };
        let old = h.st.player_ev.crouch.get(&p).copied().unwrap_or(0) as usize;
        if state != old {
            let evs = player_events(k, "DisSeqEvent_PlayerCrouch");
            let fired = fire_events(k, h, evs, Obj::Pawn(p), Obj::Pawn(p), Some(vec![state]));
            if !fired.is_empty() {
                h.note(format!("player {p} stance {old} -> {state}: {} PlayerCrouch event(s)", fired.len()));
            }
            h.st.player_ev.crouch.insert(p, state as u8);
        }
        // PlayerCombat: output 0 when the check turns true, 1 when it turns false.
        let now = in_combat_now(h, p);
        let was = h.st.player_ev.combat.get(&p).copied().unwrap_or(false);
        if now != was {
            let evs = player_events(k, "DisSeqEvent_PlayerCombat");
            let fired = fire_events(k, h, evs, Obj::Pawn(p), Obj::Pawn(p), Some(vec![if now { COMBAT_ENTERED } else { COMBAT_EXITED }]));
            if !fired.is_empty() {
                h.note(format!("player {p} {} combat: {} PlayerCombat event(s)", if now { "enters" } else { "leaves" }, fired.len()));
            }
            h.st.player_ev.combat.insert(p, now);
        }
        // PlayerManaThreshold: `SetMana` (0x00AB1A30) on a change of the whole mana fires the
        // player's events with output Below when it fell (new <= old), else Above; each event's
        // `CheckActivate` (0x00B8E910) passes only a crossing of its `m_ManaThreshold`: Above
        // when old <= threshold < new, Below when new < threshold <= old.
        let mana = pl.vitals.mana.trunc() as i32;
        match h.st.player_ev.mana.insert(p, mana) {
            Some(old) if old != mana => {
                let out = if mana <= old { MANA_BELOW } else { MANA_ABOVE };
                let evs: Vec<OpId> = player_events(k, "DisSeqEvent_PlayerManaThreshold")
                    .into_iter()
                    .filter(|e| {
                        let t = k.prop_int(*e, "m_ManaThreshold");
                        if out == MANA_ABOVE { old <= t && t < mana } else { mana < t && t <= old }
                    })
                    .collect();
                let fired = fire_events(k, h, evs, Obj::Pawn(p), Obj::Pawn(p), Some(vec![out]));
                if !fired.is_empty() {
                    h.note(format!("player {p} mana {old} -> {mana}: {} PlayerManaThreshold event(s)", fired.len()));
                }
            }
            _ => {}
        }
        // PowerEquipped: equipping a power in the left hand (0x00C4BA70 / 0x00C44FE0 ->
        // 0x00BF30C0 with the event class): the output named for the power, at its level.
        // The pawn's first observed hand is its starting equipment, not an equip.
        const NO_POWER: u8 = u8::MAX;
        let now = left_power(pl.left.item).filter(|&pw| pl.inv.power_levels[pw] > 0);
        let code = now.map_or(NO_POWER, |pw| pw as u8);
        let before = h.st.player_ev.equipped.insert(p, code);
        if let (Some(old), Some(pw)) = (before, now) {
            if old != code {
                let level = pl.inv.power_levels[pw];
                let evs: Vec<OpId> = player_events(k, "DisSeqEvent_PowerEquipped");
                for ev in evs {
                    let Some(out) = power_used_output(k, ev, pw, level) else { continue };
                    if fire_events(k, h, vec![ev], Obj::Pawn(p), Obj::Pawn(p), Some(vec![out])).len() == 1 {
                        h.note(format!("power {} equipped at level {level}: {}", POWER_NAMES[pw], k.describe(ev)));
                    }
                }
            }
        }
    }
}

/// `DisSeqEvent_TaskActivated` / `TaskStateChanged` (0x00AD1080 / 0x00AD1320, from the objective
/// manager's dispatcher 0x00AD4F60): every enabled event of the player naming the task checks
/// with the player as originator; the state change passes the new state as its only output.
fn task_events(k: &mut Kismet, h: &mut H) {
    let notes = std::mem::take(&mut h.m.state.task_log);
    let Some(p) = h.m.humans.first().copied() else { return };
    for n in notes {
        let (class, path, outputs) = match &n {
            TaskNote::Activated(path) => ("DisSeqEvent_TaskActivated", path, None),
            TaskNote::Changed(path, st) => ("DisSeqEvent_TaskStateChanged", path, Some(vec![*st as usize])),
            // `DisSeqEvent_ObjectiveCompleted` (0x00AD0E00, kind 1): the player's events whose
            // `m_pObjective` is the objective, every output.
            TaskNote::ObjectiveCompleted(id) => {
                let evs: Vec<OpId> = player_events(k, "DisSeqEvent_ObjectiveCompleted")
                    .into_iter()
                    .filter(|e| matches!(k.prop(*e, "m_pObjective").and_then(|v| v.as_obj().cloned()), Some(o) if obj_name(&o).eq_ignore_ascii_case(id)))
                    .collect();
                let fired = fire_events(k, h, evs, Obj::Pawn(p), Obj::Pawn(p), None);
                if !fired.is_empty() {
                    h.note(format!("objective {id} completed: {} event(s)", fired.len()));
                }
                continue;
            }
        };
        let evs: Vec<OpId> = player_events(k, class).into_iter().filter(|e| task_matches(k, *e, path)).collect();
        let fired = fire_events(k, h, evs, Obj::Pawn(p), Obj::Pawn(p), outputs);
        if !fired.is_empty() {
            h.note(format!("{class} {path}: {} event(s)", fired.len()));
        }
    }
}

/// `DisSeqEvent_Lock` of the usables and doors the Kismet locked or unlocked since the last feed
/// (`Lock` 0x00A4E870 fires output 0 whenever it runs; `Unlock` 0x00A4E910 fires output 1 only if
/// the usable was locked): originator the usable, instigator its last user.
fn lock_events(k: &mut Kismet, h: &mut H) {
    for (a, out, user) in std::mem::take(&mut h.st.player_ev.locks) {
        let evs = k.events_for("DisSeqEvent_Lock", &[Obj::Actor(a)]);
        let fired = fire_events(k, h, evs, Obj::Actor(a), user.map(Obj::Pawn).unwrap_or_default(), Some(vec![out]));
        if !fired.is_empty() {
            h.note(format!("{} {}: {} Lock event(s)", h.db.actors[a as usize].name, if out == LOCK_LOCKED { "locked" } else { "unlocked" }, fired.len()));
        }
    }
}

/// Door keyholes, tripwires and the generic Interact event (research/dishonored/systems/
/// interactables.md).
///
/// - `DisSeqEvent_Tripwire` (0x00A21A30, from `Trip` 0x00A28B40): originator the tripwire,
///   instigator the tripper, inexact class, no output list (every output).
/// - `DisSeqEvent_KeyHoleUsed` (0x00A38580 output 0, 0x00A2E4E0 output 1): originator the door,
///   instigator the pawn, exact class.
/// - `DisSeqEvent_Interact` (0x00A3DF60 output 0 `Interact`, 0x00A3E0B0 output 1 `AltInteract`):
///   `FireGeneratedEvents(user, ...)` reaches the events registered with the user pawn
///   (`SequenceEvent::RegisterEvent`, 0x006E50B0, registers an event with its `Originator` only),
///   instigator the interactable, exact class. The Prison's four instances have no `Originator`
///   and so never fire in the original either.
fn interact_events(k: &mut Kismet, h: &mut H) {
    use dishonored_sim::interactables::InteractEvent;
    let defs = h.m.setup.interactables.get(&h.m.current_map.to_ascii_lowercase()).cloned();
    for e in h.m.take_interact_events() {
        match e {
            InteractEvent::Tripwire { tripwire, instigator } => {
                let Some(w) = defs.as_ref().and_then(|d| d.tripwires.get(tripwire)) else { continue };
                let Some(a) = h.kismet_actor(&w.package, &w.name) else { continue };
                let fired = k.trigger(h, "DisSeqEvent_Tripwire", &[Obj::Actor(a)], Obj::Pawn(instigator), None);
                if !fired.is_empty() {
                    h.note(format!("{}:{} tripped by pawn {instigator}: {} Tripwire event(s)", w.package, w.name, fired.len()));
                }
            }
            InteractEvent::KeyHole { keyhole, player, used } => {
                let Some(d) = defs.as_ref().and_then(|d| d.keyholes.get(keyhole)) else { continue };
                let Some(a) = h.kismet_actor(&d.package, &d.name) else { continue };
                let evs = k.events_for("DisSeqEvent_KeyHoleUsed", &[Obj::Actor(a)]);
                let fired = fire_events(k, h, evs, Obj::Actor(a), Obj::Pawn(player), Some(vec![if used { KEYHOLE_USED } else { KEYHOLE_UNUSED }]));
                if !fired.is_empty() {
                    h.note(format!("{}:{} keyhole {} by pawn {player}: {} event(s)", d.package, d.name, if used { "used" } else { "left" }, fired.len()));
                }
            }
            InteractEvent::AltInteract { keyhole, player } => {
                let Some(d) = defs.as_ref().and_then(|d| d.keyholes.get(keyhole)) else { continue };
                let Some(a) = h.kismet_actor(&d.package, &d.name) else { continue };
                fire_interact(k, h, player, a, INTERACT_ALT);
            }
            // The wrapper 0x00A3DF60 after the launcher's `Interact` (0x00A3B6E0) answered "used".
            InteractEvent::Launcher { launcher, player } => {
                let Some(l) = defs.as_ref().and_then(|d| d.launchers.get(launcher)) else { continue };
                let Some(a) = h.kismet_actor(&l.package, &l.name) else { continue };
                fire_interact(k, h, player, a, INTERACT_USE);
            }
        }
    }
    for (user, a) in std::mem::take(&mut h.st.player_ev.interacts) {
        fire_interact(k, h, user, a, INTERACT_USE);
    }
}

/// `FireGeneratedEvents(user, DisSeqEvent_Interact, user, interactable, exact 1, [output])`.
fn fire_interact(k: &mut Kismet, h: &mut H, user: PlayerId, interactable: u32, output: usize) {
    let evs: Vec<OpId> = k.events_for("DisSeqEvent_Interact", &[Obj::Pawn(user)]).into_iter().filter(|e| k.class(*e) == "DisSeqEvent_Interact").collect();
    let fired = fire_events(k, h, evs, Obj::Pawn(user), Obj::Actor(interactable), Some(vec![output]));
    if !fired.is_empty() {
        h.note(format!("pawn {user} {} {}: {} Interact event(s)", if output == INTERACT_ALT { "alt-interacted" } else { "interacted" }, h.db.actors[interactable as usize].name, fired.len()));
    }
}
