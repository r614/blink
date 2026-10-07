//! A static mesh's simplified collision: the elements of its `RB_BodySetup.AggGeom`
//! (`KAggregateGeom`) as mesh-space triangles, and the `UseSimpleLineCollision` /
//! `UseSimpleBoxCollision` flags that choose it over the per-polygon kDOP tree.
//!
//! `UStaticMeshComponent::LineCheck` (0x007717D0) answers a zero-extent check with the
//! simplified collision when `UseSimpleLineCollision`, an extent check when
//! `UseSimpleBoxCollision` (both default true: a cooked mesh serializes them only when false),
//! unless the check asks for complex collision; a mesh that wants the simplified collision but
//! has no body setup is not hit at all (research/dishonored/COLLISION.md).
//!
//! Elements: `ConvexElems` (`VertexData` and the hull's `FaceTriData` triangles), `BoxElems`
//! (`TM`, full sizes `X`, `Y`, `Z`), `SphereElems` (`TM`, `Radius`) and `SphylElems` (`TM`,
//! `Radius`, `Length` along the element's Z). Spheres and capsules become polyhedra
//! (APPROXIMATION, U-652); Boyle's static meshes have none.

use crate::climbable::matrix;
use glam::{Mat4, Vec3};
use ue3_package::Package;
use ue3_package::properties::{Property, Value, find};

fn f32_of(props: &[Property], n: &str) -> f32 {
    find(props, n).and_then(|p| p.as_float()).unwrap_or(0.0)
}

/// `UseSimpleLineCollision`, `UseSimpleBoxCollision` of a static mesh's tagged properties.
pub fn simple_flags(props: &[Property]) -> (bool, bool) {
    let get = |n: &str| !matches!(find(props, n).map(|p| &p.value), Some(Value::Bool(false)));
    (get("UseSimpleLineCollision"), get("UseSimpleBoxCollision"))
}

fn raw_vec3s(p: &Property) -> Vec<Vec3> {
    let Some((n, b)) = p.array_raw() else { return Vec::new() };
    (0..n)
        .filter_map(|i| {
            let c = b.get(i * 12..i * 12 + 12)?;
            let f = |o: usize| f32::from_le_bytes([c[o], c[o + 1], c[o + 2], c[o + 3]]);
            Some(Vec3::new(f(0), f(4), f(8)))
        })
        .collect()
}

fn raw_i32s(p: &Property) -> Vec<i32> {
    let Some((n, b)) = p.array_raw() else { return Vec::new() };
    (0..n).filter_map(|i| b.get(i * 4..i * 4 + 4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))).collect()
}

/// A unit sphere as an octahedron subdivided twice (128 faces).
fn unit_sphere() -> Vec<[Vec3; 3]> {
    let (x, y, z) = (Vec3::X, Vec3::Y, Vec3::Z);
    let mut f: Vec<[Vec3; 3]> = vec![[x, y, z], [y, -x, z], [-x, -y, z], [-y, x, z], [y, x, -z], [-x, y, -z], [-y, -x, -z], [x, -y, -z]];
    for _ in 0..2 {
        f = f
            .iter()
            .flat_map(|t| {
                let (a, b, c) = (t[0], t[1], t[2]);
                let (ab, bc, ca) = ((a + b).normalize(), (b + c).normalize(), (c + a).normalize());
                [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]
            })
            .collect();
    }
    f
}

