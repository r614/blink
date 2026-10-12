//! The NPC pawn states and the item contexts the AI requests them through (research/
//! ARCHITECTURE.md, migration step 6; the decode: research/dishonored/systems/tick_order.md
//! sections 5-6, research/dishonored/systems/ai.md section 13).
//!
//! - **Requests** ([`Request`]): what an NPC's brain asks of its pawn in a tick: the item context
//!   it runs or starts (`DisItemContext_NPCAttack*`, the side and back steps, the pistol, the
//!   special classes', [`context`]), a behaviour's GenericAction chain, the locomotion's speed,
//!   stance and path. The host hands them to the step in [`WorldState::requests`]
//!   (`dishonored_ai::Crowd::requests`), as it hands the crosshair's targets; they are not part
//!   of the state.
//! - **The master state machine** (`DishonoredNativeStateMachine`, pawn +0xCEC, ticked by
//!   0x00A6DE60 from `ADishonoredNPCPawn::Tick` after the pawn's physics): one state per NPC
//!   ([`Master`], its parameters in a [`StateRequest`]). A request (`TryRequestState`
//!   0x00A74FA0) is decided by the cooked transition logic (`NpcFsm`: the pending state, then the
//!   current one, must allow the new class) and waits for the pawn's next machine tick, which
//!   exits the old state and enters the new one without ticking it (0x00A5F8C0); otherwise the
//!   current state ticks, and an exit it requests applies right after. `RequestExit` (slot 92,
//!   0x00A74EE0) forces the default state, Walk. The AI's requests are taken before the machine
//!   ticks, as the port has always run the brains before the pawns (tick_order.md, "Consequences"
//!   5: the spawn order is open).
//! - **The anim player** (`DisCpntType_AnimPlayer`, pawn +0xD74): the states play
//!   `DisAnimDefinitions` chains on the full-body slot (0x00C57310: a group at random that is not
//!   the last one picked and whose steps exist, 0x00C40790 / 0x00C4D940; the step's sequence with
//!   its play-rate variation and blend-in, 0x00C52E00). After the pose (0x00C53260, the same Ark
//!   list as the locomotion, LIKELY) it plays the next step when a step ends, or stops the chain
//!   `m_fBlendOutTime` before the end of the last step (of the one step of an end-after-one
//!   group), which ends the state that played it. The `AnimState*` notifies are the player's
//!   (`DisAnimStateOwnerInterface`): no NPC state reads them.
//! - **Root motion** (U-977): a full-body chain sets the mesh's root-motion modes from its data
//!   (`m_RootMotionMode` / `m_RootRotationMode`, pawn slot 469 0x00B6E680) and switches the
//!   locomotion off; at the pose barrier (0x00746280) `RMM_Translate` moves the pawn at once
//!   ([`barrier`]), `RMM_Velocity` / `RMM_Accel` accumulate, and the locomotion's off-path branch
//!   (0x0094BEC0) turns the accumulation into the velocity the next tick's walking physics
//!   integrates, times the stance modifier's `m_fSpeedMultiplier` ([`locomotion`]). The chain's
//!   stop restores (`RMM_Velocity`, `RMRM_RotateActor`) and the locomotion (slot 470 0x00B71280).

use crate::SimConfig;
use crate::anim::{PawnAnim, SeqId, StepOut, layer, token};
use crate::events::SimEvent;
use crate::state::{PawnKind, PlayerId, PlayerState, SwordPhase, WorldState};
use dishonored_ir::npc_anim::{NpcAnim, transition};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// The item contexts an NPC's brain runs (`DisItemContext_NPC*`), by the codes of the attack
/// patterns (`dishonored_ir::ai::MeleeContextConfig::kind`, `SpecialAttack::kind`). 0: none.
pub type ContextKind = u8;

/// [`ContextKind`] values.
pub mod context {
    pub const NONE: u8 = 0;
    /// `DisItemContext_NPCAttackShort` (`DisNPCAnim_AttackShort`).
    pub const ATTACK_SHORT: u8 = 1;
    /// `DisItemContext_NPCAttackStepBack`.
    pub const ATTACK_STEP_BACK: u8 = 2;
    /// `DisItemContext_NPCAttackMedium`.
    pub const ATTACK_MEDIUM: u8 = 3;
    /// `DisItemContext_NPCAttackLong` (`DisNPCAnim_AttackLong`).
    pub const ATTACK_LONG: u8 = 4;
    /// `DisItemContext_NPCJumpAttack` (`DisNPCAnim_JumpAttack`).
    pub const JUMP_ATTACK: u8 = 5;
    /// `DisItemContext_NPCSideStep` (dodge).
    pub const SIDE_STEP: u8 = 6;
    /// `DisItemContext_NPCBackStep` (dodge).
    pub const BACK_STEP: u8 = 7;
    /// `DisAISubStateFirePistol` aiming / firing (`DisItemContext_NPCFireGun`).
    pub const FIRE_PISTOL: u8 = 8;
    /// Rat swarm attacking (rat pawns).
    pub const RAT_ATTACK: u8 = 9;
    /// Rat swarm eating a corpse.
    pub const RAT_EAT: u8 = 10;
    /// The special NPC classes' contexts (research/dishonored/systems/ai.md F6).
    pub use dishonored_ir::ai::special_moves::*;
}

