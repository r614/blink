//! Local poses and the blend operations of the AnimTree nodes.
//!
//! A pose is the local (parent-relative) rotation and translation of every skeleton bone, UE3's
//! `FBoneAtom` array without scale. The operations are the ones the runtime already decoded and
//! verified (copied with their citations; the runtime keeps its own until it is wired to this
//! crate):
//! - weighted blends accumulate sign-aligned quaternions and translations by weight and
//!   normalise (UE3 `UAnimNodeBlendBase::GetChildAtoms`; `npc_anim::Acc`);
//! - `AnimNodeBlendPerBone` in component space (`viewmodel::blend_per_bone`, `to_component`) or,
//!   with `bForceLocalSpaceBlend`, per local bone;
//! - additive deltas turn a bone by `slerp(identity, delta, w)` before its own rotation and move
//!   it by `w` times their offset (`viewmodel::add_additive`; `npc_anim::apply_additive`).

use dishonored_ir::BoneAsset;
use glam::{Quat, Vec3, Vec4};

/// Local bone transforms (rotation, translation).
pub type Pose = Vec<(Quat, Vec3)>;

/// Parent of each bone (`None` for a root), read as `bind_globals` reads `BoneAsset::parent`
/// (`viewmodel::parents`).
pub fn parents(bones: &[BoneAsset]) -> Vec<Option<usize>> {
    bones.iter().enumerate().map(|(i, b)| (i != 0 && b.parent >= 0 && (b.parent as usize) < i).then_some(b.parent as usize)).collect()
}

/// The reference (bind) pose.
pub fn bind_pose(bones: &[BoneAsset]) -> Pose {
    bones.iter().map(|b| (b.rotation, b.translation)).collect()
}

/// The identity pose additive deltas start from: no turn, no offset.
pub fn zero_pose(n: usize) -> Pose {
    vec![(Quat::IDENTITY, Vec3::ZERO); n]
}

pub fn compose(parent: (Quat, Vec3), local: (Quat, Vec3)) -> (Quat, Vec3) {
    ((parent.0 * local.0).normalize(), parent.1 + parent.0 * local.1)
}

pub fn relative(parent: (Quat, Vec3), comp: (Quat, Vec3)) -> (Quat, Vec3) {
    let inv = parent.0.inverse();
    ((inv * comp.0).normalize(), inv * (comp.1 - parent.1))
}

/// Two transforms blended by `w` (normalised lerp on the near hemisphere, linear translation).
pub fn mix(a: (Quat, Vec3), b: (Quat, Vec3), w: f32) -> (Quat, Vec3) {
    let q = if a.0.dot(b.0) < 0.0 { -b.0 } else { b.0 };
    (a.0.lerp(q, w).normalize(), a.1.lerp(b.1, w))
}

/// Component-space (mesh-space) transforms of a local pose.
pub fn to_component(parents: &[Option<usize>], local: &[(Quat, Vec3)]) -> Pose {
    let mut c: Pose = Vec::with_capacity(local.len());
    for (i, l) in local.iter().enumerate() {
        let v = match parents.get(i).copied().flatten() {
            Some(p) => compose(c[p], *l),
            None => *l,
        };
        c.push(v);
    }
    c
}

/// `Child2PerBoneWeight` of an `AnimNodeBlendPerBone` (`BuildWeightList`): 1 at the branch
/// bones (`BranchStartBoneName`) and everything below them, 0 elsewhere; Arkane's
/// `m_bExcludeBranchBones` gives the branch bones themselves 0 (`FaceFilter`: the face slot
/// drives the bones below `head_jnt`, not `head_jnt`; ANIMATION.md section 2).
pub fn branch_weights(bones: &[BoneAsset], branches: &[String], exclude_branch: bool) -> Vec<f32> {
    let par = parents(bones);
    let mut under = vec![false; bones.len()];
    let mut w = vec![0.0; bones.len()];
    for (i, b) in bones.iter().enumerate() {
        let branch = branches.iter().any(|n| b.name.eq_ignore_ascii_case(n));
        let below = par[i].is_some_and(|p| under[p]);
        under[i] = branch || below;
        w[i] = if below || (branch && !exclude_branch) { 1.0 } else { 0.0 };
    }
    w
}

