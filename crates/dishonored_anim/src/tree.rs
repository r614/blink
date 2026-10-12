//! [`AnimTreeInstance`]: one skeletal mesh's state over a cooked AnimTree
//! (`dishonored_ir::anim_tree::AnimTreeAsset`), its tick and its pose.
//!
//! The instance is a [`TreeShape`] and a [`TreeState`]. The shape is what the cooked tree, the
//! skeleton and the mesh rotation fix (the graph, the tick order, each node's parameters, the
//! reference pose, the sync groups), built once and shared through an `Arc`; the state is what a
//! tick changes (weights and their blends, sequence players, slot channels, the locomotion and
//! look-at nodes, the blink timer), `Clone` and serializable, so the simulation keeps one per
//! pawn in its snapshots and rebuilds the instance with [`AnimTreeInstance::from_parts`].
//!
//! The tick follows UE3's `USkeletalMeshComponent::TickAnimNodes` over the tree's
//! `AnimTickArray` (parents before children: a node is added when the last of its parents
//! reaches it, then its own children follow; the order reproduces every cooked
//! `TickArrayIndex` of `NPC_Human_at`, which is how the oracle capture numbers the nodes, so
//! CONFIRMED). A node ticks when its `NodeTotalWeight` (the sum over its parent links of the
//! parent's total times the link's weight, at most 1) is relevant: blend nodes move their child
//! weights toward their targets, sequence nodes advance; the capture shows a sequence whose
//! weight reaches 0 stop advancing on that tick (CONFIRMED). Sequences in a sync group advance
//! afterwards (`UAnimTree::UpdateAnimNodeSeqGroups`): the heaviest synchronized relevant member
//! is the master, advanced by its rate times the group's rate scale; the other synchronized
//! members take the master's relative position (reversed with `bReverseSync`); members with
//! `bSynchronize` false advance on their own at the group's rate scale (stock UE3, LIKELY).
//! Requests (slot plays, filter targets, stack channels, blinks) land after the advance: a
//! requested sequence reads time 0 on its tick (the capture's second blink, CONFIRMED).
//!
//! The pose walks the graph from the root (`GetBoneAtoms`), per node class. Replaying the
//! capture's node states of `stealth/detect_1300` reproduces its bones to the capture's rounding
//! (0.008 deg mean over the body outside the legs and hands, 0.014 deg over `head_jnt` and the
//! face; tests/npc_human_replay.rs), so the classes that capture exercises are CONFIRMED there:
//! - `AnimNodeSequence`, `DishonoredAnimNodeSeq*`: the clip at the node's time; bones without a
//!   track keep the reference pose (the pose under the node with
//!   `m_bDontUseRefPoseOnNonDefinedBones`); `RootBoneOption` / `RootRotationOption` other than
//!   default keep the root bone's reference translation / rotation on that axis
//!   (`UAnimNodeSequence::GetAnimationPose`, stock UE3, LIKELY: the guard's clips leave the root
//!   bone undriven; every root option of `NPC_Human_at` is set on all three axes);
//! - `AnimNodeBlendList` and subclasses, `AnimNodeBlendByPhysics`, slots, the locomotion node:
//!   the children blended by their weights (`pose::Accumulator`); a slot's channels blend in and
//!   out linearly over the requested times (the capture's `FullBodySlot` channel rises 1/12 per
//!   tick for the 0.2 s `m_fBlendInTime`, CONFIRMED);
//! - `AnimNodeBlendPerBone`: `pose::blend_per_bone` with `Child2PerBoneWeight` from the branch
//!   bones, in component space unless `bForceLocalSpaceBlend` (component space CONFIRMED against
//!   the player tree in viewmodel.rs; `FaceFilter`'s local, branch-excluded blend by the face
//!   bones of the capture; the source child's weight reads 1 there);
//! - `AnimNodeAdditiveBlending`: the base plus the additive child's delta (children reached
//!   through a `bIsAdditive` link start from the identity pose; exercised by the capture only at
//!   the look-at's centred aim);
//! - `ArkAnimNodeStack`: each channel over the source in order, by its weight, a channel's
//!   undefined bones keeping the pose below (LIKELY: Arkane's class was not decompiled; U-971);
//! - `ArkAnimNodeBlendPose`: an `AnimNodeBlendList` subclass (its class default carries the
//!   list's `bForceChildFullWeightWhenBecomingRelevant`) whose second input is unconnected in
//!   `NPC_Human_at`; blended as a list, the unconnected input as the reference pose (LIKELY;
//!   U-971);
//! - `ArkAnimNodeLocomotion` (`crate::locomotion`) and `ArkAnimNodeLookAt` (`crate::lookat`);
//! - other classes (the player tree's `Dishonored*` nodes) pass their first connected child
//!   through (U-972).

use crate::clip::{ClipRef, Clips};
use crate::locomotion::{self, LocoParams};
use crate::lookat::{self, BlinkTimer};
use crate::pose::{self, Accumulator, Pose};
use dishonored_ir::BoneAsset;
use dishonored_ir::anim_tree::{AnimTreeAsset, NodeProps, RootBoneOption, RootRotationOption};
use dishonored_ir::npc_anim::{LocoConfig, LookatMode};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// UE3 `ZERO_ANIMWEIGHT_THRESH`: weights at or below it are not relevant.
pub const ZERO_WEIGHT: f32 = 0.00001;

/// `AnimNodeBlendByPhysics::TickAnim` switches to the owner's physics child over 0.1 s (stock
/// UE3, LIKELY).
const PHYSICS_BLEND: f32 = 0.1;

/// The look-at component's blink node (0x00956660 caches it and plays `Blink` on its channel).
const BLINK_NODE: &str = "BlinkBlender";

/// One sequence player's state (`AnimNodeSequence`: `AnimSeqName`, `CurrentTime`, `Rate`,
/// `bPlaying`, `bLooping`, `bReverseSync`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeqState {
    pub name: String,
    pub time: f32,
    pub rate: f32,
    pub playing: bool,
    pub looping: bool,
    /// `bReverseSync` (the locomotion node sets it from its loops' direction).
    reverse_sync: bool,
    /// This update's advance: (time before, signed move), for root motion.
    moved: Option<(f32, f32)>,
}

/// A sequence node's fixed options: its sync group (`SynchGroupName`), `bSynchronize`,
/// `RootBoneOption` / `RootRotationOption` per axis, `m_bDontUseRefPoseOnNonDefinedBones`.
#[derive(Clone, Debug)]
struct SeqShape {
    group: Option<usize>,
    synchronize: bool,
    root_bone: [RootBoneOption; 3],
    root_rotation: [RootRotationOption; 3],
    keep_undefined: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct LocoState {
    /// The node's smoothed velocity (angle from facing in degrees, speed).
    vel: Option<(f32, f32)>,
    modifier: Option<String>,
    /// The modifier fading out and its weight (0x00916770).
    prev: Option<(String, f32)>,
    /// The "Specific anim" child: sequence, weight, blend time, fading out.
    specific: Option<(String, f32, f32, bool)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct LookState {
    /// `Anims[].AnimName`: nine head, then nine torso cells (the cooked grids until a look-at
    /// mode names its own).
    names: Vec<String>,
    /// The cells' weights.
    cells: Vec<f32>,
    /// The look-at's own weight.
    weight: f32,
}

/// A node's class and its fixed parameters.
#[derive(Clone, Debug)]
enum Kind {
    Sequence,
    Physics,
    List,
    PerBone { mask: Vec<f32>, local_space: bool },
    Additive,
    Slot,
    Stack,
    Locomotion(LocoParams),
    LookAt,
    Pass,
}

/// The class-specific part of a node's state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum KindState {
    /// Sequences (their player is `NodeState::seq`), lists, additive blends, stacks and the
    /// nodes passed through.
    Plain,
    Physics { last: Option<u8> },
    /// `Child2WeightTarget` and the time left to reach it.
    PerBone { target: f32, left: f32 },
    Slot { active: usize, blend_out: Option<f32> },
    Locomotion(Box<LocoState>),
    LookAt(Box<LookState>),
}

/// A node's place in the graph and its fixed parameters.
#[derive(Clone, Debug)]
struct NodeShape {
    name: String,
    kind: Kind,
    children: Vec<Option<usize>>,
    additive: Vec<bool>,
    /// The cooked `TickArrayIndex`.
    tick_index: Option<usize>,
    seq: Option<SeqShape>,
}

/// What a tick changes on a node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct NodeState {
    kind: KindState,
    /// `Children[i].Weight` (for `AnimNodeBlendPerBone`: 1 and `Child2Weight`).
    weights: Vec<f32>,
    /// `TargetWeight` and `BlendTimeToGo` of lists and slots.
    target: Vec<f32>,
    left: f32,
    /// `NodeTotalWeight`.
    total: f32,
    seq: Option<SeqState>,
}