/// What an NPC's brain asks of its pawn this tick (the original's AI asks the item contexts, the
/// pawn's state machine and the locomotion component; the port's brain produces this beside the
/// pawn's [`crate::input::PlayerInput`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// The item context the brain runs ([`context`]): held while it runs.
    pub context: ContextKind,
    /// The context starts this tick (the AI's start request, `0x00C18230`): an attack, a side or
    /// back step; a context that is only held (the pistol's aim, a tune) never starts.
    pub start: bool,
    /// The started context's variation: the side step's direction group (0 left, 1 right:
    /// `m_NextRandomSideStepDirection`, 0x00C277A0); -1 for a group at random.
    pub variant: i8,
    /// The pawn's locomotion config (an index into `AiConfig::locomotion`) and the stance
    /// modifier its body intention selects (an index into that config's `m_Modifiers`).
    pub loco: Option<(u16, u8)>,
    /// A behaviour's GenericAction chain (`eDisNPCAnimations`) and the serial of the request
    /// (`dishonored_ai::AnimRequest`): a new serial starts it.
    pub play_anim: Option<(u8, u32)>,
    /// The speed the brain asks of the locomotion (uu/s): the authored speed of the loop its
    /// stance modifier and speed index select, times the modifier's `m_fSpeedMultiplier`
    /// (`ArkComponentLocomotionConfig`); 0 leaves the pawn's own speeds.
    pub loco_speed: f32,
    /// The navmesh path the locomotion request follows this tick, when it follows one.
    pub path: Option<LocoPath>,
}

/// Where a locomotion request's path leads (`ArkComponentLocomotion`): the point walked to
/// (`P[k]`, +0xB8), the segment's start (`P[k-1]`, the direction +0xA0), the point after (the
/// next segment, +0xAC), the destination (+0xC4) and whether the request is accurate (the
/// request record's +0x24: a scripted go-to that must end on its point).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LocoPath {
    pub from: Vec3,
    pub to: Vec3,
    pub next: Option<Vec3>,
    pub dest: Vec3,
    pub accurate: bool,
}

/// The request of pawn `id` this step (none: the default).
pub fn request(state: &WorldState, id: PlayerId) -> Request {
    state.requests.iter().find(|(p, _)| *p == id).map(|(_, r)| *r).unwrap_or_default()
}

// ---- the master state machine ----

/// `StateNPCMaster*`: the states of an NPC's master state machine the simulation runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Master {
    /// `StateNPCMasterWalk`: the locomotion; the default state of a living NPC (0x00A7BA50).
    #[default]
    Walk,
    /// `StateNPCMasterActionMelee`: an attack context's `m_MainAttack.m_Animation`, its group at
    /// random, the step its `m_MeleeState` (0x00A6AA40).
    ActionMelee,
    /// `StateNPCActionParry`: a parried attacker's `Parried_Lose` (`m_MeleeState` 7): `Parry`
    /// group 2 on the full body (0x00A6A330).
    ActionParry,
    /// `StateNPCMasterPlayAnim`: a chain played whole on the full body: the behaviours'
    /// GenericActions, the side and back steps (0x00A65A60).
    PlayAnim,
    /// `StateNPCMasterThrown`: the throw's `PushToRagdoll` and its ragdoll (`crate::anim::play_throw`).
    Thrown,
    /// `StateNPCMasterDead_Limp`: the death chain, then the ragdoll (0x00A9FC40).
    Dead,
}

impl Master {
    /// The state's class (the transition logic's names).
    pub fn class(self) -> &'static str {
        match self {
            Master::Walk => "StateNPCMasterWalk",
            Master::ActionMelee => "StateNPCMasterActionMelee",
            Master::ActionParry => "StateNPCActionParry",
            Master::PlayAnim => "StateNPCMasterPlayAnim",
            Master::Thrown => "StateNPCMasterThrown",
            Master::Dead => "StateNPCMasterDead_Limp",
        }
    }
}

/// A state request (`{class, params}`, applied to the state object at once by
/// `TryRequestState`): the state, the chain it plays (`eDisNPCAnimations`), the group (-1: at
/// random) and step, and the item context it runs for (its firing object).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StateRequest {
    pub state: Master,
    pub anim: u8,
    pub group: i8,
    pub step: u8,
    pub context: ContextKind,
}

impl StateRequest {
    pub fn new(state: Master, anim: u8, group: i8, step: u8, context: ContextKind) -> Self {
        Self { state, anim, group, step, context }
    }
}

/// The anim player's full-body record (+0x40): the chain playing, the group and step it plays,
/// the step's `m_bLoop` and `m_bNoAutoTransitions`, the group's `m_fBlendOutTime`, and the layer
/// token its sequence plays with.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChainPlay {
    pub anim: u8,
    pub group: u8,
    pub step: u8,
    pub looping: bool,
    pub no_auto: bool,
    pub blend_out: f32,
    pub token: u32,
}

/// An NPC's pawn states (`WorldState::pawn_states`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PawnStates {
    /// The current state with its parameters, and the pending request.
    pub current: StateRequest,
    pub pending: Option<StateRequest>,
    /// The full-body chain (`None`: the slot plays nothing for the anim player).
    pub chain: Option<ChainPlay>,
    /// The anim player's last random group (+0x3C, shared by every chain of the pawn); -1 none.
    pub last_group: i8,
    /// The GenericAction request serial last taken and its chain.
    pub play_serial: u32,
    pub play_anim: u8,
    /// The mesh's `RootMotionMode` (+0x384), `RootMotionRotationMode` (+0x38C) and
    /// `PreviousRMM` (+0x385). An NPC starts in (`RMM_Velocity`, `RMRM_Ignore`):
    /// `Default__DishonoredNPCPawn.AnimMesh` over `Engine.Default__SkeletalMeshComponent`.
    pub rm_mode: u8,
    pub rm_rotation: u8,
    pub rm_previous: u8,
    /// The locomotion component is switched off (comp+0x24 bit 0: a full-body chain plays).
    pub loco_off: bool,
    /// The root translation the barrier accumulated this tick (`RootMotionDelta.Translation`,
    /// world uu), consumed by the locomotion the same tick.
    #[serde(skip)]
    pub rm_accum: Vec3,
    /// The death's ragdoll began (`DishonoredNotify_Ragdoll`, pawn slot 352): no more root motion.
    pub ragdoll: bool,
    /// Chains played so far (wrapping): the layer token of the next one.
    pub serial: u32,
}

