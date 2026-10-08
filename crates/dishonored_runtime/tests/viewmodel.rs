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

/// `AnimNodeBlendDirectional`: the cooked snapshot (DirAngle 0.959408) weighs forward 0.3892219
/// and right 0.6107781.
#[test]
fn directional_weights_match_the_cooked_snapshot() {
    let w = dishonored_runtime::viewmodel::directional_weights(0.959408);
    assert!((w[0] - 0.3892219).abs() < 1e-5 && (w[3] - 0.6107781).abs() < 1e-5 && w[1] == 0.0 && w[2] == 0.0, "{w:?}");
    let b = dishonored_runtime::viewmodel::directional_weights(std::f32::consts::PI - 1e-4);
    assert!(b[1] > 0.99, "{b:?}");
}

#[test]
fn notify_windows_wrap_for_loops() {
    use dishonored_runtime::viewmodel::crossed;
    assert!(crossed(0.0, 0.0, 0.016, 1.0, false));
    assert!(!crossed(0.5, 0.0, 0.4, 1.0, false));
    assert!(crossed(0.05, 0.98, 1.06, 1.0, true));
    assert!(!crossed(0.5, 0.98, 1.06, 1.0, true));
}

/// `ArkAnimNodeRat` (0x00913ED0) with `Default__ArkAnimNodeRat`'s moves 55 / 98 / 302 uu/s.
#[test]
fn rat_move_blend() {
    use dishonored_runtime::world_models::rat_move;
    let len = |_: &str| 1.0;
    // Between WalkN (98) and RunN (302): weights by speed.
    let (p, _) = rat_move(200.0, 0.0, 0.0, &len);
    assert_eq!(p.len(), 2);
    assert_eq!((p[0].0, p[1].0), ("WalkN", "RunN"));
    assert!((p[1].1 - (200.0 - 98.0) / 204.0).abs() < 1e-5);
    // Above RunN alone, the rate capped at 1.5: one second at 600 uu/s moves the phase 0.5.
    let (p, pct) = rat_move(600.0, 0.0, 1.0, &len);
    assert_eq!(p.len(), 1);
    assert!((pct - 0.5).abs() < 1e-5, "{pct}");
    // Below WalkSlowN alone, slowed: 27.5 uu/s plays it at half rate.
    let (p, pct) = rat_move(27.5, 0.0, 1.0, &len);
    assert_eq!(p[0].0, "WalkSlowN");
    assert!((pct - 0.5).abs() < 1e-5);
}
/// `m_ArmPitch_OffsetKeys` (0x00ACB8D0): flat between -10 and 10 degrees, a cosine ease between
/// keys, the end keys' differences beyond them.
#[test]
fn arm_pitch_offset_keys() {
    use dishonored_runtime::viewmodel::{ARM_PITCH_KEYS_DEFAULT, ARM_PITCH_KEYS_GUN, arm_pitch_offset};
    let k = &ARM_PITCH_KEYS_DEFAULT;
    assert_eq!(arm_pitch_offset(k, 0.0), 0.0);
    assert_eq!(arm_pitch_offset(k, -90.0), 10.0);
    assert_eq!(arm_pitch_offset(k, -120.0), 10.0);
    assert_eq!(arm_pitch_offset(k, 95.0), -15.0);
    assert!((arm_pitch_offset(k, -50.0) - 5.0).abs() < 1e-4);
    assert!((arm_pitch_offset(k, 50.0) + 7.5).abs() < 1e-4);
    // Eased, not linear: a quarter of the way from 10 to 90 gives (1 - cos(pi / 4)) / 2 of -15.
    let q = arm_pitch_offset(k, 30.0);
    assert!((q + 15.0 * (1.0 - std::f32::consts::FRAC_PI_4.cos()) * 0.5).abs() < 1e-4, "{q}");
    assert!((arm_pitch_offset(&ARM_PITCH_KEYS_GUN, -90.0) - 2.0).abs() < 1e-6);
}