#[derive(Clone, Debug)]
struct Group {
    name: String,
    members: Vec<usize>,
}

/// A slot request (`AnimNodeSlot::PlayCustomAnim` / `StopCustomAnim`).
#[derive(Clone, Debug)]
pub enum SlotAction {
    /// Play a sequence on a free channel: blend it in over `blend_in`; at its end (or
    /// `blend_out` before it) blend back to the source over `blend_out`, or hold the last frame
    /// with `None` (UE3's negative blend-out time).
    Play { sequence: String, rate: f32, blend_in: f32, blend_out: Option<f32>, looping: bool, start: f32 },
    /// Blend back to the source.
    Stop { blend_out: f32 },
}

#[derive(Clone, Debug)]
pub struct SlotRequest {
    /// The slot node's `NodeName` (`FullBodySlot`, `UpperBodySlot`, `FaceSlot`, ...).
    pub slot: String,
    pub action: SlotAction,
}

/// `AnimNodeBlend::SetBlendTarget` on a per-bone filter: `Child2Weight` toward `target` over
/// `time` seconds.
#[derive(Clone, Debug)]
pub struct FilterTarget {
    pub node: String,
    pub target: f32,
    pub time: f32,
}

/// A stack channel driven from outside (matinee anim tracks: `MatineeStackNode` channels 0-5):
/// its sequence, position and weight this tick.
#[derive(Clone, Debug)]
pub struct StackChannel {
    pub stack: String,
    /// 0-based channel (the node's child `channel + 1`).
    pub channel: usize,
    pub sequence: String,
    pub time: f32,
    pub weight: f32,
}

/// What the gameplay layer asks of the locomotion node this tick.
#[derive(Clone, Debug)]
pub struct LocoInput<'a> {
    /// The pawn tweak's `ArkComponentLocomotionConfig`.
    pub config: &'a LocoConfig,
    /// The requested modifier (`m_Modifiers[].m_Name`).
    pub modifier: &'a str,
    /// The pawn's velocity relative to its facing: angle (degrees, 0 forward, +90 right) and
    /// ground speed (uu/s), before the node's smoothing.
    pub velocity: (f32, f32),
    /// The sequence the locomotion component asks for in the "Specific anim" child (start,
    /// stop, turn, transition) and its blend time.
    pub specific: Option<(&'a str, f32)>,
}

/// What the gameplay layer asks of the look-at node this tick.
#[derive(Clone, Debug)]
pub struct LookInput<'a> {
    /// The look-at mode for the stance and gait (its ranges, and its grids when they are named).
    pub mode: &'a LookatMode,
    /// Head and torso aims (degrees: yaw right, pitch up), split as 0x009176F0 does.
    pub head: [f32; 2],
    pub torso: [f32; 2],
    pub weight: f32,
}

/// The gameplay layer's requests for one tick.
#[derive(Clone, Debug, Default)]
pub struct TreeInputs<'a> {
    /// The owner's `Physics` (`EPhysics`: 1 walking, 2 falling, ... 10 rigid body) for
    /// `AnimNodeBlendByPhysics`.
    pub physics: Option<u8>,
    pub slots: &'a [SlotRequest],
    pub filters: &'a [FilterTarget],
    pub stacks: &'a [StackChannel],
    pub locomotion: Option<LocoInput<'a>>,
    pub look: Option<LookInput<'a>>,
    /// Start a blink at this play rate (the look-at component's roll; see `set_blink_timer`).
    pub blink: Option<f32>,
}

/// What the cooked tree, the skeleton and the mesh rotation fix: the graph and its tick order,
/// each node's parameters (per-bone masks, sequence options and sync groups, the locomotion
/// node's tunables), the skeleton's parents and reference pose, and the state the cooked values
/// start from. Built once per tree and skeleton and shared by the meshes that use them.
#[derive(Clone, Debug)]
pub struct TreeShape {
    nodes: Vec<NodeShape>,
    root: Option<usize>,
    order: Vec<usize>,
    /// Parents of each node (unique), for the slots' end-of-anim handling.
    node_parents: Vec<Vec<usize>>,
    bones: Vec<Option<usize>>,
    bind: Pose,
    groups: Vec<Group>,
    mesh_rotation: Quat,
    /// The look-at component's blink node (`BlinkBlender`).
    blink_node: Option<usize>,
    /// The cooked children weights, sequences, times and rates, filter targets and look-at
    /// grids ([`Self::initial_state`]).
    initial: TreeState,
}

/// What a tick changes: per node the children weights, their targets and blend time and
/// `NodeTotalWeight`, the sequence players, the slots' active channels, the filters' targets,
/// the physics node's last physics, the locomotion node's smoothed velocity and modifiers, the
/// look-at cells; the sync groups' rate scales, the last root motion and the blink timer. It
/// reads only against the [`TreeShape`] it was made from (`TreeShape::initial_state`, then the
/// ticks of an instance of that shape).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TreeState {
    nodes: Vec<NodeState>,
    /// `FAnimGroup::RateScale` of each sync group (the locomotion node's
    /// `m_fMostRelevantRateScale`).
    group_rates: Vec<f32>,
    root_motion: (Vec3, f32),
    blink: Option<BlinkTimer>,
}

/// One mesh's evaluation of an AnimTree: the shared [`TreeShape`] and the mesh's
/// [`TreeState`].
#[derive(Clone, Debug)]
pub struct AnimTreeInstance {
    shape: Arc<TreeShape>,
    state: TreeState,
}

