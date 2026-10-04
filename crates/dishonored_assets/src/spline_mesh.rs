//! `SplineMeshComponent` deformation (UE3 `USE_SPLINEDEFORM` vertex factory), baked on the
//! CPU into a per-component mesh copy.
//!
//! The mesh's local X axis is mapped onto a cubic Hermite segment
//! `SplineParams{StartPos, StartTangent, EndPos, EndTangent}` (component space);
//! local Y/Z become offsets along a frame built from the spline direction and
//! `SplineXDir`, scaled by the interpolated `StartScale`/`EndScale`.
//! APPROXIMATION: roll (`StartRoll`/`EndRoll`, absent on Boyle Ext) and offsets are ignored,
//! and the frame convention (`X = SplineXDir x Dir`) was chosen to keep cables upright;
//! it has not been checked against the original shader.

use dishonored_ir::StaticMeshAsset;
use glam::{Vec2, Vec3};
use ue3_package::properties::{Property, Value, find};

#[derive(Debug, Clone, Copy)]
pub struct SplineParams {
    pub start_pos: Vec3,
    pub start_tangent: Vec3,
    pub start_scale: Vec2,
    pub end_pos: Vec3,
    pub end_tangent: Vec3,
    pub end_scale: Vec2,
    pub x_dir: Vec3,
}

pub fn read_params(props: &[Property]) -> Option<SplineParams> {
    let Some(Value::Struct(_, s)) = find(props, "SplineParams").map(|p| &p.value) else {
        return None;
    };
    let v = |n: &str| find(s, n).and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
    let v2 = |n: &str| match find(s, n).map(|p| &p.value) {
        Some(Value::Vector2D(x)) => Vec2::from(*x),
        _ => Vec2::ONE,
    };
    Some(SplineParams {
        start_pos: v("StartPos"),
        start_tangent: v("StartTangent"),
        start_scale: v2("StartScale"),
        end_pos: v("EndPos"),
        end_tangent: v("EndTangent"),
        end_scale: v2("EndScale"),
        x_dir: find(props, "SplineXDir").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::Z),
    })
}

impl SplineParams {
    fn pos(&self, a: f32) -> Vec3 {
        let (a2, a3) = (a * a, a * a * a);
        self.start_pos * (2.0 * a3 - 3.0 * a2 + 1.0)
            + self.start_tangent * (a3 - 2.0 * a2 + a)
            + self.end_pos * (-2.0 * a3 + 3.0 * a2)
            + self.end_tangent * (a3 - a2)
    }
    fn dir(&self, a: f32) -> Vec3 {
        let a2 = a * a;
        let d = self.start_pos * (6.0 * a2 - 6.0 * a)
            + self.start_tangent * (3.0 * a2 - 4.0 * a + 1.0)
            + self.end_pos * (-6.0 * a2 + 6.0 * a)
            + self.end_tangent * (3.0 * a2 - 2.0 * a);
        d.try_normalize().unwrap_or((self.end_pos - self.start_pos).normalize_or(Vec3::X))
    }
    /// (position, direction, side axis, up axis) at `a`, side/up already scaled.
    fn frame(&self, a: f32) -> (Vec3, Vec3, Vec3, Vec3) {
        let d = self.dir(a);
        let x = self.x_dir.cross(d).try_normalize().unwrap_or_else(|| d.any_orthonormal_vector());
        let y = d.cross(x).normalize_or_zero();
        let s = self.start_scale.lerp(self.end_scale, a);
        (self.pos(a), d, x * s.x, y * s.y)
    }
}

/// Deformed copy of `mesh` (positions, normals, tangents; bounds recomputed).
pub fn deform(mesh: &StaticMeshAsset, p: &SplineParams) -> StaticMeshAsset {
    let min_x = mesh.positions.iter().map(|v| v.x).fold(f32::MAX, f32::min);
    let max_x = mesh.positions.iter().map(|v| v.x).fold(f32::MIN, f32::max);
    let len = (max_x - min_x).max(1e-3);
    let mut out = mesh.clone();
    for (i, v) in mesh.positions.iter().enumerate() {
        let a = (v.x - min_x) / len;
        let (o, d, x, y) = p.frame(a);
        out.positions[i] = o + x * v.y + y * v.z;
        let xf = x.normalize_or_zero();
        let yf = y.normalize_or_zero();
        let rot = |n: Vec3| (d * n.x + xf * n.y + yf * n.z).normalize_or_zero();
        if let Some(n) = mesh.normals.get(i) {
            out.normals[i] = rot(*n);
        }
        if let Some(t) = mesh.tangents.get(i) {
            out.tangents[i] = rot(*t);
        }
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in &out.positions {
        lo = lo.min(*v);
        hi = hi.max(*v);
    }
    out.bounds_min = lo;
    out.bounds_max = hi;
    out.source = format!("{} (spline)", mesh.source);
    out
}
