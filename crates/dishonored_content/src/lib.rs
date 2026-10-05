//! One-stop loading of match content from the local Dishonored install.
//!
//! Derived data (collision BVH, gameplay config) is cached under `cache/` next to the
//! working directory, keyed by map name; the cache is regenerable and never committed.

use anyhow::{Context, Result};
use dishonored_assets::{Content, ImportOptions, import_level, map_packages};
use dishonored_ir::DishonoredMap;
use dishonored_sim::SimConfig;
use dishonored_world::CollisionWorld;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_MAP: &str = "L_Boyle_Ext_P";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spawn {
    pub position: Vec3,
    pub yaw: f32,
}

pub struct MatchContent {
    pub map_name: String,
    pub collision: CollisionWorld,
    pub spawns: Vec<Spawn>,
    pub sim: SimConfig,
    /// Original AI points (nav/target/watch) and guard spawners, for crowds.
    pub ai_points: Vec<(Vec3, f32)>,
    pub guard_spawns: Vec<(Vec3, f32)>,
    /// Named original actors (target points, spawners, player starts, triggers...) for missions.
    pub markers: Vec<dishonored_assets::npcs::LevelMarker>,
    /// Original travel doors of this map (extracted from its Kismet + the global Kismet).
    pub travel: Vec<dishonored_assets::travel::TravelLink>,
    /// `PlayerStart`s with their travel tags.
    pub tagged_starts: Vec<dishonored_assets::travel::TaggedStart>,
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("DISHONORED_CACHE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cache"))
}

/// Import geometry only (no textures) for simulation use.
pub fn import_geometry(content: &Content, map: &str) -> Result<DishonoredMap> {
    let pkgs = map_packages(content, map);
    let opts = ImportOptions { load_textures: false, ..Default::default() };
    let (m, _) = import_level(content, map, &pkgs, &opts)?;
    Ok(m)
}

/// Hand-placed spawn points are preferred when present (`fixtures/spawns/<map>.json`),
/// then the map's PlayerStarts.
fn load_spawns(map: &str, m: Option<&DishonoredMap>) -> Vec<Spawn> {
    for base in ["fixtures/spawns", "../../fixtures/spawns"] {
        let p = Path::new(base).join(format!("{map}.json"));
        if let Ok(t) = std::fs::read_to_string(&p) {
            if let Ok(s) = serde_json::from_str::<Vec<Spawn>>(&t) {
                if !s.is_empty() {
                    return s;
                }
            }
        }
    }
    m.map(|m| {
        m.spawn_candidates
            .iter()
            .map(|s| Spawn { position: s.position + Vec3::Z * 5.0, yaw: s.yaw_deg })
            .collect()
    })
    .unwrap_or_default()
}

pub fn load_sim_config(content: &Content) -> Result<SimConfig> {
    let path = cache_dir().join("gameplay_config.json");
    if let Ok(t) = std::fs::read_to_string(&path) {
        if let Ok(c) = SimConfig::from_json(&t) {
            return Ok(c);
        }
    }
    let cfg = dishonored_assets::gameplay::load_gameplay_config(content)?;
    std::fs::create_dir_all(cache_dir()).ok();
    std::fs::write(&path, serde_json::to_string(&cfg)?).ok();
    Ok(SimConfig::new(cfg))
}

pub fn load_match(map: &str) -> Result<MatchContent> {
    let content = Content::locate()?;
    let sim = load_sim_config(&content).context("gameplay config")?;
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).ok();
    let coll_path = dir.join(format!("{map}.collision.bin"));
    let spawn_path = dir.join(format!("{map}.spawns.json"));
    let (collision, spawns) = match (CollisionWorld::load(&coll_path), std::fs::read_to_string(&spawn_path)) {
        (Ok(w), Ok(s)) => (w, serde_json::from_str::<Vec<Spawn>>(&s).unwrap_or_default()),
        _ => {
            let m = import_geometry(&content, map)?;
            let w = CollisionWorld::from_map(&m);
            w.save(&coll_path).ok();
            let s: Vec<Spawn> = m
                .spawn_candidates
                .iter()
                .map(|s| Spawn { position: s.position + Vec3::Z * 5.0, yaw: s.yaw_deg })
                .collect();
            std::fs::write(&spawn_path, serde_json::to_string(&s)?).ok();
            (w, s)
        }
    };
    let curated = load_spawns(map, None);
    let spawns = if curated.is_empty() { spawns } else { curated };
    let npc_path = dir.join(format!("{map}.npcs3.json"));
    let npcs: dishonored_assets::npcs::NpcData = match std::fs::read_to_string(&npc_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(n) => n,
        None => {
            let mut pkgs = map_packages(&content, map);
            if map.starts_with("L_Boyle_Ext") {
                pkgs.push("L_BoyleStreet_Ext_Script".into());
            }
            let n = dishonored_assets::npcs::extract_npc_data(&content, &pkgs).unwrap_or_default();
            std::fs::write(&npc_path, serde_json::to_string(&n)?).ok();
            n
        }
    };
    let ai_points = npcs.points.iter().map(|p| (p.position, p.yaw)).collect();
    let guard_spawns = npcs.spawns.iter().filter(|s| s.role == dishonored_assets::npcs::NpcRole::Guard).map(|s| (s.position, s.yaw)).collect();
    let markers = npcs.markers.clone();
    let travel_path = dir.join(format!("{map}.travel2.json"));
    let (travel, tagged_starts): (Vec<dishonored_assets::travel::TravelLink>, Vec<dishonored_assets::travel::TaggedStart>) = match std::fs::read_to_string(&travel_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(t) => t,
        None => {
            let pkgs = map_packages(&content, map);
            let t = (dishonored_assets::travel::extract_travel(&content, &pkgs).unwrap_or_default(), dishonored_assets::travel::tagged_starts(&content, &pkgs));
            std::fs::write(&travel_path, serde_json::to_string(&t)?).ok();
            t
        }
    };
    Ok(MatchContent { map_name: map.to_string(), collision, spawns, sim, ai_points, guard_spawns, markers, travel, tagged_starts })
}

