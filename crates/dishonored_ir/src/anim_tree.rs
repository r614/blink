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
    /// `AnimNodeSequence`: a single sequence with playback state.
    Sequence {
        seq: String,           // Sequence name
        rate: f32,              // PlayRate
        looping: bool,          // bLooping
        playing: bool,          // bPlaying
        no_notifies: bool,      // bDontUseRefPoseOnNonDefinedBones / similar
        sync_group: String,     // SyncGroup
    },
    /// `AnimNodeBlendPerBone`: blend by branch bone (exclude option).
    BlendPerBone {
        branches: Vec<String>,  // Bone names for per-bone blending
        local_space: bool,      // bLocalSpaceBlend
        exclude_branch: bool,   // bExcludeBranchBones
    },
    /// `DisAnimNodeSlot` / `AnimNodeSlot`: a slot that plays child animations in channels.
    Slot {
        early_anim_end: bool,
    },
    /// `AnimNodeAdditiveBlending`: additive blending (base + additive).
    AdditiveBlending,
    /// `AnimNodeBlendByPhysics`: physics-based blending (RigidBody/Ragdoll/etc).
    BlendByPhysics,
    /// `AnimNodeBlendList`: list of children, selected by index (not commonly used in retail).
    BlendList,
    /// `ArkAnimNodeLookAt`: look-at aiming (AimOffset with aim grids).
    AimOffset {
        grids: Vec<String>,  // Grid sequence names if present
    },
    /// `ArkAnimNodeLocomotion`: locomotion with speed modes and foot placement.
    Locomotion {
        rate: f32,           // PlayRate (from `m_fPlayRate` or class default 2000)
        range: f32,          // LegacyPlayRateRange (from `m_fPlayRateRange` or 100)
        min_ratio: f32,      // LegacyMinPlayRateRatio (from `m_fMinPlayRateRatio` or 0.1)
        modifier_blend: f32, // ModifierBlendTime (from `m_fModifierBlendTime` or 0)
    },
    /// An `ArkAnimNodeStack` or other unmodelled node: keep raw properties.
    Other,
}

impl Default for NodeProps {
    fn default() -> Self {
        NodeProps::Other
    }
}
