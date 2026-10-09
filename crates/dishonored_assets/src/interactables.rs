//! Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission. Placing a door's keyhole socket through the mesh's bind pose follows dhcook/src/lib.rs keyhole_socket and the door scan; the socket facing, the physics-asset wire bodies and the damage-type table are decoded here.
//!
//! Door keyholes and tripwires of a map (`dishonored_ir::interactables`).
//!
//! - Keyholes: every `DisDoor` whose skeletal mesh has the socket named by the door's
//!   `m_KeyHoleSocketName` (class default `KeyHole`). The socket is placed through the mesh's bind
//!   pose and the door's transform, with the door closed (`GetSocketWorldLocationAndRotation`,
//!   0x00A1EF10, is read from the door's mesh component); its facing is the socket rotation's X
//!   axis. `m_fKeyHoleFOV` (class default 75) is the door's own. The hold time that starts the peek
//!   is `DisTweaks_PlayerInput.m_fAltInteractActivateTime`.
//! - Tripwires: every `DisTripwire`. Its skeletal mesh and physics asset are set at runtime from
//!   its `DisTweaks_Tripwire` (`m_pSkeletalMesh`, `m_pPhysicsAsset`: 0x00A28AC0), so the bodies
//!   are read from the tweak's physics asset at the tweak's mesh bind pose under the actor's
//!   transform.

use crate::content::Content;
use crate::skeletal_mesh::read_skeletal_mesh;
use crate::tweaks::{TVal, TweakDb, get, get_f32};
use anyhow::Result;
use dishonored_ir::interactables::{Interactables, KeyholeDef, TripwireDef};
use glam::{Mat4, Vec3};
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

fn name_prop(props: &[Property], n: &str) -> Option<String> {
    match find(props, n).map(|p| &p.value) {
        Some(Value::Name(s)) if s != "None" => Some(s.clone()),
        _ => None,
    }
}

