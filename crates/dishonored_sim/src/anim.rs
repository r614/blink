//! Animation state in the simulation (research/ARCHITECTURE.md, migration step 5): per pawn the
//! sequences its gameplay states play, advanced by the simulation's own clock in the original's
//! order (research/dishonored/systems/tick_order.md, "Consequences for the port"), with the
//! notifies they cross firing gameplay from the playhead instead of copied constants.
//!
//! - **Sequences** are the config's table (`GameplayConfig::sequences`, [`SeqTable`]): lengths,
//!   rate scales and notifies decoded from the AnimSets, so the simulation steps them and fires
//!   their notifies with or without the install. A sequence is named by its index in the table
//!   ([`SeqId`]); `SimEvent::AnimNotify` carries it.
//! - **Layers** ([`layer`]): the player plays the sequences of its states on a slot-like
//!   [`SeqPlay`] per layer (the full-body, upper-body and left-arm pickers of `Ply_Player_at` as
//!   `viewmodel.rs` documents them from `Twk_Pawn_Corvo`'s state tweaks, without the picker tree,
//!   whose `DishonoredAnimNodeStatePicker` semantics are not decoded: U-972), plus the held
//!   crossbow's own mesh and an additive over the whole pose (the step-up's head); the player's
//!   states' sequences are `crate::player_anim`'s. An NPC plays its action chains on the full body
//!   (`FullBodySlot`), the upper body (`UpperBodySlot`) and the additive reactions
//!   (`AdditiveReactSlot`); a human player's third-person body plays its own chains
//!   (`crate::player_anim::Body`) on its tree.
//! - **Order** (tick_order.md section 4 and "Consequences" 2-3): a pawn's states run after its
//!   physics; the requests they make play in the same tick's advance (the first step of a new
//!   sequence is `[0, RateScale x Rate x dt)`); the advance is `DT x` the pawn's time scale; every
//!   notify whose `Time` lies in `[t0, t1)` fires during the advance (`UAnimNodeSequence::
//!   IssueNotifies` 0x00599A50), its handler seeing the step's start `t0`; a notify with a
//!   duration is one of Arkane's windows (`0x00A8F340`: start, then `Tick(seq, 0, elapsed,
//!   duration)` and either the end at once or entry into the active list; each later step
//!   `0x00A9BC90`: `remaining -= MoveDelta`, `Tick(before, after)`, the end at `remaining <= 0`).
//!   Requests made after the pawn group (the paired moves the sweep starts, a death, a throw)
//!   play from the next tick's advance. Root motion is published after every pawn advanced
//!   (`sync_motion`), before the melee sweep.
//! - **Handlers** act during the advance where the original's do (the attack zone's tick
//!   handler opens the damage zone, 0x00A8E760; the casts' and the throw's notifies fire their
//!   power or projectile; the ragdoll notify ends a throw's flight); `AnimStateExit`,
//!   `AttackInterruptable` and `AnimStateAdvance` raise flags the states read on their next tick
//!   (`m_bAttackNowInterruptable`, melee.md 2.1), which is how the measured actions end (the
//!   crossbow capture: action ends 24 ticks after the press for a 0.398 s exit, ranged.md F14).
//! - **NPC trees**: with the install, the host's [`AnimSource`] gives an NPC its tree
//!   (`NPC_Human_at`, `NPC_Dead_at` when dead), skeleton and clips; [`PawnAnim::tree`] keeps the
//!   evaluator's state (`dishonored_anim::TreeState`), driven by the sim's own state and the
//!   layers' requests, and publishes the root motion of its slots. Without the install the NPC
//!   steps only its layers. The NPC states' root-motion modes are not decoded (U-976), so the
//!   tree's root motion is published, not applied.

use crate::events::SimEvent;
use crate::input::LeftHand;
use crate::left_hand::LeftAction;
use crate::state::{LeftPhase, PawnKind, PlayerId, PlayerState, ProjectileKind, SwordPhase, WorldState};
use crate::{DT, SimConfig};
use dishonored_anim::tree::{FilterTarget, LocoInput, SlotAction, SlotRequest, TreeInputs, TreeShape, TreeState};
use dishonored_anim::Clips;
use dishonored_ir::gameplay::{GameplayConfig, NotifyInfo, SequenceInfo, sequence_key};
use dishonored_ir::npc_anim::{LocoConfig, NpcAnim};
use dishonored_world::WorldQuery;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A sequence's index in [`SeqTable`] (the sorted keys of `GameplayConfig::sequences`).
pub type SeqId = u16;

/// The config's sequences in key order: a sequence's index is its [`SeqId`].
#[derive(Debug, Clone, Default)]
pub struct SeqTable {
    keys: Vec<String>,
    infos: Vec<SequenceInfo>,
}

impl SeqTable {
    pub fn new(seqs: &BTreeMap<String, SequenceInfo>) -> Self {
        Self { keys: seqs.keys().cloned().collect(), infos: seqs.values().cloned().collect() }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The id of a key (`<set>.<sequence>`); the table keeps `BTreeMap`'s byte order.
    pub fn id(&self, key: &str) -> Option<SeqId> {
        self.keys.binary_search_by(|k| k.as_str().cmp(key)).ok().map(|i| i as SeqId)
    }

    /// The id of a sequence of a set.
    pub fn id_of(&self, set: &str, sequence: &str) -> Option<SeqId> {
        self.id(&sequence_key(set, sequence))
    }

    /// The id of `sequence` in the first of `sets` (searched from the last, UE3's
    /// `FindAnimSequence`) whose table entry exists.
    pub fn find(&self, sets: &[String], sequence: &str) -> Option<SeqId> {
        sets.iter().rev().find_map(|s| self.id_of(s, sequence))
    }

    pub fn info(&self, id: SeqId) -> Option<&SequenceInfo> {
        self.infos.get(id as usize)
    }

    pub fn key(&self, id: SeqId) -> Option<&str> {
        self.keys.get(id as usize).map(String::as_str)
    }

    /// The sequence name of an id (the key without its set).
    pub fn name(&self, id: SeqId) -> Option<&str> {
        self.key(id).map(|k| k.split_once('.').map_or(k, |(_, n)| n))
    }
}

/// A sequence's info, required: the simulation's rules read these entries of the decoded table.
pub fn info<'a>(g: &'a GameplayConfig, set: &str, sequence: &str) -> &'a SequenceInfo {
    g.sequence(set, sequence).unwrap_or_else(|| panic!("gameplay config without the sequence {set}.{sequence} (regenerate it with gameplay_config)"))
}

/// The layers a pawn plays sequences on.
pub mod layer {
    /// The full body (the player's `FullBodyStatePicker`, the NPC's `FullBodySlot`).
    pub const FULL_BODY: usize = 0;
    /// Through `UpperBodyFilter` (the player's upper-body picker).
    pub const UPPER_BODY: usize = 1;
    /// Through `LeftArmFilter` (the player's left-arm picker).
    pub const LEFT_ARM: usize = 2;
    /// The held crossbow's own mesh (`Crossbows_as.Crossbow_as`), which carries the shot's
    /// `DishonoredNotify_FireProjectile`.
    pub const WEAPON: usize = 3;
    /// An additive over the whole first-person pose (`StepUpMantle`'s `ADD_Head_Mantle_Low`,
    /// `DisAnimNodeBlendByStepUpMantle`).
    pub const ADDITIVE: usize = 4;
    pub const COUNT: usize = 5;
}

/// The sequence id of the reference pose (an empty child of a blend: the player's climb at rest,
/// `Ply_Player_Climb_at`): no table entry, nothing advances but the play time.
pub const REF_POSE: SeqId = SeqId::MAX;

/// What a layer's sequence plays for (the gameplay state instance that requested it): its kind
/// in the high byte, an instance number below.
pub mod token {
    /// A sword swing, with `SwordState::swings` (wrapping) below.
    pub const SWING: u32 = 1 << 24;
    /// The block (`Sword_Ready_ParryIdle_In`).
    pub const BLOCK: u32 = 2 << 24;
    /// A paired move's master, with `SwordState::assassination_move` below.
    pub const PAIRED: u32 = 3 << 24;
    /// A left-hand action, with its kind below ([`super::left_kind`]).
    pub const LEFT: u32 = 4 << 24;
    /// The crossbow's fire on its own mesh.
    pub const WEAPON: u32 = 5 << 24;
    /// A thrown NPC's Windblast throw (`Generic_WindBlast_*`).
    pub const THROWN: u32 = 6 << 24;
    /// An NPC's death.
    pub const DEATH: u32 = 7 << 24;
    /// An NPC state's chain (`crate::pawn_states`), with the pawn's chain serial below.
    pub const CHAIN: u32 = 8 << 24;
    /// A parried NPC attacker's `Parry` chain (`StateNPCActionParry`).
    pub const PARRY: u32 = 9 << 24;
    /// A player state's sequence that continues while the state asks for it (navigation, the
    /// mantle, choke and carry masters, the stun, slide, climb, swim, the casts' aims): its sync
    /// group ([`super::sync`]) at bit 16 and the sequence id below, so that asking again for the
    /// same sequence keeps it playing.
    pub const NAV: u32 = 10 << 24;
    /// A player state's one-shot (equips, switches, the jump and crouch reactions, the gunshot
    /// reaction, the block's out, the won parry, the step-up's head), with the pawn's shot serial
    /// below: each request restarts.
    pub const ACTION: u32 = 11 << 24;
    /// A human player's third-person body chain (`crate::player_anim::Body`), with its serial
    /// below.
    pub const BODY: u32 = 12 << 24;
    pub const KIND: u32 = 0xFF << 24;
}