impl TreeShape {
    /// The shape of `tree` for a mesh with `skeleton` and the mesh's `RotOrigin` rotation (mesh
    /// space -> the pawn's frame, for [`AnimTreeInstance::root_delta`]): per-bone masks of the
    /// per-bone filters, the sequences' options and sync groups, the tick order, and the state
    /// from the cooked values (children weights, sequences, times, rates).
    pub fn new(tree: &AnimTreeAsset, skeleton: &[BoneAsset], mesh_rotation: Quat) -> Self {
        let mut nodes: Vec<NodeShape> = Vec::with_capacity(tree.nodes.len());
        let mut states: Vec<NodeState> = Vec::with_capacity(tree.nodes.len());
        for n in &tree.nodes {
            let children: Vec<Option<usize>> = n.children.iter().map(|c| c.node).collect();
            let additive = n.children.iter().map(|c| c.additive).collect();
            let mut weights: Vec<f32> = n.children.iter().map(|c| c.weight).collect();
            let (mut seq, mut seq_state) = (None, None);
            let (kind, kind_state) = match &n.props {
                NodeProps::Sequence { seq: name, rate, looping, playing, synchronize, reverse_sync, root_bone, root_rotation, keep_undefined, .. } => {
                    let time = raw_f32(&n.raw, "CurrentTime").unwrap_or(0.0);
                    seq = Some(SeqShape { group: None, synchronize: *synchronize, root_bone: *root_bone, root_rotation: *root_rotation, keep_undefined: *keep_undefined });
                    seq_state = Some(SeqState { name: name.clone(), time, rate: *rate, playing: *playing, looping: *looping, reverse_sync: *reverse_sync, moved: None });
                    (Kind::Sequence, KindState::Plain)
                }
                NodeProps::BlendByPhysics => (Kind::Physics, KindState::Physics { last: None }),
                NodeProps::BlendList => (Kind::List, KindState::Plain),
                NodeProps::BlendPerBone { branches, local_space, exclude_branch } => {
                    // `Child2Weight` / `Child2WeightTarget` as cooked (`UpperBodyFilter` 1,
                    // `FaceFilter` the class default 0); the source's weight is 1.
                    let child2 = raw_f32(&n.raw, "Child2Weight").unwrap_or(0.0);
                    let target = raw_f32(&n.raw, "Child2WeightTarget").unwrap_or(child2);
                    weights = vec![1.0, child2];
                    (Kind::PerBone { mask: pose::branch_weights(skeleton, branches, *exclude_branch), local_space: *local_space }, KindState::PerBone { target, left: 0.0 })
                }
                NodeProps::AdditiveBlending => (Kind::Additive, KindState::Plain),
                NodeProps::Slot { .. } => (Kind::Slot, KindState::Slot { active: 0, blend_out: None }),
                NodeProps::Locomotion { rate, range, min_ratio, modifier_blend } => (
                    Kind::Locomotion(LocoParams { rate: *rate, range: *range, min_ratio: *min_ratio, modifier_blend: *modifier_blend }),
                    KindState::Locomotion(Box::new(LocoState { vel: None, modifier: None, prev: None, specific: None })),
                ),
                NodeProps::AimOffset { grids } => (Kind::LookAt, KindState::LookAt(Box::new(LookState { names: grids.clone(), cells: vec![0.0; grids.len()], weight: 0.0 }))),
                NodeProps::Other if n.class == "ArkAnimNodeStack" => (Kind::Stack, KindState::Plain),
                NodeProps::Other if n.class == "ArkAnimNodeBlendPose" => (Kind::List, KindState::Plain),
                _ => (Kind::Pass, KindState::Plain),
            };
            let target = weights.clone();
            nodes.push(NodeShape { name: n.name.clone(), kind, children, additive, tick_index: raw_usize(&n.raw, "TickArrayIndex"), seq });
            states.push(NodeState { kind: kind_state, weights, target, left: 0.0, total: 0.0, seq: seq_state });
        }
        // Sync groups by name, in name order (deterministic).
        let mut names: Vec<String> = tree.nodes.iter().filter_map(|n| match &n.props {
            NodeProps::Sequence { sync_group, .. } if !sync_group.is_empty() && sync_group != "None" => Some(sync_group.clone()),
            _ => None,
        }).collect();
        names.sort();
        names.dedup();
        let mut groups: Vec<Group> = names.into_iter().map(|name| Group { name, members: Vec::new() }).collect();
        for (i, n) in tree.nodes.iter().enumerate() {
            if let NodeProps::Sequence { sync_group, .. } = &n.props {
                if let Some(g) = groups.iter().position(|g| &g.name == sync_group) {
                    groups[g].members.push(i);
                    if let Some(s) = nodes[i].seq.as_mut() {
                        s.group = Some(g);
                    }
                }
            }
        }
        let mut node_parents = vec![Vec::new(); nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            for c in n.children.iter().flatten() {
                if !node_parents[*c].contains(&i) {
                    node_parents[*c].push(i);
                }
            }
        }
        let order = tick_order(&nodes, &node_parents, tree.root);
        let blink_node = nodes.iter().position(|n| n.name.eq_ignore_ascii_case(BLINK_NODE));
        let initial = TreeState { nodes: states, group_rates: vec![1.0; groups.len()], root_motion: (Vec3::ZERO, 0.0), blink: None };
        TreeShape { nodes, root: tree.root, order, node_parents, bones: pose::parents(skeleton), bind: pose::bind_pose(skeleton), groups, mesh_rotation, blink_node, initial }
    }

    /// The state [`AnimTreeInstance::new`] starts from: the cooked values, no blink timer.
    pub fn initial_state(&self) -> TreeState {
        self.initial.clone()
    }

    /// The node named `name` (`NodeName`, else the export's name).
    pub fn node_index(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name.eq_ignore_ascii_case(name))
    }

    /// The node at a cooked `TickArrayIndex` (the oracle capture's node numbering; 0 is the tree
    /// object itself, which has no node here).
    pub fn node_at_tick_index(&self, index: usize) -> Option<usize> {
        self.nodes.iter().position(|n| n.tick_index == Some(index))
    }

    /// The cooked `TickArrayIndex` of a node.
    pub fn tick_index(&self, node: usize) -> Option<usize> {
        self.nodes.get(node).and_then(|n| n.tick_index)
    }

    /// Nodes in tick order (parents before children), the root first.
    pub fn tick_order(&self) -> &[usize] {
        &self.order
    }

    /// The mesh's `RotOrigin` rotation (mesh space -> the pawn's frame).
    pub fn mesh_rotation(&self) -> Quat {
        self.mesh_rotation
    }
}

impl AnimTreeInstance {
    /// Instantiate `tree` for a mesh with `skeleton`: per-node state from the cooked values
    /// (children weights, sequences, times, rates), per-bone masks of the per-bone filters.
    pub fn new(tree: &AnimTreeAsset, skeleton: &[BoneAsset]) -> Self {
        Self::from_shape(Arc::new(TreeShape::new(tree, skeleton, Quat::IDENTITY)))
    }

    /// An instance of a shared shape, from its cooked state.
    pub fn from_shape(shape: Arc<TreeShape>) -> Self {
        let state = shape.initial_state();
        AnimTreeInstance { shape, state }
    }

    /// An instance of a shared shape resumed from a state of that shape (a snapshot's). Panics
    /// when the state's nodes or sync groups are not the shape's.
    pub fn from_parts(shape: Arc<TreeShape>, state: TreeState) -> Self {
        assert!(state.nodes.len() == shape.nodes.len() && state.group_rates.len() == shape.groups.len(), "TreeState of another TreeShape ({} nodes, {} groups; the shape has {}, {})", state.nodes.len(), state.group_rates.len(), shape.nodes.len(), shape.groups.len());
        AnimTreeInstance { shape, state }
    }

    /// The mesh's `RotOrigin` rotation (mesh space -> the pawn's frame), for [`Self::root_delta`].
    pub fn with_mesh_rotation(mut self, rotation: Quat) -> Self {
        Arc::make_mut(&mut self.shape).mesh_rotation = rotation;
        self
    }

    /// The shared shape.
    pub fn shape(&self) -> &Arc<TreeShape> {
        &self.shape
    }

    /// The per-mesh state.
    pub fn state(&self) -> &TreeState {
        &self.state
    }

    /// The per-mesh state, the shape dropped.
    pub fn into_state(self) -> TreeState {
        self.state
    }

    /// Let the instance roll its own blinks (the look-at component's timer; `lookat::BlinkTimer`).
    pub fn set_blink_timer(&mut self, timer: Option<BlinkTimer>) {
        self.state.blink = timer;
    }

