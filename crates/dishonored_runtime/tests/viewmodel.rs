//! Tests of `dishonored_runtime::viewmodel` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use dishonored_assets::util::rotator_matrix;
use dishonored_ir::BoneAsset;
use dishonored_runtime::viewmodel::{blend_per_bone, branch_mask, parents, to_component};
use glam::{Mat4, Quat, Vec3};

/// root -> pelvis -> spine -> head, and pelvis -> collar -> hand (the left collarbone of
/// `Skm_Player` hangs off bone 1).
fn chain() -> Vec<BoneAsset> {
    let b = |name: &str, parent: i32, t: Vec3| BoneAsset { name: name.into(), parent, rotation: Quat::IDENTITY, translation: t };
    vec![b("root", 0, Vec3::ZERO), b("pelvis", 0, Vec3::Z * 10.0), b("spine", 1, Vec3::Z * 5.0), b("head", 2, Vec3::Z * 5.0), b("collar", 1, Vec3::Y * 3.0), b("hand", 4, Vec3::X * 4.0)]
}

#[test]
fn branch_mask_takes_subtrees() {
    let bones = chain();
    assert_eq!(branch_mask(&bones, &["collar"]), vec![false, false, false, false, true, true]);
    assert_eq!(branch_mask(&bones, &["spine", "Collar"]), vec![false, false, true, true, true, true]);
}

fn same(a: (Quat, Vec3), b: (Quat, Vec3)) -> bool {
    (a.1 - b.1).length() < 1e-4 && a.0.dot(b.0).abs() > 1.0 - 1e-6
}

/// A full-weight branch keeps the target's component-space pose whatever the source does to the
/// bones above it (the gun arm does not inherit the walk cycle's pelvis twist); other bones keep
/// the source.
#[test]
fn component_space_branch_ignores_source_parents() {
    let bones = chain();
    let par = parents(&bones);
    let bind: Vec<(Quat, Vec3)> = bones.iter().map(|b| (b.rotation, b.translation)).collect();
    let mut source = bind.clone();
    source[1].0 = Quat::from_rotation_z(0.4);
    let mut target = bind.clone();
    target[5].0 = Quat::from_rotation_x(0.3);
    let mask = branch_mask(&bones, &["collar"]);
    let out = blend_per_bone(&par, &source, &target, &mask, 1.0);
    let (co, ct, cs) = (to_component(&par, &out), to_component(&par, &target), to_component(&par, &source));
    assert!(same(co[4], ct[4]) && same(co[5], ct[5]));
    assert!(same(co[1], cs[1]) && same(co[2], cs[2]) && same(co[3], cs[3]));
    assert!(!same(cs[4], ct[4]));
    assert_eq!(blend_per_bone(&par, &source, &target, &mask, 0.0), source);
}

fn close(a: Mat4, b: Mat4) -> bool {
    a.to_cols_array().iter().zip(b.to_cols_array()).all(|(x, y)| (x - y).abs() < 1e-4)
}

/// The weapons' RotOrigin (0, -16384, 16384) is a 120-degree axis cycle, so the original's
/// attach (socket `LeftHandWpn` = the same rotator, then RotOrigin) equals the inverse
/// RotOrigin the guns use.
#[test]
fn gun_socket_equals_inverse_rot_origin() {
    let r = rotator_matrix(0, -16384, 16384);
    assert!(close(r * r, r.inverse()));
}

/// The sword's `RightHandWpn` socket (yaw +90) then RotOrigin puts the blade (mesh +Z) where
/// the earlier visual choice Rz(180) * inverse(RotOrigin) did; the two differ by a roll
/// about the blade.
#[test]
fn sword_socket_attach() {
    let r = rotator_matrix(0, -16384, 16384);
    let s = rotator_matrix(0, 16384, 0);
    let decoded = s * r;
    let chosen = Mat4::from_rotation_z(std::f32::consts::PI) * r.inverse();
    assert!((decoded.transform_vector3(glam::Vec3::Z) - chosen.transform_vector3(glam::Vec3::Z)).length() < 1e-4);
    assert!(!close(decoded, chosen));
}