/// The sync groups of the player's navigation loops (`Ply_Nav_LeftHand_at`'s `MasterIdleSync`,
/// `MasterRunSync`, `MasterSprintSync` with the full body's idle, run and sprint): a left-hand
/// loop in the full body's group takes its relative position after every advance (the full
/// body's walk is in `WalkCycles`, which the left hand does not join).
pub mod sync {
    pub const NONE: u32 = 0;
    pub const IDLE: u32 = 1;
    pub const RUN: u32 = 2;
    pub const SPRINT: u32 = 3;
}

/// The left-hand actions a layer plays (the low bits of [`token::LEFT`]).
pub mod left_kind {
    pub const CAST_BEND_TIME: u32 = 1;
    pub const CAST_SWARM: u32 = 2;
    pub const CAST_POSSESSION: u32 = 3;
    pub const GRENADE: u32 = 4;
    pub const SPRING_RAZOR: u32 = 5;
    pub const FIRE: u32 = 6;
    pub const RELOAD_IN: u32 = 7;
    pub const RELOAD_OUT: u32 = 8;
}

/// The blend time of the state that plays a layer's sequence (the renderer's blend in and out;
/// nothing in the simulation reads it):
/// - the player's (`m_fBlendTime` of `Twk_Pawn_Corvo_Release`'s animation states, as
///   `viewmodel.rs` documented them): melee attacks and `AnimMeleeBlock` 0.1; gun fire, reload
///   in / out and the power casts 0.1; the grenade and Spring Razor deploys 0.2; the
///   assassinations and finishers (`Assassinate_*`, `Fatality_*`, the generic
///   `Fatality_GenericA` / `B`) 0.2;
/// - the crossbow mesh's fire: its own tree (`Crossbow_at`) was not decoded; 0.1 is the port's
///   value for its clip changes, not a decoded one;
/// - an NPC's full-body chains (swings, deaths, throws): `DisAnimation.m_fBlendInTime`'s class
///   default 0.2, the value `step_tree` requests the tree's slots with.
pub fn state_blend(layer: usize, token: u32) -> f32 {
    match token & token::KIND {
        token::SWING if layer == layer::FULL_BODY => 0.2,
        token::THROWN | token::DEATH => 0.2,
        token::SWING | token::BLOCK => 0.1,
        token::PAIRED => 0.2,
        token::LEFT => match token & !token::KIND {
            left_kind::GRENADE | left_kind::SPRING_RAZOR => 0.2,
            _ => 0.1,
        },
        token::WEAPON => 0.1,
        _ => 0.2,
    }
}

/// An active window of a notify with a duration (Arkane's list at +0x114 of the sequence node:
/// the notify and the time left).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub notify: u16,
    pub remaining: f32,
}

/// One layer's sequence player (`AnimNodeSequence`: `AnimSeqName`, `CurrentTime`, `Rate`,
/// `bPlaying`, `bLooping`) with the flags its notifies raised for the states.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeqPlay {
    pub seq: SeqId,
    pub token: u32,
    /// `CurrentTime` (sequence seconds) and the last step's start.
    pub time: f32,
    pub prev: f32,
    /// `Rate` (the sequence's `RateScale` applies on top).
    pub rate: f32,
    pub looping: bool,
    /// Advancing; a non-looping sequence stops at its end and holds its last frame.
    pub playing: bool,
    /// The pawn's yaw at the request: the frame of the sequence's root-motion keys.
    pub yaw: f32,
    /// `DishonoredNotify_AnimStateExit`, `DishonoredNotify_AttackInterruptable` and
    /// `DishonoredNotify_AnimStateAdvance` crossed.
    pub exited: bool,
    pub interruptible: bool,
    pub advanced: bool,
    pub windows: Vec<Window>,
    /// Play time since the request (the pawn's seconds) and the slot channel's blend-in (an NPC
    /// chain step's `m_fBlendInTime`; 0: at once): the channel's weight, which scales the
    /// sequence's root motion, is `elapsed / blend_in` up to 1 (`AnimNodeSlot`'s linear blend).
    #[serde(default)]
    pub elapsed: f32,
    #[serde(default)]
    pub blend_in: f32,
    /// The slot's blend-out when the sequence stops (an NPC chain group's `m_fBlendOutTime`).
    #[serde(default)]
    pub blend_out: f32,
    /// The requesting state's `m_fBlendTime` ([`state_blend`]): how long the layer takes to
    /// blend in (and, in the renderer, out). Read by the presentation only.
    #[serde(default)]
    pub blend: f32,
    /// The play time ([`Self::elapsed`]) at which the state let go of the layer: it blends out
    /// over [`Self::blend`] from there and is removed once out (`AnimNodeBlendBase::
    /// SetActiveChild` toward the source; [`PawnAnim::release`]).
    #[serde(default)]
    pub out: Option<f32>,
    /// `bNoNotifies` (the left hand's navigation loops, `Ply_Nav_LeftHand_at`): the advance fires
    /// no notify.
    #[serde(default)]
    pub quiet: bool,
}

impl SeqPlay {
    /// The state is done with its sequence: its `AnimStateExit` crossed, or the sequence ended.
    pub fn done(&self) -> bool {
        self.exited || !self.playing
    }

    /// The sequence's play time left to its end (sequence time over `RateScale x Rate`).
    pub fn time_left(&self, info: &SequenceInfo) -> f32 {
        (info.duration - self.time).max(0.0) / (info.rate_scale_or_one() * self.rate).abs().max(1e-6)
    }
}

/// An NPC's AnimTree evaluator state (with the install): the tree it was made from (0
/// `NPC_Human_at`, 1 `NPC_Dead_at`), the evaluator's state and the slot requests already handed
/// to it (by layer token). The shape is the host's ([`AnimSource`]), shared and not serialized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NpcTree {
    pub tree: u8,
    pub state: TreeState,
    pub requested: [u32; layer::COUNT],
    /// The last update's root motion (pawn frame, uu; yaw degrees).
    pub root: (Vec3, f32),
    /// The blend-out of the sequence each slot was last asked to play (its stop's).
    #[serde(default)]
    pub blend_out: [f32; layer::COUNT],
}

/// A pawn's animation state, beside its `PlayerState` in [`WorldState::anim`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PawnAnim {
    pub layers: [Option<SeqPlay>; layer::COUNT],
    /// The pawn's stream for chain picks (xorshift32, seeded from its id; its other rolls keep
    /// their own stream).
    pub rng: u32,
    #[serde(default)]
    pub tree: Option<NpcTree>,
    /// The player's state as the last tick left it (its states' transitions start one-shots:
    /// `crate::player_anim`); `None` for an NPC.
    #[serde(default)]
    pub prev: Option<crate::player_anim::PlayerPrev>,
    /// One-shots played so far (wrapping): the next [`token::ACTION`]'s serial.
    #[serde(default)]
    pub shots: u32,
    /// A human player's third-person body (`NPC_Human_at` driven by the player's states:
    /// `crate::player_anim::body_requests`); `None` for an NPC, whose tree takes its layers.
    #[serde(default)]
    pub body: Option<crate::player_anim::Body>,
    /// The player's navigation state the full body last showed (0 `Nav_Default_Ready`, 1
    /// `Nav_Crouched`, 2 `Nav_Sprinting_Ready`): a change of state blends over the state's time.
    #[serde(default)]
    pub nav_tree: Option<u8>,
}

impl PawnAnim {
    pub fn new(id: PlayerId) -> Self {
        Self { rng: id.wrapping_mul(0x85EB_CA6B) ^ 0x2545_F491 | 1, ..Default::default() }
    }

    /// Play `seq` on `layer` for a player state with its own blend time and notifies (`quiet`:
    /// `bNoNotifies`).
    #[allow(clippy::too_many_arguments)]
    pub fn play_state(&mut self, layer: usize, seq: SeqId, token: u32, rate: f32, looping: bool, yaw: f32, blend: f32, quiet: bool) {
        self.play(layer, seq, token, rate, looping, yaw);
        if let Some(l) = self.layers[layer].as_mut() {
            l.blend = blend;
            l.quiet = quiet;
        }
    }

    /// The next one-shot token ([`token::ACTION`]).
    pub fn next_shot(&mut self) -> u32 {
        self.shots = self.shots.wrapping_add(1);
        token::ACTION | (self.shots & 0x00FF_FFFF)
    }

    /// The state lets go of the layer: it blends out over its blend time from now (at once
    /// without one), then is removed by the advance.
    pub fn release(&mut self, layer: usize) {
        match self.layers[layer].as_mut() {
            Some(l) if l.blend > 0.0 => {
                if l.out.is_none() {
                    l.out = Some(l.elapsed);
                }
            }
            _ => self.layers[layer] = None,
        }
    }

