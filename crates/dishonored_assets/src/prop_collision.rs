//! Collision of placed skeletal props: the physics asset bodies of a `DisSkeletalBreakable`'s
//! `SkeletalMeshComponent` (its `CollisionComponent`; `DishonoredUsableObject` defaults
//! `bCollideActors`, `bBlockActors` and the component's `BlockActors`, every gameplay trace type),
//! posed at the mesh's reference pose.
//!
//! UE3 collides pawns with a skeletal mesh component through its physics asset: one body per
//! bone (`RB_BodySetup.BoneName`) with box / sphere / capsule elements (`KAggregateGeom`), each
//! with a transform relative to the bone. A level transition door (`LTD_*`, Boyle's four doors to
//! the interior) is closed in its reference pose and never opens in place, so its bodies are
//! static collision.

use crate::content::Content;
use crate::skeletal_mesh::read_skeletal_mesh;
use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec3};
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

/// The collision bodies of one placed prop.
#[derive(Debug, Clone, Default)]
pub struct PropBodies {
    /// `<package>:<actor name>`.
    pub actor: String,
    /// Boxes as transforms of the unit cube `[-1, 1]^3` (half extents in the scale), world space.
    /// Spheres and capsules are carried as their bounding boxes (APPROXIMATION).
    pub boxes: Vec<Mat4>,
}

fn obj(props: &[ue3_package::properties::Property], n: &str) -> Option<i32> {
    find(props, n).and_then(|p| p.as_object()).filter(|o| *o != 0)
}

fn f32_of(props: &[ue3_package::properties::Property], n: &str) -> f32 {
    find(props, n).and_then(|p| p.as_float()).unwrap_or(0.0)
}

fn export_of(content: &Content, r: &(String, String)) -> Option<(std::sync::Arc<ue3_package::Package>, usize)> {
    let pkg = content.package(&r.0).ok()?;
    let i = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&r.1))?;
    Some((pkg, i))
}

/// Read the physics bodies of the given placed actors (`<package>:<actor name>`) of `packages`.
pub fn extract_prop_bodies(content: &Content, packages: &[String], actors: &[String]) -> Vec<PropBodies> {
    let mut out = Vec::new();
    for a in actors {
        match read_prop_bodies(content, packages, a) {
            Ok(b) => out.push(b),
            Err(e) => eprintln!("warning: collision of {a}: {e:#}"),
        }
    }
    out
}