    /// The node named `name` (`NodeName`, else the export's name).
    pub fn node_index(&self, name: &str) -> Option<usize> {
        self.shape.node_index(name)
    }

    /// The node at a cooked `TickArrayIndex` (the oracle capture's node numbering; 0 is the tree
    /// object itself, which has no node here).
    pub fn node_at_tick_index(&self, index: usize) -> Option<usize> {
        self.shape.node_at_tick_index(index)
    }

    /// The cooked `TickArrayIndex` of a node.
    pub fn tick_index(&self, node: usize) -> Option<usize> {
        self.shape.tick_index(node)
    }

    /// Nodes in tick order (parents before children), the root first.
    pub fn tick_order(&self) -> &[usize] {
        self.shape.tick_order()
    }

    /// A node's children weights (for the look-at node, its cell weights).
    pub fn children_weights(&self, node: usize) -> Vec<f32> {
        let n = &self.state.nodes[node];
        match &n.kind {
            KindState::LookAt(l) => l.cells.clone(),
            _ => n.weights.clone(),
        }
    }

    /// A node's `NodeTotalWeight` after the last update.
    pub fn total_weight(&self, node: usize) -> f32 {
        self.state.nodes[node].total
    }

    /// A sequence node's state.
    pub fn sequence(&self, node: usize) -> Option<&SeqState> {
        self.state.nodes.get(node).and_then(|n| n.seq.as_ref())
    }

    /// Replay: a sequence node's state as a capture records it (`NodeTotalWeight`,
    /// `AnimSeqName`, `CurrentTime`, `Rate`). On another node only the weight is set.
    pub fn set_node_state(&mut self, node: usize, weight: f32, sequence: &str, time: f32, rate: f32) {
        let n = &mut self.state.nodes[node];
        n.total = weight;
        if let Some(s) = n.seq.as_mut() {
            if s.name != sequence {
                s.name = sequence.to_string();
            }
            s.time = time;
            s.rate = rate;
        }
    }

    /// Replay: a blend node's children weights as a capture records them (`Children[i].Weight`;
    /// for the look-at node, its cell weights at full look-at weight).
    pub fn set_children_weights(&mut self, node: usize, weights: &[f32]) {
        let n = &mut self.state.nodes[node];
        match &mut n.kind {
            KindState::LookAt(l) => {
                for (c, w) in l.cells.iter_mut().zip(weights) {
                    *c = *w;
                }
                l.weight = 1.0;
            }
            KindState::PerBone { target, left } => {
                for (c, w) in n.weights.iter_mut().zip(weights) {
                    *c = *w;
                }
                *target = n.weights.get(1).copied().unwrap_or(0.0);
                *left = 0.0;
            }
            _ => {
                for (c, w) in n.weights.iter_mut().zip(weights) {
                    *c = *w;
                }
                n.target.clone_from(&n.weights);
                n.left = 0.0;
            }
        }
    }

    /// The root bone's travel (in the pawn's frame through the mesh rotation) and yaw (degrees)
    /// over the last update, from the sequences whose root options extract it (`RBA_Translate`
    /// axes, `RRO_Extract`), weighted by their `NodeTotalWeight`; sampled from the clip's root
    /// track as `dishonored_assets::fatality::load_root_motion` samples the paired moves.
    pub fn root_delta(&self) -> (Vec3, f32) {
        self.state.root_motion
    }

    /// Advance one tick of `dt` seconds. The nodes tick first, reading the continuous inputs
    /// (physics, locomotion, look-at) as their `TickAnim` reads the pawn; the requests (slot
    /// plays and stops, filter targets, stack channels, a blink) land after the advance, so a
    /// requested sequence reads time 0 on the tick it is asked for and advances from the next
    /// (the capture's second blink reads 0 on its first tick: CONFIRMED for the look-at
    /// component's blink, the same order assumed for the other requesters, LIKELY).
    pub fn update(&mut self, clips: &dyn Clips, inputs: &TreeInputs, dt: f32) {
        self.state.update(&self.shape, clips, inputs, dt);
    }

    /// The local pose of every skeleton bone (the reference pose where nothing drives a bone).
    pub fn pose(&self, clips: &dyn Clips) -> Pose {
        self.state.pose(&self.shape, clips)
    }
}

impl TreeState {
    /// The last update's root motion ([`AnimTreeInstance::root_delta`]).
    pub fn root_delta(&self) -> (Vec3, f32) {
        self.root_motion
    }

    /// [`AnimTreeInstance::update`] on a state kept apart from its shape (the simulation keeps
    /// the state per NPC and shares the shape); `shape` must be the one the state was made from.
    pub fn update(&mut self, shape: &TreeShape, clips: &dyn Clips, inputs: &TreeInputs, dt: f32) {
        self.root_motion = (Vec3::ZERO, 0.0);
        for n in self.nodes.iter_mut() {
            if let Some(s) = n.seq.as_mut() {
                s.moved = None;
            }
        }
        for n in self.nodes.iter_mut() {
            n.total = 0.0;
        }
        if let Some(r) = shape.root {
            self.nodes[r].total = 1.0;
        }
        for &i in &shape.order {
            if self.nodes[i].total > ZERO_WEIGHT {
                self.tick_node(shape, i, clips, inputs, dt);
            }
            let total = self.nodes[i].total;
            let children = &shape.nodes[i].children;
            for k in 0..children.len().min(self.nodes[i].weights.len()) {
                let Some(c) = children[k] else { continue };
                let w = self.nodes[i].weights[k];
                let t = &mut self.nodes[c].total;
                *t = (*t + total * w).min(1.0);
            }
        }
        self.tick_groups(shape, clips, dt);
        self.collect_root_motion(shape, clips);
        self.apply_requests(shape, inputs);
        let blinking = shape.blink_node.and_then(|b| shape.nodes[b].children.get(1).copied().flatten()).and_then(|c| self.nodes[c].seq.as_ref()).is_some_and(|s| s.playing);
        let rolled = self.blink.as_mut().and_then(|t| t.tick(dt, blinking));
        if let Some(rate) = inputs.blink.or(rolled) {
            self.start_blink(shape, rate);
        }
    }

    /// The requests of `inputs` (slot plays and stops, filter targets, stack channels, a blink)
    /// landed without a tick, as the end of [`Self::update`] lands them: a requester outside the
    /// one that drives the update (the oracle's scenario runner pinning an NPC's ambient idle)
    /// asks between two updates; the continuous inputs are not read.
    pub fn request(&mut self, shape: &TreeShape, inputs: &TreeInputs) {
        self.apply_requests(shape, inputs);
        if let Some(rate) = inputs.blink {
            self.start_blink(shape, rate);
        }
    }

    /// [`AnimTreeInstance::pose`] of a state kept apart from its shape.
    pub fn pose(&self, shape: &TreeShape, clips: &dyn Clips) -> Pose {
        match shape.root {
            Some(r) => self.eval(shape, r, clips, Fill::Bind),
            None => shape.bind.clone(),
        }
    }

    // ---- requests ----