    /// Next roll in [0, 1) of the chain-pick stream.
    pub fn roll(&mut self) -> f32 {
        let mut x = if self.rng == 0 { 0x2545_F491 } else { self.rng };
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Play `seq` on `layer` from its start (`AnimNodeSlot::PlayCustomAnim`), blending in over
    /// the requesting state's blend time.
    pub fn play(&mut self, layer: usize, seq: SeqId, token: u32, rate: f32, looping: bool, yaw: f32) {
        self.layers[layer] = Some(SeqPlay { seq, token, time: 0.0, prev: 0.0, rate, looping, playing: true, yaw, exited: false, interruptible: false, advanced: false, windows: Vec::new(), elapsed: 0.0, blend_in: 0.0, blend_out: 0.0, blend: state_blend(layer, token), out: None, quiet: false });
    }

    pub fn stop(&mut self, layer: usize) {
        self.layers[layer] = None;
    }

    /// The layer's sequence, when it plays for `token` (and its state still holds it).
    pub fn with(&self, layer: usize, token: u32) -> Option<&SeqPlay> {
        self.layers[layer].as_ref().filter(|s| s.token == token && s.out.is_none())
    }

    /// Whether a window of a notify of `class` is open on the layer.
    pub fn window_open(&self, layer: usize, table: &SeqTable, class: &str) -> bool {
        let Some(s) = self.layers[layer].as_ref() else { return false };
        let Some(info) = table.info(s.seq) else { return false };
        s.windows.iter().any(|w| info.notifies.get(w.notify as usize).is_some_and(|n| n.class == class))
    }
}

/// The animation data of an NPC tree, from the host.
#[derive(Clone)]
pub struct NpcTreeAssets {
    pub shape: Arc<TreeShape>,
    pub clips: Arc<dyn Clips + Send + Sync>,
    /// The profile's `ArkComponentLocomotionConfig`.
    pub loco: Arc<LocoConfig>,
}

/// Clip data the host provides: the simulation cannot decode packages. The match content
/// (`dishonored_content::MatchContent`) gives the install's trees, skeletons and clips (loaded on
/// first use); without the install ([`NoClips`]) the simulation steps sequences by their decoded
/// lengths and fires their notifies from the config's table, without poses or clip root motion.
pub trait AnimSource: Send + Sync {
    /// The NPC profile's (`GameplayConfig::npc_anim`) tree, `NPC_Dead_at` when `dead`.
    fn npc_tree(&self, dead: bool) -> Option<NpcTreeAssets>;
}

/// No clips: durations and notifies only.
pub struct NoClips;

impl AnimSource for NoClips {
    fn npc_tree(&self, _dead: bool) -> Option<NpcTreeAssets> {
        None
    }
}

// ---- requests ----

/// The player's swing clips by `SwordState::swing_clip` (`Ply_Sword_Attack_as`).
pub const SWING_CLIPS: [&str; 5] = ["Sword_Ready_AttackRight_A_Small", "Sword_Ready_AttackLeft_A_Small", "Sword_Ready_AttackRight_B_Small", "Sword_Ready_AttackLeft_B_Small", "Sword_Sneak_Attack_Small"];
pub const SWORD_ATTACK_SET: &str = "Ply_Sword_Attack_as";
pub const SWORD_VERSUS_SET: &str = "Ply_Sword_Versus_as";
pub const ASSASSINATION_SET: &str = "Ply_Sword_Assassination_as";
pub const FATALITY_SET: &str = "Ply_Sword_Fatality_as";
pub const CHOKE_SET: &str = "Ply_Sword_Choke_as";
pub const CARRY_SET: &str = "Ply_Empty_CarryCorpse_as";
pub const POWERS_SET: &str = "Ply_Powers_as";
pub const PISTOL_SET: &str = "Ply_Pistol_as";
pub const CROSSBOW_SET: &str = "Ply_Crossbow_as";
pub const GUNS_SET: &str = "Ply_Guns_as";
pub const GRENADE_SET: &str = "Ply_Gadgets_Grenade_as";
pub const RAZOR_SET: &str = "Ply_Gadgets_SpringRazor_as";
/// The crossbow mesh's set.
pub const CROSSBOW_MESH_SET: &str = "Crossbow_as";

/// The master of a paired move (`SwordState::assassination_move`) and the layer it plays on.
pub fn paired_master(p: &PlayerState, cfg: &SimConfig) -> Option<(SeqId, usize)> {
    use dishonored_ir::gameplay::ASSASSINATION_MASTERS;
    let m = p.sword.assassination_move as usize;
    let t = &cfg.anim;
    if m < 8 {
        return t.id_of(ASSASSINATION_SET, ASSASSINATION_MASTERS[m]).map(|s| (s, layer::FULL_BODY));
    }
    if m < ASSASSINATION_MASTERS.len() {
        return t.id_of(FATALITY_SET, ASSASSINATION_MASTERS[m]).map(|s| (s, layer::FULL_BODY));
    }
    if p.sword.assassination_move == crate::fatality::FATALITY_MOVE {
        let run = p.sword.fatality?;
        let clip = cfg.gameplay.fatality.moves.get(run.action as usize)?.get(run.variant as usize)?;
        let seq = t.id_of(FATALITY_SET, &clip.master)?;
        return Some((seq, if clip.upper_body { layer::UPPER_BODY } else { layer::FULL_BODY }));
    }
    None
}

/// The sequence of a step of an NPC chain group (`DisAnimationChain`): the step's name (one of
/// its alternatives, picked with `roll`) with the item prefix when the step has one
/// (`m_FirstItemPrefix`), looked up in the profile's sets.
fn chain_step(cfg: &SimConfig, anim: NpcAnim, group: usize, step: usize, roll: f32, armed: bool) -> Option<SeqId> {
    let g = cfg.gameplay.npc_anim.defs.chain(anim)?.groups.get(group)?;
    let s = g.steps.get(step).or(g.steps.first())?;
    crate::pawn_states::step_sequence(cfg, s, roll, armed)
}

/// The sequences a player's left hand plays for its state on the tick its action starts (the
/// left-arm picker's actions, `viewmodel.rs`): casts, gadget deploys, the guns' fire (with the
/// crossbow mesh's own clip) and reload stages. The guns' actions still end by the tick count
/// against their exits (`left_hand`, the measured reload ticks); their sequences give the
/// notifies.
fn left_requests(p: &PlayerState, cfg: &SimConfig, a: &mut PawnAnim) {
    let t = &cfg.anim;
    let start = p.left.timer == 0.0;
    let item = p.left.item;
    let want: Option<(usize, &str, &str, u32)> = match (p.left.phase, item) {
        (LeftPhase::Recovering, LeftHand::BendTime) => Some((layer::LEFT_ARM, POWERS_SET, "Powers_Cast_BendTime", left_kind::CAST_BEND_TIME)),
        (LeftPhase::Recovering, LeftHand::DevouringSwarm) => Some((layer::LEFT_ARM, POWERS_SET, "Powers_Cast_Swarm", left_kind::CAST_SWARM)),
        (LeftPhase::Recovering, LeftHand::Possession) => Some((layer::LEFT_ARM, POWERS_SET, "Powers_Cast_Possession_Out", left_kind::CAST_POSSESSION)),
        (LeftPhase::Recovering, LeftHand::Grenade | LeftHand::StickyGrenade) => Some((layer::LEFT_ARM, GRENADE_SET, "Gadgets_Grenade_Deploy", left_kind::GRENADE)),
        (LeftPhase::Recovering, LeftHand::SpringRazor) => Some((layer::LEFT_ARM, RAZOR_SET, "Gadgets_SpringRazor_Deploy", left_kind::SPRING_RAZOR)),
        (LeftPhase::Recovering, LeftHand::Pistol) => Some((layer::LEFT_ARM, PISTOL_SET, "Pistol_Fire", left_kind::FIRE)),
        (LeftPhase::Recovering, LeftHand::Crossbow | LeftHand::SleepDart | LeftHand::IncendiaryBolt) => {
            let fast = crate::left_hand::crossbow_fast_fire(p, &cfg.gameplay, &cfg.tuning);
            Some((layer::LEFT_ARM, CROSSBOW_SET, if fast { "Crossbow_Fire_FastReload" } else { "Crossbow_Fire" }, left_kind::FIRE))
        }
        _ => None,
    };
    let token = |k: u32| token::LEFT | k;
    match want {
        Some((l, set, seq, kind)) if start => {
            // `FireReloadFastCrossbow`'s `m_fPlayRateOverride` on the arms (the crossbow mesh's
            // own clip keeps rate 1: its tree was not decoded).
            let rate = if seq == "Crossbow_Fire_FastReload" && cfg.tuning.crossbow_fast_fire_rate > 0.0 { cfg.tuning.crossbow_fast_fire_rate } else { 1.0 };
            if let Some(id) = t.id_of(set, seq) {
                a.play(l, id, token(kind), rate, false, p.yaw);
            }
            // The crossbow mesh plays its own fire clip with the arms.
            if kind == left_kind::FIRE && item.is_crossbow() {
                if let Some(id) = t.id_of(CROSSBOW_MESH_SET, seq) {
                    a.play(layer::WEAPON, id, token::WEAPON, 1.0, false, p.yaw);
                }
            }
        }
        Some(_) => {}
        None if p.left.phase == LeftPhase::Reloading => {
            let g = crate::left_hand::gun_reload(p, &cfg.gameplay, &cfg.tuning);
            let tick = crate::left_hand::ticks_of(p.left.timer, DT);
            let set = if item.is_crossbow() { CROSSBOW_SET } else { PISTOL_SET };
            let pre = if item.is_crossbow() { "Crossbow" } else { "Pistol" };
            // The reload's actions (`DisItemContext_ReloadGunPlayer`): In on its start (the fast
            // reload's own action with `Attribute_FastReload`), Out when the timed stage loads.
            let req = if start {
                Some(if g.fast > 0.0 { (format!("{pre}_{}", if item.is_crossbow() { "RapidReload" } else { "FastReload" }), left_kind::RELOAD_IN) } else { (format!("{pre}_Reload_In"), left_kind::RELOAD_IN) })
            } else if g.fast <= 0.0 && tick == g.load_ticks {
                Some((format!("{pre}_Reload_Out"), left_kind::RELOAD_OUT))
            } else {
                None
            };
            if let Some((seq, kind)) = req {
                if a.with(layer::LEFT_ARM, token(kind)).is_none() {
                    if let Some(id) = t.id_of(set, &seq) {
                        // The crossbow's reload actions at `m_fPlayRateOverride` 1.3
                        // (`Twk_Inv_CrossbowCorvo`, as `ReloadTiming::crossbow` times them); a
                        // fast reload stretched over its stage, cut in (`m_fBlendTime` 0) (the
                        // stretch is the former picker's, LIKELY: the fast reload actions' rate
                        // is not loaded).
                        let len = t.info(id).map_or(0.0, |i| i.play_length());
                        let (rate, blend) = if g.fast > 0.0 {
                            (if g.end > 0.0 && len > 0.0 { (len / g.end).clamp(0.25, 4.0) } else { 1.0 }, 0.0)
                        } else if item.is_crossbow() {
                            (crate::tuning::CROSSBOW_RELOAD_RATE, state_blend(layer::LEFT_ARM, token(kind)))
                        } else {
                            (1.0, state_blend(layer::LEFT_ARM, token(kind)))
                        };
                        a.play(layer::LEFT_ARM, id, token(kind), rate, false, p.yaw);
                        if let Some(l) = a.layers[layer::LEFT_ARM].as_mut() {
                            l.blend = blend;
                        }
                    }
                }
            }
        }
        None => {
            // The action is over: its sequence keeps playing out on the arm; the crossbow mesh's
            // fire stops with the crossbow.
            if !item.is_crossbow() {
                a.stop(layer::WEAPON);
            }
        }
    }
}

/// The requests of a player's states this tick, made after its states ran and before its advance
/// (a request made here plays in this tick's advance): the swing, the left hand's actions, then
/// the rest of its states (`crate::player_anim`). The NPCs' chains are their pawn states'
/// (`crate::pawn_states`).
pub fn sync_requests(state: &mut WorldState, i: usize, cfg: &SimConfig) {
    let p = state.players[i];
    if p.kind != PawnKind::Human {
        return;
    }
    let a = state.anim.entry(p.id).or_insert_with(|| PawnAnim::new(p.id));
    let t = &cfg.anim;
    let s = &p.sword;
    let swing_layer = layer::UPPER_BODY;
    let swing_token = token::SWING | s.swings as u32;
    let mut swing_seq = None;
    if p.alive && s.phase == SwordPhase::Startup && s.timer == 0.0 && a.with(swing_layer, swing_token).is_none() {
        let seq = t.id_of(SWORD_ATTACK_SET, SWING_CLIPS[(s.swing_clip as usize).min(4)]);
        if let Some(seq) = seq {
            a.play(swing_layer, seq, swing_token, 1.0, false, p.yaw);
        }
        swing_seq = Some(seq.unwrap_or(SeqId::MAX));
    }
    // A swing cut short stops its sequence: staggered, blocking, in a paired move, dead, or back
    // to Idle before its `AttackInterruptable` (a hit interrupted it). A finished one plays out.
    let cut = !p.alive || matches!(s.phase, SwordPhase::Staggered | SwordPhase::Blocking | SwordPhase::Assassinating);
    let swinging = a.layers[swing_layer].as_ref().filter(|l| l.token & token::KIND == token::SWING);
    if swinging.is_some_and(|l| cut || (s.phase == SwordPhase::Idle && l.token == swing_token && !l.interruptible && !l.done())) {
        a.stop(swing_layer);
    }
    left_requests(&p, cfg, a);
    // The rest of the player's states: the navigation and its full-body states, the block (whose
    // in's `DisNotify_BlockParryZone` is the parry window), the transitions' one-shots, the left
    // hand's aims and navigation (`crate::player_anim`).
    crate::player_anim::requests(&p, cfg, a);
    if let Some(seq) = swing_seq {
        state.players[i].sword.swing_seq = seq;
    }
}

/// The paired moves the sweep started this tick (`combat::resolve_sword_hits`,
/// `fatality::try_start`): the master plays from the press's own tick, as the original's
/// assassination and fatality contexts start it from the attack press before the advance (the
/// port's target search needs the other pawns, so it runs after the group): its first step is
/// advanced, its notifies fired and its root motion applied now.
pub fn start_paired(state: &mut WorldState, cfg: &SimConfig, world: &dyn WorldQuery, out: &mut StepOut) {
    for i in 0..state.players.len() {
        let p = state.players[i];
        if p.kind != PawnKind::Human || p.sword.phase != SwordPhase::Assassinating || p.time_scale <= 0.0 {
            continue;
        }
        let paired = token::PAIRED | p.sword.assassination_move as u32;
        let Some((seq, l)) = paired_master(&p, cfg) else { continue };
        let a = state.anim.entry(p.id).or_insert_with(|| PawnAnim::new(p.id));
        if a.with(l, paired).is_some() {
            continue;
        }
        a.play(l, seq, paired, 1.0, false, p.sword.root_yaw);
        a.stop(layer::LEFT_ARM);
        if l == layer::FULL_BODY {
            a.stop(layer::UPPER_BODY);
        }
        advance_pawn(state, i, &[l], DT * p.time_scale, cfg, world, out);
        crate::sync_motion::step_one(state, i, world, cfg);
    }
}

/// The requests made after the pawn group (they play from the next tick's advance): the end of a
/// paired move; a throw's sequence (its first update runs in the hit's tick, `play_throw`); an
/// NPC's death (`StateNPCMasterDead_Limp`, pending until its next machine tick: `Death` by the
/// killer's direction, 0x00BBA970's eight ways as the runtime's `eight_way`, U-143;
/// `DeathSwarmAttack` for the rats) from this tick's deaths. A paired move's victim plays its
/// slave as its death (not ported: nothing here).
pub fn after_group(state: &mut WorldState, cfg: &SimConfig, events: &[SimEvent], world: &dyn WorldQuery, out: &mut StepOut) {
    for i in 0..state.players.len() {
        let p = state.players[i];
        if p.kind == PawnKind::Rat {
            continue;
        }
        let a = state.anim.entry(p.id).or_insert_with(|| PawnAnim::new(p.id));
        if p.sword.phase != SwordPhase::Assassinating {
            for l in [layer::FULL_BODY, layer::UPPER_BODY] {
                if a.layers[l].as_ref().is_some_and(|s| s.token & token::KIND == token::PAIRED) {
                    a.stop(l);
                }
            }
        }
        if p.thrown.active && !p.thrown.by_notify {
            play_throw(state, p.id, p.thrown.direction as usize, p.thrown.time, cfg, world, out);
        }
    }
    let slaves: Vec<PlayerId> = state.players.iter().filter(|p| p.sword.phase == SwordPhase::Assassinating).filter_map(|p| p.sword.assassination_target).collect();
    // The paired moves' victims play their slaves (`StateNPCMasterBeingAssassinated`): the
    // assassinations' `Assassination` step by direction (front, left, right, back), the drop
    // assassinations' `AssassinationDrop`, the synced short fatalities' forehand / backhand
    // group, a finisher's state's slave chain and variant (`SimEvent::Fatality`).
    let masters: Vec<(PlayerId, u8)> = state.players.iter().filter(|p| p.sword.phase == SwordPhase::Assassinating).filter_map(|p| p.sword.assassination_target.map(|v| (v, p.sword.assassination_move))).collect();
    for (victim, m) in masters {
        let slave = match m {
            0..4 => Some((NpcAnim::Assassination, -1, m)),
            4..8 => Some((NpcAnim::AssassinationDrop, -1, m - 4)),
            8..10 => Some((NpcAnim::ShortFatalityForehand, (m - 8) as i8, 0)),
            10..12 => Some((NpcAnim::ShortFatalityBackhand, (m - 10) as i8, 0)),
            _ => None,
        };
        if let Some((anim, group, step)) = slave {
            start_slave(state, victim, anim, group, step, cfg, world, out);
        }
    }
    for e in events {
        if let SimEvent::Fatality { victim, variant, slave, .. } = *e {
            if let Some(anim) = NpcAnim::fatality_slave(slave) {
                start_slave(state, victim, anim, variant as i8, 0, cfg, world, out);
            }
        }
    }
    for e in events {
        let SimEvent::Death { victim, killer, cause } = *e else { continue };
        let Some(vi) = state.players.iter().position(|p| p.id == victim) else { continue };
        let v = state.players[vi];
        if matches!(v.kind, PawnKind::Human | PawnKind::Rat) || slaves.contains(&victim) {
            continue;
        }
        let from = killer.and_then(|k| state.player(k)).map(|k| k.position);
        let angle = from.map_or(0.0, |f| {
            let d = (f - v.position).truncate();
            let fwd = crate::movement::forward_vector(v.yaw).truncate();
            let right = crate::movement::right_vector(v.yaw).truncate();
            d.dot(right).atan2(d.dot(fwd)).to_degrees()
        });
        state.anim.entry(victim).or_insert_with(|| PawnAnim::new(victim));
        // The death chains named after their damage (`DisNPCAnim_DeathSwarmAttack`,
        // `DisNPCAnim_DeathPlague`: by name, LIKELY, U-890), else the directional `Death`.
        let (chain, step) = match cause {
            crate::events::DamageCause::Rats | crate::events::DamageCause::RatBite => (NpcAnim::DeathSwarmAttack, 0),
            crate::events::DamageCause::Plague if cfg.gameplay.npc_anim.defs.chain(NpcAnim::DeathPlague).is_some() => (NpcAnim::DeathPlague, 0),
            _ => (NpcAnim::Death, eight_way(angle) as u8),
        };
        crate::pawn_states::request_death(state, victim, chain, step, cfg);
    }
    hit_reactions(state, cfg, events);
    // The human players' third-person bodies.
    crate::player_anim::body_requests(state, cfg, events);
}

/// The NPCs' reactions to this tick's damage (the damage path 0x00B5CCD0 -> 0x00B57E60 /
/// 0x00B57F00: the damage type's `m_ExpectedHitReaction`, else Weak, Strong when the pawn's
/// `m_bDisableWeakHitReaction`, false for every cooked combat tweak; dispatched at 0x00B5D209),
/// pending until the pawn's next machine tick:
/// - Weak (the player's sword, `DisDamageType_FastHit_*`; the Windblast's): slot 421 (0x00B733F0)
///   plays `HitReactSmall_Additive` on the anim player's `AdditiveReactSlot` (no pawn state), the
///   step the hit's eight-way direction (0x00BBE900), cut in and out;
/// - Projectile (the crossbow's `DisDamageType_Arrow`): slot 427 (0x00B73F60): a hit from the front
///   (or on a leg) plays `StateNPCMasterProjectileHitReact` with the hit region (head for a
///   headshot, else the torso: the hit bone is not in the event), else StrongHitReact;
/// - Strong (`DisDamageType_Slide`): slot 422 (0x00B73630 -> 0x00A78920): StrongHitReact, the step
///   `m_Direction` (0x00A6D5F0's cardinal sectors);
/// - Knockdown (the player's bullet, `Twk_Proj_Bullet.m_pDamageType` =
///   `DishonoredDamageType_Bullet`; `DisDamageType_Grenade`): slot 423 (0x00B736E0) requests
///   `StateNPCMasterThrown` (`crate::pawn_states::request_knockdown`) with the hit's location and
///   momentum. The bullet's location is its impact on the pawn and its momentum `m_fMass x
///   m_fSpeed x m_fHitMomentumFactor` along its flight (LIKELY); an explosion's (the damage of
///   `gadgets::explode_with`) the pawn's own location and the explosion's radial momentum (LIKELY:
///   the momentum the port gives players, `m_fRadialMomentum x k` out from the blast plus
///   `m_fExtraUpForce x k`). The sticky grenade's types (`DisDamageType_StickyGrenade`,
///   `_StickyGrenadeExplosion`) set no reaction: Weak.
fn hit_reactions(state: &mut WorldState, cfg: &SimConfig, events: &[SimEvent]) {
    use crate::events::DamageCause as C;
    use crate::pawn_states::{Master, StateRequest, context};
    for e in events {
        let SimEvent::Damage { victim, attacker, cause, .. } = *e else { continue };
        let Some(v) = state.player(victim).copied() else { continue };
        if !crate::pawn_states::has_states(&v) || !v.alive || v.incapacitated() || v.thrown.active || v.thrown.ragdoll {
            continue;
        }
        let from = attacker.and_then(|k| state.player(k)).map(|k| k.position);
        let local = from.map_or(glam::Vec2::new(1.0, 0.0), |f| {
            let d = (f - v.position).truncate();
            glam::Vec2::new(d.dot(crate::movement::forward_vector(v.yaw).truncate()), d.dot(crate::movement::right_vector(v.yaw).truncate()))
        });
        let angle = local.y.atan2(local.x).to_degrees();
        match cause {
            C::Sword | C::SwordCounter | C::Windblast | C::StickyGrenade => {
                let Some(a) = state.anim.get_mut(&victim) else { continue };
                let r = a.roll();
                if let Some(seq) = chain_step(cfg, NpcAnim::HitReactSmallAdditive, 0, eight_way(angle), r, v.kind == PawnKind::Guard) {
                    let tok = a.next_shot();
                    a.play(layer::ADDITIVE, seq, tok, 1.0, false, v.yaw);
                    if let Some(l) = a.layers[layer::ADDITIVE].as_mut() {
                        l.blend_in = 0.0;
                        l.blend_out = 0.0;
                    }
                }
            }
            C::Crossbow | C::CrossbowHeadshot => {
                let region = if cause == C::CrossbowHeadshot { 1 } else { 2 };
                let req = if local.x > 0.0 {
                    StateRequest::new(Master::ProjectileHitReact, NpcAnim::HitReactProjectile as u8, 0, region, context::NONE)
                } else {
                    StateRequest::new(Master::StrongHitReact, NpcAnim::HitReactStrong as u8, -1, crate::pawn_states::cardinal_step(local.x, local.y), context::NONE)
                };
                crate::pawn_states::try_request(state, victim, req, cfg);
            }
            C::Slide => {
                let req = StateRequest::new(Master::StrongHitReact, NpcAnim::HitReactStrong as u8, -1, crate::pawn_states::cardinal_step(local.x, local.y), context::NONE);
                crate::pawn_states::try_request(state, victim, req, cfg);
            }
            C::Pistol | C::PistolHeadshot => {
                // The bullet's impact on this pawn and its flight: the spawn this tick
                // (`SimEvent::Fired`, a bullet at `m_fSpeed` 100000 hits within its launch tick
                // inside 1667 uu), else from the shooter's eye.
                let impact = events.iter().rev().find_map(|e| match *e {
                    SimEvent::ProjectileImpact { projectile, point, victim: Some(v) } if v == victim => Some((projectile, point)),
                    _ => None,
                });
                let Some((projectile, point)) = impact else { continue };
                let fired = events.iter().find_map(|e| match *e {
                    SimEvent::Fired { projectile: p, velocity, .. } if p == projectile => Some(velocity),
                    _ => None,
                });
                let eye = attacker.and_then(|k| state.player(k)).map(|k| crate::movement::eye_position(k, &cfg.gameplay.movement));
                let dir = fired.map(|v| v.normalize_or_zero()).or_else(|| eye.map(|e| (point - e).normalize_or_zero())).unwrap_or(Vec3::ZERO);
                let t = &cfg.tuning;
                let momentum = dir * t.bullet_mass * t.bullet_speed * t.bullet_hit_momentum;
                let source = from.unwrap_or(point);
                crate::pawn_states::request_knockdown(state, victim, point, momentum, source, cfg);
            }
            C::Grenade => {
                let Some(blast) = events.iter().filter_map(|e| match *e {
                    SimEvent::Explosion { position, sticky: false, .. } => Some(position),
                    _ => None,
                }).min_by(|a, b| a.distance_squared(v.position).total_cmp(&b.distance_squared(v.position))) else { continue };
                let e = &crate::gadgets::grenade_cfg(&cfg.gameplay, crate::state::ProjectileKind::Grenade).explosion;
                let d = v.position.distance(blast);
                let k = (1.0 - d / e.damage_radius.max(1.0)).clamp(0.0, 1.0);
                let momentum = (v.position - blast).normalize_or_zero() * e.radial_momentum * k + Vec3::Z * e.extra_up_force * k;
                crate::pawn_states::request_knockdown(state, victim, v.position, momentum, blast, cfg);
            }
            _ => {}
        }
    }
}

/// A paired move's slave entered on its victim (`crate::pawn_states::start_slave`), its first
/// step advanced in this tick with its master's.
#[allow(clippy::too_many_arguments)]
fn start_slave(state: &mut WorldState, victim: PlayerId, anim: NpcAnim, group: i8, step: u8, cfg: &SimConfig, world: &dyn WorldQuery, out: &mut StepOut) {
    let mut ao = StepOut::default();
    if crate::pawn_states::start_slave(state, victim, anim, group, step, cfg, &mut ao) {
        if let Some(vi) = state.players.iter().position(|p| p.id == victim) {
            let ts = state.players[vi].time_scale;
            advance_pawn(state, vi, &[layer::FULL_BODY], DT * ts, cfg, world, out);
        }
    }
    out.events.extend(ao.events);
}

/// `eDis8WayDirection` / `eDisDeathDirection` of an angle from the pawn's facing (degrees, +90
/// right): 45-degree sectors (Forward, Forward_Left, Forward_Right, Left, Right, Back, Back_Left,
/// Back_Right), as the runtime's `npc_anim::eight_way` (sector thresholds LIKELY, U-143).
pub fn eight_way(angle: f32) -> usize {
    let a = angle.abs();
    let left = angle < 0.0;
    if a <= 22.5 {
        0
    } else if a <= 67.5 {
        if left { 1 } else { 2 }
    } else if a <= 112.5 {
        if left { 3 } else { 4 }
    } else if a <= 157.5 {
        if left { 6 } else { 7 }
    } else {
        5
    }
}

/// A thrown NPC's Windblast throw (`PushToRagdoll` chain, group 1, step `m_ThrowDirection`:
/// `Generic_WindBlast_{Front,Left,Right,Back}`), played from the hit with its first update of
/// `first_dt` in the hit's tick (`movement::start_throw`); its `DishonoredNotify_Ragdoll` ends the
/// flight.
pub fn play_throw(state: &mut WorldState, id: PlayerId, direction: usize, first_dt: f32, cfg: &SimConfig, world: &dyn WorldQuery, out: &mut StepOut) {
    let Some(i) = state.players.iter().position(|p| p.id == id) else { return };
    let yaw = state.players[i].yaw;
    crate::pawn_states::enter_thrown(state, id);
    let a = state.anim.entry(id).or_insert_with(|| PawnAnim::new(id));
    let r = a.roll();
    let Some(seq) = chain_step(cfg, NpcAnim::PushToRagdoll, 1, direction, r, false) else { return };
    a.play(layer::FULL_BODY, seq, token::THROWN, 1.0, false, yaw);
    // The step's blend-in and the group's blend-out (0: the throw plays at full weight to its
    // ragdoll notify and on).
    let grp = cfg.gameplay.npc_anim.defs.chain(NpcAnim::PushToRagdoll).and_then(|c| c.groups.get(1));
    if let (Some(l), Some(g)) = (a.layers[layer::FULL_BODY].as_mut(), grp) {
        l.blend_in = g.steps.get(direction).or(g.steps.first()).map_or(0.2, |s| s.blend_in);
        l.blend_out = g.blend_out;
    }
    state.players[i].thrown.by_notify = true;
    advance_pawn(state, i, &[layer::FULL_BODY], first_dt, cfg, world, out);
}

// ---- the advance ----

/// What the advance hands back to the step (the world effects of the notifies' handlers).
#[derive(Default)]
pub struct StepOut {
    pub events: Vec<SimEvent>,
    pub actions: Vec<(usize, LeftAction)>,
    pub spawns: Vec<(PlayerId, ProjectileKind, Vec3, Vec3)>,
}

/// One crossing of a notify.
#[derive(Clone, Copy, Debug)]
enum Phase {
    Start,
    Tick { before: f32, after: f32 },
    End,
}

#[derive(Clone, Copy, Debug)]
struct Fired {
    seq: SeqId,
    token: u32,
    notify: u16,
    phase: Phase,
    t0: f32,
}

/// Advance one layer by `dt` of play time: the active windows tick (`0x00A9BC90`), then the
/// notifies in `[t0, t0 + move)` fire (looping sequences wrap), then the time moves (a
/// non-looping sequence stops at its end). Flags are raised here; the crossings are returned.
fn advance_layer(s: &mut SeqPlay, info: &SequenceInfo, dt: f32, fired: &mut Vec<Fired>) {
    if !s.playing {
        return;
    }
    let mv = info.rate_scale_or_one() * s.rate * dt;
    if mv <= 0.0 {
        return;
    }
    s.elapsed += dt;
    let t0 = s.time;
    let len = info.duration;
    let base = Fired { seq: s.seq, token: s.token, notify: 0, phase: Phase::Start, t0 };
    // The windows opened by earlier steps.
    let mut k = 0;
    while k < s.windows.len() {
        let w = s.windows[k];
        let d = info.notifies.get(w.notify as usize).map_or(0.0, |n| n.duration);
        let remaining = w.remaining - mv;
        fired.push(Fired { notify: w.notify, phase: Phase::Tick { before: d - w.remaining, after: d - remaining }, ..base });
        if remaining <= 0.0 {
            fired.push(Fired { notify: w.notify, phase: Phase::End, ..base });
            s.windows.remove(k);
        } else {
            s.windows[k].remaining = remaining;
            k += 1;
        }
    }
    // The notifies this step crosses, in order (a looping step past the end wraps).
    let mut cross = |from: f32, to: f32, offset: f32, s: &mut SeqPlay| {
        for (ni, n) in info.notifies.iter().enumerate() {
            if n.time < from || n.time >= to {
                continue;
            }
            let notify = ni as u16;
            match n.class.as_str() {
                "DishonoredNotify_AnimStateExit" => s.exited = true,
                "DishonoredNotify_AttackInterruptable" => s.interruptible = true,
                "DishonoredNotify_AnimStateAdvance" => s.advanced = true,
                _ => {}
            }
            fired.push(Fired { notify, phase: Phase::Start, ..base });
            if n.duration > 0.0 {
                let elapsed = (to - n.time) + offset;
                if elapsed >= n.duration {
                    fired.push(Fired { notify, phase: Phase::Tick { before: 0.0, after: n.duration }, ..base });
                    fired.push(Fired { notify, phase: Phase::End, ..base });
                } else {
                    fired.push(Fired { notify, phase: Phase::Tick { before: 0.0, after: elapsed }, ..base });
                    s.windows.push(Window { notify, remaining: n.duration - elapsed });
                }
            }
        }
    };
    let t1 = t0 + mv;
    if s.looping && len > 0.0 && t1 >= len {
        cross(t0, len, t1 - len, s);
        let mut start = 0.0;
        let mut end = t1 - len;
        while end >= len {
            cross(start, len, end - len, s);
            end -= len;
            start = 0.0;
        }
        cross(start, end, 0.0, s);
        s.time = end;
    } else {
        cross(t0, t1, 0.0, s);
        if !s.looping && t1 >= len {
            s.time = len;
            s.playing = false;
        } else {
            s.time = t1;
        }
    }
    s.prev = t0;
}

/// Advance a sequence player by `dt` without firing its notifies (its flags are raised).
pub fn advance_quiet(s: &mut SeqPlay, info: &SequenceInfo, dt: f32) {
    let mut none = Vec::new();
    advance_layer(s, info, dt, &mut none);
}

/// Advance the given layers of pawn `i` by `dt` and run the handlers of what they crossed.
fn advance_pawn(state: &mut WorldState, i: usize, layers: &[usize], dt: f32, cfg: &SimConfig, world: &dyn WorldQuery, out: &mut StepOut) {
    let id = state.players[i].id;
    let mut fired = Vec::new();
    if let Some(a) = state.anim.get_mut(&id) {
        for &l in layers {
            let Some(s) = a.layers[l].as_mut() else { continue };
            // A layer the state let go of is removed once blended out.
            if s.out.is_some_and(|o| s.elapsed - o >= s.blend) {
                a.layers[l] = None;
                continue;
            }
            let Some(info) = cfg.anim.info(s.seq) else {
                // The reference pose: only its play time moves (the blend's clock).
                if s.seq == REF_POSE {
                    s.elapsed += dt;
                }
                continue;
            };
            let was_playing = s.playing;
            let mut quiet = Vec::new();
            advance_layer(s, info, dt, if s.quiet { &mut quiet } else { &mut fired });
            // A sequence that had ended keeps its release's clock running.
            if !was_playing && s.out.is_some() {
                s.elapsed += dt;
            }
        }
        crate::player_anim::sync_groups(a, &cfg.anim);
    }
    for f in fired {
        if matches!(f.phase, Phase::Start) {
            out.events.push(SimEvent::AnimNotify { pawn: id, sequence: f.seq, notify: f.notify, t0: f.t0 });
        }
        let Some(n) = cfg.anim.info(f.seq).and_then(|s| s.notifies.get(f.notify as usize)) else { continue };
        handle(state, i, &f, n, cfg, world, out);
    }
}

/// The animation advance of every pawn (after the pawn group's physics and states): each pawn's
/// layers by `DT x` its time scale, the notifies' handlers, then its NPC tree.
pub fn step(state: &mut WorldState, cfg: &SimConfig, world: &dyn WorldQuery, out: &mut StepOut) {
    let ids: Vec<PlayerId> = state.players.iter().map(|p| p.id).collect();
    state.anim.retain(|id, _| ids.contains(id));
    for i in 0..state.players.len() {
        let p = state.players[i];
        if p.kind == PawnKind::Rat || p.time_scale <= 0.0 {
            continue;
        }
        state.anim.entry(p.id).or_insert_with(|| PawnAnim::new(p.id));
        let dt = DT * p.time_scale;
        advance_pawn(state, i, &[layer::FULL_BODY, layer::UPPER_BODY, layer::LEFT_ARM, layer::WEAPON, layer::ADDITIVE], dt, cfg, world, out);
        if p.kind == PawnKind::Human {
            // The player's third-person body (`crate::player_anim::Body`) on its own tree.
            let p = state.players[i];
            if let Some(a) = state.anim.get_mut(&p.id) {
                crate::player_anim::body_advance(a, &p, cfg, dt);
            }
        }
        step_tree(state, i, dt, cfg);
    }
}

// ---- handlers ----

/// The handler of a notify crossing, by the notify's class (the vtables of tick_order.md
/// section 4), for the state that requested the sequence.
fn handle(state: &mut WorldState, i: usize, f: &Fired, n: &NotifyInfo, cfg: &SimConfig, world: &dyn WorldQuery, out: &mut StepOut) {
    let kind = f.token & token::KIND;
    // A dead or held pawn's hand does nothing (`left_hand::update` returns for it), though its
    // sequences play out.
    let p = &state.players[i];
    if matches!(kind, token::LEFT | token::WEAPON) && (!p.alive || p.incapacitated()) {
        return;
    }
    match (n.class.as_str(), f.phase) {
        ("DishonoredNotify_AttackZone", phase) => attack_zone(state, i, f, n, phase, cfg, out),
        ("DisNotify_BendTime", Phase::Start) if f.token == token::LEFT | left_kind::CAST_BEND_TIME => cast(state, i, LeftHand::BendTime, cfg, out),
        ("DisNotify_DevouringSwarm", Phase::Start) if f.token == token::LEFT | left_kind::CAST_SWARM => cast(state, i, LeftHand::DevouringSwarm, cfg, out),
        // The possession cast's notify (`Powers_Cast_Possession_Out`): the power takes its target.
        ("DisNotify_Possession", Phase::Start) if f.token == token::LEFT | left_kind::CAST_POSSESSION => {
            let p = &state.players[i];
            if p.alive && p.left.item == LeftHand::Possession && p.left.phase == LeftPhase::Recovering {
                out.actions.push((i, LeftAction::PossessRelease));
            }
        }
        ("DishonoredNotify_FireProjectile", Phase::Start) if f.token == token::WEAPON => crossbow_shot(state, i, cfg, world, out),
        ("DishonoredNotify_FireProjectile", Phase::Start) if f.token == token::LEFT | left_kind::GRENADE => {
            let p = &mut state.players[i];
            crate::left_hand::throw_grenade(p, &cfg.gameplay, &mut out.spawns);
        }
        ("DisNotify_PlaceSpringRazor", Phase::Start) if f.token == token::LEFT | left_kind::SPRING_RAZOR => {
            let p = &mut state.players[i];
            if p.left.item == LeftHand::SpringRazor && crate::inventory::remove_ammo(p, dishonored_ir::items::AmmoType::SpringRazor, 1) {
                out.actions.push((i, LeftAction::PlaceTrap));
            }
        }
        // The throw's ragdoll notify (0x00B6FBB0, event 5) ends the flight (0x00A7CBA0).
        ("DishonoredNotify_Ragdoll", Phase::Start) if kind == token::THROWN => {
            let p = &mut state.players[i];
            if p.thrown.active {
                p.thrown.active = false;
                p.thrown.ragdoll = true;
            }
        }
        // A death's ragdoll notify (0x00A8CFE0 -> pawn slot 352): the body goes to the ragdoll; its
        // chain's root motion stops.
        ("DishonoredNotify_Ragdoll", Phase::Start) if kind == token::DEATH => {
            let id = state.players[i].id;
            crate::pawn_states::ragdolled(state, id);
        }
        ("DisNotify_BendTime_Ranged" | "DisNotify_AdrenalineBendTime_Ranged", Phase::Start | Phase::End) if kind == token::PAIRED => {
            crate::fatality::bend_window(state, i, n, matches!(f.phase, Phase::Start));
        }
        _ => {}
    }
}

/// `DishonoredNotify_AttackZone`'s tick handler (0x00A8E760): the damage zone opens on the step
/// whose window time `[before, after)` holds `Duration - m_fDamageZoneMaxTime` (clamped at 0),
/// which enters the melee context's damage zone (slot 115, 0x00C13140: `m_bDamageIsPosssible`,
/// `m_AlreadyHitActors` cleared): the swing goes Active. The window's end (0x00A8E710) ends the
/// zone: Recovery. A generic finisher's swing kills its victim when its damage zone opens.
fn attack_zone(state: &mut WorldState, i: usize, f: &Fired, n: &NotifyInfo, phase: Phase, cfg: &SimConfig, out: &mut StepOut) {
    let kind = f.token & token::KIND;
    let swing = kind == token::SWING && f.token == token::SWING | state.players[i].sword.swings as u32;
    match phase {
        Phase::Tick { before, after } => {
            let x = (n.duration - n.payload.get("m_fDamageZoneMaxTime").copied().unwrap_or(0.0)).max(0.0);
            if !(before <= x && x < after) {
                return;
            }
            let p = &mut state.players[i];
            if swing && p.sword.phase == SwordPhase::Startup {
                p.sword.phase = SwordPhase::Active;
                p.sword.timer = 0.0;
                p.sword.hit_count = 0;
            } else if kind == token::PAIRED {
                crate::fatality::generic_hit(state, i, &cfg.gameplay, &mut out.events);
            }
        }
        Phase::End if swing && state.players[i].sword.phase == SwordPhase::Active => {
            let p = &mut state.players[i];
            p.sword.phase = SwordPhase::Recovery;
            p.sword.timer = 0.0;
        }
        _ => {}
    }
}

/// A cast's notify (`DisNotify_BendTime`, `DisNotify_DevouringSwarm`): the power is used and its
/// cost taken (MEASURED 2026-10-08: Bend Time's mana 9 ticks after the press).
fn cast(state: &mut WorldState, i: usize, item: LeftHand, cfg: &SimConfig, out: &mut StepOut) {
    let p = &mut state.players[i];
    if p.left.item != item || p.left.phase != LeftPhase::Recovering {
        return;
    }
    let swarm = item == LeftHand::DevouringSwarm;
    let cost = if swarm { cfg.gameplay.items.swarm.mana_cost } else { cfg.gameplay.items.bend_time.mana_cost };
    if crate::vitals::spend_mana(p, cost, &cfg.gameplay.vitals) {
        out.actions.push((i, if swarm { LeftAction::Swarm } else { LeftAction::BendTime }));
    }
}

/// The crossbow's `DishonoredNotify_FireProjectile` (0x00A96890 -> the item context's fire,
/// 0x00C38230): the bolt (or the loaded dart / incendiary bolt) leaves.
fn crossbow_shot(state: &mut WorldState, i: usize, cfg: &SimConfig, world: &dyn WorldQuery, out: &mut StepOut) {
    let p = &mut state.players[i];
    if !p.left.item.is_crossbow() || p.left.phase != LeftPhase::Recovering {
        return;
    }
    let mvc = crate::powers::effective_movement(p, &cfg.gameplay);
    let ctx = crate::movement::MoveCtx { world, cfg: &mvc, dt: DT * p.time_scale };
    let mut local = Vec::new();
    crate::left_hand::crossbow_shot(p, &ctx, &cfg.gameplay, &cfg.tuning, &mut local, &mut out.events);
    out.spawns.extend(local.into_iter().map(|(k, pos, vel)| (p.id, k, pos, vel)));
}

// ---- root motion ----

/// The root motion of a paired move's master this tick (`RMM_Translate` masters with
/// `RMRM_RotateActor`: the paired moves' states, `DisTweaks_PlayerPawn_FullBodyAnimation`), from
/// the config's root-motion keys (sampled in clip time) between the step's start and end, in the
/// frame of the pawn's yaw at the request; `None` unless the full-body layer plays the master of
/// the pawn's current paired move.
pub fn root_delta(a: &PawnAnim, p: &PlayerState, cfg: &SimConfig) -> Option<Vec3> {
    if p.sword.phase != SwordPhase::Assassinating {
        return None;
    }
    let s = a.with(layer::FULL_BODY, token::PAIRED | p.sword.assassination_move as u32)?;
    if s.time <= s.prev {
        return None;
    }
    let info = cfg.anim.info(s.seq)?;
    let rm = cfg.gameplay.root_motion.get(info.root_motion.as_ref()?)?;
    let (a0, b0) = (rm.at(s.prev), rm.at(s.time));
    Some(crate::movement::forward_vector(s.yaw) * (b0[0] - a0[0]) + crate::movement::right_vector(s.yaw) * (b0[1] - a0[1]))
}

// ---- NPC trees ----

/// The `EPhysics` the tree's `AnimNodeBlendByPhysics` reads for a pawn (1 walking, 2 falling, 3
/// swimming, 4 flying for a throw's flight, 9 ladder, 10 rigid body for a ragdoll).
fn physics(p: &PlayerState) -> u8 {
    use crate::state::MoveMode;
    if p.thrown.active {
        return 4;
    }
    if p.thrown.ragdoll || !p.alive {
        return 10;
    }
    match p.mode {
        MoveMode::Walking | MoveMode::Mantling | MoveMode::Blinking => 1,
        MoveMode::Falling | MoveMode::Dead => 2,
        MoveMode::Swimming => 3,
        MoveMode::Climbing => 9,
    }
}

/// The locomotion modifier of an NPC from its brain's awareness (the AI head's `head_modifier`
/// rule: the elite guard's sword stances, the civilians' empty ones; U-892).
fn modifier(p: &PlayerState) -> &'static str {
    use crate::state::awareness as aw;
    // A human player's body: `Sword` armed (`Aim` while aiming a gun), `Empty` unarmed (the
    // former third-person picker's rule through the NPC profile's config).
    if p.kind == PawnKind::Human {
        let gun_aim = p.left.phase == LeftPhase::Aiming && matches!(p.left.item, LeftHand::Pistol | LeftHand::Crossbow);
        return if !crate::player_anim::body_armed(p) { "Empty" } else if gun_aim { "Aim" } else { "Sword" };
    }
    let guard = p.kind == PawnKind::Guard;
    match p.ai_awareness {
        aw::IN_COMBAT => if guard { "Sword" } else { "Empty" },
        aw::FEARFUL | aw::BEGGING => if guard { "PanicSword" } else { "PanicEmpty" },
        aw::SURPRISED | aw::SUSPICIOUS => if guard { "SearchSword" } else { "SearchEmpty" },
        _ => if guard { "SwordPatrol" } else { "Empty" },
    }
}

