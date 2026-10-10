//! Limbs cut off (`DishonoredNotify_SeverLimb`, 0x00A966D0 -> the deferred gore request run by
//! 0x00B6CC10): the head chop finishers' slaves cut at `neck_jnt`, the named targets' dramatic
//! deaths cut an arm or the head.
//!
//! The cut (0x00B6CC10) takes a limb whose bone the mesh and its physics asset know, unless it or
//! a limb above it is already cut; it plays the limb's `DisSeveredLimbInfo` particle effects
//! "when severed" (with `m_bUseBlood`), and unless the notify only hides the limb
//! (`m_bHideLimb`), spawns the limb as a piece of its own (0x00B6B500) pushed by the impulse:
//! the severed bone's X, Y or Z axis (`m_ImpulseMode`, the rows of its matrix) times
//! `m_fImpulseStrength`. The body keeps animating without the limb. A camera within the limb
//! info's `m_CameraEffectEmitterInfoWhenSevered` distance gets its lens effect.
//!
//! Here, presentation only like the ragdoll: the body draws the cut subtree collapsed onto the cut
//! (its bones scaled to nothing at the severed bone's origin, as UE3's hidden bones), and the
//! severed piece is a second instance of the pawn's model showing only that subtree, posed as at
//! the cut, carried by a rigid body (`dropped_weapons::Dropped`: four point masses on the
//! level's floor and walls) sized by the subtree's physics bodies. The piece's mass is not decoded
//! (the spawner is not decompiled): the impulse is taken as its velocity change (LIKELY).

use crate::characters::{AccessoryJoint, CharacterLibrary, Joint, to_bevy};
use crate::dropped_weapons::Dropped;
use bevy::prelude::*;
use std::collections::HashMap;

/// A cut this frame (Dishonored space).
pub struct SeverRequest {
    pub pawn: u32,
    pub archetype: usize,
    /// The pawn's root (character mesh frame) in the world.
    pub root: glam::Mat4,
    /// The severed bone's world frame.
    pub bone: glam::Mat4,
    /// Pawn-space joint frames of the severed piece, per part and bone (the bones outside the
    /// subtree collapsed onto the cut), and per accessory.
    pub joints: Vec<Vec<glam::Mat4>>,
    pub accessories: Vec<glam::Mat4>,
    /// The rigid body's box in the severed bone's frame.
    pub lo: glam::Vec3,
    pub hi: glam::Vec3,
    pub velocity: glam::Vec3,
    /// The limb info's lens effect for a camera close by (class, distance, probability).
    pub lens: Option<dishonored_ir::npc_anim::LensOnContact>,
}

#[derive(Resource, Default)]
pub struct SeveredLimbs {
    /// Per pawn, the bones cut (subtree roots), until it lives again.
    pub severed: HashMap<u32, Vec<String>>,
    pub pending: Vec<SeverRequest>,
    live: Vec<Limb>,
    lens: HashMap<String, Option<dishonored_ir::LensEffectDef>>,
    content: Option<Option<dishonored_assets::Content>>,
    seed: u64,
}

struct Limb {
    pawn: u32,
    body: Dropped,
    bone0: glam::Mat4,
    root0: glam::Mat4,
    entity: Entity,
}

/// A severed piece's root: its frozen joint frames.
#[derive(Component)]
pub struct SeveredPiece {
    joints: Vec<Vec<glam::Mat4>>,
    accessories: Vec<glam::Mat4>,
    posed: bool,
}

/// Most severed pieces kept (the oldest goes first).
const MAX_PIECES: usize = 32;

impl SeveredLimbs {
    /// Whether `bone` of `pawn`, or a bone above it (`ancestors`, names), is already cut.
    pub fn is_cut(&self, pawn: u32, chain: &[&str]) -> bool {
        self.severed.get(&pawn).is_some_and(|v| v.iter().any(|b| chain.iter().any(|c| c.eq_ignore_ascii_case(b))))
    }
}

/// Per bone of a skeleton, the index of the cut subtree root it belongs to, if any (bones are
/// ordered parents first).
pub fn cut_roots(bones: &[dishonored_ir::BoneAsset], cut: &[String]) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = vec![None; bones.len()];
    for (i, b) in bones.iter().enumerate() {
        out[i] = if cut.iter().any(|c| c.eq_ignore_ascii_case(&b.name)) {
            Some(i)
        } else if i > 0 && b.parent >= 0 && (b.parent as usize) < i {
            out[b.parent as usize]
        } else {
            None
        };
    }
    out
}

