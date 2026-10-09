//! Placed breakables, whale oil batteries and receptacles of a map (`dishonored_ir::world_actors`).
//!
//! - Breakables: the `DishonoredBreakable` family (`DishonoredMovable`, `DishonoredBreakableNavBlock`,
//!   `DisSpeaker_PA`, `DisWhaleOilBattery`, ...) with their `DisTweaks_StaticBreakable`
//!   (`m_pBreakableTweaks`, or `m_pMovableTweaks` / `m_pWhaleOilTweaks` for the subclasses):
//!   `m_Health`, `m_DamageThreshold`, `m_Steps`, the immune / instant-break damage types and the
//!   loot step. Actors whose tweak leaves `m_Health` at -1 never take damage (0x00a26a90 tests
//!   `m_CurHealth >= 0`) and are skipped, except batteries (their charges set the health).
//! - Receptacles: `DisWhaleOilReceptacle` with its `DisTweaks_WhaleOilReceptacle` and the battery
//!   placed in it (`m_pBattery`).

use crate::ai_level::LOUDNESS_NAMES;
use crate::collision_audit::PropResolver;
use crate::content::Content;
use crate::tweaks::{TMap, TVal, TweakDb, get, get_f32};
use anyhow::Result;
use dishonored_ir::ai::NoiseContext;
use dishonored_ir::world_actors::{BatteryDef, BreakStepDef, BreakableDef, MovableDef, ReceptacleDef, WallOfLightDef, WorldActors};
use glam::Vec3;
use std::sync::Arc;
use ue3_package::properties::find;
use ue3_package::{ObjRef, Package};

/// Placed classes of the `DishonoredBreakable` family (`oracle index class`).
pub const BREAKABLE_CLASSES: &[&str] = &["DishonoredBreakable", "DishonoredBreakableNavBlock", "DishonoredMovable", "DisSpeaker_PA", "DisWhaleOilBattery", "DisDLC05WhaleOilBattery", "DisDLC05_Movable"];

fn int(m: &TMap, k: &str) -> Option<i32> {
    match get(m, k)? {
        TVal::I(v) => Some(*v),
        TVal::F(v) => Some(*v as i32),
        _ => None,
    }
}

fn boolean(m: &TMap, k: &str) -> Option<bool> {
    match get(m, k)? {
        TVal::B(v) => Some(*v),
        _ => None,
    }
}

/// Class names of an object list (`array<class<DishonoredDamageType>>`): the last path segment.
fn classes(m: &TMap, k: &str) -> Vec<String> {
    match m.get(k) {
        Some(TVal::List(l)) => l.iter().filter_map(|e| e.values().find_map(|v| if let TVal::Obj(s) = v { Some(s.rsplit('.').next().unwrap_or(s).to_string()) } else { None })).filter(|s| !s.is_empty() && s != "None").collect(),
        _ => Vec::new(),
    }
}