/// Tick an NPC's tree (with the install): the slot requests of its layers' new sequences
/// (`FullBodySlot` / `UpperBodySlot`, blended in and out over the chain step's and group's times),
/// the owner's physics, the locomotion from its velocity relative to its facing and its stance
/// modifier. The slot requests land before the advance, so a slot plays its layer's sequence at
/// the layer's time.
fn step_tree(state: &mut WorldState, i: usize, dt: f32, cfg: &SimConfig) {
    const FACE_FILTER_BLEND: f32 = 0.1;
    let p = state.players[i];
    let dead = !p.alive;
    let Some(assets) = cfg.anim_source.npc_tree(dead) else { return };
    // The locomotion component's start, stop or turn sequence in the node's "Specific anim" child.
    let specific = crate::npc_loco::tree_request(state, p.id, cfg);
    let state_chain = state.pawn_states.get(&p.id).and_then(|s| s.chain);
    let Some(a) = state.anim.get_mut(&p.id) else { return };
    let want = dead as u8;
    if a.tree.as_ref().is_none_or(|t| t.tree != want) {
        let mut st = assets.shape.initial_state();
        if want == 0 {
            // The look-at component's blinks on `NPC_Human_at`'s `BlinkBlender` (0x00952EC0 with
            // the config's class defaults: every 0.5-6 s at a rate of 1-2), rolled per pawn.
            st.set_blink_timer(Some(dishonored_anim::lookat::BlinkTimer::new(BLINK_INTERVAL, BLINK_RATE, p.id)));
        }
        a.tree = Some(NpcTree { tree: want, state: st, requested: [0; layer::COUNT], root: (Vec3::ZERO, 0.0), blend_out: [0.0; layer::COUNT] });
    }
    let mut slots = Vec::new();
    let mut filters = Vec::new();
    let human = p.kind == PawnKind::Human;
    let chain_face = |l: usize| -> Option<&str> {
        // The chain step's `m_FaceAnimName` (`crate::pawn_states`' full-body record; a human
        // player's body chain).
        let (anim, group, step) = if human {
            let c = a.body.as_ref()?.chain?;
            (c.anim, c.group, c.step)
        } else if l == layer::FULL_BODY {
            let c = state_chain?;
            (c.anim, c.group, c.step)
        } else {
            return None;
        };
        let f = cfg.gameplay.npc_anim.defs.chains.get(anim as usize)?.groups.get(group as usize)?.steps.get(step as usize)?.face.as_str();
        (!f.is_empty() && f != "None").then_some(f)
    };
    let tree = a.tree.as_mut().unwrap();
    for (l, slot) in [(layer::FULL_BODY, "FullBodySlot"), (layer::UPPER_BODY, "UpperBodySlot"), (layer::ADDITIVE, "AdditiveReactSlot")] {
        // An NPC's layers; a human player's body chain (its layers are its first-person rig's).
        let src = if human { a.body.as_ref().and_then(|b| b.layer.as_ref()).filter(|_| l == layer::FULL_BODY) } else { a.layers[l].as_ref() };
        let cur = src.map_or(0, |s| s.token);
        if cur == tree.requested[l] {
            continue;
        }
        tree.requested[l] = cur;
        // The anim player's `PlayCustomAnim` (0x00C52E00 -> 0x005B7860): the chain step's
        // `m_fBlendInTime`, blend-out 0 (`PendingBlendOutTime` -1 for a looping step: held), from
        // where the layer's step started; its stop (0x00C48CF0) blends out over the group's
        // `m_fBlendOutTime`. A death or a throw thus plays at full weight past its ragdoll notify.
        let action = match src.and_then(|s| cfg.anim.name(s.seq).map(|n| (n, s))) {
            Some((name, s)) => {
                tree.blend_out[l] = s.blend_out.max(0.0);
                let blend_out = if s.looping || s.blend_out < 0.0 { None } else { Some(0.0) };
                let start = s.prev;
                // Every step start on a slot also plays its face on `FaceSlot` (the step's
                // `m_FaceAnimName`, else `<sequence>_Face`) at the body's rate and blend-in, and
                // sets `FaceFilter` to 1 when one played, else 0, over 0.1 s (0x00C52E00: U-974).
                let face = chain_face(l).map(str::to_string).unwrap_or_else(|| format!("{name}_Face"));
                let has_face = assets.clips.find(&face).is_some();
                if has_face {
                    slots.push(SlotRequest { slot: "FaceSlot".to_string(), action: SlotAction::Play { sequence: face, rate: s.rate, blend_in: s.blend_in.max(0.0), blend_out: if s.looping { None } else { Some(0.0) }, looping: s.looping, start } });
                }
                filters.push(FilterTarget { node: "FaceFilter".to_string(), target: if has_face { 1.0 } else { 0.0 }, time: FACE_FILTER_BLEND });
                SlotAction::Play { sequence: name.to_string(), rate: s.rate, blend_in: s.blend_in.max(0.0), blend_out, looping: s.looping, start }
            }
            None => SlotAction::Stop { blend_out: tree.blend_out[l] },
        };
        slots.push(SlotRequest { slot: slot.to_string(), action });
    }
    let fwd = crate::movement::forward_vector(p.yaw).truncate();
    let right = crate::movement::right_vector(p.yaw).truncate();
    let v = p.velocity.truncate();
    let velocity = (v.dot(right).atan2(v.dot(fwd)).to_degrees(), v.length());
    let m = modifier(&p);
    // The profile's config may lack the modifier a human player's body asks for (`Aim`).
    let m = if assets.loco.modifier(m).is_none() && human { "Sword" } else { m };
    let locomotion = (!dead).then(|| LocoInput { config: &assets.loco, modifier: m, velocity, specific });
    // The states' slot requests land before the tree's advance (tick_order.md section 6: the
    // state machine ticks inside the pawn's `Tick`, its skeletal component after it, so a slot
    // played in the tick is advanced in that tick's `TickAnimNodes`; U-975): the slot's channel
    // reads the layer's time.
    tree.state.request(&assets.shape, &TreeInputs { slots: &slots, filters: &filters, ..Default::default() });
    let inputs = TreeInputs { physics: Some(physics(&p)), locomotion, ..Default::default() };
    tree.state.update(&assets.shape, assets.clips.as_ref(), &inputs, dt);
    tree.root = tree.state.root_delta();
}

