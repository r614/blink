//! Tests of `dishonored_runtime::viewmodel` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use dishonored_assets::util::rotator_matrix;
use glam::Mat4;

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