/// `ERootMotionMode` values.
pub mod root_mode {
    pub const TRANSLATE: u8 = 0;
    pub const VELOCITY: u8 = 1;
    pub const IGNORE: u8 = 2;
    pub const ACCEL: u8 = 3;
}

impl PawnStates {
    pub fn new() -> Self {
        Self { last_group: -1, rm_mode: root_mode::VELOCITY, rm_previous: root_mode::IGNORE, ..Default::default() }
    }

    pub fn master(&self) -> Master {
        self.current.state
    }
}

/// Whether a pawn has pawn states (the NPCs; rats and players do not).
pub fn has_states(p: &PlayerState) -> bool {
    matches!(p.kind, PawnKind::Guard | PawnKind::Civilian)
}

/// The master state of pawn `id` (Walk without states).
pub fn master(state: &WorldState, id: PlayerId) -> Master {
    state.pawn_states.get(&id).map_or(Master::Walk, |s| s.master())
}

/// Whether the pawn's locomotion is switched off by a full-body chain (its input moves nothing:
/// the root motion does).
pub fn locomotion_off(state: &WorldState, id: PlayerId) -> bool {
    state.pawn_states.get(&id).is_some_and(|s| s.loco_off)
}

/// `TryRequestState` (0x00A74FA0): the transition from the pending state, then from the current
/// one, must be allowed by the cooked logic (`NpcFsm::result`; Call asks the state's
/// `CanTransitionTo`, slot 78). Returns whether the request is pending.
pub fn try_request(state: &mut WorldState, id: PlayerId, req: StateRequest, cfg: &SimConfig) -> bool {
    let Some(s) = state.pawn_states.get_mut(&id) else { return false };
    let fsm = &cfg.gameplay.npc_anim.fsm;
    let allows = |from: &StateRequest| match fsm.result(from.state.class(), req.state.class()) {
        transition::NOT_ALLOWED => false,
        transition::ALLOWED => true,
        _ => can_transition_to(from, &req),
    };
    if s.pending.as_ref().is_some_and(|p| !allows(p)) || !allows(&s.current) {
        return false;
    }
    s.pending = Some(req);
    true
}

/// `CanTransitionTo` (slot 78) for the transitions the logic leaves to the state: Walk allows
/// every request unless soiree reactions are off (0x00A81B30; the port has no soiree reaction
/// switch); ActionMelee toward ActionParry needs `m_MeleeState` 6 / 7 (0x00A743D0), which a
/// Parried_Lose request is. The other states' slot 78 are not decoded: their Call transitions
/// are allowed (LIKELY).
fn can_transition_to(_from: &StateRequest, _to: &StateRequest) -> bool {
    true
}

/// `RequestExit` (slot 92, 0x00A74EE0): the default state, forced (0x00A72190 skips the logic).
fn request_exit(s: &mut PawnStates) {
    s.pending = Some(StateRequest::default());
}

/// The item context gate (NPC pawn slot 366, 0x00B74400: the current or pending state's slot 95
/// `AllowsContext`): Walk takes any; ActionMelee only once its context's attack is interruptible
/// (`m_bAttackNowInterruptable`, 0x00A6D1D0); the other action states only the teleport spell
/// (0x00A64C80, 0x00A65B00), which the port's NPCs do not request through here.
pub fn allows_context(state: &WorldState, id: PlayerId) -> bool {
    let Some(s) = state.pawn_states.get(&id) else { return true };
    let st = s.pending.unwrap_or(s.current);
    match st.state {
        Master::Walk => true,
        Master::ActionMelee => state.anim.get(&id).and_then(|a| a.layers[layer::FULL_BODY].as_ref()).is_none_or(|l| l.token & token::KIND != token::SWING || l.interruptible),
        _ => false,
    }
}

// ---- the anim player ----

/// The item prefix of a chain step (`m_FirstItemPrefix` 2 / 3: the held item's, 4 / 5 the
/// intended one, pawn slots 377 / 378): the profile's primary item for a guard, its empty hands
/// for a civilian (the port's guards hold their sword; LIKELY for an unarmed guard).
fn armed(p: &PlayerState) -> bool {
    p.kind == PawnKind::Guard
}

/// The sequence of a chain step: one of its `m_Name` alternatives (picked with `roll`) with the
/// item prefix when the step has one, looked up in the profile's sets (`0x00C4D580`).
pub fn step_sequence(cfg: &SimConfig, step: &dishonored_ir::npc_anim::AnimStep, roll: f32, armed: bool) -> Option<SeqId> {
    let npc = &cfg.gameplay.npc_anim;
    let name = step.names.get(((roll * step.names.len() as f32) as usize).min(step.names.len().saturating_sub(1)))?;
    let prefix = if armed { npc.item_prefix.as_str() } else { npc.empty_prefix.as_str() };
    let full = if step.item_prefix == 0 { name.clone() } else { format!("{prefix}{name}") };
    cfg.anim.find(&npc.sets, &full)
}

/// Whether every non-optional step of a group has a sequence (0x00C4D940).
fn playable(cfg: &SimConfig, g: &dishonored_ir::npc_anim::AnimGroup, armed: bool) -> bool {
    g.steps.iter().filter(|s| !s.optional).all(|s| (0..s.names.len().max(1)).any(|k| step_sequence(cfg, s, k as f32 / s.names.len().max(1) as f32, armed).is_some()))
}

