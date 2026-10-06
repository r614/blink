//! Climbable chains of a map: every `DisClimbable` (a `DishonoredKAsset`, rigid-body chain) of the
//! level packages, with its skeletal mesh's bones in the rest pose and the bounds of its physics
//! asset. The climbing player follows the chain's closest bone (`APawn::physLadder` override
//! 0x00AA82C0), between `Location.z - CollisionHeight` and `Location.z - 2 x Bounds.SphereRadius`;
//! see research/dishonored/PLAYER.md "Climbing".
//!
//! At rest the chain hangs straight down: both Boyle meshes (`Climbable.chain_400`,
//! `Climbable.chain_1400`) are authored vertical, a bone every 100 uu below the anchor.

use crate::content::Content;
use crate::skeletal_mesh::read_skeletal_mesh;
use anyhow::{Context, Result};
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ClimbableDef {
    pub name: String,
    pub package: String,
    pub mesh: String,
    pub anchor: Vec3,
    /// World positions of every bone, in the mesh's bone order.
    pub bones: Vec<Vec3>,
    pub bone_names: Vec<String>,
    /// Axis-aligned bounds of the physics bodies (`PhysicsAsset::CalcAABB` over `BoundsBodies`).
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// `FBoxSphereBounds(box).SphereRadius` = the box's half diagonal.
    pub sphere_radius: f32,
    /// Smallest box half extent of the non-fixed bodies (the chain's thickness).
    pub body_half_width: f32,
    /// Top and bottom of the non-fixed bodies (their bounds' centre line).
    pub top: Vec3,
    pub bottom: Vec3,
}

fn obj(props: &[Property], n: &str) -> Option<i32> {
    find(props, n).and_then(|p| p.as_object()).filter(|o| *o != 0)
}

fn f32_of(props: &[Property], n: &str) -> f32 {
    find(props, n).and_then(|p| p.as_float()).unwrap_or(0.0)
}

/// A cooked `Matrix` struct as a glam matrix. The property reader hands the 16-byte-aligned
/// struct over as 64 raw bytes starting 4 bytes early (a zero dword, then rows X, Y, Z and the
/// W row without its final 1.0): the identity of a fixed root body reads
/// `0, 1,0,0,0, 0,1,0,0, 0,0,1,0, x,y,z`.
fn matrix(v: Option<&Value>) -> Mat4 {
    match v {
        Some(Value::Raw(_, b)) if b.len() >= 64 => {
            let f = |i: usize| f32::from_le_bytes(b[4 + i * 4..8 + i * 4].try_into().unwrap());
            crate::util::fmatrix_rows([[f(0), f(1), f(2), 0.0], [f(4), f(5), f(6), 0.0], [f(8), f(9), f(10), 0.0], [f(12), f(13), f(14), 1.0]])
        }
        Some(Value::Struct(_, rows)) => {
            let row = |n: &str| match find(rows, n).map(|x| &x.value) {
                Some(Value::Struct(_, c)) => [f32_of(c, "X"), f32_of(c, "Y"), f32_of(c, "Z"), f32_of(c, "W")],
                _ => [0.0; 4],
            };
            crate::util::fmatrix_rows([row("XPlane"), row("YPlane"), row("ZPlane"), row("WPlane")])
        }
        _ => Mat4::IDENTITY,
    }
}

