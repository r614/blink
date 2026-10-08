//! Collision of placed skeletal props: the physics asset bodies of a `DisSkeletalBreakable`'s
//! `SkeletalMeshComponent` (its `CollisionComponent`; `DishonoredUsableObject` defaults
//! `bCollideActors`, `bBlockActors` and the component's `BlockActors`, every gameplay trace type),
//! posed at the mesh's reference pose or at the last frame of a door's stage animation.
//!
//! UE3 collides pawns with a skeletal mesh component through its physics asset: one body per
//! bone (`RB_BodySetup.BoneName`) with convex / box / sphere / capsule elements
//! (`KAggregateGeom`), each placed by its `TM` relative to the bone. A door at rest is in its
//! reference pose when closed and on the last frame of its open animation when opened
//! (`DisDoor`, research/dishonored/COLLISION.md).

use crate::content::Content;
use crate::skeletal_mesh::read_skeletal_mesh;
use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec3};
use std::collections::HashMap;
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

/// The collision bodies of one placed prop as world-space triangles.
#[derive(Debug, Clone, Default)]
pub struct PropBodies {
    /// `<package>:<actor name>`.
    pub actor: String,
    /// The bodies' elements, each as world-space triangles.
    pub tris: Vec<Vec<[Vec3; 3]>>,
}

/// A bone's local rotation and (when animated) translation, by bone name (lowercase).
pub type BonePose = HashMap<String, (Quat, Option<Vec3>)>;

fn obj(props: &[ue3_package::properties::Property], n: &str) -> Option<i32> {
    find(props, n).and_then(|p| p.as_object()).filter(|o| *o != 0)
}

fn export_of(content: &Content, r: &(String, String)) -> Option<(std::sync::Arc<ue3_package::Package>, usize)> {
    let pkg = content.package(&r.0).ok()?;
    let i = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&r.1))?;
    Some((pkg, i))
}

/// The package exporting an object path: one of `packages`, else the library packages.
fn locate_path(content: &Content, packages: &[String], path: &str) -> Option<(String, String)> {
    for p in packages.iter().map(String::as_str).chain(crate::level::LIBRARY_PACKAGES.iter().copied()) {
        let Ok(q) = content.package(p) else { continue };
        if (0..q.exports.len()).any(|i| q.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path)) {
            return Some((q.name.clone(), path.to_string()));
        }
    }
    None
}

/// Read the physics bodies of the given placed actors (`<package>:<actor name>`) of `packages`
/// at their reference pose.
pub fn extract_prop_bodies(content: &Content, packages: &[String], actors: &[String]) -> Vec<PropBodies> {
    let mut out = Vec::new();
    for a in actors {
        match read_prop_bodies(content, packages, a, &BonePose::new()) {
            Ok(b) => out.push(b),
            Err(e) => eprintln!("warning: collision of {a}: {e:#}"),
        }
    }
    out
}

/// The physics bodies of one placed actor with the bones in `pose` (others at the reference
/// pose): the skeletal mesh component its `SkeletalMeshComponent` names (the
/// `DisSkeletalBreakable` usables and doors), else its `CollisionComponent` when that is a
/// skeletal mesh component (`DisRiverKrust`, `DisWatchTower`: their skeletal component is a
/// native member, `m_pSkeletalMeshCpnt` / `m_pTowerMeshCpnt`, that only `CollisionComponent`
/// names among the serialized properties).
pub fn read_prop_bodies(content: &Content, packages: &[String], actor: &str, pose: &BonePose) -> Result<PropBodies> {
    read_component_bodies(content, packages, actor, None, pose)
}

