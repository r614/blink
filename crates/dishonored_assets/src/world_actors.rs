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
use dishonored_ir::world_actors::{BatteryDef, BreakStepDef, BreakableDef, ReceptacleDef, WorldActors};
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

/// Read the world actors of a map. `packages` are the map's packages; `loudness` is
/// `m_LoudnessDistanceArray` (the steps' AI noise loudness as a distance).
pub fn extract_world_actors(content: &Content, packages: &[String], loudness: &[f32]) -> Result<WorldActors> {
    let pkgs: Vec<String> = packages.iter().filter(|p| content.has_package(p)).cloned().collect();
    let db = TweakDb::load_with(content, &pkgs)?;
    let mut res = PropResolver::new(content);
    let mut out = WorldActors::default();
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
            let class = pkg.export_class(i);
            if BREAKABLE_CLASSES.contains(&class.as_str()) {
                if let Some(b) = breakable(&pkg, i, name, &class, &db, &mut res, loudness) {
                    out.breakables.push(b);
                }
            } else if class == "DisWhaleOilReceptacle" {
                if let Some(r) = receptacle(&pkg, i, name, &db, &mut res) {
                    out.receptacles.push(r);
                }
            }
        }
    }
    Ok(out)
}