/// The simplified collision of a body setup export as mesh-space triangles.
pub fn agg_geom_triangles(pkg: &Package, body: usize) -> Vec<[Vec3; 3]> {
    let Ok(h) = pkg.read_object(body) else { return Vec::new() };
    let Some(Value::Struct(_, agg)) = find(&h.properties, "AggGeom").map(|p| &p.value) else { return Vec::new() };
    let elems = |n: &str| find(agg, n).and_then(|x| x.array_structs(pkg).ok()).unwrap_or_default();
    let mut out = Vec::new();
    for e in elems("ConvexElems") {
        let verts = find(&e, "VertexData").map(raw_vec3s).unwrap_or_default();
        let tris = find(&e, "FaceTriData").map(raw_i32s).unwrap_or_default();
        for t in tris.chunks_exact(3) {
            let v = |i: i32| verts.get(i as usize).copied();
            if let (Some(a), Some(b), Some(c)) = (v(t[0]), v(t[1]), v(t[2])) {
                out.push([a, b, c]);
            }
        }
    }
    for e in elems("BoxElems") {
        let half = Vec3::new(f32_of(&e, "X"), f32_of(&e, "Y"), f32_of(&e, "Z")) * 0.5;
        out.extend(dishonored_world_box(matrix(find(&e, "TM").map(|x| &x.value)) * Mat4::from_scale(half)));
    }
    let sphere = unit_sphere();
    for e in elems("SphereElems") {
        let m = matrix(find(&e, "TM").map(|x| &x.value)) * Mat4::from_scale(Vec3::splat(f32_of(&e, "Radius")));
        out.extend(sphere.iter().map(|t| t.map(|p| m.transform_point3(p))));
    }
    for e in elems("SphylElems") {
        // A capsule along Z: the sphere's halves pushed apart by half the length.
        let (r, l) = (f32_of(&e, "Radius"), f32_of(&e, "Length"));
        let m = matrix(find(&e, "TM").map(|x| &x.value));
        let push = |p: Vec3| p * r + Vec3::Z * (l * 0.5) * p.z.signum();
        out.extend(sphere.iter().map(|t| t.map(|p| m.transform_point3(push(p)))));
        // The cylinder between the caps.
        let ring: Vec<Vec3> = (0..16).map(|k| { let a = k as f32 / 16.0 * std::f32::consts::TAU; Vec3::new(a.cos() * r, a.sin() * r, 0.0) }).collect();
        for k in 0..16 {
            let (a, b) = (ring[k], ring[(k + 1) % 16]);
            let (lo, hi) = (Vec3::Z * (-l * 0.5), Vec3::Z * (l * 0.5));
            out.push([m.transform_point3(a + lo), m.transform_point3(b + lo), m.transform_point3(b + hi)]);
            out.push([m.transform_point3(a + lo), m.transform_point3(b + hi), m.transform_point3(a + hi)]);
        }
    }
    out
}

/// The twelve triangles of the unit cube `[-1, 1]^3` under `m`.
fn dishonored_world_box(m: Mat4) -> Vec<[Vec3; 3]> {
    let c = |x: f32, y: f32, z: f32| m.transform_point3(Vec3::new(x, y, z));
    let v = [c(-1., -1., -1.), c(1., -1., -1.), c(1., 1., -1.), c(-1., 1., -1.), c(-1., -1., 1.), c(1., -1., 1.), c(1., 1., 1.), c(-1., 1., 1.)];
    let quads = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [3, 0, 4, 7]];
    quads.iter().flat_map(|q| [[v[q[0]], v[q[1]], v[q[2]]], [v[q[0]], v[q[2]], v[q[3]]]]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_sphere_is_closed_and_round() {
        let s = unit_sphere();
        assert_eq!(s.len(), 128);
        for t in &s {
            for p in t {
                assert!((p.length() - 1.0).abs() < 1e-5);
            }
        }
        let b = dishonored_world_box(Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0)));
        let area: f32 = b.iter().map(|f| (f[1] - f[0]).cross(f[2] - f[0]).length() * 0.5).sum();
        assert!((area - 8.0 * (6.0 + 12.0 + 8.0)).abs() < 1e-3, "{area}");
    }

    /// Boyle's 5 m stair floor uses its simplified collision: six hulls and nine boxes.
    #[test]
    fn stairs_body_setup() {
        let Ok(c) = crate::Content::locate() else { return };
        let Ok(pkg) = c.package("L_Boyle_Ext_Env") else { return };
        let Some(i) = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "StaticMesh" && pkg.obj_path(ue3_package::ObjRef::Export(i)) == "int_stairs_02.stairs_5m_floor_00") else { return };
        let props = pkg.read_object(i).unwrap().properties;
        assert_eq!(simple_flags(&props), (true, true));
        let raw = crate::static_mesh::read_static_mesh(&pkg, i).unwrap();
        let ue3_package::ObjRef::Export(b) = ue3_package::ObjRef::from_index(raw.body_setup) else { panic!("no body setup") };
        let t = agg_geom_triangles(&pkg, b);
        assert_eq!(t.len(), 9 * 12 + (4 + 12 + 12 + 12 + 12 + 16), "{}", t.len());
    }
}