/// Play a chain on pawn `i`'s full body (0x00C57310): the group at random when `group` < 0 (the
/// pick `(int)(rand * n)` moved on by one when it repeats the last pick and there are several,
/// then the first playable group from there on), the step (0 when out of range), its sequence at
/// `1 + m_fPlayRateModifierRange * (2u - 1)` with its blend-in; the chain's root-motion modes go
/// on the mesh and the locomotion switches off (slot 469, 0x00B6E680: modes 0 and 1 also zero the
/// velocity). `token` names what the layer plays for (0: a new chain token). False when the chain
/// or its step has no sequence.
pub fn play_chain(state: &mut WorldState, i: usize, anim: u8, group: i8, step: u8, token: u32, cfg: &SimConfig) -> bool {
    play_step(state, i, anim, group, step, token, true, cfg)
}

/// [play_chain]'s step start (0x00C52E00); the chain's start (`first`) also sets the mesh's
/// modes and switches the locomotion off (slot 469: 0x00C57310 calls it for the chain, not for
/// the steps the anim player goes on to).
#[allow(clippy::too_many_arguments)]
fn play_step(state: &mut WorldState, i: usize, anim: u8, group: i8, step: u8, token: u32, first: bool, cfg: &SimConfig) -> bool {
    let p = state.players[i];
    let armed = armed(&p);
    let Some(chain) = cfg.gameplay.npc_anim.defs.chains.get(anim as usize).filter(|c| !c.groups.is_empty()) else { return false };
    let (Some(s), Some(a)) = (state.pawn_states.get_mut(&p.id), state.anim.get_mut(&p.id)) else { return false };
    let n = chain.groups.len();
    let g = if group < 0 {
        let mut pick = ((a.roll() * n as f32) as usize).min(n - 1);
        if n > 1 && pick as i8 == s.last_group {
            pick = (pick + 1) % n;
        }
        let Some(g) = (0..n).map(|k| (pick + k) % n).find(|&k| playable(cfg, &chain.groups[k], armed)) else { return false };
        s.last_group = g as i8;
        g
    } else {
        group as usize
    };
    let Some(grp) = chain.groups.get(g) else { return false };
    let k = if (step as usize) < grp.steps.len() { step as usize } else { 0 };
    let st = &grp.steps[k];
    let r = a.roll();
    let Some(seq) = step_sequence(cfg, st, r, armed) else { return false };
    let rate = 1.0 + st.rate_range * (2.0 * a.roll() - 1.0);
    s.serial = s.serial.wrapping_add(1);
    let tok = if token == 0 { token::CHAIN | (s.serial & 0x00FF_FFFF) } else { token };
    a.play(layer::FULL_BODY, seq, tok, rate, st.looped, p.yaw);
    if let Some(l) = a.layers[layer::FULL_BODY].as_mut() {
        l.blend_in = st.blend_in;
        l.blend_out = grp.blend_out;
    }
    s.chain = Some(ChainPlay { anim, group: g as u8, step: k as u8, looping: st.looped, no_auto: st.no_auto_transitions, blend_out: grp.blend_out, token: tok });
    if first {
        set_root_modes(state, i, chain.root_motion_mode, chain.root_rotation_mode);
    }
    true
}

/// Pawn slot 469 (0x00B6E680): the mesh's modes (slot 347, 0x008FCFA0, written directly), the
/// locomotion switched off (0x00946520), and for `RMM_Translate` / `RMM_Velocity` the velocity
/// and acceleration zeroed.
fn set_root_modes(state: &mut WorldState, i: usize, mode: u8, rotation: u8) {
    let id = state.players[i].id;
    if let Some(s) = state.pawn_states.get_mut(&id) {
        s.rm_mode = mode;
        s.rm_rotation = rotation;
        s.loco_off = true;
    }
    if mode <= root_mode::VELOCITY {
        let p = &mut state.players[i];
        p.velocity = Vec3::ZERO;
        p.root_velocity = None;
    }
}

/// The chain's stop on the full body (0x00C48CF0): the slot stops, the mesh goes back to
/// (`RMM_Velocity`, `RMRM_RotateActor`) and the locomotion switches on (slot 470, 0x00B71280).
fn stop_chain(state: &mut WorldState, id: PlayerId) {
    let Some(s) = state.pawn_states.get_mut(&id) else { return };
    let Some(c) = s.chain.take() else { return };
    s.rm_mode = root_mode::VELOCITY;
    s.rm_rotation = 1;
    s.loco_off = false;
    if let Some(a) = state.anim.get_mut(&id) {
        if a.layers[layer::FULL_BODY].as_ref().is_some_and(|l| l.token == c.token) {
            a.stop(layer::FULL_BODY);
        }
    }
}

/// The anim player's tick after the pose (0x00C53260) for every NPC: a step without its loop or
/// `m_bNoAutoTransitions` whose time left (sequence time over its effective rate, 0x005ACC40)
/// falls to 0.0001 plays the next step; the last step (the one step of an end-after-one group)
/// stops the chain at `m_fBlendOutTime` before its end, and so does a slot whose sequence is
/// gone. A stop is the chain's end (event type 1) for the state that played it.
pub fn chain_tick(state: &mut WorldState, cfg: &SimConfig) {
    for i in 0..state.players.len() {
        let p = state.players[i];
        let Some(c) = state.pawn_states.get(&p.id).and_then(|s| s.chain) else { continue };
        if p.time_scale <= 0.0 {
            continue;
        }
        let layer_seq = state.anim.get(&p.id).and_then(|a| a.layers[layer::FULL_BODY].as_ref()).filter(|l| l.token == c.token).cloned();
        let Some(l) = layer_seq else {
            stop_chain(state, p.id);
            chain_ended(state, i);
            continue;
        };
        if l.looping || c.no_auto {
            continue;
        }
        let Some(grp) = cfg.gameplay.npc_anim.defs.chains.get(c.anim as usize).and_then(|ch| ch.groups.get(c.group as usize)) else { continue };
        let next = c.step as usize + 1;
        let last = grp.end_after_one || next >= grp.steps.len();
        let threshold = if last { c.blend_out } else { 1e-4 };
        let info = cfg.anim.info(l.seq);
        let rate = info.map_or(1.0, |n| n.rate_scale_or_one()) * l.rate;
        let left = info.map_or(0.0, |n| (n.duration - l.time).max(0.0)) / rate.max(1e-6);
        if left > threshold {
            continue;
        }
        if last {
            stop_chain(state, p.id);
            chain_ended(state, i);
        } else {
            // The next step keeps the chain's group (the record's); a state chain's step plays
            // with a token of its own, so that the slot sees a new sequence.
            let tok = if c.token & token::KIND == token::CHAIN { 0 } else { c.token };
            if !play_step(state, i, c.anim, c.group as i8, next as u8, tok, false, cfg) {
                stop_chain(state, p.id);
                chain_ended(state, i);
            }
        }
    }
}