/// [`read_prop_bodies`] for a named component of the actor (`component`: the component
/// export's name, e.g. `SkeletalMeshComponent0`; None: the actor's skeletal component as above).
pub fn read_component_bodies(content: &Content, packages: &[String], actor: &str, component: Option<&str>, pose: &BonePose) -> Result<PropBodies> {
    let (pkg_name, name) = actor.split_once(':').context("actor is <package>:<name>")?;
    let pkg = content.package(pkg_name)?;
    let i = (0..pkg.exports.len())
        .find(|&i| pkg.obj_name(ObjRef::Export(i)).eq_ignore_ascii_case(name) && pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) == "PersistentLevel")
        .context("actor not found")?;
    let h = pkg.read_object(i)?;
    let p = &h.properties;
    let mut out = PropBodies { actor: actor.to_string(), tris: Vec::new() };
    let skeletal = |o: i32| matches!(ObjRef::from_index(o), ObjRef::Export(c) if pkg.export_class(c).contains("SkeletalMeshComponent"));
    let comp = match component {
        Some(c) => (0..pkg.exports.len()).find(|&e| pkg.exports[e].outer == i as i32 + 1 && pkg.obj_name(ObjRef::Export(e)).eq_ignore_ascii_case(c)).map(|e| e as i32 + 1),
        None => obj(p, "SkeletalMeshComponent").or_else(|| obj(p, "CollisionComponent").filter(|o| skeletal(*o))),
    };
    let ObjRef::Export(ci) = comp.map(ObjRef::from_index).context("no SkeletalMeshComponent")? else { anyhow::bail!("component is an import") };
    let ch = pkg.read_object(ci)?;
    let c = &ch.properties;
    // The mesh and physics asset, set on the component or inherited from its archetype (a
    // usable object's template, e.g. Boyle's bins).
    let mut resolver = crate::collision_audit::PropResolver::new(content);
    let mut asset = |name: &str| -> Option<(String, String)> {
        match obj(c, name) {
            Some(o) => crate::usable_props::defining_package(content, &pkg, ObjRef::from_index(o), packages),
            None => {
                let path = resolver.inherited_object_path(&pkg, ci, name)?;
                locate_path(content, packages, &path)
            }
        }
    };
    let mesh_ref = asset("SkeletalMesh").context("mesh not found")?;
    let phys_ref = asset("PhysicsAsset").context("physics asset not found")?;
    let (mpkg, mi) = export_of(content, &mesh_ref).context("mesh export")?;
    let mesh = read_skeletal_mesh(&mpkg, mi)?;
    let bones: Vec<dishonored_ir::BoneAsset> = mesh
        .bones
        .iter()
        .map(|b| {
            let (rotation, translation) = match pose.get(&b.name.to_ascii_lowercase()) {
                Some((q, t)) => (*q, t.unwrap_or(b.position)),
                None => (b.orientation, b.position),
            };
            dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation, translation }
        })
        .collect();
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
        let bone_world = to_world * g[bi];
        for e in crate::simple_collision::agg_triangles(&ppkg, agg) {
            out.tris.push(e.iter().map(|t| t.map(|v| bone_world.transform_point3(v))).collect());
        }
    }
    Ok(out)
}

/// The local bone pose of the last frame of `anim` from a prop's AnimSets (Edge joints bound to
/// the set's tracks in the mesh's bone order, as the runtime poses doors).
pub fn last_frame_pose(content: &Content, anim_sets: &[(String, String)], bone_order: &[String], anim: &str) -> Option<BonePose> {
    frame_pose(content, anim_sets, bone_order, anim, true)
}

/// The local bone pose of the first (`last` false) or last frame of `anim`.
pub fn frame_pose(content: &Content, anim_sets: &[(String, String)], bone_order: &[String], anim: &str, last: bool) -> Option<BonePose> {
    for (pkg, set) in anim_sets.iter().rev() {
        let Ok(mut lib) = crate::characters::load_anim_set(content, pkg, set, &[anim]) else { continue };
        let Some(clip) = lib.clips.values().next().cloned() else { continue };
        let idx = |n: &String| bone_order.iter().position(|b| b.eq_ignore_ascii_case(n)).unwrap_or(usize::MAX);
        lib.tracks.sort_by_key(idx);
        let mut pose = BonePose::new();
        // The last frame (sampling at the duration would wrap to the first one).
        let frame = if last { clip.num_frames.checked_sub(1)? } else { 0 };
        for (j, tn) in lib.tracks.iter().enumerate() {
            let q = clip.rotations.get(frame).and_then(|f| f.get(j)).copied().flatten();
            let t = clip.translations.get(frame).and_then(|f| f.get(j)).copied().flatten();
            if let Some(q) = q {
                pose.insert(tn.to_ascii_lowercase(), (q, t));
            }
        }
        return Some(pose);
    }
    None
}

