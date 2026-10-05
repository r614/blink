//! One-stop loading of match content from the local Dishonored install.
//!
//! Derived data (collision BVH, gameplay config) is cached under `cache/` next to the
//! working directory, keyed by map name; the cache is regenerable and never committed.

use anyhow::{Context, Result};
use dishonored_assets::{Content, ImportOptions, import_level, map_packages};
use dishonored_ir::DishonoredMap;
use dishonored_sim::SimConfig;
use dishonored_world::CollisionWorld;
use dishonored_world::nav::NavWorld;
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
    /// `DisFleePointActor` positions (civilian panic destinations).
    pub flee_points: Vec<Vec3>,
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
    let path = cache_dir().join("gameplay_config.v2.json");
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

/// The map's original navigation mesh (the `L_*_Nav` pylons plus doors and nav blockers),
/// cached as `cache/<map>.nav.bin`.
pub fn load_nav(content: &Content, map: &str) -> Result<NavWorld> {
    let path = cache_dir().join(format!("{map}.nav.bin"));
    if let Ok(w) = NavWorld::load(&path) {
        if !w.polys.is_empty() {
            return Ok(w);
        }
    }
    let data = dishonored_assets::navigation::load_nav_data(content, &map_packages(content, map))?;
    anyhow::ensure!(!data.pylons.is_empty(), "no pylons in the map packages");
    let w = NavWorld::from_data(&data);
    std::fs::create_dir_all(cache_dir()).ok();
    w.save(&path).ok();
    Ok(w)
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
    let mut collision = collision;
    match load_nav(&content, map) {
        Ok(nav) => collision.nav = Some(std::sync::Arc::new(nav)),
        Err(e) => eprintln!("warning: no original navigation for {map}: {e:#}"),
    }
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
    let flee_points = markers.iter().filter(|m| m.class == "DisFleePointActor").map(|m| m.position).collect();
    Ok(MatchContent { map_name: map.to_string(), collision, spawns, sim, ai_points, guard_spawns, markers, flee_points })
}

/// Build a mission game setup from loaded match content (markers resolved against the
/// level's named actors, crowd from the original placement data).
pub fn mission_setup(mc: &MatchContent, def: dishonored_mission::MissionDef) -> Result<dishonored_multiplayer::game::GameSetup, String> {
    let actors: Vec<dishonored_multiplayer::mission::LevelActor> = mc.markers.iter().map(|m| (m.package.clone(), m.name.clone(), m.position, m.yaw)).collect();
    let starts: Vec<(Vec3, f32)> = mc.spawns.iter().map(|s| (s.position, s.yaw)).collect();
    let setup = dishonored_multiplayer::mission::MissionSetup::new(def, &actors, &starts, &mc.ai_points, &mc.guard_spawns, &mc.collision)?;
    Ok(dishonored_multiplayer::game::GameSetup::Mission(Box::new(setup)))
}
