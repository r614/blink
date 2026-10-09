//! Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission. Placing a door's keyhole socket through the mesh's bind pose follows dhcook/src/lib.rs keyhole_socket and the door scan; the socket facing, the physics-asset wire bodies and the damage-type table are decoded here.
//!
//! Door keyholes, tripwires and projectile launchers of a map (`dishonored_ir::interactables`).
//!
//! The launcher scan (a `DisProjectileLauncher` whose tweak `DisTweaks_ProjectileLauncher` puts
//! its mesh and physics asset on the component at runtime, like the tripwire's) follows the same
//! idea (Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with
//! permission: the muzzle socket through the mesh's bind pose, from dhcook/src/lib.rs): the muzzle
//! socket and its forward axis, the target actor's location, the collision bounds, the
//! projectile tweak with the launcher's own speed and damage, its explosion tweak, the harvest
//! counts and the interactable tweaks' texts.
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
use crate::tweaks::{TVal, TweakDb, get, get_f32, get_obj};
use anyhow::Result;
use dishonored_ir::interactables::{Interactables, KeyholeDef, LauncherDef, LauncherProjectile, ShotKind, TripwireDef};
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

/// An object of the map's packages or the shared libraries by its `Pkg.Obj` path.
fn locate_object(content: &Content, packages: &[String], path: &str) -> Option<(std::sync::Arc<Package>, usize)> {
    locate_object_where(content, packages, path, |_, _| true)
}

/// Like [`locate_object`], taking the first object `accept` agrees to (a mesh whose data reads:
/// a level script package can hold a data-less export of a mesh that lives in another package).
fn locate_object_where(content: &Content, packages: &[String], path: &str, accept: impl Fn(&std::sync::Arc<Package>, usize) -> bool) -> Option<(std::sync::Arc<Package>, usize)> {
    for q in packages.iter().map(String::as_str).chain(crate::level::LIBRARY_PACKAGES.iter().copied()) {
        let Ok(q) = content.package(q) else { continue };
        if let Some(e) = (0..q.exports.len()).find(|&e| {
            let op = q.obj_path(ObjRef::Export(e));
            (op.eq_ignore_ascii_case(path) || path.to_ascii_lowercase().ends_with(&format!(".{}", op.to_ascii_lowercase()))) && accept(&q, e)
        }) {
            return Some((q, e));
        }
    }
    None
}

