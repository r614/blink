//! Pickup actors placed in a map's levels (`DisPickup_Base` subclasses) with what their tweak
//! gives, and the spawners that carry one as `m_pStealablePickup` (pickpocketing).

use crate::content::Content;
use crate::tweaks::{TMap, TVal, TweakDb, get, get_f32, get_obj};
use anyhow::Result;
use dishonored_ir::items::{AmmoType, ElixirType, PickupKind, PlacedPickup};
use glam::Vec3;
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

/// Actor class -> the property holding its tweak.
fn tweak_property(class: &str) -> Option<&'static str> {
    Some(match class {
        "DisAbstractItemPickup" | "DisAbstractItemPickupNote" | "DisAbstractItemPickupAudioLog" | "DisWhaleBoneCharm" | "DisDLC07WhaleBoneCharmCracked" => "m_pTweaks",
        "DisStatPickup" => "m_pStatPickupTweaks",
        "DishonoredInventoryPickup" => "m_pInvPickupTweaks",
        "DisElixirHealth" | "DisElixirMana" => "m_pPickupTweaks",
        _ => return None,
    })
}

fn kind_of(class: &str, tweak: &TMap) -> Option<PickupKind> {
    let obj = |k: &str| match get(tweak, k) {
        Some(TVal::Obj(o)) => o.clone(),
        _ => String::new(),
    };
    Some(match class {
        "DisAbstractItemPickup" | "DisAbstractItemPickupNote" | "DisAbstractItemPickupAudioLog" => PickupKind::Abstract {
            item: obj("m_pAbstractItem"),
            quantity: get_f32(tweak, "m_Quantity").map(|v| v as i32).unwrap_or(1),
            note: class != "DisAbstractItemPickup",
            key_item: matches!(get(tweak, "m_bIsKeyItem"), Some(TVal::B(true))),
        },
        "DisStatPickup" => {
            let mut ammo = [(0i16, 0i16); 8];
            for (i, a) in ammo.iter_mut().enumerate() {
                let k = if i == 0 { "m_AmmoRanges".to_string() } else { format!("m_AmmoRanges[{i}]") };
                let lo = get_f32(tweak, &format!("{k}.m_MinValue")).unwrap_or(0.0) as i16;
                let hi = get_f32(tweak, &format!("{k}.m_MaxValue")).unwrap_or(0.0) as i16;
                *a = (lo, hi.max(lo));
            }
            PickupKind::Stat {
                health: get_f32(tweak, "m_HealthChange").unwrap_or(0.0) as i32,
                mana: get_f32(tweak, "m_ManaChange").unwrap_or(0.0) as i32,
                food: matches!(get(tweak, "m_Type"), Some(TVal::Name(n)) if n == "eStatPickupType_Food"),
                ammo,
            }
        }
        "DisElixirHealth" => PickupKind::Elixir(ElixirType::Health),
        "DisElixirMana" => PickupKind::Elixir(ElixirType::Mana),
        "DisWhaleBoneCharm" => PickupKind::BoneCharm(None),
        "DisDLC07WhaleBoneCharmCracked" => PickupKind::CrackedCharm(None),
        "DishonoredInventoryPickup" => PickupKind::Inventory { item: obj("m_pItem") },
        _ => return None,
    })
}