/// UE3 rotators in pawn space (X forward, Y right, Z up): pitch lifts the forward axis, yaw turns
/// it toward +Y, and the frame-to-rotator conversion inverts them.
#[test]
fn rotators_round_trip() {
    use dishonored_runtime::viewmodel::{frame_rotator, rotator_quat};
    let q = rotator_quat(30.0, 0.0, 0.0);
    assert!((q * Vec3::X - Vec3::new(30f32.to_radians().cos(), 0.0, 30f32.to_radians().sin())).length() < 1e-5);
    assert!((rotator_quat(0.0, 90.0, 0.0) * Vec3::X - Vec3::Y).length() < 1e-5);
    // Positive roll lowers the right axis (`FRotationMatrix`: Y = (0, cos r, -sin r)).
    assert!((rotator_quat(0.0, 0.0, 90.0) * Vec3::Y - Vec3::NEG_Z).length() < 1e-5);
    for (p, y, r) in [(20.0, -35.0, 10.0), (-60.0, 120.0, -25.0), (5.0, 0.0, 0.0)] {
        let q = rotator_quat(p, y, r);
        let (p2, y2, r2) = frame_rotator(q * Vec3::X, q * Vec3::Y, q * Vec3::Z);
        assert!((p - p2).abs() < 1e-3 && (y - y2).abs() < 1e-3 && (r - r2).abs() < 1e-3, "{p} {y} {r} -> {p2} {y2} {r2}");
    }
}

/// The view takes the look pitch plus `m_RollAmount` (0.7) of the camera bone's animated
/// rotation (0x00AABB70).
#[test]
fn view_takes_seventy_percent_of_the_bone() {
    use dishonored_runtime::viewmodel::{ROLL_AMOUNT, frame_rotator, rotator_quat, view_rotation};
    let look = -30.0;
    let bone = rotator_quat(look + 10.0, 20.0, 5.0);
    let v = view_rotation(bone * Vec3::X, bone * Vec3::Y, bone * Vec3::Z, look);
    let (p, y, r) = frame_rotator(v * Vec3::X, v * Vec3::Y, v * Vec3::Z);
    assert!((p - (look + 10.0 * ROLL_AMOUNT)).abs() < 1e-3 && (y - 20.0 * ROLL_AMOUNT).abs() < 1e-3 && (r - 5.0 * ROLL_AMOUNT).abs() < 1e-3, "{p} {y} {r}");
    // A bone that only carries the look pitch gives exactly the look.
    let still = rotator_quat(look, 0.0, 0.0);
    let v = view_rotation(still * Vec3::X, still * Vec3::Y, still * Vec3::Z, look);
    assert!(v.dot(still).abs() > 1.0 - 1e-6);
}

/// The look controls (actor-space added rotation): the bone keeps its position and its children
/// turn about it by the pitch in pawn space, whatever the mesh's RotOrigin.
#[test]
fn look_control_turns_about_the_bone_in_pawn_space() {
    use dishonored_runtime::viewmodel::apply_look_controls;
    let bones = chain();
    let par = parents(&bones);
    for m2p in [Mat4::IDENTITY, Mat4::from_translation(Vec3::new(0.0, 87.5, 0.0)) * rotator_matrix(0, -16384, 16384)] {
        let mut local: Vec<(Quat, Vec3)> = bones.iter().map(|b| (b.rotation, b.translation)).collect();
        let before = to_component(&par, &local);
        apply_look_controls(&par, &mut local, m2p, &[(4, 30.0)]);
        let after = to_component(&par, &local);
        let pawn = |c: &[(Quat, Vec3)], i: usize| m2p.transform_point3(c[i].1);
        let (after, before) = (&after[..], &before[..]);
        assert!((pawn(after, 4) - pawn(before, 4)).length() < 1e-4);
        let turn = Quat::from_rotation_y(-30f32.to_radians());
        let expect = pawn(before, 4) + turn * (pawn(before, 5) - pawn(before, 4));
        assert!((pawn(after, 5) - expect).length() < 1e-3, "{:?} vs {expect:?}", pawn(after, 5));
        // Bones off the controlled branch are untouched.
        assert!((pawn(after, 3) - pawn(before, 3)).length() < 1e-5);
    }
}

/// `AnimNodeSequenceBlendByAim`: centred aim weighs only the centre; full right weighs only the
/// right centre; halfway up-right splits four ways.
#[test]
fn aim_grid_weights() {
    use dishonored_runtime::viewmodel::aim_weights;
    let c = aim_weights(0.0, 0.0);
    assert_eq!(c[4], 1.0);
    let r = aim_weights(1.0, 0.0);
    assert_eq!(r[7], 1.0);
    let ur = aim_weights(0.5, 0.5);
    assert!((ur[4] - 0.25).abs() < 1e-6 && (ur[6] - 0.25).abs() < 1e-6 && (ur[3] - 0.25).abs() < 1e-6 && (ur[7] - 0.25).abs() < 1e-6);
    assert!((ur.iter().sum::<f32>() - 1.0).abs() < 1e-6);
}