/// Persistent maps a mission uses: its start map, then the maps reachable through travel.
pub fn mission_maps(def: &dishonored_mission::MissionDef) -> Vec<String> {
    let mut v = vec![def.map.clone()];
    for m in &def.maps {
        if !v.iter().any(|x| x.eq_ignore_ascii_case(m)) {
            v.push(m.clone());
        }
    }
    v
}

/// Localized original strings (`Localization/INT/*.int`), loaded once.
pub fn localization() -> &'static dishonored_assets::localization::Localization {
    static LOC: std::sync::OnceLock<dishonored_assets::localization::Localization> = std::sync::OnceLock::new();
    LOC.get_or_init(|| Content::locate().map(|c| dishonored_assets::localization::Localization::load(&c, "INT")).unwrap_or_default())
}

/// The original prompt of a travel door: its usable tweak's localized `m_InteractText`.
pub fn travel_prompt(link: &dishonored_assets::travel::TravelLink, use_key: &str) -> String {
    match localization().get(&link.door_tweak, "m_InteractText") {
        Some(t) => dishonored_assets::localization::with_keys(t, use_key),
        None => format!("Press {use_key} to travel"),
    }
}

/// Build a mission game setup from the loaded maps (markers resolved against the levels'
/// named actors, crowd from the original placement data, travel doors from the original
/// Kismet with their localized prompts).
pub fn mission_setup(maps: &MapContents, def: dishonored_mission::MissionDef) -> Result<dishonored_multiplayer::game::GameSetup, String> {
    let mut inputs = Vec::new();
    for name in mission_maps(&def) {
        let mc = maps.get(&name).ok_or_else(|| format!("map {name} not loaded"))?;
        let doors = mc
            .travel
            .iter()
            .filter_map(|l| {
                Some(dishonored_multiplayer::mission::TravelDoor {
                    map: mc.map_name.clone(),
                    door: l.door.clone(),
                    position: l.door_position,
                    prompt: travel_prompt(l, "F"),
                    dest_map: l.destination_map.clone()?,
                    dest_tag: l.destination_tag.clone(),
                })
            })
            .collect();
        inputs.push(dishonored_multiplayer::mission::MapInput {
            name: mc.map_name.clone(),
            actors: mc.markers.iter().map(|m| (m.package.clone(), m.name.clone(), m.position, m.yaw)).collect(),
            default_starts: mc.spawns.iter().map(|s| (s.position, s.yaw)).collect(),
            tagged_starts: mc.tagged_starts.iter().map(|s| (s.tag.clone(), s.position, s.yaw)).collect(),
            doors,
            ai_points: &mc.ai_points,
            guard_spawns: &mc.guard_spawns,
            world: &mc.collision,
        });
    }
    let setup = dishonored_multiplayer::mission::MissionSetup::new(def, &inputs)?;
    Ok(dishonored_multiplayer::game::GameSetup::Mission(Box::new(setup)))
}

impl dishonored_world::WorldQuery for MatchContent {
    fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32, channels: u8) -> Option<dishonored_world::RayHit> {
        self.collision.raycast(origin, dir, max_dist, channels)
    }
    fn capsule_contacts(&self, cap: &dishonored_world::Capsule, channels: u8, out: &mut Vec<dishonored_world::Contact>) {
        self.collision.capsule_contacts(cap, channels, out)
    }
    fn sweep_capsule(&self, cap: &dishonored_world::Capsule, delta: Vec3, channels: u8) -> Option<dishonored_world::SweepHit> {
        self.collision.sweep_capsule(cap, delta, channels)
    }
}

/// The persistent maps a session can travel between, loaded up front (the original swaps
/// persistent maps; loading both avoids a hitch mid-game) behind a [`dishonored_world::MapSet`].
pub struct MapContents {
    pub maps: Vec<std::sync::Arc<MatchContent>>,
    pub set: dishonored_world::MapSet,
}

impl MapContents {
    pub fn load(maps: &[String]) -> Result<Self> {
        let mut loaded = Vec::new();
        for m in maps {
            if !loaded.iter().any(|x: &std::sync::Arc<MatchContent>| x.map_name.eq_ignore_ascii_case(m)) {
                loaded.push(std::sync::Arc::new(load_match(m)?));
            }
        }
        let set = dishonored_world::MapSet::new(loaded.iter().map(|m| (m.map_name.clone(), m.clone() as std::sync::Arc<dyn dishonored_world::WorldQuery + Send + Sync>)).collect());
        Ok(MapContents { maps: loaded, set })
    }
    pub fn get(&self, name: &str) -> Option<&std::sync::Arc<MatchContent>> {
        self.maps.iter().find(|m| m.map_name.eq_ignore_ascii_case(name))
    }
}