fn noise_radius(m: &TMap, loudness: &[f32]) -> f32 {
    match m.get("m_AINoiseLoudness") {
        Some(TVal::Name(n)) => LOUDNESS_NAMES.iter().position(|x| x == n).and_then(|i| loudness.get(i).copied()).unwrap_or(0.0),
        Some(TVal::I(v)) => loudness.get(*v as usize).copied().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn steps(m: &TMap, loudness: &[f32]) -> Vec<BreakStepDef> {
    let Some(TVal::List(l)) = m.get("m_Steps") else { return Vec::new() };
    l.iter()
        .map(|s| BreakStepDef {
            health_threshold: int(s, "m_HealthThreshold").unwrap_or(0),
            step_timer: get_f32(s, "m_fStepTimer").unwrap_or(0.0),
            sound: matches!(s.get("m_pSoundEvent"), Some(TVal::Obj(o)) if !o.is_empty() && o != "None"),
            noise_radius: noise_radius(s, loudness),
            noise_context: match s.get("m_AINoiseContext") {
                Some(TVal::Name(n)) => NoiseContext::from_name(n).unwrap_or_default(),
                Some(TVal::I(v)) => NoiseContext::ALL.get(*v as usize).copied().unwrap_or_default(),
                _ => NoiseContext::default(),
            },
            explosion: matches!(s.get("m_pExplosion"), Some(TVal::Obj(o)) if !o.is_empty() && o != "None"),
        })
        .collect()
}

fn breakable(pkg: &Arc<Package>, i: usize, package: &str, class: &str, db: &TweakDb, res: &mut PropResolver, loudness: &[f32]) -> Option<BreakableDef> {
    let battery = class.contains("WhaleOilBattery");
    let names: &[&str] = if battery { &["m_pWhaleOilTweaks", "m_pMovableTweaks", "m_pBreakableTweaks"] } else { &["m_pBreakableTweaks", "m_pMovableTweaks"] };
    let tweak = names.iter().find_map(|n| res.inherited_object_path(pkg, i, n))?;
    let m = db.try_resolve(&tweak)?;
    let health = int(&m, "m_Health").unwrap_or(-1);
    if health < 0 && !battery {
        return None;
    }
    let props = pkg.read_object(i).ok()?.properties;
    let position = Vec3::from(find(&props, "Location").and_then(|x| x.as_vector()).unwrap_or([0.0; 3]));
    let battery = battery.then(|| BatteryDef {
        max_charges: int(&m, "m_NumberOfCharges").unwrap_or(0),
        initial_charges: int(&m, "m_InitialNumberOfCharges").unwrap_or(0),
        pawn_cost: int(&m, "m_PawnChargeCost").unwrap_or(0),
        animal_cost: int(&m, "m_AmbientAnimalChargeCost").unwrap_or(0),
        watchtower_cost: int(&m, "m_WatchtowerChargeCost").unwrap_or(0),
        ignore_shield: boolean(&m, "m_bIgnoreShield").unwrap_or(false),
        eject_speed: get_f32(&m, "m_fEjectSpeed").unwrap_or(0.0),
    });
    Some(BreakableDef {
        name: pkg.obj_name(ObjRef::Export(i)).to_string(),
        package: package.to_string(),
        class: class.to_string(),
        position,
        tweak,
        health,
        damage_threshold: int(&m, "m_DamageThreshold").unwrap_or(0),
        steps: steps(&m, loudness),
        immune: classes(&m, "m_pImmuneToDamageTypes"),
        instant_break: classes(&m, "m_pInstantBreakDamageTypes"),
        loot_step: int(&m, "m_WhichStepDropsLoot").unwrap_or(0),
        battery,
    })
}

/// Placed classes of the movables (`DishonoredMovable` and the DLC05 subclass).
pub const MOVABLE_CLASSES: &[&str] = &["DishonoredMovable", "DisDLC05_Movable"];

fn weight_class(m: &TMap) -> u8 {
    match get(m, "m_MovableParams.m_WeightClass") {
        Some(TVal::Name(n)) => ["MWC_Tiny", "MWC_Small", "MWC_Medium", "MWC_Large"].iter().position(|x| x == n).unwrap_or(0) as u8,
        Some(TVal::I(v)) => (*v).clamp(0, 3) as u8,
        _ => 0,
    }
}

/// The static mesh of an actor's `StaticMeshComponent` (its own, else the archetype's): its path
/// and its local bounding box (min, max; zero when the mesh is not found).
pub(crate) fn actor_mesh_bounds(pkg: &Arc<Package>, own: &[ue3_package::properties::Property], res: &mut PropResolver) -> (String, Vec3, Vec3) {
    let mut mesh = String::new();
    let (mut bmin, mut bmax) = (Vec3::ZERO, Vec3::ZERO);
    let comp = find(own, "StaticMeshComponent").and_then(|p| p.as_object()).filter(|o| *o != 0).map(|o| ObjRef::from_index(o));
    if let Some(ObjRef::Export(ci)) = comp.filter(|c| matches!(c, ObjRef::Export(_))) {
        let cown = pkg.read_object(ci).map(|h| h.properties).unwrap_or_default();
        let located = match find(&cown, "StaticMesh").and_then(|p| p.as_object()).filter(|o| *o != 0) {
            Some(o) => {
                mesh = pkg.obj_path(ObjRef::from_index(o));
                res.locate(pkg, ObjRef::from_index(o), &[])
            }
            None => res.inherited_object_path(pkg, ci, "StaticMesh").and_then(|p| {
                mesh = p.clone();
                res.locate_path(&p)
            }),
        };
        if let Some((mp, mi)) = located {
            if let Ok(sm) = crate::static_mesh::read_static_mesh(&mp, mi) {
                bmin = sm.bounds_origin - sm.bounds_extent;
                bmax = sm.bounds_origin + sm.bounds_extent;
            }
        }
    }
    (mesh, bmin, bmax)
}

/// A placed movable: its `DisTweaks_Movable` (class defaults `DisTweaks_Movable` /
/// `DisTweaks_StaticBreakable` when the tweak leaves a field), the static mesh and its box.
fn movable(content: &Content, pkg: &Arc<Package>, i: usize, package: &str, class: &str, db: &TweakDb, res: &mut PropResolver) -> Option<MovableDef> {
    let tweak = res.inherited_object_path(pkg, i, "m_pMovableTweaks")?;
    let m = db.try_resolve(&tweak)?;
    let own = pkg.read_object(i).ok()?.properties;
    let eff = res.effective(pkg, i);
    let rot = find(&eff, "Rotation").and_then(|x| x.as_rotator()).unwrap_or([0; 3]);
    let flag = |k: &str| boolean(&m, k).unwrap_or(false);
    let interact = get(&m, "m_pInteractableTweaks").and_then(|v| if let TVal::Obj(o) = v { db.try_resolve(o) } else { None }).unwrap_or_default();
    let max_distance = get_f32(&interact, "m_fMaxDistance").unwrap_or(200.0);
    let prompt = match interact.get("m_InteractText") {
        Some(TVal::Str(s)) => s.clone(),
        _ => String::new(),
    };
    // The static mesh of the actor's `StaticMeshComponent` (its own, else the archetype's).
    let (mesh, bmin, bmax) = actor_mesh_bounds(pkg, &own, res);
    let _ = content;
    Some(MovableDef {
        name: pkg.obj_name(ObjRef::Export(i)).to_string(),
        package: package.to_string(),
        class: class.to_string(),
        position: Vec3::from(find(&eff, "Location").and_then(|x| x.as_vector()).unwrap_or([0.0; 3])),
        rotation: Vec3::new(rot[0] as f32, rot[1] as f32, rot[2] as f32) * (360.0 / 65536.0),
        tweak,
        weight: weight_class(&m),
        damage: int(&m, "m_MovableParams.m_Damage").unwrap_or(5),
        interactable: flag("m_bInteractable"),
        key_item: flag("m_bIsKeyItem"),
        blocks_player: flag("m_bBlocksPlayer"),
        never_blocks_player: flag("m_bNeverBlocksPlayer"),
        max_distance,
        prompt,
        min_speed_to_cause_damage: get_f32(&m, "m_fMinSpeedToCauseDamage").unwrap_or(250.0),
        min_speed_to_be_damaged: get_f32(&m, "m_fMinSpeedToBeDamaged").unwrap_or(100.0),
        min_speed_to_be_damaged_on_drop: get_f32(&m, "m_fMinSpeedToBeDamagedOnDrop").unwrap_or(800.0),
        contact_type: match m.get("m_pContactTypeOverride") {
            Some(TVal::Obj(o)) | Some(TVal::Name(o)) => o.rsplit('.').next().unwrap_or(o).to_string(),
            _ => String::new(),
        },
        mesh,
        bounds_min: bmin,
        bounds_max: bmax,
        breakable: None,
    })
}

fn receptacle(pkg: &Arc<Package>, i: usize, package: &str, db: &TweakDb, res: &mut PropResolver) -> Option<ReceptacleDef> {
    let props = pkg.read_object(i).ok()?.properties;
    let tweak = res.inherited_object_path(pkg, i, "m_pWhaleOilTweaks").unwrap_or_default();
    let m = db.try_resolve(&tweak).unwrap_or_default();
    let name_of = |o: i32| (o != 0).then(|| pkg.obj_name(ObjRef::from_index(o)).to_string()).filter(|s| !s.is_empty() && s != "None");
    Some(ReceptacleDef {
        name: pkg.obj_name(ObjRef::Export(i)).to_string(),
        package: package.to_string(),
        position: Vec3::from(find(&props, "Location").and_then(|x| x.as_vector()).unwrap_or([0.0; 3])),
        battery: find(&props, "m_pBattery").and_then(|x| x.as_object()).and_then(name_of),
        allow_empty: boolean(&m, "m_bAllowEmptyBatteries").unwrap_or(false),
        detection_radius: get_f32(&m, "m_fDetectionRadius").unwrap_or(0.0),
        detection_height: get_f32(&m, "m_fDetectionHeight").unwrap_or(0.0),
        attached: find(&props, "m_aAttachedActors").and_then(|x| x.array_objects().ok()).unwrap_or_default().into_iter().filter_map(name_of).collect(),
    })
}

/// Object names of a tweak's object list (`array<DisTweaks_Faction>`): full paths.
fn object_list(m: &TMap, k: &str) -> Vec<String> {
    match m.get(k) {
        Some(TVal::List(l)) => l.iter().filter_map(|e| e.values().find_map(|v| if let TVal::Obj(s) = v { Some(s.clone()) } else { None })).filter(|s| !s.is_empty() && s != "None").collect(),
        _ => Vec::new(),
    }
}

/// A placed `DisWallOfLight`: its tweak (`m_pWallOfLightTweaks`), placed flags and friendly
/// factions override, and its static mesh's collision triangles under the actor's and the
/// component's transforms. The receptacle is resolved by the caller.
fn wall_of_light(content: &Content, packages: &[String], pkg: &Arc<Package>, i: usize, package: &str, db: &TweakDb, res: &mut PropResolver) -> Option<(WallOfLightDef, Option<String>)> {
    let tweak = res.inherited_object_path(pkg, i, "m_pWallOfLightTweaks")?;
    let m = db.try_resolve(&tweak)?;
    let own = pkg.read_object(i).ok()?.properties;
    let eff = res.effective(pkg, i);
    let flag = |k: &str, d: bool| match find(&eff, k).map(|x| &x.value) {
        Some(ue3_package::properties::Value::Bool(b)) => *b,
        _ => d,
    };
    let over: Vec<String> = find(&own, "m_FriendlyFactionsOverride").and_then(|x| x.array_objects().ok()).unwrap_or_default().into_iter().filter(|&o| o != 0).map(|o| pkg.obj_path(ObjRef::from_index(o))).collect();
    let receptacle = find(&own, "m_pAttachedReceptacle").and_then(|x| x.as_object()).filter(|&o| o != 0).map(|o| pkg.obj_name(ObjRef::from_index(o)).to_string());
    // The collision triangles: the mesh's kDOP tree (per-poly collision), local to world. The
    // tweak's `m_pStaticMesh`, when set, replaces the component's mesh (the tweaks interface's
    // apply, 0x00A29110: `SetStaticMesh`); otherwise the component's own or inherited mesh.
    let mut triangles = Vec::new();
    let comp = find(&own, "StaticMeshComponent").or_else(|| find(&own, "CollisionComponent")).and_then(|p| p.as_object()).filter(|o| *o != 0).map(ObjRef::from_index);
    if let Some(ObjRef::Export(ci)) = comp {
        let cown = pkg.read_object(ci).map(|h| h.properties).unwrap_or_default();
        let tweak_mesh = match m.get("m_pStaticMesh") {
            Some(TVal::Obj(o)) if !o.is_empty() && o != "None" => Some(o.clone()),
            _ => None,
        };
        let located = match (tweak_mesh, find(&cown, "StaticMesh").and_then(|p| p.as_object()).filter(|o| *o != 0)) {
            (Some(p), _) => crate::interactables::locate_object_where(content, packages, &p, |q, e| q.export_class(e) == "StaticMesh" && crate::static_mesh::read_static_mesh(q, e).is_ok()),
            (None, Some(o)) => res.locate(pkg, ObjRef::from_index(o), &[]),
            (None, None) => res.inherited_object_path(pkg, ci, "StaticMesh").and_then(|p| res.locate_path(&p)),
        };
        if let Some((mp, mi)) = located {
            if let Ok(sm) = crate::static_mesh::read_static_mesh(&mp, mi) {
                let tf = crate::level::actor_transform(&eff) * crate::level::component_transform(&cown);
                let at = |k: u16| sm.positions.get(k as usize).map(|&v| tf.transform_point3(v));
                let src: Vec<[u16; 3]> = if sm.kdop_triangles.is_empty() { sm.indices.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect() } else { sm.kdop_triangles.iter().map(|t| [t[0], t[1], t[2]]).collect() };
                triangles = src.iter().filter_map(|t| Some([at(t[0])?, at(t[1])?, at(t[2])?])).collect();
            }
        }
    }
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for v in triangles.iter().flatten() {
        lo = lo.min(*v);
        hi = hi.max(*v);
    }
    let position = Vec3::from(find(&eff, "Location").and_then(|x| x.as_vector()).unwrap_or([0.0; 3]));
    if lo.x > hi.x {
        (lo, hi) = (position, position);
    }
    let def = WallOfLightDef {
        name: pkg.obj_name(ObjRef::Export(i)).to_string(),
        package: package.to_string(),
        position,
        receptacle: None,
        friendly_factions: if over.is_empty() { object_list(&m, "m_FriendlyFactions") } else { over },
        reversed: flag("m_bReversed", false),
        enabled: flag("m_bEnabled", true),
        damage: int(&m, "m_Damage").unwrap_or(0),
        jib: boolean(&m, "m_bJib").unwrap_or(false),
        activation_time: get_f32(&m, "m_ActivationTime").unwrap_or(0.0),
        activation_time_pawn: get_f32(&m, "m_ActivationTimeForPawnEffect").unwrap_or(0.0),
        triangles,
        bounds_min: lo,
        bounds_max: hi,
    };
    Some((def, receptacle))
}

/// Read the world actors of a map. `packages` are the map's packages; `loudness` is
/// `m_LoudnessDistanceArray` (the steps' AI noise loudness as a distance).
pub fn extract_world_actors(content: &Content, packages: &[String], loudness: &[f32]) -> Result<WorldActors> {
    let pkgs: Vec<String> = packages.iter().filter(|p| content.has_package(p)).cloned().collect();
    let db = TweakDb::load_with(content, &pkgs)?;
    let mut res = PropResolver::new(content);
    let mut out = WorldActors::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut wall_receptacles = Vec::new();
    for name in &pkgs {
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        for i in 0..pkg.exports.len() {
            if pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            let class = pkg.export_class(i);
            if MOVABLE_CLASSES.contains(&class.as_str()) {
                if let Some(mut mv) = movable(content, &pkg, i, name, &class, &db, &mut res) {
                    mv.package = name.to_string();
                    out.movables.push(mv);
                }
            }
            if BREAKABLE_CLASSES.contains(&class.as_str()) {
                if let Some(b) = breakable(&pkg, i, name, &class, &db, &mut res, loudness) {
                    out.breakables.push(b);
                }
            } else if class == "DisWhaleOilReceptacle" {
                if let Some(r) = receptacle(&pkg, i, name, &db, &mut res) {
                    out.receptacles.push(r);
                }
            } else if class == "DisWallOfLight" {
                if let Some((w, r)) = wall_of_light(content, &pkgs, &pkg, i, name, &db, &mut res) {
                    out.walls_of_light.push(w);
                    wall_receptacles.push(r);
                }
            }
        }
    }
    // The wall's receptacle: its own `m_pAttachedReceptacle`, else the receptacle that lists it
    // (`m_aAttachedActors`, which registers the wall as a consumer: 0x00C73430).
    for (w, r) in out.walls_of_light.iter_mut().zip(wall_receptacles) {
        w.receptacle = match r {
            Some(n) => out.receptacles.iter().position(|x| x.name.eq_ignore_ascii_case(&n) && x.package.eq_ignore_ascii_case(&w.package)).or_else(|| out.receptacles.iter().position(|x| x.name.eq_ignore_ascii_case(&n))),
            None => out.receptacles.iter().position(|x| x.attached.iter().any(|a| a.eq_ignore_ascii_case(&w.name))),
        };
    }
    // A movable that breaks is the same actor as its breakable.
    for m in out.movables.iter_mut() {
        m.breakable = out.breakables.iter().position(|b| b.name.eq_ignore_ascii_case(&m.name) && b.package.eq_ignore_ascii_case(&m.package));
    }
    Ok(out)
}
