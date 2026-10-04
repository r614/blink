//! Conversion between Dishonored world space (Z-up, left-handed, Unreal units) and
//! Bevy space (Y-up, right-handed, metres-ish). Presentation only: simulation stays
//! in Dishonored units.

use bevy::math::{Mat4, Vec3, Vec4};

/// Presentation scale. 1 uu is treated as 2 cm (UE3 convention); this does not affect
/// any gameplay value.
pub const UU_TO_BEVY: f32 = 0.02;

/// Swapping Y and Z converts handedness and up-axis in one step.
#[inline]
pub fn pos(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, v.y) * UU_TO_BEVY
}

#[inline]
pub fn dir(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, v.y)
}

/// Basis change C (swap + scale) so that `C * M * C^-1` maps Bevy-space mesh vertices
/// (already converted with [`pos`]) the way `M` maps Dishonored-space vertices.
pub fn transform(m: Mat4) -> Mat4 {
    let swap = Mat4::from_cols(Vec4::X, Vec4::Z, Vec4::Y, Vec4::W);
    let c = Mat4::from_scale(Vec3::splat(UU_TO_BEVY)) * swap;
    let c_inv = swap * Mat4::from_scale(Vec3::splat(1.0 / UU_TO_BEVY));
    c * m * c_inv
}
