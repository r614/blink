//! UE3 AnimTree asset (node graph IR): the cooked animation blending tree
//! (`AnimTree` export, `Animations` property -> root node, `Children` struct arrays,
//! `SkelControlLists` -> skeletal control chains). See research/dishonored/ANIMATION.md.

use serde::{Deserialize, Serialize};

/// The cooked `AnimTree` asset: a node graph with a root node, all reachable nodes, and skeletal
/// control chains applied to specific bones. A shared child (reachable from multiple parents)
/// appears once (deduped by export index) and is indexed from each parent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnimTreeAsset {
    /// Object path of the `AnimTree` export (e.g. `Npc_Common_at.NPC_Human_at`).
    pub name: String,
    /// Index into `nodes` of the root blend node, or `None` if it could not be read.
    pub root: Option<usize>,
    /// All reachable nodes (root first, then depth-first by child index).
    pub nodes: Vec<AnimNode>,
    /// Skeletal control chains (bone name -> list of (class, name) controls).
    pub skel_controls: Vec<SkelControlEntry>,
}

/// One node of the tree: a blend, slot, locomotion, or other node class with optional children.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnimNode {
    /// `NodeName` property if present, else the export's final path component (the name after `.`).
    pub name: String,
    /// Object path in `Package:ObjectPath` format (what `pkg.obj_path` returns).
    pub export: String,
    /// UE3 class name (e.g., `AnimNodeBlendByPhysics`, `ArkAnimNodeLocomotion`).
    pub class: String,
    /// Children of this node (from `Children` struct array), indexed into `nodes[]`.
    pub children: Vec<AnimChild>,
    /// Parsed node-specific properties (sequence names, blend branches, locomotion rates, etc.).
    pub props: NodeProps,
    /// Every property as (name, printed value string) for debugging and future fields.
    pub raw: Vec<(String, String)>,
}

/// A child link: the `AnimBlendChild` struct with name, animation ref, weight, and flags.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnimChild {
    /// `Name` field of the `AnimBlendChild` struct.
    pub name: String,
    /// Index into the parent node's `nodes[]` array, or `None` if the child is missing/None.
    pub node: Option<usize>,
    /// `Weight` (usually 1.0, may be < 1 for weighted blends).
    pub weight: f32,
    /// `bMirrorSkeleton` flag.
    pub mirror: bool,
    /// `bIsAdditive` flag.
    pub additive: bool,
}

/// A skeletal control chain: a bone and its applied controls (read from `ControlHead` ->.
/// `NextControl` linked list).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkelControlEntry {
    /// Bone name (from the entry's `BoneName`).
    pub bone: String,
    /// List of (class, name) for each control in the chain.
    pub controls: Vec<(String, String)>,
}

/// Node-specific properties: parsed fields that drive evaluation. Unknown classes use `Other`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeProps {
    /// `AnimNodeSequence` and the `DishonoredAnimNodeSeq*` subclasses: one sequence player.
    Sequence {
        /// `AnimSeqName`.
        seq: String,
        /// `Rate` (1 when the node does not set it).
        rate: f32,
        /// `bLooping`.
        looping: bool,
        /// `bPlaying`.
        playing: bool,
        /// `bNoNotifies`.
        no_notifies: bool,
        /// `SynchGroupName` (`LocoGroup` for the locomotion loops).
        sync_group: String,
    },
    /// `AnimNodeBlendPerBone` / `DisAnimNodeBlendPerBone`: the target child from the branch
    /// bones down.
    BlendPerBone {
        /// `BranchStartBoneName`.
        branches: Vec<String>,
        /// `bForceLocalSpaceBlend`.
        local_space: bool,
        /// `m_bExcludeBranchBones`: the branch bones themselves stay with the source.
        exclude_branch: bool,
    },
    /// `DisAnimNodeSlot` / `AnimNodeSlot`: the source child under channels that play requested
    /// sequences.
    Slot {
        /// `bEarlyAnimEndNotify`.
        early_anim_end: bool,
    },
    /// `AnimNodeAdditiveBlending`: additive blending (base + additive).
    AdditiveBlending,
    /// `AnimNodeBlendByPhysics`: physics-based blending (RigidBody/Ragdoll/etc).
    BlendByPhysics,
    /// `AnimNodeBlendList`: list of children, selected by index (not commonly used in retail).
    BlendList,
    /// `ArkAnimNodeLookAt`: the additive aim grids (`Anims[].AnimName`: nine head, nine torso).
    AimOffset {
        grids: Vec<String>,
    },
    /// `DishonoredAnimNodeStatePicker` / `DishonoredAnimNodeTreeRef_*`: the player's state
    /// trees, instantiated from other AnimTree assets at run time (`m_PossibleTemplates[]`:
    /// `m_AnimTree_Name`, `m_pAnimTree_Template`). The templates are not walked here; a tree
    /// evaluator loads them by path.
    TreeRef {
        /// (template name, object path of the AnimTree asset).
        templates: Vec<(String, String)>,
    },
    /// `ArkAnimNodeLocomotion` (ANIMATION.md section 3), the node's value over the class
    /// default (`Engine.Default__ArkAnimNodeLocomotion`: 2000, 100, 0.1, 0.5).
    Locomotion {
        /// `m_fVelocityMaxBlendSpeed`.
        rate: f32,
        /// `m_fVelocityMaxDif`.
        range: f32,
        /// `m_fVelocityMinBlendSpeedRatio`.
        min_ratio: f32,
        /// `m_fModifierBlendDuration`.
        modifier_blend: f32,
    },
    /// An `ArkAnimNodeStack` or other unmodelled node: keep raw properties.
    Other,
}

impl Default for NodeProps {
    fn default() -> Self {
        NodeProps::Other
    }
}
