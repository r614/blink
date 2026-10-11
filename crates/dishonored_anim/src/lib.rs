//! A generic evaluator of the cooked UE3 AnimTree graph (research/ARCHITECTURE.md, step 3 of
//! the migration): one [`AnimTreeInstance`] per skeletal mesh holds the per-node state of a
//! decoded `dishonored_ir::anim_tree::AnimTreeAsset` (sequence times and rates, children
//! weights and blend timers, slot channels, the locomotion node's smoothing, the look-at cells,
//! the blink), advances it by a fixed `dt` from the gameplay layer's [`TreeInputs`], and
//! evaluates the local pose of every bone from the clips a [`Clips`] implementation resolves.
//! Bevy-free and deterministic (no wall clock; nodes, groups and sets are iterated in index or
//! name order), with ARCHITECTURE.md rule 2's shape: state + assets + inputs -> outputs. The
//! instance is a [`TreeShape`] shared through an `Arc` (what the tree, the skeleton and the mesh
//! rotation fix) and a serializable [`TreeState`] (what a tick changes), so the simulation keeps
//! one state per pawn in its snapshots.
//!
//! Node classes and confidence (research/dishonored/ANIMATION.md sections 2, 3, 6, 9;
//! research/dishonored/animation/ANIMTREE_NPC_HUMAN.md), measured against the oracle capture of
//! the Boyle party guard idling in `stealth/detect_1300` (tests/npc_human_replay.rs: the
//! capture's node states replayed reproduce its bones to 0.008 deg mean over the body and 0.014
//! deg over the head and face; the evaluator's own tick from the gameplay requests reproduces
//! its node weights within 0.0003 and times within 0.0007 s):
//! - `AnimNodeSequence`, `DishonoredAnimNodeSeq*`: CONFIRMED (clip time, track binding, every
//!   clip translation used); the root options are stock UE3 (LIKELY: the guard's clips leave the
//!   root bone undriven);
//! - `AnimNodeBlendByPhysics`, `AnimNodeBlendList`, `AnimNodeSlot` / `DisAnimNodeSlot`,
//!   `AnimNodeBlendPerBone` (with Arkane's `m_bExcludeBranchBones`), `AnimNodeAdditiveBlending`:
//!   stock UE3 semantics; the weights, linear blends and masks are CONFIRMED by the capture, the
//!   channel choice, the early blend-out of slots and the physics blend time are LIKELY;
//! - sync groups (`SynchGroupName`, `bSynchronize`, `bReverseSync`): stock UE3, LIKELY (the
//!   capture's group has one unsynchronized member at rest);
//! - `ArkAnimNodeLocomotion`: the decoded node (ANIMATION.md section 3) except the assignment of
//!   the loops to its eight speed-mode children (SPECULATIVE, U-973);
//! - `ArkAnimNodeLookAt`: the grid weighting is the runtime's bilinear approximation (U-281);
//! - `ArkAnimNodeStack`, `ArkAnimNodeBlendPose`: LIKELY (U-971).
//!
//! Not modelled yet: the player tree's `Dishonored*` nodes (state pickers, tree references,
//! blend-by-speed / aim / directional, ...) evaluate as their first child (U-972); skeletal
//! controls (foot placement, the spine bender, the eye controls) are not applied (U-280: the
//! captured legs differ by 2.3 deg mean, `upper_leg_R_jnt` 2.84 deg on every tick, the error
//! confined to the two chains the foot placement drives; LIKELY its effect); notifies are not
//! dispatched (ARCHITECTURE.md step 5).

pub mod clip;
pub mod locomotion;
pub mod lookat;
pub mod pose;
pub mod tree;

pub use clip::{AnimSets, BakedClip, ClipRef, Clips, SetFlags};
pub use pose::Pose;
pub use tree::{AnimTreeInstance, FilterTarget, LocoInput, LookInput, SeqState, SlotAction, SlotRequest, StackChannel, TreeInputs, TreeShape, TreeState};