/// The bodies of a usable object (not a door) as the map starts: posed by the first frame of its
/// first stage's transition animation (`m_Stages[0].m_TransitionAnimName` of its usable-object
/// tweak), else at the reference pose.
pub fn usable_start_bodies(content: &Content, packages: &[String], prop: &dishonored_ir::SkeletalProp, tweak: Option<&crate::tweaks::TMap>) -> Result<PropBodies> {
    let anim = tweak.and_then(|t| match t.get("m_Stages") {
        Some(crate::tweaks::TVal::List(l)) => match l.first().and_then(|s| s.get("m_TransitionAnimName")) {
            Some(crate::tweaks::TVal::Name(n)) if n != "None" => Some(n.clone()),
            _ => None,
        },
        _ => None,
    });
    let mut pose = BonePose::new();
    if let Some(anim) = anim {
        let (mpkg, mi) = export_of(content, &prop.mesh).context("usable mesh")?;
        let order: Vec<String> = read_skeletal_mesh(&mpkg, mi)?.bones.iter().map(|b| b.name.clone()).collect();
        if let Some(p) = frame_pose(content, &prop.anim_sets, &order, &anim, false) {
            pose = p;
        }
    }
    read_prop_bodies(content, packages, &prop.actor, &pose)
}

/// A door's bodies in every state it can rest in: (`EDisDoorState`, triangles); 0 closed (the
/// reference pose), 1 opened clockwise, 2 opened counter-clockwise (the last frame of the open
/// animation of its tweak's stage).
pub fn door_states(content: &Content, packages: &[String], prop: &dishonored_ir::SkeletalProp) -> Result<Vec<(u8, Vec<Vec<[Vec3; 3]>>)>> {
    let door = prop.door.as_ref().context("not a door")?;
    let mut out = vec![(0u8, read_prop_bodies(content, packages, &prop.actor, &BonePose::new())?.tris)];
    let (mpkg, mi) = export_of(content, &prop.mesh).context("door mesh")?;
    let order: Vec<String> = read_skeletal_mesh(&mpkg, mi)?.bones.iter().map(|b| b.name.clone()).collect();
    for (state, anim) in [(1u8, &door.open_cw), (2u8, &door.open_ccw)] {
        if anim.is_empty() {
            continue;
        }
        if let Some(pose) = last_frame_pose(content, &prop.anim_sets, &order, anim) {
            out.push((state, read_prop_bodies(content, packages, &prop.actor, &pose)?.tris));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(t: &[Vec<[Vec3; 3]>]) -> (Vec3, Vec3) {
        t.iter().flatten().flatten().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)))
    }

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
        assert_eq!(b[0].tris.len(), 2, "two box leaves");
        let (lo, hi) = bounds(&b[0].tris);
        eprintln!("front door bodies {lo:.0?}..{hi:.0?}");
        assert!(lo.y < 1360.0 && hi.y > 1490.0, "{lo:?}..{hi:?}");
        assert!(lo.x > 1400.0 && hi.x < 1600.0, "{lo:?}..{hi:?}");
        assert!(lo.z < 20.0 && hi.z > 250.0, "{lo:?}..{hi:?}");
    }

    /// A Boyle `DisDoor` opened swings its leaf out of the closed doorway.
    #[test]
    fn disdoor_open_pose_moves_the_leaf() {
        let Ok(c) = Content::locate() else { return };
        if !c.has_package("L_Boyle_Ext_P") {
            return;
        }
        let pk = crate::map_packages(&c, "L_Boyle_Ext_P");
        let props = crate::usable_props::read_props(&c, &pk);
        let Some(d) = props.iter().find(|p| p.class == "DisDoor" && p.door.as_ref().is_some_and(|d| !d.open_cw.is_empty())) else { return };
        let s = door_states(&c, &pk, d).unwrap();
        assert!(s.len() >= 2, "{}: {} states", d.actor, s.len());
        let (c0, c1) = (bounds(&s[0].1), bounds(&s[1].1));
        eprintln!("{} closed {c0:.0?} open {c1:.0?}", d.actor);
        assert!((c0.0 - c1.0).length() + (c0.1 - c1.1).length() > 20.0, "open pose equals the closed one");
    }
}