    fn apply_requests(&mut self, shape: &TreeShape, inputs: &TreeInputs) {
        for r in inputs.slots {
            let Some(i) = shape.nodes.iter().position(|n| matches!(n.kind, Kind::Slot) && n.name.eq_ignore_ascii_case(&r.slot)) else { continue };
            match &r.action {
                SlotAction::Play { sequence, rate, blend_in, blend_out, looping, start } => {
                    let channel = self.free_channel(shape, i);
                    let Some(c) = channel.and_then(|ch| shape.nodes[i].children[ch]) else { continue };
                    if let Some(s) = self.nodes[c].seq.as_mut() {
                        s.name = sequence.clone();
                        s.time = *start;
                        s.rate = *rate;
                        s.looping = *looping;
                        s.playing = true;
                    }
                    let ch = channel.unwrap_or(0);
                    self.set_active(i, ch, *blend_in);
                    if let KindState::Slot { active, blend_out: bo } = &mut self.nodes[i].kind {
                        *active = ch;
                        *bo = *blend_out;
                    }
                }
                SlotAction::Stop { blend_out } => {
                    self.set_active(i, 0, *blend_out);
                    if let KindState::Slot { active, .. } = &mut self.nodes[i].kind {
                        *active = 0;
                    }
                }
            }
        }
        for f in inputs.filters {
            let Some(i) = shape.node_index(&f.node) else { continue };
            let n = &mut self.nodes[i];
            if let KindState::PerBone { target, left } = &mut n.kind {
                *target = f.target;
                *left = f.time.max(0.0);
                if f.time <= 0.0 && n.weights.len() > 1 {
                    n.weights[1] = f.target;
                }
            }
        }
        for s in inputs.stacks {
            let Some(i) = shape.nodes.iter().position(|n| matches!(n.kind, Kind::Stack) && n.name.eq_ignore_ascii_case(&s.stack)) else { continue };
            let k = s.channel + 1;
            let Some(c) = shape.nodes[i].children.get(k).copied().flatten() else { continue };
            if let Some(q) = self.nodes[c].seq.as_mut() {
                q.name = s.sequence.clone();
                q.time = s.time;
                q.playing = false;
            }
            self.nodes[i].weights[k] = s.weight;
        }
    }

    /// `FindBestChildToPlayAnim`: the least weighted channel other than the active one, the
    /// first unused one at once (stock UE3, LIKELY).
    fn free_channel(&self, shape: &TreeShape, slot: usize) -> Option<usize> {
        let (children, n) = (&shape.nodes[slot].children, &self.nodes[slot]);
        let active = match n.kind {
            KindState::Slot { active, .. } => active,
            _ => 0,
        };
        let mut best: Option<(usize, f32)> = None;
        for c in 1..children.len() {
            if c == active || children[c].is_none() {
                continue;
            }
            let w = n.weights[c];
            if best.is_none_or(|b| w < b.1) {
                best = Some((c, w));
            }
            if w <= ZERO_WEIGHT {
                break;
            }
        }
        best.map(|b| b.0)
    }

    /// `SetActiveChild`: one child's target weight 1, the others 0, reached linearly over `time`
    /// (at once for 0).
    fn set_active(&mut self, node: usize, child: usize, time: f32) {
        let n = &mut self.nodes[node];
        for (k, t) in n.target.iter_mut().enumerate() {
            *t = if k == child { 1.0 } else { 0.0 };
        }
        n.left = time.max(0.0);
        if time <= 0.0 {
            n.weights.clone_from(&n.target);
        }
    }

    fn start_blink(&mut self, shape: &TreeShape, rate: f32) {
        let Some(b) = shape.blink_node else { return };
        let Some(c) = shape.nodes[b].children.get(1).copied().flatten() else { return };
        // 0x00956660: the channel at full weight; the sequence restarts at the rolled rate.
        self.nodes[b].weights[1] = 1.0;
        if let Some(s) = self.nodes[c].seq.as_mut() {
            s.time = 0.0;
            s.rate = rate;
            s.looping = false;
            s.playing = true;
        }
    }

    // ---- tick ----

    fn tick_node(&mut self, shape: &TreeShape, i: usize, clips: &dyn Clips, inputs: &TreeInputs, dt: f32) {
        match &shape.nodes[i].kind {
            Kind::Sequence => {
                let ungrouped = shape.nodes[i].seq.as_ref().is_some_and(|o| o.group.is_none());
                if let Some(rate) = self.nodes[i].seq.as_ref().filter(|_| ungrouped).map(|s| s.rate) {
                    self.advance(shape, i, rate * dt, clips);
                }
            }
            Kind::Physics => {
                let last = match self.nodes[i].kind {
                    KindState::Physics { last } => last,
                    _ => None,
                };
                if let Some(p) = inputs.physics.filter(|p| Some(*p) != last) {
                    if (p as usize) < shape.nodes[i].children.len() {
                        self.set_active(i, p as usize, PHYSICS_BLEND);
                    }
                    self.nodes[i].kind = KindState::Physics { last: Some(p) };
                }
                blend_weights(&mut self.nodes[i], dt);
            }
            Kind::List => blend_weights(&mut self.nodes[i], dt),
            Kind::PerBone { .. } => {
                // `AnimNodeBlend::TickAnim`: `Child2Weight` moves linearly to its target.
                let n = &mut self.nodes[i];
                if let KindState::PerBone { target, left } = &mut n.kind {
                    if n.weights.len() > 1 && n.weights[1] != *target {
                        if *left < dt {
                            n.weights[1] = *target;
                            *left = 0.0;
                        } else {
                            n.weights[1] += (*target - n.weights[1]) * (dt / *left);
                            *left -= dt;
                        }
                    }
                }
            }
            Kind::Slot => {
                self.early_blend_out(shape, i, clips);
                blend_weights(&mut self.nodes[i], dt);
            }
            Kind::Locomotion(params) => {
                if let Some(input) = &inputs.locomotion {
                    self.tick_locomotion(shape, i, params, clips, input, dt);
                }
            }
            Kind::LookAt => {
                if let (Some(look), KindState::LookAt(l)) = (&inputs.look, &mut self.nodes[i].kind) {
                    // The mode's grids replace the cooked ones when it names them.
                    if look.mode.head.len() == 9 && look.mode.torso.len() == 9 && l.names.len() == 18 {
                        l.names[..9].clone_from_slice(&look.mode.head);
                        l.names[9..].clone_from_slice(&look.mode.torso);
                    }
                    l.cells = lookat::node_weights(look.mode, look.head, look.torso).to_vec();
                    l.weight = look.weight;
                }
            }
            Kind::Additive | Kind::Stack | Kind::Pass => {}
        }
    }

    /// A slot's active channel blends back to the source so that the blend ends with the
    /// sequence: when the time left is within the pending blend-out time (`AnimNodeSlot::
    /// TickAnim`, with `bEarlyAnimEndNotify` the end notification fires then; stock UE3, LIKELY).
    fn early_blend_out(&mut self, shape: &TreeShape, i: usize, clips: &dyn Clips) {
        let KindState::Slot { active, blend_out: Some(bo) } = self.nodes[i].kind else { return };
        if active == 0 {
            return;
        }
        let Some(c) = shape.nodes[i].children.get(active).copied().flatten() else { return };
        let Some(s) = self.nodes[c].seq.as_ref() else { return };
        if s.looping || !s.playing || s.rate == 0.0 {
            return;
        }
        let Some(len) = clips.find(&s.name).map(|r| r.clip.duration) else { return };
        let left = if s.rate > 0.0 { (len - s.time) / s.rate } else { s.time / -s.rate };
        if left <= bo {
            self.set_active(i, 0, left.max(0.0));
            self.nodes[i].kind = KindState::Slot { active: 0, blend_out: Some(bo) };
        }
    }