/// The anim player's event type 1 (the chain ended) to the state that played it: the action
/// states request their exit (ActionMelee 0x00A64CC0, PlayAnim 0x00A659C0, the parry's base);
/// the dead stay dead (0x00A7D1A0: the ragdoll follows).
fn chain_ended(state: &mut WorldState, i: usize) {
    let id = state.players[i].id;
    let Some(s) = state.pawn_states.get_mut(&id) else { return };
    match s.current.state {
        Master::ActionMelee | Master::ActionParry | Master::PlayAnim => request_exit(s),
        Master::Walk | Master::Thrown | Master::Dead => {}
    }
}

// ---- the machine's tick ----

/// The NPCs' pawn states this tick, after the pawns' physics (each machine runs after its own
/// pawn's physics; no state reads another pawn's): the AI's requests, then the machine's tick.
pub fn tick(state: &mut WorldState, cfg: &SimConfig, out: &mut StepOut) {
    let ids: Vec<PlayerId> = state.players.iter().filter(|p| has_states(p)).map(|p| p.id).collect();
    state.pawn_states.retain(|id, _| ids.contains(id));
    for i in 0..state.players.len() {
        let p = state.players[i];
        if !has_states(&p) {
            continue;
        }
        state.pawn_states.entry(p.id).or_insert_with(PawnStates::new);
        state.anim.entry(p.id).or_insert_with(|| PawnAnim::new(p.id));
        if p.time_scale <= 0.0 {
            continue;
        }
        if p.alive && !p.incapacitated() {
            take_request(state, i, cfg, out);
        }
        machine_tick(state, i, cfg, out);
        // The context the pawn runs, published for the presentation (`PlayerInput::npc_move`).
        let ctx = state.pawn_states.get(&p.id).map(|s| s.current.context).filter(|c| *c != context::NONE).unwrap_or(request(state, p.id).context);
        state.players[i].last_input.npc_move = ctx;
    }
}

/// The chain of an elite melee context (`m_MainAttack.m_Animation` of the elite sword's
/// `DisItemContext_NPCAttack*` tweaks). The special classes' contexts name chains of their own
/// profiles, which the simulation's one profile does not hold (U-978): None.
fn melee_chain(kind: ContextKind) -> Option<NpcAnim> {
    Some(match kind {
        context::ATTACK_SHORT => NpcAnim::AttackShort,
        context::ATTACK_STEP_BACK => NpcAnim::AttackStepBack,
        context::ATTACK_MEDIUM => NpcAnim::AttackMedium,
        context::ATTACK_LONG => NpcAnim::AttackLong,
        context::JUMP_ATTACK => NpcAnim::JumpAttack,
        _ => return None,
    })
}

/// Whether a context is a melee attack (`DisItemContext_MeleeAttackNPCBase` and the special
/// classes' attack contexts): the ones that swing.
pub fn swings(kind: ContextKind) -> bool {
    use context as c;
    matches!(kind, c::ATTACK_SHORT | c::ATTACK_STEP_BACK | c::ATTACK_MEDIUM | c::ATTACK_LONG | c::JUMP_ATTACK | c::WH_BITE | c::WH_JUMP_ATTACK | c::WH_ARM_ATTACK | c::TALLBOY_STOMP | c::OVERSEER_BASH)
}

/// The AI's requests: a started context through the context gate (an elite attack requests
/// ActionMelee, the melee start 0x00C24020; a special class's attack, whose chain the profile
/// does not hold, swings with its timed phases; the side and back steps play `SideStep` /
/// `JumpBack` through PlayAnim, 0x00C277A0 / 0x00C20C50), and a GenericAction of a new serial
/// (PlayAnim, the GenericAction tick 0x00B46EF0).
fn take_request(state: &mut WorldState, i: usize, cfg: &SimConfig, out: &mut StepOut) {
    let p = state.players[i];
    let req = request(state, p.id);
    if req.start && allows_context(state, p.id) {
        match req.context {
            k if swings(k) => match melee_chain(k) {
                Some(anim) if p.sword.phase == SwordPhase::Idle => {
                    try_request(state, p.id, StateRequest::new(Master::ActionMelee, anim as u8, -1, 0, k), cfg);
                }
                None if p.sword.phase == SwordPhase::Idle => start_swing(&mut state.players[i], k, crate::anim::SeqId::MAX, &mut out.events),
                _ => {}
            },
            context::SIDE_STEP => {
                try_request(state, p.id, StateRequest::new(Master::PlayAnim, NpcAnim::SideStep as u8, req.variant, 0, context::SIDE_STEP), cfg);
            }
            context::BACK_STEP => {
                try_request(state, p.id, StateRequest::new(Master::PlayAnim, NpcAnim::JumpBack as u8, -1, 0, context::BACK_STEP), cfg);
            }
            _ => {}
        }
    }
    if let Some((anim, serial)) = req.play_anim {
        let s = state.pawn_states.get_mut(&p.id).unwrap();
        if serial != s.play_serial {
            s.play_serial = serial;
            s.play_anim = anim;
            try_request(state, p.id, StateRequest::new(Master::PlayAnim, anim, -1, 0, context::NONE), cfg);
        }
    }
}