/// Keyholes of the doors of `packages`.
pub fn keyholes(content: &Content, packages: &[String]) -> Vec<KeyholeDef> {
    let mut out = Vec::new();
    for prop in crate::usable_props::read_props(content, packages) {
        if prop.class != "DisDoor" {
            continue;
        }
        let Some((pkg_name, name)) = prop.actor.split_once(':') else { continue };
        let Ok(pkg) = content.package(pkg_name) else { continue };
        let Some(i) = (0..pkg.exports.len()).find(|&i| pkg.obj_name(ObjRef::Export(i)).eq_ignore_ascii_case(name) && pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) == "PersistentLevel") else { continue };
        let Ok(h) = pkg.read_object(i) else { continue };
        let socket = name_prop(&h.properties, "m_KeyHoleSocketName").unwrap_or_else(|| "KeyHole".to_string());
        let Ok(sockets) = crate::characters::mesh_sockets(content, &prop.mesh.0, &prop.mesh.1) else { continue };
        let Some(s) = sockets.iter().find(|s| s.name.eq_ignore_ascii_case(&socket)) else { continue };
        let Ok(mpkg) = content.package(&prop.mesh.0) else { continue };
        let Some(mi) = (0..mpkg.exports.len()).find(|&e| mpkg.export_class(e) == "SkeletalMesh" && mpkg.obj_path(ObjRef::Export(e)).eq_ignore_ascii_case(&prop.mesh.1)) else { continue };
        let Ok(mesh) = read_skeletal_mesh(&mpkg, mi) else { continue };
        let bones: Vec<dishonored_ir::BoneAsset> = mesh.bones.iter().map(|b| dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
        let g = crate::characters::bind_globals(&bones);
        let Some(bi) = bones.iter().position(|b| b.name.eq_ignore_ascii_case(&s.bone)) else { continue };
        let m2c = crate::util::rotator_matrix(mesh.rot_origin[0], mesh.rot_origin[1], mesh.rot_origin[2]) * Mat4::from_translation(mesh.mesh_origin);
        let to_world = prop.transform * m2c;
        // The socket's own transform on its bone: the rotator is the relative rotation.
        let local = Mat4::from_translation(s.location) * crate::util::rotator_matrix(s.rotation[0], s.rotation[1], s.rotation[2]);
        let socket_world = to_world * g[bi] * local;
        // The closed door's collision bounds (the physics asset at the bind pose): what the
        // interactable focus traces.
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        if let Ok(b) = crate::prop_collision::read_prop_bodies(content, packages, &prop.actor, &crate::prop_collision::BonePose::new()) {
            for t in b.tris.iter().flatten() {
                for v in t {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
        }
        if lo.x > hi.x {
            lo = socket_world.w_axis.truncate();
            hi = lo;
        }
        out.push(KeyholeDef {
            bounds_min: lo,
            bounds_max: hi,
            name: name.to_string(),
            package: pkg_name.to_string(),
            position: find(&h.properties, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO),
            socket: socket_world.w_axis.truncate(),
            facing: socket_world.x_axis.truncate().normalize_or_zero(),
            fov: find(&h.properties, "m_fKeyHoleFOV").and_then(|p| p.as_float()).unwrap_or(75.0),
            initial_state: prop.door.as_ref().map(|d| d.initial).unwrap_or(0),
        });
    }
    out
}

fn obj(props: &[Property], n: &str) -> Option<i32> {
    find(props, n).and_then(|p| p.as_object()).filter(|o| *o != 0)
}

fn tripwire(content: &Content, packages: &[String], pkg: &std::sync::Arc<Package>, i: usize, db: &TweakDb) -> Option<TripwireDef> {
    let h = pkg.read_object(i).ok()?;
    let p = &h.properties;
    let mut res = crate::collision_audit::PropResolver::new(content);
    let tweak_path = res.inherited_object_path(pkg, i, "m_pTripwireTweaks")?;
    let t = db.try_resolve(&tweak_path)?;
    let text = |k: &str| match get(&t, k) {
        Some(TVal::Name(n)) if n != "None" => n.clone(),
        Some(TVal::Obj(o)) if !o.is_empty() && o != "None" => o.clone(),
        _ => String::new(),
    };
    let name = pkg.obj_name(ObjRef::Export(i)).to_string();
    let mut def = TripwireDef { name: name.clone(), package: pkg.name.clone(), position: find(p, "Location").and_then(|x| x.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO), tripped_anim: text("m_TrippedAnimName"), triangles: Vec::new() };
    // The tweak's mesh and physics asset (set on the component at runtime), at the bind pose.
    let mesh_path = text("m_pSkeletalMesh");
    let phys_path = text("m_pPhysicsAsset");
    let locate = |path: &str| -> Option<(std::sync::Arc<Package>, usize)> {
        for q in packages.iter().map(String::as_str).chain(crate::level::LIBRARY_PACKAGES.iter().copied()) {
            let Ok(q) = content.package(q) else { continue };
            if let Some(e) = (0..q.exports.len()).find(|&e| { let op = q.obj_path(ObjRef::Export(e)); op.eq_ignore_ascii_case(path) || path.to_ascii_lowercase().ends_with(&format!(".{}", op.to_ascii_lowercase())) }) {
                return Some((q, e));
            }
        }
        None
    };
    let (mpkg, mi) = locate(&mesh_path)?;
    let mesh = read_skeletal_mesh(&mpkg, mi).ok()?;
    let bones: Vec<dishonored_ir::BoneAsset> = mesh.bones.iter().map(|b| dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
    let g = crate::characters::bind_globals(&bones);
    let m2c = crate::util::rotator_matrix(mesh.rot_origin[0], mesh.rot_origin[1], mesh.rot_origin[2]) * Mat4::from_translation(mesh.mesh_origin);
    // The component transform: the placed component's own offset, when it has one.
    let comp = obj(p, "SkeletalMeshComponent").or_else(|| obj(p, "CollisionComponent"));
    let comp_tf = match comp.map(ObjRef::from_index) {
        Some(ObjRef::Export(c)) => pkg.read_object(c).ok().map(|ch| crate::level::component_transform(&ch.properties)).unwrap_or(Mat4::IDENTITY),
        _ => Mat4::IDENTITY,
    };
    let to_world = crate::level::actor_transform(p) * comp_tf * m2c;
    let (ppkg, pi) = locate(&phys_path)?;
    let ph = ppkg.read_object(pi).ok()?;
    for b in find(&ph.properties, "BodySetup").and_then(|x| x.array_objects().ok()).unwrap_or_default() {
        let ObjRef::Export(e) = ObjRef::from_index(b) else { continue };
        let Ok(bh) = ppkg.read_object(e) else { continue };
        let bp = &bh.properties;
        let Some(bone) = name_prop(bp, "BoneName") else { continue };
        let Some(bi) = bones.iter().position(|x| x.name.eq_ignore_ascii_case(&bone)) else { continue };
        let Some(Value::Struct(_, agg)) = find(bp, "AggGeom").map(|x| &x.value) else { continue };
        let bone_world = to_world * g[bi];
        for tris in crate::simple_collision::agg_triangles(&ppkg, agg) {
            def.triangles.extend(tris.iter().map(|t| t.map(|v| bone_world.transform_point3(v))));
        }
    }
    Some(def)
}

/// Read the keyholes and tripwires of the map's packages.
pub fn extract_interactables(content: &Content, packages: &[String]) -> Result<Interactables> {
    let pkgs: Vec<String> = packages.iter().filter(|p| content.has_package(p)).cloned().collect();
    let db = TweakDb::load_with(content, &pkgs)?;
    let alt = db.try_resolve("Twk_Pawn_DefaultPlayer.Twk_Pawn_PlayerInput").or_else(|| db.class_defaults("DisTweaks_PlayerInput").ok()).and_then(|m| get_f32(&m, "m_fAltInteractActivateTime")).unwrap_or(0.3);
    let mut out = Interactables { keyholes: keyholes(content, &pkgs), alt_interact_time: alt, ..Default::default() };
    for class in db.subclasses_of("DishonoredDamageType") {
        if matches!(db.class_defaults(&class).ok().as_ref().and_then(|m| get(m, "m_bDontTriggerTripWire")), Some(TVal::B(true))) {
            out.no_trip_damage_types.push(class);
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in &pkgs {
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        for i in 0..pkg.exports.len() {
            if pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" || pkg.export_class(i) != "DisTripwire" {
                continue;
            }
            if let Some(t) = tripwire(content, &pkgs, &pkg, i, &db) {
                out.tripwires.push(t);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped data (skipped without the retail install): Boyle Int's doors have keyholes
    /// facing along their leaf's thickness at the door's own field of view, the Prison Sewer's
    /// three tripwires are thin bodies, and only Windblast is exempt from tripping them.
    #[test]
    fn retail_keyholes_and_tripwires() {
        let Ok(content) = Content::locate() else { return };
        let boyle = extract_interactables(&content, &crate::map_packages(&content, "L_Boyle_Int_P")).unwrap();
        assert_eq!(boyle.alt_interact_time, 0.3);
        assert_eq!(boyle.no_trip_damage_types, ["DisDamageType_WindBlast"]);
        assert!(boyle.keyholes.len() >= 17, "{} keyholes", boyle.keyholes.len());
        for k in &boyle.keyholes {
            assert_eq!(k.fov, 75.0, "{}", k.name);
            assert!(k.facing.x.abs().max(k.facing.y.abs()) > 0.99, "{} faces along an axis of the level: {:?}", k.name, k.facing);
            assert!(k.bounds_min.cmplt(k.bounds_max).all(), "{} has its collision bounds", k.name);
            assert!(k.socket.cmpge(k.bounds_min - glam::Vec3::splat(1.0)).all() && k.socket.cmple(k.bounds_max + glam::Vec3::splat(1.0)).all(), "{}: the keyhole is in the door", k.name);
        }
        let sewer = extract_interactables(&content, &crate::map_packages(&content, "L_PrsnSewer_P")).unwrap();
        assert_eq!(sewer.tripwires.len(), 3);
        for w in &sewer.tripwires {
            assert_eq!(w.tripped_anim, "TripodTrap_Fire");
            assert!(!w.triangles.is_empty());
            let (mut lo, mut hi) = (glam::Vec3::splat(f32::INFINITY), glam::Vec3::splat(f32::NEG_INFINITY));
            for v in w.triangles.iter().flatten() {
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
            let size = hi - lo;
            let mut s = [size.x, size.y, size.z];
            s.sort_by(f32::total_cmp);
            assert!(s[0] < 15.0 && s[1] < 15.0 && s[2] > 100.0, "{} is a thin wire: {size:?}", w.name);
        }
    }

    /// Prints the keyholes and tripwires of a few maps (needs the retail install).
    /// `cargo test -p dishonored_assets dump_interactables -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_interactables() {
        let Ok(content) = Content::locate() else { return };
        for map in ["L_PrsnSewer_P", "L_Distillery_P", "L_Ovrsr_Back_P", "L_Boyle_Int_P", "L_Prison_P"] {
            let pkgs = crate::map_packages(&content, map);
            let ia = extract_interactables(&content, &pkgs).unwrap();
            println!("== {map}: {} keyholes, {} tripwires; no-trip damage types {:?}", ia.keyholes.len(), ia.tripwires.len(), ia.no_trip_damage_types);
            for k in &ia.keyholes {
                println!("  keyhole {}:{} socket {:?} facing {:?} fov {} state {} bounds {:?}..{:?}", k.package, k.name, k.socket, k.facing, k.fov, k.initial_state, k.bounds_min, k.bounds_max);
            }
            for t in &ia.tripwires {
                let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
                for tri in &t.triangles {
                    for v in tri {
                        lo = lo.min(*v);
                        hi = hi.max(*v);
                    }
                }
                println!("  tripwire {} at {:?} anim {:?} tris {} bounds {:?}..{:?}", t.name, t.position, t.tripped_anim, t.triangles.len(), lo, hi);
            }
        }
    }
}
