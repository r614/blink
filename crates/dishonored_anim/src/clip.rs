//! Baked clips and the AnimSets that bind them to a skeleton.
//!
//! [`BakedClip`] is a decoded Sony Edge clip (`dishonored_assets::edge_anim::EdgeClip::bake`,
//! research/dishonored/animation/EDGE_ANIM.md): per frame and Edge joint, the local rotation and
//! translation. It lives here so the evaluator (and the simulation after it) samples clips without
//! the package decoders; `dishonored_assets::edge_anim` re-exports it.
//!
//! [`AnimSets`] is UE3's `USkeletalMeshComponent.AnimSets` with each set's mesh linkup
//! (`FAnimSetMeshLinkup`):
//! - a sequence is found from the last set to the first (`FindAnimSequence`, ANIMATION.md
//!   section 1: `Npc_Aristo_Empty_as` overrides the guard's `Empty_WalkN`);
//! - Edge joint `j` of a set is the set's `j`-th track in the bone order of the skeleton it was
//!   compressed against (`AnimLibrary::edge_order`, EDGE_ANIM.md), bound to the animated
//!   skeleton's bone of that name;
//! - per bone, whether the clip's translation is used: UE3 keeps the reference translation for
//!   every bone but the root when the set has `bAnimRotationOnly` and the bone is not in
//!   `UseTranslationBoneNames`, and for the bones of `ForceMeshTranslationBoneNames`
//!   (`FAnimSetMeshLinkup::BuildLinkup`, `UAnimSequence::GetAnimationPose`; stock UE3, LIKELY).
//!   `Engine.Default__AnimSet` serializes no property (`bAnimRotationOnly` false) and none of the
//!   Boyle NPC sets sets one, so their clips' translations are all used. CONFIRMED by the
//!   `stealth/detect_1300` capture: the face bones' positions match to 0.032 uu mean (0.064 max)
//!   with every translation, 0.102 (1.020) with the runtime's root-and-children rule
//!   (ANIMATION.md section 6).

use dishonored_ir::BoneAsset;
use glam::{Quat, Vec3};
use std::collections::BTreeMap;

/// A clip decoded to per-frame local joint transforms (`None` = joint not driven).
#[derive(Debug, Clone)]
pub struct BakedClip {
    pub num_frames: usize,
    pub sample_rate: f32,
    pub duration: f32,
    /// [frame][joint]
    pub rotations: Vec<Vec<Option<Quat>>>,
    pub translations: Vec<Vec<Option<Vec3>>>,
}

impl BakedClip {
    /// Sample at `time` seconds (looping), blending neighbouring frames.
    pub fn sample(&self, time: f32, joint: usize) -> (Option<Quat>, Option<Vec3>) {
        let len = (self.num_frames.max(1) - 1).max(1) as f32;
        self.sample_frame((time * self.sample_rate).rem_euclid(len), joint)
    }

    /// Sample at `time` seconds without looping: past the end holds the last frame (a door's
    /// open pose, a matinee's final pose). Looping sampling wrapped the clip's end back to its
    /// first frame, so a door that finished opening snapped shut.
    pub fn sample_held(&self, time: f32, joint: usize) -> (Option<Quat>, Option<Vec3>) {
        let len = (self.num_frames.max(1) - 1) as f32;
        self.sample_frame((time * self.sample_rate).clamp(0.0, len), joint)
    }

    fn sample_frame(&self, fpos: f32, joint: usize) -> (Option<Quat>, Option<Vec3>) {
        if self.num_frames == 0 || self.rotations.first().is_none_or(|r| joint >= r.len()) {
            return (None, None);
        }
        let f0 = fpos.floor() as usize;
        let f1 = (f0 + 1).min(self.num_frames - 1);
        let t = fpos - f0 as f32;
        let r = match (self.rotations[f0][joint], self.rotations[f1][joint]) {
            (Some(a), Some(mut b)) => {
                if a.dot(b) < 0.0 {
                    b = -b;
                }
                Some(a.lerp(b, t).normalize())
            }
            (a, b) => a.or(b),
        };
        let p = match (self.translations[f0][joint], self.translations[f1][joint]) {
            (Some(a), Some(b)) => Some(a.lerp(b, t)),
            (a, b) => a.or(b),
        };
        (r, p)
    }

    /// The number of Edge joints of the clip.
    pub fn joints(&self) -> usize {
        self.rotations.first().map_or(0, Vec::len)
    }
}

/// A sequence as the mesh's AnimSets resolve it: the clip and its linkup to the skeleton.
#[derive(Clone, Copy)]
pub struct ClipRef<'a> {
    pub clip: &'a BakedClip,
    /// Per Edge joint, the skeleton bone it drives (`BoneToTrackTable`, inverted).
    pub bones: &'a [Option<usize>],
    /// Per Edge joint, whether the clip's translation is used (`BoneUseAnimTranslation` and not
    /// `ForceUseMeshTranslation`).
    pub translate: &'a [bool],
}

/// Clip access for the evaluator: maps an `AnimSeqName` to its clip and linkup. [`AnimSets`] is
/// one implementation; the runtime's animation bank another.
pub trait Clips {
    fn find(&self, name: &str) -> Option<ClipRef<'_>>;
}

/// The translation flags of one `AnimSet` (all false / empty for the Boyle NPC sets).
#[derive(Debug, Clone, Default)]
pub struct SetFlags {
    /// `bAnimRotationOnly`.
    pub rotation_only: bool,
    /// `UseTranslationBoneNames`.
    pub use_translation: Vec<String>,
    /// `ForceMeshTranslationBoneNames`.
    pub force_mesh_translation: Vec<String>,
}

struct BoundSet {
    bones: Vec<Option<usize>>,
    translate: Vec<bool>,
    /// Lowercase sequence name -> clip (FName comparison ignores case).
    clips: BTreeMap<String, BakedClip>,
}

/// AnimSets bound to one skeleton, searched from the last to the first.
#[derive(Default)]
pub struct AnimSets {
    sets: Vec<BoundSet>,
}

impl AnimSets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a set (later sets win a name): its tracks in Edge joint order, its baked clips and
    /// its translation flags, bound to `skeleton` by bone name.
    pub fn push(&mut self, tracks: &[String], clips: impl IntoIterator<Item = (String, BakedClip)>, flags: &SetFlags, skeleton: &[BoneAsset]) {
        let bones: Vec<Option<usize>> = tracks.iter().map(|t| skeleton.iter().position(|b| b.name.eq_ignore_ascii_case(t))).collect();
        let listed = |names: &[String], t: &str| names.iter().any(|n| n.eq_ignore_ascii_case(t));
        let translate = tracks
            .iter()
            .zip(&bones)
            .map(|(t, b)| {
                let root = *b == Some(0);
                let anim = !flags.rotation_only || root || listed(&flags.use_translation, t);
                anim && !listed(&flags.force_mesh_translation, t)
            })
            .collect();
        let clips = clips.into_iter().map(|(n, c)| (n.to_ascii_lowercase(), c)).collect();
        self.sets.push(BoundSet { bones, translate, clips });
    }

    /// The number of sets.
    pub fn len(&self) -> usize {
        self.sets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }
}

impl Clips for AnimSets {
    fn find(&self, name: &str) -> Option<ClipRef<'_>> {
        if name.is_empty() || name.eq_ignore_ascii_case("None") {
            return None;
        }
        let key = name.to_ascii_lowercase();
        self.sets.iter().rev().find_map(|s| s.clips.get(&key).map(|clip| ClipRef { clip, bones: &s.bones, translate: &s.translate }))
    }
}