/// `DefaultAI.ini` `[DishonoredGame.DisAINoiseManager] m_LoudnessDistanceArray` (what the
/// `EAINoiseLoudness` names of the explosion tweaks mean as distances).
fn loudness_table(content: &Content) -> Vec<f32> {
    let ini = crate::ini::Ini::load_layered(&content.root, "AI");
    let d: Vec<f32> = ini.sections.get("DishonoredGame.DisAINoiseManager").and_then(|s| s.get("m_LoudnessDistanceArray")).map(|v| v.iter().filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
    if d.is_empty() { dishonored_ir::ai::AiConfig::default().loudness_distances } else { d }
}

/// A `DisTweaks_Explosion` as the grenades and the Tallboy's arrow read it (Normal difficulty).
fn explosion_config(db: &TweakDb, path: &str, loudness: &[f32]) -> Option<dishonored_ir::items::ExplosionConfig> {
    let e = db.try_resolve(path)?;
    let d = dishonored_ir::items::ExplosionConfig::default();
    Some(dishonored_ir::items::ExplosionConfig {
        radial_damage: get_f32(&e, "m_RadialDamage.m_fBaseValue2_Normal").unwrap_or(d.radial_damage),
        damage_radius: get_f32(&e, "m_fDamageRadius").unwrap_or(d.damage_radius),
        full_damage_radius: get_f32(&e, "m_fFullDamageRadius").unwrap_or(d.full_damage_radius),
        player_damage_radius: get_f32(&e, "m_fPlayerDamageRadius").unwrap_or(d.player_damage_radius),
        player_full_damage_radius: get_f32(&e, "m_fPlayerFullDamageRadius").unwrap_or(d.player_full_damage_radius),
        radial_momentum: get_f32(&e, "m_fRadialMomentum").unwrap_or(d.radial_momentum),
        extra_up_force: get_f32(&e, "m_fExtraUpForce").unwrap_or(d.extra_up_force),
        // An arrow's explosion has no attached damage (no pawn it is stuck to).
        attached_damage: 0.0,
        ai_noise_radius: crate::items_config::loudness_radius(&e, "m_AINoiseLoudness", loudness).unwrap_or(d.ai_noise_radius),
        radial_damage_type: match get(&e, "m_pRadialDamageType") {
            Some(TVal::Obj(o)) => o.rsplit('.').next().unwrap_or(o).to_string(),
            _ => d.radial_damage_type,
        },
    })
}

/// The launcher's projectile (`m_pProjectileTweak` with the launcher's own speed and damage).
fn launcher_projectile(db: &TweakDb, t: &crate::tweaks::TMap, loudness: &[f32]) -> LauncherProjectile {
    let tweak = get_obj(t, "m_pProjectileTweak").unwrap_or_default().to_string();
    let pm = db.try_resolve(&tweak).unwrap_or_default();
    let class = db.class_of(&tweak).unwrap_or_default();
    let explosion = get_obj(&pm, "m_pExplosionTweak").and_then(|p| explosion_config(db, p, loudness));
    LauncherProjectile {
        kind: if explosion.is_some() { ShotKind::ExplosiveArrow } else { ShotKind::Bolt },
        class,
        speed: get_f32(t, "m_fProjectileSpeed").unwrap_or(0.0) * get_f32(&pm, "m_fSpeedMultiplier").unwrap_or(1.0),
        damage: get_f32(t, "m_fProjectileDamage").unwrap_or(0.0) as i32,
        headshot_mult: get_f32(t, "m_fProjectileHeadshotMult").unwrap_or(1.0),
        damage_multiplier: get_f32(&pm, "m_fDamageMultiplier").unwrap_or(1.0),
        gravity_mult: get_f32(&pm, "m_fGravityMultiplier").unwrap_or(1.0),
        invisible: matches!(get(&pm, "m_bInvisible"), Some(TVal::B(true))),
        explosion,
        tweak,
    }
}

/// A placed `DisProjectileLauncher`: its tweak (`DisTweaks_ProjectileLauncher`; the mesh and
/// physics asset are put on the component at runtime like the tripwire's), the muzzle socket
/// through the mesh's bind pose, the target actor's location, the collision bounds the focus
/// traces, and the interactable tweak's texts.
fn launcher(content: &Content, packages: &[String], pkg: &std::sync::Arc<Package>, i: usize, db: &TweakDb, loudness: &[f32]) -> Option<LauncherDef> {
    let h = pkg.read_object(i).ok()?;
    let p = &h.properties;
    let mut res = crate::collision_audit::PropResolver::new(content);
    let tweak_path = res.inherited_object_path(pkg, i, "m_pProjectileLauncherTweaks")?;
    let t = db.try_resolve(&tweak_path)?;
    let text = |m: &crate::tweaks::TMap, k: &str| match get(m, k) {
        Some(TVal::Name(n)) if n != "None" => n.clone(),
        Some(TVal::Str(s)) => s.clone(),
        Some(TVal::Obj(o)) if !o.is_empty() && o != "None" => o.clone(),
        _ => String::new(),
    };
    let (mpkg, mi) = locate_object_where(content, packages, &text(&t, "m_UnbrokenMeshInfo.m_pMesh"), |q, e| q.export_class(e) == "SkeletalMesh" && read_skeletal_mesh(q, e).is_ok())?;
    let mesh = read_skeletal_mesh(&mpkg, mi).ok()?;
    let bones: Vec<dishonored_ir::BoneAsset> = mesh.bones.iter().map(|b| dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
    let g = crate::characters::bind_globals(&bones);
    let m2c = crate::util::rotator_matrix(mesh.rot_origin[0], mesh.rot_origin[1], mesh.rot_origin[2]) * Mat4::from_translation(mesh.mesh_origin);
    let comp = obj(p, "SkeletalMeshComponent").or_else(|| obj(p, "CollisionComponent"));
    let comp_tf = match comp.map(ObjRef::from_index) {
        Some(ObjRef::Export(c)) => pkg.read_object(c).ok().map(|ch| crate::level::component_transform(&ch.properties)).unwrap_or(Mat4::IDENTITY),
        _ => Mat4::IDENTITY,
    };
    let to_world = crate::level::actor_transform(p) * comp_tf * m2c;
    // `m_FiringSocketName` (class default `Muzzle`): the socket on its bone.
    let socket = Some(text(&t, "m_FiringSocketName")).filter(|s| !s.is_empty()).unwrap_or_else(|| "Muzzle".to_string());
    let sockets = crate::characters::mesh_sockets(content, &mpkg.name, &mpkg.obj_path(ObjRef::Export(mi))).ok()?;
    let s = sockets.iter().find(|s| s.name.eq_ignore_ascii_case(&socket))?;
    let bi = bones.iter().position(|b| b.name.eq_ignore_ascii_case(&s.bone))?;
    let local = Mat4::from_translation(s.location) * crate::util::rotator_matrix(s.rotation[0], s.rotation[1], s.rotation[2]);
    let muzzle = to_world * g[bi] * local;
    // The collision bodies of the tweak's physics asset at the bind pose.
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    if let Some((ppkg, pi)) = locate_object(content, packages, &text(&t, "m_UnbrokenMeshInfo.m_pPhysicsAsset")) {
        if let Ok(ph) = ppkg.read_object(pi) {
            for b in find(&ph.properties, "BodySetup").and_then(|x| x.array_objects().ok()).unwrap_or_default() {
                let ObjRef::Export(e) = ObjRef::from_index(b) else { continue };
                let Ok(bh) = ppkg.read_object(e) else { continue };
                let Some(bone) = name_prop(&bh.properties, "BoneName") else { continue };
                let Some(bi) = bones.iter().position(|x| x.name.eq_ignore_ascii_case(&bone)) else { continue };
                let Some(Value::Struct(_, agg)) = find(&bh.properties, "AggGeom").map(|x| &x.value) else { continue };
                let bone_world = to_world * g[bi];
                for tris in crate::simple_collision::agg_triangles(&ppkg, agg) {
                    for tri in tris {
                        for v in tri {
                            let w = bone_world.transform_point3(v);
                            lo = lo.min(w);
                            hi = hi.max(w);
                        }
                    }
                }
            }
        }
    }
    let position = find(p, "Location").and_then(|x| x.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
    if lo.x > hi.x {
        lo = muzzle.w_axis.truncate();
        hi = lo;
    }
    // `m_pTarget`: a `TargetPoint` placed in the level.
    let target = obj(p, "m_pTarget").and_then(|o| match ObjRef::from_index(o) {
        ObjRef::Export(e) => pkg.read_object(e).ok().and_then(|th| find(&th.properties, "Location").and_then(|x| x.as_vector())).map(Vec3::from),
        _ => None,
    });
    // The interactable tweaks: the launcher's (prompt, name, use message, reach) and the
    // projectile's (`m_CannotUseMessage`, shown when there was nothing to take: 0x00A3B6E0).
    let it = get_obj(&t, "m_pInteractableTweaks").and_then(|o| db.try_resolve(o)).unwrap_or_default();
    let projectile = launcher_projectile(db, &t, loudness);
    let pit = db.try_resolve(&projectile.tweak).and_then(|pm| get_obj(&pm, "m_pInteractableTweaks").map(str::to_string)).and_then(|o| db.try_resolve(&o)).unwrap_or_default();
    let harvest: Vec<(u8, i32)> = (0..12u8).filter_map(|a| get_f32(&t, &if a == 0 { "m_HarvestedAmmo".to_string() } else { format!("m_HarvestedAmmo[{a}]") }).map(|n| (a, n as i32)).filter(|(_, n)| *n != 0)).collect();
    let item = text(&t, "m_pItemRequiredToHarvestAmmo");
    Some(LauncherDef {
        name: pkg.obj_name(ObjRef::Export(i)).to_string(),
        package: pkg.name.clone(),
        position,
        muzzle: muzzle.w_axis.truncate(),
        muzzle_forward: muzzle.x_axis.truncate().normalize_or_zero(),
        target,
        armed: !matches!(find(p, "m_bArmed").map(|x| &x.value), Some(Value::Bool(false))),
        bounds_min: lo,
        bounds_max: hi,
        tripped_anim: text(&t, "m_TrippedAnimName"),
        deactivated_anim: text(&t, "m_DeactivatedAnimName"),
        projectile,
        harvest,
        required_item: item.rsplit('.').next().unwrap_or_default().to_string(),
        label: text(&it, "m_Name"),
        prompt: text(&it, "m_InteractText"),
        use_message: text(&it, "m_UseMessage"),
        cannot_use_message: text(&pit, "m_CannotUseMessage"),
        max_distance: get_f32(&it, "m_fMaxDistance").unwrap_or(200.0),
    })
}

/// Read the keyholes, tripwires and launchers of the map's packages.
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
    let loudness = loudness_table(content);
    let mut seen = std::collections::BTreeSet::new();
    for name in &pkgs {
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        for i in 0..pkg.exports.len() {
            if pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            match pkg.export_class(i).as_str() {
                "DisTripwire" => out.tripwires.extend(tripwire(content, &pkgs, &pkg, i, &db)),
                "DisProjectileLauncher" => out.launchers.extend(launcher(content, &pkgs, &pkg, i, &db, &loudness)),
                _ => {}
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

    /// The shipped launchers (skipped without the retail install): the Prison Sewer's three tripods
    /// fire the trap's explosive arrow (`TrapExplosiveArrow_twk`, speed 2000, `m_Damage` 10,
    /// explosion `TrapArrowExplosion_twk`) at a TargetPoint and give one incendiary bolt; the
    /// Overseers' Back one fires a plain arrow and gives one bolt.
    #[test]
    fn retail_launchers() {
        let Ok(content) = Content::locate() else { return };
        let sewer = extract_interactables(&content, &crate::map_packages(&content, "L_PrsnSewer_P")).unwrap();
        assert_eq!(sewer.launchers.len(), 3);
        let loud4 = loudness_table(&content)[15];
        for l in &sewer.launchers {
            assert!(l.armed, "{}", l.name);
            assert_eq!(l.projectile.kind, ShotKind::ExplosiveArrow);
            assert_eq!(l.projectile.class, "DisTweaks_Arrow_Explosive");
            assert_eq!((l.projectile.speed, l.projectile.damage, l.projectile.headshot_mult), (2000.0, 10, 1.0));
            assert_eq!((l.projectile.damage_multiplier, l.projectile.gravity_mult, l.projectile.invisible), (1.0, 0.5, true));
            let x = l.projectile.explosion.as_ref().expect("m_pExplosionTweak");
            assert_eq!((x.radial_damage, x.damage_radius, x.full_damage_radius, x.player_damage_radius, x.player_full_damage_radius), (30.0, 600.0, 300.0, 600.0, 150.0));
            assert_eq!((x.radial_momentum, x.extra_up_force, x.ai_noise_radius), (1500.0, 500.0, loud4));
            assert_eq!(x.radial_damage_type, "DishonoredDamageType_Explosion");
            assert_eq!(l.harvest, vec![(4, 1)], "one incendiary bolt");
            assert_eq!((l.tripped_anim.as_str(), l.deactivated_anim.as_str()), ("TripodTrap_Fire", "TripodTrap_Defuse"));
            assert_eq!((l.prompt.as_str(), l.label.as_str(), l.use_message.as_str(), l.max_distance), ("`GBA_Use` Disarm", "Projectile Launcher", "Launcher disarmed", 200.0));
            assert!(l.target.is_some(), "{} has a TargetPoint", l.name);
            assert!(l.muzzle.distance(l.position) < 150.0, "{}: the muzzle {:?} is on the tripod at {:?}", l.name, l.muzzle, l.position);
            assert!((l.muzzle_forward.length() - 1.0).abs() < 1e-3);
            assert!(l.bounds_min.cmplt(l.bounds_max).all(), "{} has collision bodies", l.name);
        }
        let back = extract_interactables(&content, &crate::map_packages(&content, "L_Ovrsr_Back_P")).unwrap();
        assert_eq!(back.launchers.len(), 1);
        let l = &back.launchers[0];
        assert_eq!(l.projectile.kind, ShotKind::Bolt);
        assert_eq!((l.projectile.speed, l.projectile.damage, l.projectile.gravity_mult), (2000.0, 10, 1.0));
        assert_eq!(l.harvest, vec![(2, 1)], "one bolt");
        let dist = extract_interactables(&content, &crate::map_packages(&content, "L_Distillery_P")).unwrap();
        assert_eq!(dist.launchers.len(), 4);
        assert_eq!(dist.launchers.iter().filter(|l| l.target.is_none()).count(), 2, "two aim along their muzzle");
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
            for l in &ia.launchers {
                println!("  launcher {}:{} at {:?} muzzle {:?} fwd {:?} target {:?} armed {} bounds {:?}..{:?} {:?} harvest {:?}", l.package, l.name, l.position, l.muzzle, l.muzzle_forward, l.target, l.armed, l.bounds_min, l.bounds_max, l.projectile, l.harvest);
            }
        }
    }
}