    /// `AnimNodeSequence::AdvanceBy`: loops wrap; a non-looping sequence stops at its end, and a
    /// slot playing it as its active channel blends back to its source (`OnChildAnimEnd` ->
    /// `StopCustomAnim(PendingBlendOutTime)`; held with no blend-out time).
    fn advance(&mut self, shape: &TreeShape, i: usize, delta: f32, clips: &dyn Clips) {
        let Some(s) = self.nodes[i].seq.as_ref() else { return };
        if !s.playing || delta == 0.0 {
            return;
        }
        let Some(len) = clips.find(&s.name).map(|r| r.clip.duration) else { return };
        let s = self.nodes[i].seq.as_mut().unwrap();
        let from = s.time;
        let mut t = from + delta;
        let mut ended = false;
        if s.looping {
            if len > 0.0 {
                t = t.rem_euclid(len);
            }
        } else if t >= len || t < 0.0 {
            t = t.clamp(0.0, len);
            s.playing = false;
            ended = true;
        }
        s.time = t;
        s.moved = Some((from, if s.looping { delta } else { t - from }));
        if ended {
            for &p in &shape.node_parents[i] {
                if let KindState::Slot { active, blend_out } = self.nodes[p].kind {
                    if active != 0 && shape.nodes[p].children.get(active).copied().flatten() == Some(i) {
                        if let Some(bo) = blend_out {
                            self.set_active(p, 0, bo);
                            self.nodes[p].kind = KindState::Slot { active: 0, blend_out };
                        }
                    }
                }
            }
        }
    }

    fn tick_groups(&mut self, shape: &TreeShape, clips: &dyn Clips, dt: f32) {
        let synchronized = |m: usize| shape.nodes[m].seq.as_ref().is_some_and(|o| o.synchronize);
        for (g, group) in shape.groups.iter().enumerate() {
            let members = &group.members;
            let scale = self.group_rates[g];
            let relevant = |me: &Self, m: usize| me.nodes[m].total > ZERO_WEIGHT && me.nodes[m].seq.as_ref().is_some_and(|s| s.playing && clips.find(&s.name).is_some());
            let mut master: Option<usize> = None;
            for &m in members {
                if !relevant(self, m) {
                    continue;
                }
                let s = self.nodes[m].seq.as_ref().unwrap();
                if !synchronized(m) {
                    let rate = s.rate;
                    self.advance(shape, m, rate * scale * dt, clips);
                } else if master.is_none_or(|x| self.nodes[m].total > self.nodes[x].total) {
                    master = Some(m);
                }
            }
            let Some(master) = master else { continue };
            let rate = self.nodes[master].seq.as_ref().map_or(0.0, |s| s.rate);
            self.advance(shape, master, rate * scale * dt, clips);
            let Some(pos) = self.group_position(master, clips) else { continue };
            for &m in members {
                if m == master || !relevant(self, m) || !synchronized(m) {
                    continue;
                }
                let s = self.nodes[m].seq.as_ref().unwrap();
                let Some(len) = clips.find(&s.name).map(|r| r.clip.duration) else { continue };
                let c = if s.reverse_sync { 1.0 - pos } else { pos };
                let t = if len > 0.0 { (c * len).rem_euclid(len) } else { 0.0 };
                let s = self.nodes[m].seq.as_mut().unwrap();
                let from = s.time;
                let mut d = t - from;
                // The slave's move on its own timeline, the short way around the loop.
                if s.looping && len > 0.0 {
                    if d > len * 0.5 {
                        d -= len;
                    } else if d < -len * 0.5 {
                        d += len;
                    }
                }
                s.time = t;
                s.moved = Some((from, d));
            }
        }
    }

    /// `GetGroupRelativePosition`: the node's time over its length, reversed with
    /// `bReverseSync`.
    fn group_position(&self, node: usize, clips: &dyn Clips) -> Option<f32> {
        let s = self.nodes[node].seq.as_ref()?;
        let len = clips.find(&s.name)?.clip.duration;
        if len <= 0.0 {
            return Some(0.0);
        }
        let p = (s.time / len).rem_euclid(1.0);
        Some(if s.reverse_sync { 1.0 - p } else { p })
    }

    /// The locomotion node's tick: the velocity smoothed, the loops of the modifier (and the one
    /// fading out) weighted into the speed-mode children, the "Specific anim" child blended over
    /// them, the `LocoGroup` rate scale. The assignment of the loops to the eight
    /// `SpeedModeSlot` children is not decoded (U-973): the idle loop takes the cooked
    /// unsynchronized slot 0, the moving loops the synchronized slots after it, each with its
    /// rate sign and `bReverseSync` set from the loop's direction.
    fn tick_locomotion(&mut self, shape: &TreeShape, i: usize, params: &LocoParams, clips: &dyn Clips, input: &LocoInput, dt: f32) {
        let KindState::Locomotion(st) = &self.nodes[i].kind else { return };
        let mut st = st.as_ref().clone();
        let cfg = input.config;
        st.vel = Some(match st.vel {
            Some(v) => locomotion::smooth_velocity(v, input.velocity, dt, params),
            None => input.velocity,
        });
        let vel = st.vel.unwrap_or_default();
        if st.modifier.as_deref() != Some(input.modifier) {
            if let Some(old) = st.modifier.take() {
                st.prev = Some((old, 1.0));
            }
            st.modifier = Some(input.modifier.to_string());
        }
        let Some(mi) = cfg.modifier(input.modifier) else { return };
        let (main, speed) = locomotion::loco_entries(cfg, mi, vel);
        let w_prev = st.prev.as_ref().map_or(0.0, |p| p.1);
        let mut entries: Vec<(locomotion::LocoEntry, f32)> = main.iter().map(|e| (e.clone(), e.weight * (1.0 - w_prev))).collect();
        if let Some((pm, w)) = st.prev.clone() {
            if let Some(pi) = cfg.modifier(&pm) {
                entries.extend(locomotion::loco_entries(cfg, pi, vel).0.into_iter().map(|e| {
                    let w2 = e.weight * w;
                    (e, w2)
                }));
            }
            let w = w - dt / params.modifier_blend.max(0.01);
            st.prev = (w > 0.0).then_some((pm, w));
        }
        let lengths: Vec<Option<f32>> = main.iter().map(|e| clips.find(&e.name).map(|r| r.clip.duration)).collect();
        let multiplier = cfg.modifiers.get(mi).map_or(1.0, |m| m.speed_multiplier);
        let rate_scale = locomotion::group_rate(&main, &lengths, speed, multiplier);
        // The "Specific anim" child (the last one).
        let children = &shape.nodes[i].children;
        let specific_child = children.len().saturating_sub(1);
        match (input.specific, st.specific.take()) {
            (Some((name, blend)), cur) if cur.as_ref().is_none_or(|c| c.0 != name || c.3) => {
                if let Some(c) = children.get(specific_child).copied().flatten() {
                    if let Some(s) = self.nodes[c].seq.as_mut() {
                        s.name = name.to_string();
                        s.time = 0.0;
                        s.rate = 1.0;
                        s.looping = false;
                        s.playing = true;
                    }
                }
                st.specific = Some((name.to_string(), cur.map_or(0.0, |c| c.1), blend, false));
            }
            (Some(_), cur) => st.specific = cur,
            (None, Some(c)) => st.specific = Some((c.0, c.1, c.2, true)),
            (None, None) => {}
        }
        let mut w_specific = 0.0;
        if let Some((name, w, blend, fading)) = st.specific.take() {
            let ended = children.get(specific_child).copied().flatten().and_then(|c| self.nodes[c].seq.as_ref()).is_some_and(|s| !s.playing);
            let fading = fading || ended;
            let step = if blend > 0.0 { dt / blend } else { 1.0 };
            let w = if fading { w - step } else { (w + step).min(1.0) };
            if w > 0.0 {
                w_specific = w;
                st.specific = Some((name, w, blend, fading));
            }
        }
        let n_slots = specific_child;
        let mut weights = vec![0.0; children.len()];
        let (mut idle_used, mut next) = (false, 1);
        for (e, w) in entries {
            let slot = if e.single && !idle_used && n_slots > 0 {
                idle_used = true;
                0
            } else if next < n_slots {
                next += 1;
                next - 1
            } else {
                continue;
            };
            weights[slot] = w * (1.0 - w_specific);
            if let Some(c) = children.get(slot).copied().flatten() {
                if let Some(s) = self.nodes[c].seq.as_mut() {
                    if !s.name.eq_ignore_ascii_case(&e.name) {
                        s.name = e.name.clone();
                        s.time = 0.0;
                    }
                    s.rate = if e.reversed { -1.0 } else { 1.0 };
                    s.reverse_sync = e.reversed;
                    s.looping = true;
                    s.playing = true;
                }
            }
        }
        if specific_child < weights.len() && specific_child >= n_slots {
            weights[specific_child] = w_specific;
        }
        self.nodes[i].weights = weights;
        // `m_fMostRelevantRateScale` sent to the loops' group (0x0059BBE0).
        let group = children.iter().flatten().find_map(|c| shape.nodes[*c].seq.as_ref().and_then(|o| o.group));
        if let Some(g) = group {
            self.group_rates[g] = rate_scale;
        }
        self.nodes[i].kind = KindState::Locomotion(Box::new(st));
    }