/// An NPC swing's start (the melee context's start, 0x00C24020 -> slot 125 0x00C0D670: the zones
/// reset, `m_MeleeState` 0): the attacker side of `crate::combat` takes it from here.
pub fn start_swing(p: &mut PlayerState, kind: ContextKind, seq: SeqId, events: &mut Vec<SimEvent>) {
    let s = &mut p.sword;
    s.phase = SwordPhase::Startup;
    s.timer = 0.0;
    s.hit_done = false;
    s.hit_count = 0;
    s.offscreen_to = None;
    s.npc_move = kind;
    s.swing_clip = s.swings % 4;
    s.swings = s.swings.wrapping_add(1);
    s.swing_seq = seq;
    events.push(SimEvent::SwordSwing { id: p.id });
}

/// One tick of pawn `i`'s machine (0x00A6DE60): a pending request of another state exits the
/// current one and enters it, without a tick; a re-request restarts the state and ticks it;
/// otherwise the state ticks, and an exit it requests applies right after.
fn machine_tick(state: &mut WorldState, i: usize, cfg: &SimConfig, out: &mut StepOut) {
    let id = state.players[i].id;
    let pending = state.pawn_states.get_mut(&id).and_then(|s| s.pending.take());
    match pending {
        Some(p) if p.state != state.pawn_states[&id].current.state => {
            transition(state, i, p, cfg, out);
            return;
        }
        Some(p) => {
            // Re-requested (slot 75): the state restarts with the new parameters.
            stop_chain(state, id);
            if let Some(s) = state.pawn_states.get_mut(&id) {
                s.current = p;
            }
            enter(state, i, cfg, out);
        }
        None => {}
    }
    state_tick(state, i);
    if let Some(p) = state.pawn_states.get_mut(&id).and_then(|s| s.pending.take()) {
        if p.state != state.pawn_states[&id].current.state {
            transition(state, i, p, cfg, out);
        }
    }
}

/// 0x00A5F8C0: the old state's exit, the switch, the new state's enter.
fn transition(state: &mut WorldState, i: usize, next: StateRequest, cfg: &SimConfig, out: &mut StepOut) {
    exit(state, i, next.state);
    let id = state.players[i].id;
    if let Some(s) = state.pawn_states.get_mut(&id) {
        s.current = next;
    }
    enter(state, i, cfg, out);
}

/// A state's enter (slot 73, the action states' common part slot 93).
fn enter(state: &mut WorldState, i: usize, cfg: &SimConfig, out: &mut StepOut) {
    let id = state.players[i].id;
    let cur = state.pawn_states[&id].current;
    match cur.state {
        // StateNPCMasterWalk's enter (0x00A873B0): (RMM_Velocity, RMRM_RotateActor).
        Master::Walk => {
            if let Some(s) = state.pawn_states.get_mut(&id) {
                s.rm_mode = root_mode::VELOCITY;
                s.rm_rotation = 1;
            }
        }
        // The melee context's chain (0x00A6AA40): its step is the context's `m_MeleeState` (0, the
        // small attack; the big follow-up on a big hit is not ported), full body; the swing starts
        // with it. Its play failing exits.
        Master::ActionMelee => {
            let tok = token::SWING | state.players[i].sword.swings.wrapping_add(1) as u32;
            if play_chain(state, i, cur.anim, cur.group, cur.step, tok, cfg) {
                let seq = state.anim.get(&id).and_then(|a| a.layers[layer::FULL_BODY].as_ref()).map_or(SeqId::MAX, |l| l.seq);
                start_swing(&mut state.players[i], cur.context, seq, &mut out.events);
            } else {
                request_exit(state.pawn_states.get_mut(&id).unwrap());
            }
        }
        // The parry state's chain (0x00A6A330): `m_MeleeState` 7 (Parried_Lose) plays `Parry`
        // group 2 on the full body, its step from the swing directions of the two contexts
        // (0x00C174C0); the defender's side is not modelled: the attack's own direction picks
        // Right* (steps 0, 1) or Left* (2, 3) (LIKELY).
        Master::ActionParry => {
            let step = parried_step(state, i, cfg);
            if !play_chain(state, i, NpcAnim::Parry as u8, 2, step, token::PARRY, cfg) {
                request_exit(state.pawn_states.get_mut(&id).unwrap());
            }
        }
        // StateNPCMasterPlayAnim (0x00A65A60): the chain on slot 0; a play failure exits.
        Master::PlayAnim => {
            if !play_chain(state, i, cur.anim, cur.group, cur.step, 0, cfg) {
                request_exit(state.pawn_states.get_mut(&id).unwrap());
            }
        }
        // Dead_Limp's enter (0x00A9FC40): the death chain, its step the death direction.
        Master::Dead => {
            if !play_chain(state, i, cur.anim, cur.group, cur.step, token::DEATH, cfg) {
                state.anim.get_mut(&id).map(|a| a.stop(layer::FULL_BODY));
            }
        }
        Master::Thrown => {}
    }
}