/// A joint frame collapsed onto `at` (scaled to nothing there).
pub fn collapsed(at: glam::Vec3) -> glam::Mat4 {
    glam::Mat4::from_translation(at) * glam::Mat4::from_scale(glam::Vec3::ZERO)
}

/// The Bevy transform of a pawn-space joint frame; a collapsed one (no scale left, whose rotation
/// a decomposition cannot recover) keeps its point at a vanishing scale.
pub fn joint_transform(m: glam::Mat4) -> Transform {
    if m.x_axis.truncate().length_squared() < 1e-12 {
        let at = to_bevy(glam::Mat4::from_translation(m.w_axis.truncate())).w_axis.truncate();
        return Transform { translation: at, rotation: Quat::IDENTITY, scale: Vec3::splat(1e-5) };
    }
    Transform::from_matrix(to_bevy(m))
}

/// Spawn this frame's pieces, run them, and remove those whose pawn lives again or is gone.
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    limbs: Option<ResMut<SeveredLimbs>>,
    lib: Option<Res<CharacterLibrary>>,
    session: Res<crate::session::Session>,
    view: Res<crate::session::SessionView>,
    time: Res<Time>,
    cams: Query<&GlobalTransform, With<crate::players::GameCamera>>,
    mut lens: Option<ResMut<crate::lens_effects::LensEffects>>,
    mut roots: Query<&mut Transform, With<SeveredPiece>>,
) {
    let (Some(mut limbs), Some(lib)) = (limbs, lib) else { return };
    let mv = &session.content.sim.gameplay.movement;
    let world = &session.content.collision;
    let cam = cams.iter().next().map(|c| {
        let t = c.translation();
        glam::Vec3::new(t.x, t.z, t.y) / crate::coords::UU_TO_BEVY
    });
    for r in std::mem::take(&mut limbs.pending) {
        let origin = r.bone.w_axis.truncate();
        // The lens effect of a camera close by.
        if let (Some(l), Some(c), Some(lens)) = (r.lens.as_ref(), cam, lens.as_mut()) {
            limbs.seed = limbs.seed.wrapping_add(r.pawn as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut seed = limbs.seed;
            if c.distance(origin) <= l.distance && l.probability >= crate::present_rules::next_rand(&mut seed) {
                if limbs.content.is_none() {
                    limbs.content = Some(dishonored_assets::Content::locate().ok());
                }
                let def = match (limbs.lens.get(&l.class), limbs.content.as_ref().and_then(|c| c.as_ref())) {
                    (Some(d), _) => d.clone(),
                    (None, Some(content)) => {
                        let d = dishonored_assets::characters::lens_effect_class(content, &l.class);
                        limbs.lens.insert(l.class.clone(), d.clone());
                        d
                    }
                    (None, None) => None,
                };
                if let Some(d) = def {
                    lens.local.push(d);
                }
            }
        }
        let floor_at = |x: f32, y: f32| -> f32 {
            use dishonored_world::WorldQuery;
            let p = origin + glam::Vec3::new(x, y, 0.0);
            world.raycast(p + glam::Vec3::Z * 60.0, -glam::Vec3::Z, 600.0, dishonored_world::channel::NON_PAWN).map(|h| h.point.z - origin.z).unwrap_or(-540.0)
        };
        let reach = crate::ragdoll::WALL_REACH;
        let q = dishonored_world::geom::Aabb { min: origin - glam::Vec3::new(reach, reach, 600.0), max: origin + glam::Vec3::new(reach, reach, 200.0) };
        let mut ids = Vec::new();
        world.bvh.query_aabb(&q, |i| ids.push(i));
        ids.sort_unstable();
        let walls: Vec<[glam::Vec3; 3]> = ids
            .into_iter()
            .map(|i| &world.tris[i as usize])
            .filter(|t| t.channels & dishonored_world::channel::NON_PAWN != 0)
            .map(|t| [t.a - origin, t.b - origin, t.c - origin])
            .collect();
        let body = Dropped::new(r.pawn, r.bone, r.lo, r.hi, r.velocity, &floor_at, &walls, mv.gravity_z);
        let entity = commands.spawn((Transform::from_matrix(to_bevy(r.root)), Visibility::default(), SeveredPiece { joints: r.joints, accessories: r.accessories, posed: false })).id();
        crate::characters::spawn_character(&mut commands, entity, &lib, r.archetype);
        limbs.live.push(Limb { pawn: r.pawn, body, bone0: r.bone, root0: r.root, entity });
    }
    // A pawn living again (a respawn) or gone takes its pieces back; the oldest go past the cap.
    let dead = |id: u32| view.others.iter().chain(view.me.iter()).any(|p| p.id == id && !p.alive);
    for p in view.others.iter().chain(view.me.iter()).filter(|p| p.alive) {
        limbs.severed.remove(&p.id);
    }
    let live_ids: Vec<u32> = view.others.iter().chain(view.me.iter()).map(|p| p.id).collect();
    limbs.severed.retain(|id, _| live_ids.contains(id));
    let mut keep: Vec<bool> = limbs.live.iter().map(|l| dead(l.pawn)).collect();
    let mut excess = keep.iter().filter(|k| **k).count().saturating_sub(MAX_PIECES);
    for k in keep.iter_mut() {
        if excess == 0 {
            break;
        }
        if *k {
            *k = false;
            excess -= 1;
        }
    }
    let mut i = 0;
    limbs.live.retain(|l| {
        let k = keep[i];
        i += 1;
        if !k {
            commands.entity(l.entity).despawn();
        }
        k
    });
    let dt = time.delta_secs().min(0.1);
    for l in limbs.live.iter_mut() {
        if l.body.elapsed < crate::ragdoll::MAX_TIME {
            l.body.advance(dt);
        }
        if let Ok(mut t) = roots.get_mut(l.entity) {
            // The root carried by the severed bone's motion.
            let carried = l.body.transform() * l.bone0.inverse();
            *t = Transform::from_matrix(to_bevy(carried * l.root0));
        }
    }
}

/// Pose each severed piece's joints (once their entities exist).
pub fn pose_pieces(mut pieces: Query<(&mut SeveredPiece, &Children)>, mut joints: Query<(&Joint, &mut Transform), Without<AccessoryJoint>>, mut ajoints: Query<(&AccessoryJoint, &mut Transform), Without<Joint>>) {
    for (mut piece, children) in &mut pieces {
        if piece.posed {
            continue;
        }
        let mut any = false;
        for c in children.iter() {
            if let Ok((j, mut t)) = joints.get_mut(c) {
                if let Some(m) = piece.joints.get(j.part).and_then(|v| v.get(j.bone)) {
                    *t = joint_transform(*m);
                    any = true;
                }
            }
            if let Ok((a, mut t)) = ajoints.get_mut(c) {
                if let Some(m) = piece.accessories.get(a.0) {
                    *t = joint_transform(*m);
                }
            }
        }
        piece.posed = any;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::BoneAsset;

    fn bone(name: &str, parent: i32) -> BoneAsset {
        BoneAsset { name: name.into(), parent, rotation: glam::Quat::IDENTITY, translation: glam::Vec3::X }
    }

    /// A collapsed joint keeps a finite transform at its point.
    #[test]
    fn a_collapsed_joint_stays_finite() {
        let t = joint_transform(collapsed(glam::Vec3::new(10.0, 20.0, 30.0)));
        assert!(t.rotation.is_finite() && t.translation.is_finite() && t.scale.max_element() < 1e-3);
        assert!(t.translation.distance(to_bevy(glam::Mat4::from_translation(glam::Vec3::new(10.0, 20.0, 30.0))).w_axis.truncate()) < 1e-5);
    }

    /// The cut takes the severed bone and everything below it, nothing above or beside.
    #[test]
    fn the_cut_takes_the_subtree() {
        let bones = [bone("root", -1), bone("spine", 0), bone("neck_jnt", 1), bone("head_jnt", 2), bone("jaw", 3), bone("arm", 1)];
        let r = cut_roots(&bones, &["NECK_JNT".to_string()]);
        assert_eq!(r, vec![None, None, Some(2), Some(2), Some(2), None]);
    }
}
