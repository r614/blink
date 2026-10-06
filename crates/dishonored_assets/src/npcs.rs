//! Original NPC placement data from a map's script packages: spawners (with their pawn
//! tweak archetype), navigation/target/watch points used for patrols and idling.

use crate::content::Content;
use anyhow::Result;
use glam::Vec3;
use serde::{Deserialize, Serialize};
pub use dishonored_ir::spawners::SpawnerProps;
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NpcRole {
    Guard,
    Civilian,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NpcSpawnPoint {
    pub position: Vec3,
    pub yaw: f32,
    pub tweak: String,
    pub role: NpcRole,
    pub source: String,
    /// The spawner's own settings (`DishonoredSpawner` / `DisTravelSpawner` properties).
    #[serde(default)]
    pub spawner: SpawnerProps,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointKind {
    Nav,
    Target,
    GuardWatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiPoint {
    pub position: Vec3,
    pub yaw: f32,
    pub kind: PointKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelMarker {
    /// Actor name within its level, e.g. `TargetPoint_5`.
    pub name: String,
    pub class: String,
    /// Package the actor lives in.
    pub package: String,
    pub position: Vec3,
    pub yaw: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NpcData {
    pub spawns: Vec<NpcSpawnPoint>,
    pub points: Vec<AiPoint>,
    /// Named level actors usable as mission markers (target points, spawners, nav points,
    /// player starts, triggers, flee points), in world space.
    #[serde(default)]
    pub markers: Vec<LevelMarker>,
    /// `DishonoredRoute`s (patrol routes of `DishonoredNavPoint`s) in world space.
    #[serde(default)]
    pub routes: Vec<dishonored_ir::ai::PatrolRoute>,
}

/// A `DishonoredRoute`: `RouteList` (native `ActorReference` array: object index + GUID),
/// `RouteType` (`ERouteType`, class default `ERT_Circle`), `m_Capacity` (default -1); each
/// point's `m_fGuardDuration` / `m_bGuardForever`.
fn read_route(pkg: &ue3_package::Package, props: &[ue3_package::properties::Property], name: String) -> dishonored_ir::ai::PatrolRoute {
    use dishonored_ir::ai::{PatrolRoute, RoutePoint};
    let kind = match find(props, "RouteType").map(|p| &p.value) {
        Some(Value::Enum(e)) | Some(Value::Name(e)) => match e.as_str() {
            "ERT_Linear" => 0,
            "ERT_PingPong" => 1,
            _ => 2,
        },
        Some(Value::Byte(b)) => *b,
        _ => 2,
    };
    let capacity = match find(props, "m_Capacity").map(|p| &p.value) {
        Some(Value::Int(c)) => *c,
        _ => -1,
    };
    let mut points = Vec::new();
    if let Some((n, raw)) = find(props, "RouteList").and_then(|p| p.array_raw()) {
        let stride = if n > 0 { raw.len() / n } else { 0 };
        for k in 0..n {
            let Some(b) = raw.get(k * stride..k * stride + 4) else { break };
            let r = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            let ObjRef::Export(i) = ObjRef::from_index(r) else { continue };
            let Ok(h) = pkg.read_object(i) else { continue };
            let Some(pos) = find(&h.properties, "Location").and_then(|p| p.as_vector()) else { continue };
            points.push(RoutePoint {
                position: Vec3::from(pos),
                guard_duration: find(&h.properties, "m_fGuardDuration").and_then(|p| p.as_float()).unwrap_or(0.0),
                guard_forever: matches!(find(&h.properties, "m_bGuardForever").map(|p| &p.value), Some(Value::Bool(true))),
            });
        }
    }
    // `m_SupportedSquads`: array of `DisSquadProperty { m_SquadName }`.
    let squads = find(props, "m_SupportedSquads")
        .and_then(|p| p.array_structs(pkg).ok())
        .map(|v| v.iter().filter_map(|e| match find(e, "m_SquadName").map(|x| &x.value) {
            Some(Value::Name(n)) if n != "None" => Some(n.clone()),
            _ => None,
        }).collect())
        .unwrap_or_default();
    let f = |n: &str, d: f32| find(props, n).and_then(|p| p.as_float()).unwrap_or(d);
    let active = !matches!(find(props, "m_bIsActive").map(|p| &p.value), Some(Value::Bool(false)));
    let location = find(props, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
    PatrolRoute { name, kind, capacity, points, squads, adoption_range: f("m_AdoptionRange", -1.0), neglect_threshold: f("m_NeglectThreshold", 30.0), active, location }
}

/// Role of a pawn tweak from its data: the families in its fallback chain
/// (`m_FallbackChainCooked`, nearest first, plus the tweak's own `m_TweakNameCooked`) and the
/// weapons of its `m_ContentInventoryLoadout`.
/// - Guard: the chain reaches the guard or overseer base pawns (`Pwn_Guard_Base`,
///   `Pwn_Overseer_Base`), or the loadout carries a sword or a pistol (`DisTweaks_WepSword`,
///   `DisTweaks_WepPistol`) outside the weeper family. Lord Shaw (`Twk_Pawn_LordShaw`, a "party"
///   tweak with the civilian faction) falls back to `Pwn_EliteGuard_*` / `Pwn_Guard_Base` and
///   carries a sword and `Twk_Inv_PistolShaw`: an armed elite.
/// - Civilian: the chain reaches `Pwn_Civ_Base` outside the weeper family (`Pwn_Weeper_Base`).
/// - Other: weepers, tallboys and pawns with no family.
///
/// `chain` holds the cooked fallback / tweak names, `weapons` the classes of the loadout's tweaks.
pub fn role_from_chain(chain: &[String], weapons: &[String]) -> NpcRole {
    let has = |base: &str| chain.iter().any(|n| n.to_ascii_lowercase().starts_with(base));
    let weeper = has("pwn_weeper_base");
    if has("pwn_guard_base") || has("pwn_overseer_base") || (!weeper && weapons.iter().any(|w| matches!(w.as_str(), "DisTweaks_WepSword" | "DisTweaks_WepPistol"))) {
        NpcRole::Guard
    } else if has("pwn_civ_base") && !weeper {
        NpcRole::Civilian
    } else {
        NpcRole::Other
    }
}

/// Role of a resolved pawn tweak (see `role_from_chain`); `None` when the tweak is not found.
pub fn role_from_tweak_data(db: &crate::tweaks::TweakDb, tweak: &str) -> Option<NpcRole> {
    use crate::tweaks::TVal;
    let m = db.try_resolve(tweak)?;
    let mut chain = Vec::new();
    if let Some(TVal::Name(n)) = m.get("m_TweakNameCooked") {
        chain.push(n.clone());
    }
    if let Some(TVal::List(fb)) = m.get("m_FallbackChainCooked") {
        chain.extend(fb.iter().filter_map(|f| match f.get("m_FallbackName") {
            Some(TVal::Name(n)) => Some(n.clone()),
            _ => None,
        }));
    }
    let weapons: Vec<String> = match m.get("m_ContentInventoryLoadout") {
        Some(TVal::List(l)) => l.iter().filter_map(|e| match e.get("obj") {
            Some(TVal::Obj(o)) => db.class_of(o),
            _ => None,
        }).collect(),
        _ => Vec::new(),
    };
    Some(role_from_chain(&chain, &weapons))
}

/// Name-based role, used only when the pawn tweak cannot be resolved.
pub fn role_from_tweak(t: &str) -> NpcRole {
    let l = t.to_ascii_lowercase();
    if l.contains("guard") || l.contains("overseer") || l.contains("watch") || l.contains("officer") {
        NpcRole::Guard
    } else if l.contains("aristo") || l.contains("party") || l.contains("civ") || l.contains("guest") || l.contains("servant") || l.contains("maid") || l.contains("noble") {
        NpcRole::Civilian
    } else {
        NpcRole::Other
    }
}

pub fn extract_npc_data(content: &Content, packages: &[String]) -> Result<NpcData> {
    let mut out = NpcData::default();
    let mut seen_pkgs = std::collections::BTreeSet::new();
    // The level's pawn tweaks are cooked into its packages.
    let db = crate::tweaks::TweakDb::load_with(content, packages).ok();
    let mut roles: std::collections::HashMap<String, NpcRole> = Default::default();
    for name in packages {
        // Package lists can repeat a sublevel (streaming list + name prefix); read each once.
        if !content.has_package(name) || !seen_pkgs.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        for i in 0..pkg.exports.len() {
            let outer = ObjRef::from_index(pkg.exports[i].outer);
            if pkg.obj_name(outer) != "PersistentLevel" {
                continue;
            }
            let class = pkg.export_class(i);
            if class == "DishonoredRoute" {
                if let Ok(h) = pkg.read_object(i) {
                    let r = read_route(&pkg, &h.properties, format!("{name}:{}", pkg.obj_name(ObjRef::Export(i))));
                    if !r.points.is_empty() {
                        out.routes.push(r);
                    }
                }
                continue;
            }
            let interesting = matches!(class.as_str(), "DishonoredSpawner" | "DisTravelSpawner" | "DishonoredNavPoint" | "TargetPoint" | "DisGuardWatchPoint");
            let marker = interesting || matches!(class.as_str(), "PlayerStart" | "DisTrigger" | "Trigger" | "TriggerVolume" | "DisFleePointActor" | "Note" | "DisAlarmBell");
            if !marker {
                continue;
            }
            let Ok(h) = pkg.read_object(i) else { continue };
            let pos = find(&h.properties, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
            let yaw = find(&h.properties, "Rotation").and_then(|p| p.as_rotator()).map(|r| r[1] as f32 * 360.0 / 65536.0).unwrap_or(0.0);
            out.markers.push(LevelMarker { name: pkg.obj_name(ObjRef::Export(i)).to_string(), class: class.to_string(), package: name.clone(), position: pos, yaw });
            if !interesting {
                continue;
            }
            match class.as_str() {
                "DishonoredSpawner" | "DisTravelSpawner" => {
                    let tweak = match find(&h.properties, "m_pPawnTweaks").map(|p| &p.value) {
                        Some(Value::Object(o)) => pkg.obj_path(ObjRef::from_index(*o)),
                        _ => String::new(),
                    };
                    let spawner = spawner_props(&pkg, &h.properties);
                    let role = *roles.entry(tweak.clone()).or_insert_with(|| db.as_ref().and_then(|d| role_from_tweak_data(d, &tweak)).unwrap_or_else(|| role_from_tweak(&tweak)));
                    out.spawns.push(NpcSpawnPoint { position: pos, yaw, role, tweak, source: format!("{name}:{}", pkg.obj_path(ObjRef::Export(i))), spawner });
                }
                "DishonoredNavPoint" => out.points.push(AiPoint { position: pos, yaw, kind: PointKind::Nav }),
                "TargetPoint" => out.points.push(AiPoint { position: pos, yaw, kind: PointKind::Target }),
                _ => out.points.push(AiPoint { position: pos, yaw, kind: PointKind::GuardWatch }),
            }
        }
    }
    Ok(out)
}

fn spawner_props(pkg: &ue3_package::Package, props: &[ue3_package::properties::Property]) -> SpawnerProps {
    let mut s = SpawnerProps::default();
    let b = |n: &str, d: bool| match find(props, n).map(|p| &p.value) {
        Some(Value::Bool(v)) => *v,
        _ => d,
    };
    // `DisTravelSpawner` overrides the class default to false (decompiled defaultproperties).
    let travel = find(props, "m_PlayerTravelLocationTags").is_some();
    s.on_begin_play = b("m_bSpawnOnBeginPlay", !travel);
    s.patrol_on_startup = b("m_bPatrolUponStartup", false);
    s.aware_on_startup = b("m_bAwareOfPlayerUponStartup", false);
    s.capable_of_fleeing = b("m_bCapableOfFleeing", true);
    s.spawn_dead = b("m_bSpawnDead", false);
    s.knocked_out = b("m_bTreatAsKnockedOut", false);
    s.at_suspicion = match find(props, "m_SpawnAtSuspicionLevel").map(|p| &p.value) {
        Some(Value::Enum(e)) => Some(e.clone()),
        _ => None,
    };
    if let Some(Value::Name(n)) = find(props, "m_Squad").map(|p| &p.value) {
        s.squad = n.clone();
    }
    if let Some(Value::Object(o)) = find(props, "m_pFactionTweakOverride").map(|p| &p.value) {
        s.faction = pkg.obj_path(ObjRef::from_index(*o));
    }
    // Array of FName: i32 count, then (index, number) pairs.
    if let Some(Value::Array(raw)) = find(props, "m_PlayerTravelLocationTags").map(|p| &p.value) {
        let rd = |o: usize| raw.get(o..o + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let n = rd(0).unwrap_or(0).max(0) as usize;
        for k in 0..n.min(64) {
            if let (Some(index), Some(number)) = (rd(4 + k * 8), rd(8 + k * 8)) {
                s.travel_tags.push(pkg.name(ue3_package::FName { index, number }));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn roles_come_from_the_fallback_chain_and_loadout() {
        // Lord Shaw: a "party" tweak with the civilian faction, but an elite guard by fallback.
        let shaw = s(&["Twk_Pawn_Boyle_Party+++Twk_Pawn_LordShaw DisTweaks_NPCPawn", "Pwn_EliteGuard_MTall_2+++Pwn_EliteGuard_MTall_2 DisTweaks_NPCPawn", "Pwn_Guard_Base+++Pwn_Guard_MTBase DisTweaks_NPCPawn", "Twk_Pawn_DefaultNPC+++Twk_Pawn_DefaultNPC DisTweaks_NPCPawn"]);
        assert_eq!(role_from_chain(&shaw, &s(&["DisTweaks_WepSword", "DisTweaks_WepPistol"])), NpcRole::Guard);
        assert_eq!(role_from_tweak("Twk_Pawn_Boyle_Party.Twk_Pawn_LordShaw"), NpcRole::Civilian);
        let guest = s(&["Pwn_Civ_Base+++Pwn_Civ_MTBase DisTweaks_NPCPawn", "Twk_Pawn_DefaultNPC+++Twk_Pawn_DefaultNPC DisTweaks_NPCPawn"]);
        assert_eq!(role_from_chain(&guest, &[]), NpcRole::Civilian);
        // An armed pawn with no guard family is a fighter; weepers stay apart.
        assert_eq!(role_from_chain(&guest, &s(&["DisTweaks_WepSword"])), NpcRole::Guard);
        let weeper = s(&["Pwn_Weeper_Base+++Pwn_Weeper_Base DisTweaks_NPCPawn", "Pwn_Civ_Base+++Pwn_Civ_Base DisTweaks_NPCPawn"]);
        assert_eq!(role_from_chain(&weeper, &s(&["DisTweaks_WepSword"])), NpcRole::Other);
        let tallboy = s(&["Twk_Pawn_DefaultNPC+++Twk_Pawn_DefaultNPC DisTweaks_NPCPawn"]);
        assert_eq!(role_from_chain(&tallboy, &s(&["DisTweaks_WepTallboyBow"])), NpcRole::Other);
        let overseer = s(&["Pwn_Overseer_Base+++Pwn_Overseer_MTBase DisTweaks_NPCPawn"]);
        assert_eq!(role_from_chain(&overseer, &s(&["DisTweaks_WepMusicBox"])), NpcRole::Guard);
    }
}