    fn collect_root_motion(&mut self, shape: &TreeShape, clips: &dyn Clips) {
        let mut travel = Vec3::ZERO;
        let mut yaw = 0.0;
        for (n, o) in self.nodes.iter().zip(&shape.nodes) {
            let (Some(s), Some(o)) = (n.seq.as_ref(), o.seq.as_ref()) else { continue };
            let Some((from, d)) = s.moved else { continue };
            if n.total <= ZERO_WEIGHT {
                continue;
            }
            let translate = o.root_bone.map(|x| x == RootBoneOption::Translate);
            let extract = o.root_rotation.iter().any(|x| *x == RootRotationOption::Extract);
            if !translate.iter().any(|t| *t) && !extract {
                continue;
            }
            let Some(c) = clips.find(&s.name) else { continue };
            let Some(j) = c.bones.iter().position(|b| *b == Some(0)) else { continue };
            let (dt, dq) = root_track_delta(&c, j, from, d, s.looping);
            // `ExtractRootMotion` (slot 117, 0x0058E780) zeroes the axes that are not
            // `RBA_Translate` in the mesh's `RotOrigin` frame (`SkeletalMesh.m_OriginTransform`),
            // not in the track's: the slots' [Translate, Translate, Default] keep the horizontal
            // travel. The extra turn of `mesh_rotation` (about Z) leaves Z where it is.
            let mut dt = shape.mesh_rotation * dt;
            for (k, on) in translate.iter().enumerate() {
                if !on {
                    dt[k] = 0.0;
                }
            }
            travel += dt * n.total;
            if extract {
                // The root's turn seen in the pawn's frame: where it takes the pawn's forward
                // axis (UE3 yaw: x forward, y right).
                let f = shape.mesh_rotation * (dq * (shape.mesh_rotation.inverse() * Vec3::X));
                yaw += f.y.atan2(f.x).to_degrees() * n.total;
            }
        }
        self.root_motion = (travel, yaw);
    }

    // ---- pose ----

    fn eval(&self, shape: &TreeShape, i: usize, clips: &dyn Clips, fill: Fill) -> Pose {
        let (o, n) = (&shape.nodes[i], &self.nodes[i]);
        match &o.kind {
            Kind::Sequence => self.sample(shape, i, clips, fill),
            Kind::Physics | Kind::List | Kind::Slot | Kind::Locomotion(_) => self.blend_children(shape, i, clips, fill),
            Kind::PerBone { mask, local_space } => {
                let source = self.child_pose(shape, i, 0, clips, fill);
                let w = n.weights.get(1).copied().unwrap_or(0.0);
                if w <= ZERO_WEIGHT {
                    return source;
                }
                let target = self.child_pose(shape, i, 1, clips, fill);
                pose::blend_per_bone(&shape.bones, &source, &target, mask, w, *local_space)
            }
            Kind::Additive => {
                let mut base = self.child_pose(shape, i, 0, clips, fill);
                let w = n.weights.get(1).copied().unwrap_or(0.0);
                if w > ZERO_WEIGHT && o.children.len() > 1 {
                    let delta = self.child_pose(shape, i, 1, clips, Fill::Zero);
                    pose::add_additive(&mut base, &delta, w);
                }
                base
            }
            Kind::Stack => {
                let mut out = self.child_pose(shape, i, 0, clips, fill);
                for k in 1..o.children.len() {
                    let w = n.weights[k];
                    let Some(c) = o.children[k] else { continue };
                    if w <= ZERO_WEIGHT {
                        continue;
                    }
                    let layer = self.eval(shape, c, clips, Fill::Keep(&out));
                    let w = w.min(1.0);
                    out = out.iter().zip(&layer).map(|(a, b)| pose::mix(*a, *b, w)).collect();
                }
                out
            }
            Kind::LookAt => match &n.kind {
                KindState::LookAt(l) => self.look_delta(shape, l, clips, fill),
                _ => self.fill_pose(shape, fill),
            },
            Kind::Pass => match o.children.iter().flatten().next() {
                Some(c) => self.eval(shape, *c, clips, fill),
                None => self.fill_pose(shape, fill),
            },
        }
    }

    fn fill_pose(&self, shape: &TreeShape, fill: Fill) -> Pose {
        match fill {
            Fill::Zero => pose::zero_pose(shape.bind.len()),
            Fill::Bind | Fill::Keep(_) => shape.bind.clone(),
        }
    }

    fn child_pose(&self, shape: &TreeShape, i: usize, k: usize, clips: &dyn Clips, fill: Fill) -> Pose {
        let o = &shape.nodes[i];
        match o.children.get(k).copied().flatten() {
            Some(c) => {
                let fill = if o.additive.get(k).copied().unwrap_or(false) { Fill::Zero } else { fill };
                self.eval(shape, c, clips, fill)
            }
            None => self.fill_pose(shape, fill),
        }
    }

    fn blend_children(&self, shape: &TreeShape, i: usize, clips: &dyn Clips, fill: Fill) -> Pose {
        let n = &self.nodes[i];
        let live: Vec<(usize, f32)> = n.weights.iter().enumerate().filter(|(_, w)| **w > ZERO_WEIGHT).map(|(k, w)| (k, *w)).collect();
        // One child at full weight passes through.
        if live.len() == 1 && live[0].1 >= 1.0 - ZERO_WEIGHT {
            return self.child_pose(shape, i, live[0].0, clips, fill);
        }
        let fallback = self.fill_pose(shape, fill);
        if live.is_empty() {
            return fallback;
        }
        let mut acc = Accumulator::new(shape.bind.len());
        for (k, w) in live {
            acc.add(&self.child_pose(shape, i, k, clips, fill), w);
        }
        acc.finish(&fallback)
    }

    /// A sequence node's pose: the clip at the node's time over the reference pose (the pose
    /// below with `m_bDontUseRefPoseOnNonDefinedBones`, identity in an additive branch), the
    /// root options applied.
    fn sample(&self, shape: &TreeShape, i: usize, clips: &dyn Clips, fill: Fill) -> Pose {
        let s = self.nodes[i].seq.as_ref().unwrap();
        let o = shape.nodes[i].seq.as_ref().unwrap();
        let mut out = match fill {
            Fill::Keep(p) if o.keep_undefined => p.to_vec(),
            Fill::Zero => pose::zero_pose(shape.bind.len()),
            _ => shape.bind.clone(),
        };
        let Some(c) = clips.find(&s.name) else { return out };
        for (j, b) in c.bones.iter().enumerate() {
            let Some(b) = *b else { continue };
            if b >= out.len() {
                continue;
            }
            let (q, p) = if s.looping { c.clip.sample(s.time, j) } else { c.clip.sample_held(s.time, j) };
            if let Some(q) = q {
                out[b].0 = q;
            }
            if let (Some(p), true) = (p, c.translate.get(j).copied().unwrap_or(false)) {
                out[b].1 = p;
            }
        }
        if !matches!(fill, Fill::Zero) && !out.is_empty() {
            let (rq, rt) = shape.bind[0];
            for k in 0..3 {
                if o.root_bone[k] != RootBoneOption::Default {
                    out[0].1[k] = rt[k];
                }
            }
            out[0].0 = discard_rotation(out[0].0, rq, o.root_rotation);
        }
        out
    }