/// A state's exit (slot 74): the action states stop slots 0 and 1 (0x00A64AE0 -> 0x00C4D560)
/// and release their context (slot 83 -> the context's slot 104: the melee context finishes,
/// 0x00C12FC0, so the swing ends unless the parry or the block's recoil holds it).
fn exit(state: &mut WorldState, i: usize, next: Master) {
    let id = state.players[i].id;
    let cur = state.pawn_states[&id].current.state;
    if matches!(cur, Master::ActionMelee | Master::ActionParry | Master::PlayAnim | Master::Dead) {
        stop_chain(state, id);
        if let Some(a) = state.anim.get_mut(&id) {
            a.stop(layer::UPPER_BODY);
        }
    }
    let p = &mut state.players[i];
    match cur {
        Master::ActionMelee if next != Master::ActionParry => {
            let s = &mut p.sword;
            let recoil = s.phase == SwordPhase::Recovery && s.timer < 0.0;
            if matches!(s.phase, SwordPhase::Startup | SwordPhase::Active | SwordPhase::Recovery) && !recoil {
                s.phase = SwordPhase::Idle;
                s.timer = 0.0;
            }
        }
        Master::ActionParry if p.sword.phase == SwordPhase::Staggered => {
            p.sword.phase = SwordPhase::Idle;
            p.sword.timer = 0.0;
        }
        _ => {}
    }
}

/// A state's tick (slot 77): ActionMelee exits when its slot no longer plays its chain (a hit or
/// a block cut the swing, 0x00A6AB50); Thrown hands back once the pawn is up (physics.md 3.7).
fn state_tick(state: &mut WorldState, i: usize) {
    let p = state.players[i];
    let Some(s) = state.pawn_states.get(&p.id) else { return };
    let exit = match s.current.state {
        Master::ActionMelee | Master::ActionParry | Master::PlayAnim => s.chain.is_none() && s.pending.is_none(),
        Master::Thrown => p.alive && !p.thrown.active && !p.thrown.ragdoll,
        Master::Walk | Master::Dead => false,
    };
    if exit {
        request_exit(state.pawn_states.get_mut(&p.id).unwrap());
    }
}

/// The step of a Parried_Lose `Parry` chain (0x00C174C0): Right* when the swing's attack zone
/// swings right, Left* when it swings left (`m_SwingDirection`), the defender's side the first of
/// each pair (LIKELY: the opponent's direction 0x00C03980 is not modelled).
fn parried_step(state: &WorldState, i: usize, cfg: &SimConfig) -> u8 {
    let p = &state.players[i];
    let left = cfg.anim.info(p.sword.swing_seq).and_then(|s| s.notify("DishonoredNotify_AttackZone")).and_then(|n| n.names.get("m_SwingDirection")).is_some_and(|d| d.to_ascii_lowercase().contains("left"));
    if left { 2 } else { 0 }
}

/// Requests from the step's own rules (pending until the pawn's next machine tick): a parried
/// NPC attacker's Parried_Lose, a death, a throw.
pub fn request_parried(state: &mut WorldState, id: PlayerId, cfg: &SimConfig) {
    try_request(state, id, StateRequest::new(Master::ActionParry, NpcAnim::Parry as u8, 2, 0, context::NONE), cfg);
}

/// A death (`StateNPCMasterDead_Limp`, its chain and direction step: 0x00B77E20 / 0x00BC6FC0).
pub fn request_death(state: &mut WorldState, id: PlayerId, anim: NpcAnim, step: u8, cfg: &SimConfig) {
    state.pawn_states.entry(id).or_insert_with(PawnStates::new);
    try_request(state, id, StateRequest::new(Master::Dead, anim as u8, -1, step, context::NONE), cfg);
}

/// The throw (`StateNPCMasterThrown`): entered with its first update in the hit's tick
/// (`crate::anim::play_throw` plays the chain), so forced at once.
pub fn enter_thrown(state: &mut WorldState, id: PlayerId) {
    let Some(i) = state.players.iter().position(|p| p.id == id) else { return };
    if !has_states(&state.players[i]) {
        return;
    }
    state.pawn_states.entry(id).or_insert_with(PawnStates::new);
    if state.pawn_states[&id].current.state == Master::Thrown {
        return;
    }
    exit(state, i, Master::Thrown);
    if let Some(s) = state.pawn_states.get_mut(&id) {
        s.current = StateRequest::new(Master::Thrown, NpcAnim::PushToRagdoll as u8, 1, 0, context::NONE);
        s.pending = None;
        // `StateNPCMasterThrown` (0x00A81860): `RMM_Ignore`, flying.
        s.rm_mode = root_mode::IGNORE;
        s.loco_off = true;
    }
}

/// The death's `DishonoredNotify_Ragdoll` (pawn slot 352): the pawn goes to the ragdoll.
pub fn ragdolled(state: &mut WorldState, id: PlayerId) {
    if let Some(s) = state.pawn_states.get_mut(&id) {
        s.ragdoll = true;
    }
}

// ---- root motion ----

/// The root motion of a layer's last advance in the pawn's frame (forward, right), from the
/// config's keys (`GameplayConfig::root_motion`) between the step's start and end (a looping step
/// through its end), times the slot channel's weight (`elapsed / blend_in` up to 1: the slot
/// blends root motion by its channel weight, `AnimNodeBlendBase` 0x005A3BD0).
fn layer_root(l: &crate::anim::SeqPlay, cfg: &SimConfig) -> Option<(f32, f32)> {
    let info = cfg.anim.info(l.seq)?;
    let rm = cfg.gameplay.root_motion.get(info.root_motion.as_ref()?)?;
    let len = info.duration;
    let (a, b) = (l.prev, l.time);
    let d = if l.looping && b < a && len > 0.0 {
        let (s, e, z, t) = (rm.at(a), rm.at(len), rm.at(0.0), rm.at(b));
        [e[0] - s[0] + t[0] - z[0], e[1] - s[1] + t[1] - z[1]]
    } else if b > a {
        let (s, e) = (rm.at(a), rm.at(b));
        [e[0] - s[0], e[1] - s[1]]
    } else {
        return None;
    };
    let w = if l.blend_in > 0.0 { (l.elapsed / l.blend_in).min(1.0) } else { 1.0 };
    Some((d[0] * w, d[1] * w))
}