/// Every pickup placed in `packages` (a map's level packages), with its tweak resolved
/// against the same packages.
pub fn extract_pickups(content: &Content, packages: &[String]) -> Result<Vec<PlacedPickup>> {
    let db = TweakDb::load_with(content, packages)?;
    let mut res = crate::collision_audit::PropResolver::new(content);
    let mut out: Vec<PlacedPickup> = Vec::new();
    let mut stealables: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for name in packages {
        if !content.has_package(name) || !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        for i in 0..pkg.exports.len() {
            let outer = ObjRef::from_index(pkg.exports[i].outer);
            if pkg.obj_name(outer) != "PersistentLevel" {
                continue;
            }
            let class = pkg.export_class(i);
            let is_spawner = matches!(class.as_str(), "DishonoredSpawner" | "DisTravelSpawner");
            let Some(prop) = tweak_property(&class) else {
                if !is_spawner {
                    continue;
                }
                // `m_pStealablePickup`: the pickup this NPC carries.
                let Ok(h) = pkg.read_object(i) else { continue };
                if let Some(Value::Object(o)) = find(&h.properties, "m_pStealablePickup").map(|p| &p.value) {
                    let r = ObjRef::from_index(*o);
                    if r != ObjRef::Null {
                        stealables.push((pkg.obj_name(r).to_string(), pkg.obj_name(ObjRef::Export(i)).to_string()));
                    }
                }
                continue;
            };
            let Ok(h) = pkg.read_object(i) else { continue };
            let position = find(&h.properties, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
            let tweak = match find(&h.properties, prop).map(|p| &p.value) {
                Some(Value::Object(o)) => pkg.obj_path(ObjRef::from_index(*o)),
                _ => String::new(),
            };
            let resolved = db.try_resolve(&tweak).unwrap_or_default();
            let Some(mut kind) = kind_of(&class, &resolved) else { continue };
            // `DisWhaleBoneCharm.m_Effect` on the placed actor (class default -1: no effect).
            if let PickupKind::BoneCharm(e) | PickupKind::CrackedCharm(e) = &mut kind {
                if let Some(Value::Int(v)) = find(&h.properties, "m_Effect").map(|p| &p.value) {
                    *e = (*v >= 0).then_some(*v);
                }
            }
            // A note actor can name its abstract item directly (`m_pAbstractItem` on the actor).
            if let PickupKind::Abstract { item, .. } = &mut kind {
                if item.is_empty() {
                    if let Some(Value::Object(o)) = find(&h.properties, "m_pAbstractItem").map(|p| &p.value) {
                        *item = pkg.obj_path(ObjRef::from_index(*o));
                    }
                }
            }
            // The world box of its mesh (the crosshair's collision stand-in): the mesh's local
            // bounds scaled by `DrawScale` x `DrawScale3D` and turned by the actor's rotation
            // (the component's `Bounds`), relative to the actor's location.
            let (_, mut lo, mut hi) = crate::world_actors::actor_mesh_bounds(&pkg, &h.properties, &mut res);
            // A component without a mesh takes its tweak's (`m_pPickupStaticMesh`, set on the
            // component at runtime: the notes' sheets).
            if !hi.cmpgt(lo).any() {
                if let Some(path) = get_obj(&resolved, "m_pPickupStaticMesh").filter(|p| !p.is_empty() && *p != "None") {
                    if let Some((mp, mi)) = crate::interactables::locate_object_where(content, packages, path, |q, e| q.export_class(e) == "StaticMesh" && crate::static_mesh::read_static_mesh(q, e).is_ok()) {
                        if let Ok(sm) = crate::static_mesh::read_static_mesh(&mp, mi) {
                            lo = sm.bounds_origin - sm.bounds_extent;
                            hi = sm.bounds_origin + sm.bounds_extent;
                        }
                    }
                }
            }
            let eff = res.effective(&pkg, i);
            let rot = find(&eff, "Rotation").and_then(|x| x.as_rotator()).unwrap_or([0; 3]);
            let scale3 = find(&eff, "DrawScale3D").and_then(|x| x.as_vector()).map(Vec3::from).unwrap_or(Vec3::ONE);
            let scale = match find(&eff, "DrawScale").map(|p| &p.value) {
                Some(Value::Float(f)) => *f,
                _ => 1.0,
            };
            let (bounds_min, bounds_max) = if hi.cmpgt(lo).any() { dishonored_ir::items::rotated_box(lo, hi, scale3 * scale, [rot[0] as f32 * (360.0 / 65536.0), rot[1] as f32 * (360.0 / 65536.0), rot[2] as f32 * (360.0 / 65536.0)]) } else { (Vec3::ZERO, Vec3::ZERO) };
            let s = scale3 * scale;
            let (local_min, local_max) = if hi.cmpgt(lo).any() { ((lo * s).min(hi * s), (lo * s).max(hi * s)) } else { (Vec3::ZERO, Vec3::ZERO) };
            let rotation = Vec3::new(rot[0] as f32, rot[1] as f32, rot[2] as f32) * (360.0 / 65536.0);
            out.push(PlacedPickup { name: pkg.obj_name(ObjRef::Export(i)).to_string(), package: name.clone(), position, tweak, kind, stealable_from: None, bounds_min, bounds_max, local_min, local_max, rotation });
        }
    }
    for (pickup, spawner) in stealables {
        if let Some(p) = out.iter_mut().find(|p| p.name == pickup) {
            p.stealable_from = Some(spawner);
        }
    }
    Ok(out)
}

/// The ammo a stat pickup can give (min, max) by type, for display.
pub fn ammo_summary(k: &PickupKind) -> Vec<(AmmoType, i16, i16)> {
    match k {
        PickupKind::Stat { ammo, .. } => ammo.iter().enumerate().filter(|(_, a)| a.1 > 0).filter_map(|(i, a)| AmmoType::from_index(i).map(|t| (t, a.0, a.1))).collect(),
        _ => Vec::new(),
    }
}