    /// The look-at node's additive delta: each grid's cells combined by weight
    /// (`pose::accumulate_additive`), the torso grid then the head grid, each at the look-at's
    /// weight (`npc_anim`'s `apply_additive` per grid).
    fn look_delta(&self, shape: &TreeShape, l: &LookState, clips: &dyn Clips, fill: Fill) -> Pose {
        let n = shape.bind.len();
        let mut out = self.fill_pose(shape, fill);
        if l.weight <= ZERO_WEIGHT {
            return out;
        }
        for grid in [9..18, 0..9] {
            let mut acc = pose::zero_pose(n);
            for k in grid {
                let (Some(name), Some(w)) = (l.names.get(k), l.cells.get(k)) else { continue };
                if *w < 1e-3 {
                    continue;
                }
                let Some(c) = clips.find(name) else { continue };
                let mut delta = pose::zero_pose(n);
                for (j, b) in c.bones.iter().enumerate() {
                    let Some(b) = *b else { continue };
                    if b >= n {
                        continue;
                    }
                    let (q, p) = c.clip.sample_held(0.0, j);
                    if let Some(q) = q {
                        delta[b].0 = q;
                    }
                    if let (Some(p), true) = (p, c.translate.get(j).copied().unwrap_or(false)) {
                        delta[b].1 = p;
                    }
                }
                pose::accumulate_additive(&mut acc, &delta, *w);
            }
            pose::add_additive(&mut out, &acc, l.weight);
        }
        out
    }
}

/// What a node's undefined bones start from.
#[derive(Clone, Copy)]
enum Fill<'a> {
    /// The reference pose.
    Bind,
    /// The identity delta (under a `bIsAdditive` link).
    Zero,
    /// The pose below a stack channel (for sequences with `m_bDontUseRefPoseOnNonDefinedBones`).
    Keep(&'a Pose),
}

/// A cooked scalar from a node's raw properties (`CurrentTime`, `Child2Weight`, ...).
fn raw_f32(raw: &[(String, String)], key: &str) -> Option<f32> {
    raw.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.parse().ok())
}

/// A node's cooked `TickArrayIndex`.
fn raw_usize(raw: &[(String, String)], key: &str) -> Option<usize> {
    raw.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.parse().ok())
}

/// `AnimNodeBlendList::TickAnim`: each child weight moves `dt / BlendTimeToGo` of the way to its
/// target (linear in time), all of it on the last step.
fn blend_weights(n: &mut NodeState, dt: f32) {
    if n.left <= 0.0 {
        return;
    }
    if n.left <= dt {
        n.weights.clone_from(&n.target);
        n.left = 0.0;
        return;
    }
    let k = dt / n.left;
    for (w, t) in n.weights.iter_mut().zip(&n.target) {
        *w += (*t - *w) * k;
    }
    n.left -= dt;
}

/// The tick order of UE3's `AnimTickArray`: the root, then, depth first, each child once the
/// last of its parents has reached it.
fn tick_order(nodes: &[NodeShape], parents: &[Vec<usize>], root: Option<usize>) -> Vec<usize> {
    let mut order = Vec::with_capacity(nodes.len());
    let mut seen = vec![0usize; nodes.len()];
    fn walk(i: usize, nodes: &[NodeShape], parents: &[Vec<usize>], seen: &mut [usize], order: &mut Vec<usize>) {
        let mut visited: Vec<usize> = Vec::new();
        for c in nodes[i].children.iter().flatten() {
            if visited.contains(c) {
                continue;
            }
            visited.push(*c);
            seen[*c] += 1;
            if seen[*c] == parents[*c].len() {
                order.push(*c);
                walk(*c, nodes, parents, seen, order);
            }
        }
    }
    if let Some(r) = root {
        order.push(r);
        walk(r, nodes, parents, &mut seen, &mut order);
    }
    order
}

/// The root bone's translation and turn between two times of a clip (`from` plus the signed
/// move `d`), through the loop's end as many times as the move wraps.
fn root_track_delta(c: &ClipRef, j: usize, from: f32, d: f32, looping: bool) -> (Vec3, Quat) {
    let len = c.clip.duration;
    let at = |t: f32| {
        let (q, p) = c.clip.sample_held(t.clamp(0.0, len.max(0.0)), j);
        (q.unwrap_or(Quat::IDENTITY), p.unwrap_or(Vec3::ZERO))
    };
    let (q0, p0) = at(from);
    if !looping || len <= 0.0 {
        let (q1, p1) = at(from + d);
        return (p1 - p0, q1 * q0.inverse());
    }
    let to = from + d;
    let wraps = (to / len).floor();
    let end = to - wraps * len;
    let (qs, ps) = at(0.0);
    let (qe, pe) = at(len);
    let (q1, p1) = at(end);
    // Each full lap adds the clip's whole travel.
    let lap = pe - ps;
    let travel = p1 - p0 + lap * wraps;
    let lap_turn = qe * qs.inverse();
    let mut turn = q1 * q0.inverse();
    let mut k = wraps;
    while k >= 1.0 {
        turn = lap_turn * turn;
        k -= 1.0;
    }
    while k <= -1.0 {
        turn = lap_turn.inverse() * turn;
        k += 1.0;
    }
    (travel, turn)
}

/// The root rotation with the discarded / extracted axes (yaw, pitch, roll) at the reference
/// rotation's: all three take the reference rotation; single axes go through the rotator
/// decomposition (`viewmodel::frame_rotator` / `rotator_quat`, UE3's conventions).
fn discard_rotation(q: Quat, reference: Quat, options: [RootRotationOption; 3]) -> Quat {
    let off = options.map(|o| o != RootRotationOption::Default);
    if off == [false; 3] {
        return q;
    }
    if off == [true; 3] {
        return reference;
    }
    let rot = |q: Quat| frame_rotator(q * Vec3::X, q * Vec3::Y, q * Vec3::Z);
    let (p, y, r) = rot(q);
    let (rp, ry, rr) = rot(reference);
    rotator_quat(if off[1] { rp } else { p }, if off[0] { ry } else { y }, if off[2] { rr } else { r })
}

/// A UE3 rotator (pitch, yaw, roll in degrees) as a rotation (`viewmodel::rotator_quat`): roll
/// about X, then pitch, then yaw.
fn rotator_quat(pitch: f32, yaw: f32, roll: f32) -> Quat {
    Quat::from_rotation_z(yaw.to_radians()) * Quat::from_rotation_y(-pitch.to_radians()) * Quat::from_rotation_x(-roll.to_radians())
}

/// The rotator of a frame given by its forward, right and up axes (`viewmodel::frame_rotator`,
/// the matrix to rotator conversion of 0x0041D720).
fn frame_rotator(fwd: Vec3, right: Vec3, up: Vec3) -> (f32, f32, f32) {
    let pitch = fwd.z.atan2(fwd.truncate().length());
    let yaw = fwd.y.atan2(fwd.x);
    let y0 = Vec3::new(-yaw.sin(), yaw.cos(), 0.0);
    let roll = up.dot(y0).atan2(right.dot(y0));
    (pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees())
}
