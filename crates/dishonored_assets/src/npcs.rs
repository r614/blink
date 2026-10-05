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
}

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
                    out.spawns.push(NpcSpawnPoint { position: pos, yaw, role: role_from_tweak(&tweak), tweak, source: format!("{name}:{}", pkg.obj_path(ObjRef::Export(i))), spawner });
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