fn read_prop_bodies(content: &Content, packages: &[String], actor: &str) -> Result<PropBodies> {
    let (pkg_name, name) = actor.split_once(':').context("actor is <package>:<name>")?;
    let pkg = content.package(pkg_name)?;
    let i = (0..pkg.exports.len())
        .find(|&i| pkg.obj_name(ObjRef::Export(i)).eq_ignore_ascii_case(name) && pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) == "PersistentLevel")
        .context("actor not found")?;
    let h = pkg.read_object(i)?;
    let p = &h.properties;
    let flag = |props: &[ue3_package::properties::Property], k: &str| !matches!(find(props, k).map(|x| &x.value), Some(Value::Bool(false)));
    let mut out = PropBodies { actor: actor.to_string(), boxes: Vec::new() };
    if !flag(p, "bCollideActors") || !flag(p, "bBlockActors") {
        return Ok(out);
    }
    let ObjRef::Export(ci) = obj(p, "SkeletalMeshComponent").map(ObjRef::from_index).context("no SkeletalMeshComponent")? else { anyhow::bail!("component is an import") };
    let ch = pkg.read_object(ci)?;
    let c = &ch.properties;
    if !flag(c, "BlockActors") || !flag(c, "CollideActors") {
        return Ok(out);
    }
    let mesh_ref = crate::usable_props::defining_package(content, &pkg, ObjRef::from_index(obj(c, "SkeletalMesh").context("no SkeletalMesh")?), packages).context("mesh not found")?;
    let phys_ref = crate::usable_props::defining_package(content, &pkg, ObjRef::from_index(obj(c, "PhysicsAsset").context("no PhysicsAsset")?), packages).context("physics asset not found")?;
    let (mpkg, mi) = export_of(content, &mesh_ref).context("mesh export")?;
    let mesh = read_skeletal_mesh(&mpkg, mi)?;
    let bones: Vec<dishonored_ir::BoneAsset> = mesh.bones.iter().map(|b| dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
    let g = crate::characters::bind_globals(&bones);
    // Actor x component, then mesh space to component space (`RotOrigin`, `Origin`).
    let m2c = crate::util::rotator_matrix(mesh.rot_origin[0], mesh.rot_origin[1], mesh.rot_origin[2]) * Mat4::from_translation(mesh.mesh_origin);
    let to_world = crate::level::actor_transform(p) * crate::level::component_transform(c) * m2c;
    let (ppkg, pi) = export_of(content, &phys_ref).context("physics asset export")?;
    let ph = ppkg.read_object(pi)?;
    for b in find(&ph.properties, "BodySetup").and_then(|x| x.array_objects().ok()).unwrap_or_default() {
        let ObjRef::Export(e) = ObjRef::from_index(b) else { continue };
        let Ok(bh) = ppkg.read_object(e) else { continue };
        let bp = &bh.properties;
        let bone = match find(bp, "BoneName").map(|x| &x.value) {
            Some(Value::Name(n)) => n.clone(),
            _ => continue,
        };
        let Some(bi) = bones.iter().position(|x| x.name.eq_ignore_ascii_case(&bone)) else { continue };
        let Some(Value::Struct(_, agg)) = find(bp, "AggGeom").map(|x| &x.value) else { continue };
        let elems = |n: &str| find(agg, n).and_then(|x| x.array_structs(&ppkg).ok()).unwrap_or_default();
        let bone_world = to_world * g[bi];
        for s in elems("BoxElems") {
            let half = Vec3::new(f32_of(&s, "X"), f32_of(&s, "Y"), f32_of(&s, "Z")) * 0.5;
            out.boxes.push(bone_world * crate::climbable::matrix(find(&s, "TM").map(|x| &x.value)) * Mat4::from_scale(half));
        }
        for s in elems("SphylElems") {
            // Capsule along the element's Z: radius, cylinder length.
            let (r, l) = (f32_of(&s, "Radius"), f32_of(&s, "Length"));
            out.boxes.push(bone_world * crate::climbable::matrix(find(&s, "TM").map(|x| &x.value)) * Mat4::from_scale(Vec3::new(r, r, l * 0.5 + r)));
        }
        for s in elems("SphereElems") {
            let r = f32_of(&s, "Radius");
            out.boxes.push(bone_world * crate::climbable::matrix(find(&s, "TM").map(|x| &x.value)) * Mat4::from_scale_rotation_translation(Vec3::splat(r), Quat::IDENTITY, Vec3::ZERO));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boyle's front door (`DishonoredUsableObject_6`, the transition to the foyer): its two
    /// leaves fill the doorway the courtyard wall leaves open (y 1350..1500 at x ~1480..1520).
    #[test]
    fn boyle_front_door_bodies_fill_the_doorway() {
        let Ok(c) = Content::locate() else { return };
        if !c.has_package("L_Boyle_Ext_P") {
            return;
        }
        let pk = crate::map_packages(&c, "L_Boyle_Ext_P");
        let b = extract_prop_bodies(&c, &pk, &["L_Boyle_Ext_P:DishonoredUsableObject_6".to_string()]);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].boxes.len(), 2, "two leaves");
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for m in &b[0].boxes {
            for k in 0..8 {
                let p = m.transform_point3(Vec3::new(if k & 1 == 0 { -1.0 } else { 1.0 }, if k & 2 == 0 { -1.0 } else { 1.0 }, if k & 4 == 0 { -1.0 } else { 1.0 }));
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        eprintln!("front door bodies {lo:.0?}..{hi:.0?}");
        assert!(lo.y < 1360.0 && hi.y > 1490.0, "{lo:?}..{hi:?}");
        assert!(lo.x > 1400.0 && hi.x < 1600.0, "{lo:?}..{hi:?}");
        assert!(lo.z < 20.0 && hi.z > 250.0, "{lo:?}..{hi:?}");
    }
}