/// `AnimNodeBlendPerBone`: each bone blends from the source to the target by its per-bone
/// weight times `weight` (`Child2Weight`). In component space (the default: the nodes cook
/// `LocalToCompReqBones`) a weighted bone takes the blend of the two children's component-space
/// transforms, converted back to local space under the result's parent, so a filtered branch
/// keeps the pose its own sequence gives it relative to the root (`viewmodel::blend_per_bone`);
/// with `bForceLocalSpaceBlend` the local transforms blend.
pub fn blend_per_bone(parents: &[Option<usize>], source: &[(Quat, Vec3)], target: &[(Quat, Vec3)], weights: &[f32], weight: f32, local_space: bool) -> Pose {
    if weight <= 0.0 || source.len() != target.len() {
        return source.to_vec();
    }
    let w = weight.min(1.0);
    if local_space {
        return source.iter().zip(target).zip(weights).map(|((s, t), k)| if *k * w > 0.0 { mix(*s, *t, (*k * w).min(1.0)) } else { *s }).collect();
    }
    let cs = to_component(parents, source);
    let ct = to_component(parents, target);
    let mut out = source.to_vec();
    let mut oc: Pose = Vec::with_capacity(source.len());
    for (i, src) in source.iter().enumerate() {
        let parent = parents.get(i).copied().flatten().map(|p| oc[p]);
        let k = weights.get(i).copied().unwrap_or(0.0) * w;
        if k > 0.0 {
            let c = mix(cs[i], ct[i], k.min(1.0));
            out[i] = parent.map_or(c, |p| relative(p, c));
            oc.push(c);
        } else {
            oc.push(parent.map_or(*src, |p| compose(p, *src)));
        }
    }
    out
}

/// Weighted accumulation of child poses (UE3 `GetChildAtoms`: rotations summed on the first
/// child's hemisphere and normalised, translations summed by weight).
pub struct Accumulator {
    acc: Vec<(Vec4, Vec3)>,
    any: bool,
}

impl Accumulator {
    pub fn new(n: usize) -> Self {
        Accumulator { acc: vec![(Vec4::ZERO, Vec3::ZERO); n], any: false }
    }

    pub fn add(&mut self, pose: &[(Quat, Vec3)], w: f32) {
        for (a, (q, t)) in self.acc.iter_mut().zip(pose) {
            let mut v = Vec4::from(*q);
            if self.any && a.0.dot(v) < 0.0 {
                v = -v;
            }
            a.0 += v * w;
            a.1 += *t * w;
        }
        self.any = true;
    }

    /// The blended pose (`fallback` where nothing was added).
    pub fn finish(self, fallback: &[(Quat, Vec3)]) -> Pose {
        if !self.any {
            return fallback.to_vec();
        }
        self.acc.into_iter().zip(fallback).map(|((q, t), f)| if q.length_squared() > 1e-12 { (Quat::from_vec4(q.normalize()), t) } else { *f }).collect()
    }
}

/// `AnimNodeAdditiveBlending`: the additive child's delta (cooked as target * inverse(reference),
/// identity and zero where nothing moves) added to the base at weight `w`.
pub fn add_additive(base: &mut [(Quat, Vec3)], delta: &[(Quat, Vec3)], w: f32) {
    if w <= 0.0 {
        return;
    }
    let w = w.min(1.0);
    for (p, (dq, dt)) in base.iter_mut().zip(delta) {
        p.0 = (Quat::IDENTITY.slerp(*dq, w) * p.0).normalize();
        p.1 += *dt * w;
    }
}

/// Weighted combination of additive deltas inside one look-at grid (`npc_anim::
/// accumulate_additive`): each delta scaled from identity by its weight, then composed.
pub fn accumulate_additive(acc: &mut Pose, delta: &[(Quat, Vec3)], w: f32) {
    for (a, (dq, dt)) in acc.iter_mut().zip(delta) {
        a.0 = (Quat::IDENTITY.slerp(*dq, w) * a.0).normalize();
        a.1 += *dt * w;
    }
}