/// `ArkComponentLookatConfig`'s class defaults `MinDurationBetweenBlink` /
/// `MaxDurationBetweenBlink` (s) and `MinBlinkSpeed` / `MaxBlinkSpeed` (ANIMATION.md section 9: no
/// Boyle config overrides them).
const BLINK_INTERVAL: [f32; 2] = [0.5, 6.0];
const BLINK_RATE: [f32; 2] = [1.0, 2.0];

/// The derived timing of a sequence the rules read when no layer plays it (a config value read
/// from the table): its `AnimStateExit` play time, else its length.
pub fn exit_time(g: &GameplayConfig, set: &str, sequence: &str) -> f32 {
    info(g, set, sequence).exit_time()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::gameplay::NotifyInfo;

    fn seq(duration: f32, notifies: Vec<(f32, f32, &str)>) -> SequenceInfo {
        SequenceInfo { duration, rate_scale: 1.0, notifies: notifies.into_iter().map(|(time, d, class)| NotifyInfo { class: class.into(), time, duration: d, ..Default::default() }).collect(), root_motion: None }
    }

    fn run(info: &SequenceInfo, looping: bool, steps: usize, dt: f32) -> Vec<(usize, Fired)> {
        let mut s = SeqPlay { seq: 0, token: 0, time: 0.0, prev: 0.0, rate: 1.0, looping, playing: true, yaw: 0.0, exited: false, interruptible: false, advanced: false, windows: Vec::new(), elapsed: 0.0, blend_in: 0.0, blend_out: 0.0, blend: 0.0, out: None, quiet: false };
        let mut out = Vec::new();
        for k in 0..steps {
            let mut f = Vec::new();
            advance_layer(&mut s, info, dt, &mut f);
            out.extend(f.into_iter().map(|f| (k, f)));
        }
        out
    }

    /// `[t0, t1)`: a notify at the step's start fires, one at its end on the next step.
    #[test]
    fn half_open_window() {
        let info = seq(1.0, vec![(0.0, 0.0, "A"), (0.25, 0.0, "B"), (0.5, 0.0, "C")]);
        let f = run(&info, false, 3, 0.25);
        let starts: Vec<(usize, u16)> = f.iter().filter(|(_, x)| matches!(x.phase, Phase::Start)).map(|(k, x)| (*k, x.notify)).collect();
        assert_eq!(starts, vec![(0, 0), (1, 1), (2, 2)]);
        assert!(f.iter().all(|(k, x)| x.t0 == *k as f32 * 0.25));
    }

    /// A window: start and first tick on the crossing, ticks with (before, after), the end when
    /// its time runs out; the attack zone's damage time falls in exactly one step.
    #[test]
    fn duration_window() {
        // Binary-exact times (the window's `remaining -= MoveDelta` keeps float remainders).
        let info = seq(1.0, vec![(0.125, 0.375, "Z")]);
        let f = run(&info, false, 8, 0.125);
        let ticks: Vec<(usize, f32, f32)> = f.iter().filter_map(|(k, x)| match x.phase {
            Phase::Tick { before, after } => Some((*k, before, after)),
            _ => None,
        }).collect();
        assert_eq!(ticks.len(), 3);
        assert_eq!(ticks[0].0, 1);
        assert_eq!((ticks[0].1, ticks[0].2, ticks[2].2), (0.0, 0.125, 0.375));
        let end = f.iter().find(|(_, x)| matches!(x.phase, Phase::End)).unwrap().0;
        assert_eq!(end, 3);
        let x = 0.3;
        assert_eq!(ticks.iter().filter(|t| t.1 <= x && x < t.2).count(), 1);
    }

    /// A looping sequence wraps: the notify fires once per lap.
    #[test]
    fn looping_wraps() {
        let info = seq(0.5, vec![(0.1, 0.0, "L")]);
        let f = run(&info, true, 10, 0.2);
        assert_eq!(f.iter().filter(|(_, x)| matches!(x.phase, Phase::Start)).count(), 4);
    }

    /// The end of a non-looping sequence stops it; exit and interruptible flags are raised.
    #[test]
    fn flags_and_end() {
        let info = seq(0.3, vec![(0.1, 0.0, "DishonoredNotify_AttackInterruptable"), (0.2, 0.0, "DishonoredNotify_AnimStateExit")]);
        let mut s = SeqPlay { seq: 0, token: 0, time: 0.0, prev: 0.0, rate: 1.0, looping: false, playing: true, yaw: 0.0, exited: false, interruptible: false, advanced: false, windows: Vec::new(), elapsed: 0.0, blend_in: 0.0, blend_out: 0.0, blend: 0.0, out: None, quiet: false };
        let mut f = Vec::new();
        advance_layer(&mut s, &info, 0.15, &mut f);
        assert!(s.interruptible && !s.exited && s.playing);
        advance_layer(&mut s, &info, 0.15, &mut f);
        assert!(s.exited && !s.playing && s.time == 0.3);
        assert!(s.done());
    }

    #[test]
    fn table_ids_follow_key_order() {
        let mut m = BTreeMap::new();
        m.insert("B.y".to_string(), SequenceInfo::default());
        m.insert("A.x".to_string(), SequenceInfo::default());
        m.insert("A_b.z".to_string(), SequenceInfo::default());
        let t = SeqTable::new(&m);
        assert_eq!(t.id("A.x"), Some(0));
        assert_eq!(t.id("A_b.z"), Some(1));
        assert_eq!(t.id_of("B", "y"), Some(2));
        assert_eq!(t.name(2), Some("y"));
        assert_eq!(t.find(&["B".into(), "A".into()], "x"), Some(0));
    }
}