/// Read one climbable actor.
fn read_climbable(pkg: &Package, i: usize) -> Result<ClimbableDef> {
    let h = pkg.read_object(i)?;
    let p = &h.properties;
    let actor = crate::level::actor_transform(p);
    let comp = obj(p, "SkeletalMeshComponent").context("no SkeletalMeshComponent")?;
    let ObjRef::Export(ci) = ObjRef::from_index(comp) else { anyhow::bail!("component is an import") };
    let ch = pkg.read_object(ci)?;
    let mesh_ref = obj(&ch.properties, "SkeletalMesh").context("no SkeletalMesh")?;
    let phys_ref = obj(&ch.properties, "PhysicsAsset");
    let ObjRef::Export(mi) = ObjRef::from_index(mesh_ref) else { anyhow::bail!("mesh is an import") };
    let mesh = read_skeletal_mesh(pkg, mi)?;
    let bones: Vec<dishonored_ir::BoneAsset> =
        mesh.bones.iter().map(|b| dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
    let g = crate::characters::bind_globals(&bones);
    // Mesh space to component space (`RotOrigin`, `Origin`), as the character loader does.
    let m2c = crate::util::rotator_matrix(mesh.rot_origin[0], mesh.rot_origin[1], mesh.rot_origin[2]) * Mat4::from_translation(mesh.mesh_origin);
    let world: Vec<Mat4> = g.iter().map(|m| actor * m2c * *m).collect();
    let mut def = ClimbableDef {
        name: pkg.obj_name(ObjRef::Export(i)).to_string(),
        mesh: pkg.obj_path(ObjRef::Export(mi)),
        anchor: actor.w_axis.truncate(),
        bones: world.iter().map(|m| m.w_axis.truncate()).collect(),
        bone_names: bones.iter().map(|b| b.name.clone()).collect(),
        bounds_min: Vec3::splat(f32::INFINITY),
        bounds_max: Vec3::splat(f32::NEG_INFINITY),
        body_half_width: f32::INFINITY,
        ..Default::default()
    };
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    if let Some(ObjRef::Export(pi)) = phys_ref.map(ObjRef::from_index) {
        let ph = pkg.read_object(pi)?;
        let bodies = find(&ph.properties, "BodySetup").and_then(|x| x.array_objects().ok()).unwrap_or_default();
        for b in bodies {
            let ObjRef::Export(e) = ObjRef::from_index(b) else { continue };
            let Ok(bh) = pkg.read_object(e) else { continue };
            let bp = &bh.properties;
            let bone = match find(bp, "BoneName").map(|x| &x.value) {
                Some(Value::Name(n)) => n.clone(),
                _ => continue,
            };
            let Some(bi) = def.bone_names.iter().position(|n| n.eq_ignore_ascii_case(&bone)) else { continue };
            let fixed = matches!(find(bp, "bFixed").map(|x| &x.value), Some(Value::Bool(true)));
            let Some(Value::Struct(_, agg)) = find(bp, "AggGeom").map(|x| &x.value) else { continue };
            let boxes = find(agg, "BoxElems").and_then(|x| x.array_structs(pkg).ok()).unwrap_or_default();
            for s in boxes {
                let half = Vec3::new(f32_of(&s, "X"), f32_of(&s, "Y"), f32_of(&s, "Z")) * 0.5;
                let tm = world[bi] * matrix(find(&s, "TM").map(|x| &x.value));
                let (mut blo, mut bhi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
                for k in 0..8 {
                    let c = Vec3::new(if k & 1 == 0 { -half.x } else { half.x }, if k & 2 == 0 { -half.y } else { half.y }, if k & 4 == 0 { -half.z } else { half.z });
                    let w = tm.transform_point3(c);
                    blo = blo.min(w);
                    bhi = bhi.max(w);
                }
                def.bounds_min = def.bounds_min.min(blo);
                def.bounds_max = def.bounds_max.max(bhi);
                if !fixed {
                    lo = lo.min(blo);
                    hi = hi.max(bhi);
                    def.body_half_width = def.body_half_width.min(half.min_element());
                }
            }
        }
    }
    if def.bounds_min.x > def.bounds_max.x {
        // No physics bodies: the mesh's bones stand in.
        for b in &def.bones {
            def.bounds_min = def.bounds_min.min(*b);
            def.bounds_max = def.bounds_max.max(*b);
        }
    }
    if lo.x > hi.x {
        lo = def.bounds_min;
        hi = def.bounds_max;
    }
    if !def.body_half_width.is_finite() {
        def.body_half_width = 5.0;
    }
    def.sphere_radius = ((def.bounds_max - def.bounds_min) * 0.5).length();
    let c = (lo + hi) * 0.5;
    def.top = Vec3::new(c.x, c.y, hi.z);
    def.bottom = Vec3::new(c.x, c.y, lo.z);
    Ok(def)
}

/// Read the climbables of `packages` (persistent level and sublevels).
pub fn extract_climbables(content: &Content, packages: &[String]) -> Result<Vec<ClimbableDef>> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for name in packages.iter().filter(|p| content.has_package(p)) {
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "DisClimbable" || pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            match read_climbable(&pkg, i) {
                Ok(mut d) => {
                    d.package = name.clone();
                    out.push(d);
                }
                Err(e) => eprintln!("warning: {name} {}: {e:#}", pkg.obj_name(ObjRef::Export(i))),
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boyle's two chains, read from the retail install when present.
    #[test]
    fn boyle_chains() {
        let Ok(c) = Content::locate() else { return };
        if !c.has_package("L_Boyle_Int_Basement") {
            return;
        }
        let int = extract_climbables(&c, &["L_Boyle_Int_Basement".into()]).unwrap();
        assert_eq!(int.len(), 1);
        let d = &int[0];
        assert_eq!(d.anchor, Vec3::new(-3153.0, -420.0, -627.0));
        assert_eq!(d.bones.len(), 6);
        eprintln!("int {:?} r {} top {} bottom {} w {} bones {:?}", d.bounds_min..d.bounds_max, d.sphere_radius, d.top, d.bottom, d.body_half_width, d.bones);
        // A bone every 100 uu straight down from the anchor.
        assert!((d.bones[1] - d.anchor).length() < 0.5, "{:?}", d.bones);
        assert!((d.bones[5] - (d.anchor - Vec3::Z * 400.0)).length() < 0.5, "{:?}", d.bones);
        assert!(d.sphere_radius > 150.0 && d.sphere_radius < 260.0, "{}", d.sphere_radius);
        let ext = extract_climbables(&c, &["L_Boyle_Ext_Env".into()]).unwrap();
        assert_eq!(ext.len(), 1);
        let e = &ext[0];
        eprintln!("ext {:?} r {} top {} bottom {} w {}", e.bounds_min..e.bounds_max, e.sphere_radius, e.top, e.bottom, e.body_half_width);
        assert_eq!(e.bones.len(), 16);
        assert!((e.bones[15] - (e.anchor - Vec3::Z * 1400.0)).length() < 0.5);
    }
}