/// The pose barrier's root motion for the NPCs (0x00746280), after every pawn's advance: the
/// full-body chain's root translation turned into the world by the pawn's facing; `RMM_Translate`
/// after a tick in that mode moves the pawn now (`MoveActor`, with collision), the first tick of
/// the mode and the `RMM_Velocity` / `RMM_Accel` modes accumulate for the locomotion, `RMM_Ignore`
/// and no root motion zero the accumulation. The root's yaw (`RMRM_RotateActor`) is applied only
/// with `RMM_Translate`, and the death chains that rotate are none of the elite guard's
/// directional deaths: not applied (the turn's rotation, U-280, likewise).
pub fn barrier(state: &mut WorldState, world: &dyn dishonored_world::WorldQuery, cfg: &SimConfig) {
    for i in 0..state.players.len() {
        let p = state.players[i];
        let Some(s) = state.pawn_states.get(&p.id) else { continue };
        if p.time_scale <= 0.0 {
            continue;
        }
        let (mode, previous, ragdoll) = (s.rm_mode, s.rm_previous, s.ragdoll);
        let chain = s.chain;
        let delta = chain.filter(|_| !ragdoll).and_then(|c| state.anim.get(&p.id).and_then(|a| a.layers[layer::FULL_BODY].as_ref()).filter(|l| l.token == c.token).and_then(|l| layer_root(l, cfg)));
        let s = state.pawn_states.get_mut(&p.id).unwrap();
        s.rm_previous = mode;
        let Some((f, r)) = delta.filter(|_| mode != root_mode::IGNORE) else {
            s.rm_accum = Vec3::ZERO;
            continue;
        };
        let w = crate::movement::forward_vector(p.yaw) * f + crate::movement::right_vector(p.yaw) * r;
        let w = Vec3::new(w.x, w.y, 0.0);
        if mode == root_mode::TRANSLATE && previous == root_mode::TRANSLATE {
            s.rm_accum = Vec3::ZERO;
            let mvc = crate::powers::effective_movement(&p, &cfg.gameplay);
            let ctx = crate::movement::MoveCtx { world, cfg: &mvc, dt: crate::DT * p.time_scale };
            crate::movement::walk_move(&mut state.players[i], w, &ctx);
        } else {
            s.rm_accum += w;
        }
    }
}

/// The NPC locomotion after the barrier (the Ark tick manager's list 0, 0x0094BEC0): with the
/// component switched off (a full-body chain plays), the accumulated root translation (its
/// horizontal part) over the tick's `dt`, times the stance modifier's `m_fSpeedMultiplier` (the
/// look-at factor, +0x264, is 1 for a pawn facing its target within `m_fLookAtMinAngle`; the port's
/// NPCs face it at once, U-862), projected on a sloped floor (`Floor.Z` < 0.99: the direction
/// on the floor plane at the horizontal speed), is the velocity the next tick's walking physics
/// integrates (`PlayerState::root_velocity`); not while falling, ragdolled or with `RMM_Ignore`.
/// The accumulation is zeroed. On a path the component steps the path instead (the port's NPCs
/// walk by their input: `crate::movement`).
pub fn locomotion(state: &mut WorldState, cfg: &SimConfig) {
    for i in 0..state.players.len() {
        let p = state.players[i];
        let Some(s) = state.pawn_states.get_mut(&p.id) else { continue };
        let d = std::mem::take(&mut s.rm_accum);
        if p.time_scale <= 0.0 || !s.loco_off {
            continue;
        }
        let falling = !matches!(p.mode, crate::state::MoveMode::Walking) || p.thrown.active || p.thrown.ragdoll;
        if falling || s.rm_mode == root_mode::IGNORE || s.ragdoll {
            continue;
        }
        let dt = crate::DT * p.time_scale;
        let mult = request(state, p.id).loco.and_then(|(c, m)| cfg.gameplay.ai.locomotion.get(c as usize).and_then(|l| l.modifiers.get(m as usize))).map_or(1.0, |m| m.speed_multiplier);
        let n = p.floor_normal;
        let speed = d.truncate().length() / dt * mult;
        let v = if n.z < 0.99 && n.z.abs() >= 1e-4 && d.truncate().length_squared() > 1e-8 {
            Vec3::new(d.x, d.y, -(d.x * n.x + d.y * n.y) / n.z).normalize_or_zero() * speed
        } else {
            Vec3::new(d.x, d.y, 0.0) / dt * mult
        };
        state.players[i].root_velocity = Some(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SimConfig {
        SimConfig::from_json(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json")).unwrap()).unwrap()
    }

    /// The cooked transition logic: an attack may not hand back to Walk by request (only by its
    /// forced exit), a strong hit reaction refuses another, the dead stay dead, PlayAnim takes
    /// another PlayAnim through its parent rows.
    #[test]
    fn transition_logic_from_the_data() {
        let c = cfg();
        let f = &c.gameplay.npc_anim.fsm;
        assert_eq!(f.result("StateNPCMasterActionMelee", "StateNPCMasterWalk"), transition::NOT_ALLOWED);
        assert_eq!(f.result("StateNPCMasterActionMelee", "StateNPCMasterDead_Limp"), transition::ALLOWED);
        assert_eq!(f.result("StateNPCMasterStrongHitReact", "StateNPCMasterStrongHitReact"), transition::NOT_ALLOWED);
        assert_eq!(f.result("StateNPCMasterDead_Limp", "StateNPCMasterPlayAnim"), transition::NOT_ALLOWED);
        assert_eq!(f.result("StateNPCMasterPlayAnim", "StateNPCMasterPlayAnim"), transition::ALLOWED);
        assert_eq!(f.result("StateNPCMasterWalk", "StateNPCMasterActionMelee"), transition::CALL);
        assert_eq!(f.result("StateNPCMasterActionMelee", "StateNPCActionParry"), transition::CALL);
    }
}
