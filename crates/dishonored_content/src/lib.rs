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
    /// Original travel doors of this map (extracted from its Kismet + the global Kismet).
    pub travel: Vec<dishonored_assets::travel::TravelLink>,
    /// `PlayerStart`s with their travel tags.
    pub tagged_starts: Vec<dishonored_assets::travel::TaggedStart>,
    /// `DisFleePointActor` positions (civilian panic destinations).
    pub flee_points: Vec<Vec3>,
    /// Original NPC spawners with their settings (for missions that use the level's own NPCs).
    pub spawners: Vec<dishonored_ir::spawners::Spawner>,
    /// `DishonoredRoute` patrol routes.
    pub routes: Vec<dishonored_ir::ai::PatrolRoute>,
    /// Factions, forbidden zones, alarm bells and probe lights of the level.
    pub ai_level: dishonored_ir::ai::AiLevel,
    /// Pickups placed in the map's levels.
    pub pickups: Vec<dishonored_ir::items::PlacedPickup>,
    /// Doors and usables the player uses natively (layout maps; empty on original maps, whose
    /// usables the original Kismet drives).
    pub usables: Vec<dishonored_multiplayer::mission::UsableDef>,
}

// [playground] Maps built from a layout of original assets (`maps/*.ron`).
pub mod playground;

pub fn cache_dir() -> PathBuf {
    std::env::var_os("DISHONORED_CACHE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cache"))
}

/// Import geometry only (no textures) for simulation use.
pub fn import_geometry(content: &Content, map: &str) -> Result<DishonoredMap> {
    if let Some(layout) = playground::layout_for(map) {
        return playground::import_layout_geometry(content, &layout?);
    }
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
    // v15: the integrated subsystem ports (ranged, world, melee, powers, movement, stealth).
    // v15x-physics: GameplayConfig.throw, windblast `cast.notify_time` (physics port).
    // v17: physics throw rest / get-up fields on top of v16.
    // v19: special NPC classes (ai.md F6) and AI head poses / look-at (stealth F4 / F5).
    let path = cache_dir().join("gameplay_config.v19.json");
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

/// The map's `DishonoredWaterVolume`s, cached as `cache/<map>.water1.json`.
pub fn load_water(content: &Content, map: &str) -> Vec<dishonored_world::WaterVolume> {
    let path = cache_dir().join(format!("{map}.water1.json"));
    if let Some(w) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Vec<dishonored_world::WaterVolume>>(&t).ok()) {
        return w;
    }
    let mut pkgs = map_packages(content, map);
    if map.starts_with("L_Boyle_Ext") {
        pkgs.push("L_BoyleStreet_Ext_Script".into());
    }
    let defs = dishonored_assets::water::extract_water(content, &pkgs).unwrap_or_else(|e| {
        eprintln!("warning: water volumes of {map}: {e:#}");
        Vec::new()
    });
    let w: Vec<dishonored_world::WaterVolume> = defs
        .into_iter()
        .map(|d| dishonored_world::WaterVolume { name: d.name, hulls: d.hulls, min: d.min, max: d.max, fluid_friction: d.fluid_friction, terminal_velocity: d.terminal_velocity, shallow: d.shallow })
        .collect();
    std::fs::create_dir_all(cache_dir()).ok();
    if let Ok(t) = serde_json::to_string(&w) {
        std::fs::write(&path, t).ok();
    }
    w
}

/// The map's non-water physics volumes (zone velocity, pain), cached like the water volumes.
pub fn load_physics_volumes(content: &Content, map: &str) -> Vec<dishonored_world::PhysicsVolume> {
    let path = cache_dir().join(format!("{map}.physvol1.json"));
    if let Some(v) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Vec<dishonored_world::PhysicsVolume>>(&t).ok()) {
        return v;
    }
    let mut pkgs = map_packages(content, map);
    if map.starts_with("L_Boyle_Ext") {
        pkgs.push("L_BoyleStreet_Ext_Script".into());
    }
    let defs = dishonored_assets::water::extract_physics_volumes(content, &pkgs).unwrap_or_else(|e| {
        eprintln!("warning: physics volumes of {map}: {e:#}");
        Vec::new()
    });
    let v: Vec<dishonored_world::PhysicsVolume> = defs
        .into_iter()
        .map(|d| dishonored_world::PhysicsVolume {
            name: d.name,
            hulls: d.hulls,
            min: d.min,
            max: d.max,
            priority: d.priority,
            zone_velocity: d.zone_velocity,
            velocity_affects_walking: d.velocity_affects_walking,
            pain: d.pain_causing.then_some(dishonored_world::VolumePain { damage_per_sec: d.damage_per_sec, interval: d.pain_interval, entry: d.entry_pain, damage_type: d.damage_type }),
        })
        .collect();
    std::fs::create_dir_all(cache_dir()).ok();
    if let Ok(t) = serde_json::to_string(&v) {
        std::fs::write(&path, t).ok();
    }
    v
}

/// The map's `DisClimbable` chains, cached as `cache/<map>.climb1.json`.
pub fn load_climbables(content: &Content, map: &str) -> Vec<dishonored_world::Climbable> {
    let path = cache_dir().join(format!("{map}.climb1.json"));
    if let Some(c) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Vec<dishonored_world::Climbable>>(&t).ok()) {
        return c;
    }
    let defs = dishonored_assets::climbable::extract_climbables(content, &map_packages(content, map)).unwrap_or_else(|e| {
        eprintln!("warning: climbables of {map}: {e:#}");
        Vec::new()
    });
    let c: Vec<dishonored_world::Climbable> = defs
        .into_iter()
        .map(|d| dishonored_world::Climbable { name: d.name, anchor: d.anchor, bones: d.bones, sphere_radius: d.sphere_radius, body_half_width: d.body_half_width, top: d.top, bottom: d.bottom })
        .collect();
    std::fs::create_dir_all(cache_dir()).ok();
    if let Ok(t) = serde_json::to_string(&c) {
        std::fs::write(&path, t).ok();
    }
    c
}

/// Collision the static meshes do not carry (research/dishonored/COLLISION.md): the level's
/// `BlockingVolume`s (movement only: invisible brushes; Boyle's garden and river-grate
/// PlayerStarts stand on two of them), the physics bodies of its skeletal props that block
/// (usable objects, water sources, alarm bells and the level transition doors, which are closed
/// for good: using one loads the other map) at their reference pose, and its `DisDoor`s as
/// movers with a pose per door state (they open and close with the Kismet).
pub fn add_level_blockers(w: CollisionWorld, content: &Content, map: &str) -> CollisionWorld {
    let pkgs = map_packages(content, map);
    let mut extra: Vec<(String, u8, u8, Vec<Vec<[Vec3; 3]>>)> = Vec::new();
    let mut w = w;
    // Each volume's trace types (`BrushComponent.m_CollisionTraceTypes`: the `Volume` class
    // default is the three movement types, so pawns and movers are blocked, gameplay traces
    // pass; research/dishonored/COLLISION.md).
    let sources = dishonored_assets::collision_audit::collision_sources(content, &pkgs);
    match dishonored_assets::water::extract_blocking_volumes(content, &pkgs) {
        Ok(v) => {
            for b in v {
                let tris: Vec<Vec<[Vec3; 3]>> = b.hulls.iter().map(|h| dishonored_world::hull_triangles(h)).collect();
                let src = sources.iter().find(|s| s.package == b.package && s.actor == b.name && s.component_class == "BrushComponent");
                if src.is_some_and(|s| !s.active()) {
                    continue;
                }
                let mask = src.map_or(dishonored_assets::collision_audit::mask::MOVE, |s| s.trace_mask);
                extra.push((format!("{}:{}", b.package, b.name), dishonored_world::channel::from_trace_mask(mask), dishonored_world::hit_kind::OTHER, tris));
            }
        }
        Err(e) => eprintln!("warning: blocking volumes of {map}: {e:#}"),
    }
    // Skeletal props whose physics asset blocks: the component collides and both the actor and
    // the component block (`IsBlockedBy`; none is world geometry).
    use dishonored_assets::collision_audit::mask;
    let travel: Vec<String> = dishonored_assets::travel::extract_travel(content, &pkgs).unwrap_or_default().into_iter().map(|t| format!("{}:{}", t.door_package, t.door)).collect();
    let props = dishonored_assets::usable_props::read_props(content, &pkgs);
    let tweaks = dishonored_assets::tweaks::TweakDb::load_with(content, &pkgs).ok();
    // Every blocking skeletal component but the climbable chains (their bodies swing with
    // physics and turn off while climbed; not modelled, U-654).
    // A skeletal component with no mesh (on itself or its archetype) has no physics asset
    // instance, so nothing to collide with: the mesh-less `SkeletalMeshComponent0` of the
    // usables whose visible part is their static `StaticUsablePart` (Flooded's weapon boxes,
    // the Hound Pits audio logs; that static part is imported with the level's components).
    for src in sources.iter().filter(|s| s.component_class == "SkeletalMeshComponent" && s.active() && s.block && s.actor_block && s.class != "DisClimbable" && !s.asset.is_empty()) {
        let actor = format!("{}:{}", src.package, src.actor);
        let prop = props.iter().find(|p| p.actor.eq_ignore_ascii_case(&actor));
        let is_door = src.class == "DisDoor" && prop.is_some_and(|p| p.door.is_some()) && !travel.iter().any(|t| t.eq_ignore_ascii_case(&actor));
        // The vision line of sight (`TRACE_Movers` and level geometry, no `TRACE_Others`) sees
        // doors (movers) and the props whose tweak says `m_bShouldTraceAsWorldGeometry`, unless
        // they are transparent for vision (the actor's override, else the tweak's flag;
        // `DisSkeletalBreakable::ShouldTrace` 0x00A28580, `DisDoor` 0x00A2C760).
        let tw = src.tweak.as_ref().and_then(|p| tweaks.as_ref()?.try_resolve(p));
        let tflag = |n: &str| tw.as_ref().is_some_and(|t| matches!(t.get(n), Some(dishonored_assets::tweaks::TVal::B(true))));
        let has = |n: &str| src.extra.iter().any(|e| e == n);
        let transparent = if has("m_bOverrideDefaultVisionTransparency") { has("m_bTransparentForVisionChecks") } else { tflag("m_bTransparentForVisionChecks") };
        let seen = !transparent && (is_door || src.world_geometry || tflag("m_bShouldTraceAsWorldGeometry"));
        let m = if seen { src.trace_mask } else { src.trace_mask & !mask::VISION };
        let channels = dishonored_world::channel::from_trace_mask(m);
        let info = dishonored_world::TriInfo { channels, shape: dishonored_world::shape::ALL, kind: dishonored_world::hit_kind::OTHER, elem: u32::MAX };
        if let (true, Some(prop)) = (is_door, prop) {
            match dishonored_assets::prop_collision::door_states(content, &pkgs, prop) {
                Ok(states) => {
                    let closed = states.iter().find(|s| s.0 == 0).map(|s| s.1.clone()).unwrap_or_default();
                    let mut tris = Vec::new();
                    let mut infos = Vec::new();
                    for (ei, e) in closed.iter().enumerate() {
                        tris.extend_from_slice(e);
                        infos.extend(std::iter::repeat_n(dishonored_world::TriInfo { elem: ei as u32, ..info }, e.len()));
                    }
                    w.movers.push(dishonored_world::MoverCollision {
                        actor: prop.actor.clone(),
                        actor_to_world: prop.actor_to_world,
                        tris,
                        info: infos,
                        poses: states.into_iter().filter(|s| s.0 != 0).collect(),
                        initial: prop.door.as_ref().map_or(0, |d| d.initial),
                    });
                }
                Err(e) => eprintln!("warning: door collision of {}: {e:#}", prop.actor),
            }
            continue;
        }
        // A usable object rests on the first frame of its first stage (a bin's lid closed; its
        // reference pose has it open).
        let bodies = match prop {
            Some(prop) => dishonored_assets::prop_collision::usable_start_bodies(content, &pkgs, prop, tw.as_ref()),
            None => dishonored_assets::prop_collision::read_component_bodies(content, &pkgs, &actor, Some(&src.component), &Default::default()),
        };
        match bodies {
            Ok(b) => extra.push((b.actor, channels, dishonored_world::hit_kind::OTHER, b.tris)),
            Err(e) => eprintln!("warning: collision of {actor}: {e:#}"),
        }
    }
    w.with_extra_kinds(&extra)
}

/// Spawn points for free-for-all (the port's mode; the original has none): the map's spawns
/// that stand on the original navigation mesh, in the largest group a pawn can walk between.
/// Boyle Ext keeps the garden and front-door starts of the estate grounds and drops the
/// scripted Samuel drop-off start (off the navmesh, in the canal under the visual water, outside
/// the water volume) and the roof patio and river grate starts (navmesh islands of their own,
/// left only by drops into the sea or Blink).
pub fn ffa_spawns(mc: &MatchContent) -> Vec<dishonored_multiplayer::ffa::SpawnPoint> {
    use dishonored_world::WorldQuery;
    let all: Vec<dishonored_multiplayer::ffa::SpawnPoint> = mc.spawns.iter().map(|s| dishonored_multiplayer::ffa::SpawnPoint { position: s.position, yaw: s.yaw }).collect();
    let Some(nav) = mc.collision.nav() else { return all };
    let on: Vec<usize> = (0..all.len()).filter(|&i| nav.locate(all[i].position, 60.0, 260.0).is_some()).collect();
    let mut best: Vec<usize> = Vec::new();
    for &i in &on {
        let group: Vec<usize> = on.iter().copied().filter(|&j| j == i || nav.find_path(all[i].position, all[j].position, &Default::default()).is_some()).collect();
        if group.len() > best.len() {
            best = group;
        }
    }
    if best.len() >= 2 {
        best.into_iter().map(|i| all[i]).collect()
    } else if on.len() >= 2 {
        on.into_iter().map(|i| all[i]).collect()
    } else {
        all
    }
}

pub fn load_match(map: &str) -> Result<MatchContent> {
    let content = Content::locate()?;
    let sim = load_sim_config(&content).context("gameplay config")?;
    if let Some(layout) = playground::layout_for(map) {
        return playground::load_layout_match(&content, sim, &layout?);
    }
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).ok();
    let coll_path = dir.join(format!("{map}.collision19.bin"));
    let spawn_path = dir.join(format!("{map}.spawns.json"));
    let (collision, spawns) = match (CollisionWorld::load(&coll_path), std::fs::read_to_string(&spawn_path)) {
        (Ok(w), Ok(s)) => (w, serde_json::from_str::<Vec<Spawn>>(&s).unwrap_or_default()),
        _ => {
            let m = import_geometry(&content, map)?;
            let w = add_level_blockers(CollisionWorld::from_map(&m), &content, map);
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
    collision.water = Some(std::sync::Arc::new(load_water(&content, map)));
    collision.volumes = Some(std::sync::Arc::new(load_physics_volumes(&content, map)));
    collision.climbables = Some(std::sync::Arc::new(load_climbables(&content, map)));
    let curated = load_spawns(map, None);
    let spawns = if curated.is_empty() { spawns } else { curated };
    let npc_path = dir.join(format!("{map}.npcs7.json"));
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
    let travel_path = dir.join(format!("{map}.travel4.json"));
    let (travel, tagged_starts): (Vec<dishonored_assets::travel::TravelLink>, Vec<dishonored_assets::travel::TaggedStart>) = match std::fs::read_to_string(&travel_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(t) => t,
        None => {
            let pkgs = map_packages(&content, map);
            let t = (dishonored_assets::travel::extract_travel(&content, &pkgs).unwrap_or_default(), dishonored_assets::travel::tagged_starts(&content, &pkgs));
            std::fs::write(&travel_path, serde_json::to_string(&t)?).ok();
            t
        }
    };
    let flee_points = markers.iter().filter(|m| m.class == "DisFleePointActor").map(|m| m.position).collect();
    // Each spawner's look from its pawn tweak (body + head options), resolved once per map.
    let look_path = dir.join(format!("{map}.appearance5.json"));
    let looks: std::collections::HashMap<String, dishonored_ir::spawners::Appearance> = match std::fs::read_to_string(&look_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(l) => l,
        None => {
            let mut pkgs = map_packages(&content, map);
            if map.starts_with("L_Boyle_Ext") {
                pkgs.push("L_BoyleStreet_Ext_Script".into());
            }
            let tweaks: Vec<String> = npcs.spawns.iter().map(|s| s.tweak.clone()).filter(|t| !t.is_empty()).collect();
            let l = dishonored_assets::characters::resolve_appearances(&content, &pkgs, &tweaks).unwrap_or_default();
            std::fs::write(&look_path, serde_json::to_string(&l)?).ok();
            l
        }
    };
    // Each pawn tweak's lineage (itself and its `m_FallbackChainCooked`): the chaos deeds match
    // victims by it (0x00C6F450).
    let lineage_path = dir.join(format!("{map}.lineage1.json"));
    let lineages: std::collections::HashMap<String, Vec<String>> = match std::fs::read_to_string(&lineage_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(l) => l,
        None => {
            let mut pkgs = map_packages(&content, map);
            if map.starts_with("L_Boyle_Ext") {
                pkgs.push("L_BoyleStreet_Ext_Script".into());
            }
            let tweaks: Vec<String> = npcs.spawns.iter().map(|s| s.tweak.clone()).filter(|t| !t.is_empty()).collect();
            let l = dishonored_assets::tweaks::TweakDb::load_with(&content, &pkgs).map(|db| tweaks.iter().map(|t| (t.clone(), db.lineage(t))).collect()).unwrap_or_default();
            std::fs::write(&lineage_path, serde_json::to_string(&l)?).ok();
            l
        }
    };
    let spawners = npcs.spawns.iter().map(|s| dishonored_ir::spawners::Spawner { name: s.source.rsplit('.').next().unwrap_or("").to_string(), position: s.position, yaw: s.yaw, tweak: s.tweak.clone(), guard: s.role == dishonored_assets::npcs::NpcRole::Guard, props: s.spawner.clone(), appearance: looks.get(&s.tweak).cloned().unwrap_or_default(), lineage: lineages.get(&s.tweak).cloned().unwrap_or_default() }).collect();
    let level_pkgs = || {
        let mut pkgs = map_packages(&content, map);
        if map.starts_with("L_Boyle_Ext") {
            pkgs.push("L_BoyleStreet_Ext_Script".into());
        }
        pkgs
    };
    // AI level data: factions, forbidden zones, alarm bells, probe lights (dishonored_ai).
    let ai_path = dir.join(format!("{map}.ailevel5.json"));
    let ai_level: dishonored_ir::ai::AiLevel = match std::fs::read_to_string(&ai_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(a) => a,
        None => {
            let pkgs = level_pkgs();
            let tweaks: Vec<String> = npcs.spawns.iter().map(|s| s.tweak.clone()).filter(|t| !t.is_empty()).collect();
            let a = dishonored_assets::ai_level::extract_ai_level(&content, &pkgs, &tweaks, &sim.gameplay.ai.loudness_distances).unwrap_or_default();
            std::fs::write(&ai_path, serde_json::to_string(&a)?).ok();
            a
        }
    };
    // Placed pickups (with the spawners that carry one), cached per map.
    let pickup_path = dir.join(format!("{map}.pickups2.json"));
    let pickups: Vec<dishonored_ir::items::PlacedPickup> = match std::fs::read_to_string(&pickup_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(p) => p,
        None => {
            let pkgs = level_pkgs();
            let p = dishonored_assets::pickups::extract_pickups(&content, &pkgs).unwrap_or_default();
            std::fs::write(&pickup_path, serde_json::to_string(&p)?).ok();
            p
        }
    };
    Ok(MatchContent { map_name: map.to_string(), collision, spawns, sim, ai_points, guard_spawns, markers, travel, tagged_starts, flee_points, spawners, routes: npcs.routes.clone(), ai_level, pickups, usables: Vec::new() })
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

/// An AI pawn's display name, as the original names NPCs: its pawn tweak's interactable
/// subobject's localized `m_Name` (`Twk_Pawn_Boyle_Party.int`: "Party Guest", "Watch Officer";
/// named characters such as "Lord Brisby" carry their own). Built-in archetypes use the party
/// tweak they stand for (`characters::ARCHETYPE_TWEAKS`); a look whose tweak names nobody (e.g.
/// `Twk_Pawn_Boyle_Aristocrat1`) falls back to the party tweak of its role.
pub fn pawn_name(kind: dishonored_sim::state::PawnKind, archetype: u8, appearances: &[dishonored_multiplayer::Look]) -> Option<String> {
    use dishonored_assets::characters::ARCHETYPE_TWEAKS;
    use dishonored_sim::state::PawnKind;
    let loc = localization();
    let named = |tweak: &str| loc.get(tweak, "m_Name").map(str::to_string);
    let own = match archetype.checked_sub(dishonored_multiplayer::mission::APPEARANCE_BASE) {
        Some(i) => appearances.get(i as usize).and_then(|l| named(&l.tweak)),
        None => ARCHETYPE_TWEAKS.get(archetype as usize).and_then(|t| named(t)),
    };
    own.or_else(|| match kind {
        PawnKind::Guard => named(ARCHETYPE_TWEAKS[1]),
        PawnKind::Civilian => named(ARCHETYPE_TWEAKS[0]),
        _ => None,
    })
}

/// The original prompt of a travel door: its usable tweak's localized `m_InteractText`.
pub fn travel_prompt(link: &dishonored_assets::travel::TravelLink, use_key: &str) -> String {
    match localization().get(&link.door_tweak, "m_InteractText") {
        Some(t) => dishonored_assets::localization::with_keys(t, use_key),
        None => format!("Press {use_key} to travel"),
    }
}

/// The level changes a map's own Kismet can ask for (cached per map: `<map>.exits2.json`).
pub fn map_exits(content: &Content, map: &str) -> Vec<dishonored_assets::travel::MapExit> {
    let path = cache_dir().join(format!("{map}.exits2.json"));
    if let Some(e) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        return e;
    }
    let e = dishonored_assets::travel::map_exits(content, &map_packages(content, map));
    std::fs::create_dir_all(cache_dir()).ok();
    if let Ok(t) = serde_json::to_string(&e) {
        std::fs::write(&path, t).ok();
    }
    e
}

/// The persistent maps of one campaign level (interiors and exteriors: Boyle Ext / Int, the
/// Bridge parts, the Flooded District parts, Tower Return yard / interior...), from the
/// original travel data: two maps belong together when each one's Kismet can travel to the
/// other through a global `ChangeLvl_*` handler that does not cross a mission boundary
/// ([`dishonored_assets::travel::MapExit::mission_boundary`]: the boat to and from the hub
/// tracks the mission's statistics). `map` first, then the others in the order they are reached.
pub fn level_group(content: &Content, map: &str) -> Vec<String> {
    let mut group = vec![map.to_string()];
    let mut i = 0;
    while i < group.len() {
        let here = group[i].clone();
        for e in map_exits(content, &here).into_iter().filter(|e| !e.mission_boundary) {
            for d in e.destinations {
                if group.iter().any(|g| g.eq_ignore_ascii_case(&d)) || !content.has_package(&d) {
                    continue;
                }
                let back = map_exits(content, &d).into_iter().any(|x| !x.mission_boundary && x.destinations.iter().any(|y| y.eq_ignore_ascii_case(&here)));
                if back {
                    group.push(d);
                }
            }
        }
        i += 1;
    }
    group
}

/// The global `ChangeLvl_*` remote event that starts a map's Kismet the way the campaign enters
/// it: one whose handler prepares this map only, preferring the campaign's own entry (a
/// handler that begins a mission), else one that can lead to it. None when no handler does.
pub fn entry_event(content: &Content, map: &str) -> Option<String> {
    let all = dishonored_assets::travel::global_entries(content);
    let to = |d: &Vec<String>| d.iter().any(|x| x.eq_ignore_ascii_case(map));
    let only = |d: &Vec<String>| d.len() == 1 && to(d);
    all.iter()
        .find(|(_, d, b)| *b && only(d))
        .or_else(|| all.iter().find(|(_, d, _)| only(d)))
        .or_else(|| all.iter().find(|(_, d, _)| to(d)))
        .map(|(e, _, _)| e.clone())
}

/// A free-roam session on `map`: no mission of its own, the original Kismet of the map's
/// campaign level running from its entry event ([`entry_event`]), the level populated by its
/// original spawners, and every map of the level ([`level_group`]) reachable through the
/// original travel (doors, triggers and the global `ChangeLvl_*` handlers: prepare / commit,
/// the tagged arrival start, level state and the player's inventory kept). A level change out
/// of the group (the boat back to the hub) ends the session as it ends a mission.
pub fn freeroam_def(map: &str, seed: u64) -> Result<dishonored_mission::MissionDef> {
    let content = Content::locate()?;
    let start = entry_event(&content, map).with_context(|| format!("no global ChangeLvl_* handler prepares {map}"))?;
    let group = level_group(&content, map);
    let mut def = dishonored_mission::MissionDef { title: format!("Free roam: {map}"), map: map.to_string(), maps: group[1..].to_vec(), ..Default::default() };
    def.original_npcs = group.clone();
    def.original_kismet = Some(dishonored_mission::OriginalKismet { start_event: start, seed });
    // A game started from the menu runs the global `Global_RealGameStarted`, which sets this
    // flag; the levels' travel chains test it before their `ChangeLvl_*` (Boyle's doors: True ->
    // the level change, False -> a debug console command).
    def.flags_set.push("SF_Global_RealGameStarted".into());
    def.settings.respawn = true;
    // A mission needs an end; never reached (the session ends when the player leaves the level).
    def.rules.push(dishonored_mission::RuleDef { id: None, on: dishonored_mission::Trigger::Timer(1.0e9), when: Vec::new(), then: vec![dishonored_mission::Action::Complete("end".into())], once: true, disabled: false });
    Ok(def)
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
                    choices: l.choice.as_ref().map(|c| localization().choices(&l.door_package, c)).unwrap_or_default(),
                    sound: l.sound.as_ref().map(|s| s.rsplit('.').next().unwrap_or(s).to_string()),
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
            routes: &mc.routes,
            world: &mc.collision,
            spawners: &mc.spawners,
            ai_level: &mc.ai_level,
        });
    }
    let refs = def.conversation_refs();
    // [kismet] Missions running the original Kismet take their objectives from the original
    // data (localized) and their travel from the Kismet instead of the extracted door table.
    let mut def = def;
    let kismet = match &def.original_kismet {
        Some(ok) => {
            let ks = load_kismet_setup(&def, ok).map_err(|e| format!("original Kismet: {e:#}"))?;
            def.objectives = kismet_objectives(&ks.db);
            Some(ks)
        }
        None => None,
    };
    let mut setup = dishonored_multiplayer::mission::MissionSetup::new(def, &inputs)?;
    for name in mission_maps(&setup.def) {
        if let Some(mc) = maps.get(&name) {
            setup.pickups.insert(name.to_ascii_lowercase(), mc.pickups.clone());
            setup.usables.extend(mc.usables.iter().cloned());
        }
    }
    if let Some(ks) = kismet {
        setup.doors.clear();
        setup.kismet = Some(ks);
    }
    if !refs.is_empty() || !setup.def.dialog_packages.is_empty() {
        let db = load_mission_conversations(&setup.def).map_err(|e| format!("dialog data: {e:#}"))?;
        let errs = dishonored_mission::validate_conversations(&setup.def, &db);
        if !errs.is_empty() {
            return Err(errs.join("; "));
        }
        setup.conversations = Some(db);
    }
    Ok(dishonored_multiplayer::game::GameSetup::Mission(Box::new(setup)))
}

/// [kismet] The original Kismet of a mission's maps plus the global game sequence
/// (`DishonoredGameFull_P`), with the localized texts the host shows: scripted choice options
/// and use prompts.
pub fn load_kismet_setup(def: &dishonored_mission::MissionDef, ok: &dishonored_mission::OriginalKismet) -> Result<dishonored_multiplayer::mission::kismet_host::KismetSetup> {
    let content = Content::locate()?;
    let groups: Vec<(String, Vec<String>)> = mission_maps(def).into_iter().map(|m| {
        let pk = map_packages(&content, &m);
        (m, pk)
    }).collect();
    let db = dishonored_kismet::load::load(&content, &["DishonoredGameFull_P".to_string()], &groups)?;
    let loc = localization();
    let mut choices = std::collections::BTreeMap::new();
    for o in &db.ops {
        if o.class == "DisSeqAct_DialogScriptedChoice" {
            let pkg = db.package_name(o.package).to_string();
            let mut opts = loc.choices(&pkg, &o.path);
            if opts.is_empty() {
                if let Some(dishonored_kismet::Value::Array(a)) = o.props.get("m_Choices") {
                    opts = a.iter().map(|c| c.field("m_ChoiceText").and_then(|t| t.as_str()).unwrap_or("").to_string()).collect();
                }
            }
            choices.insert(o.id, opts);
        }
    }
    let mut prompts = std::collections::BTreeMap::new();
    for (i, a) in db.actors.iter().enumerate() {
        if let Some(dishonored_kismet::Obj::Path(t)) = a.props.get("m_pUsableObjTweaks").and_then(|v| v.as_obj()) {
            if let Some(text) = loc.get(t, "m_InteractText") {
                prompts.insert(i as u32, dishonored_assets::localization::with_keys(text, "F"));
            }
        }
    }
    // `DisSeqAct_GivePickup.m_pPickup`: the pickup tweak's gift (abstract item, inventory item
    // or stat change), resolved against the mission's packages.
    let mut gives = std::collections::BTreeMap::new();
    let tweak_pkgs: Vec<String> = groups.iter().flat_map(|g| g.1.clone()).collect();
    if let Ok(tdb) = dishonored_assets::tweaks::TweakDb::load_with(&content, &tweak_pkgs) {
        use dishonored_assets::tweaks::{TVal, get, get_f32};
        for o in &db.ops {
            if o.class != "DisSeqAct_GivePickup" {
                continue;
            }
            let Some(dishonored_kismet::Obj::Path(t)) = o.props.get("m_pPickup").and_then(|v| v.as_obj()) else { continue };
            let Some(tw) = tdb.try_resolve(t) else { continue };
            let obj = |k: &str| match get(&tw, k) {
                Some(TVal::Obj(o)) => Some(o.clone()),
                _ => None,
            };
            let kind = if let Some(item) = obj("m_pAbstractItem") {
                dishonored_ir::items::PickupKind::Abstract { item, quantity: get_f32(&tw, "m_Quantity").map(|v| v as i32).unwrap_or(1), note: false, key_item: false }
            } else if let Some(item) = obj("m_pItem") {
                dishonored_ir::items::PickupKind::Inventory { item }
            } else {
                dishonored_ir::items::PickupKind::Stat { health: get_f32(&tw, "m_HealthChange").unwrap_or(0.0) as i32, mana: get_f32(&tw, "m_ManaChange").unwrap_or(0.0) as i32, food: false, ammo: [(0, 0); 8] }
            };
            gives.insert(o.id, kind);
        }
    }
    Ok(dishonored_multiplayer::mission::kismet_host::KismetSetup { db, start_event: ok.start_event.clone(), seed: ok.seed, choices, prompts, gives })
}

/// [kismet] Mission objectives from the original `DishonoredObjective`s (hidden until the
/// Kismet adds them; tasks keep their initial state and hidden flag).
pub fn kismet_objectives(db: &dishonored_kismet::KismetDb) -> Vec<dishonored_mission::ObjectiveDef> {
    db.objectives
        .iter()
        .map(|o| dishonored_mission::ObjectiveDef {
            id: o.path.rsplit('.').next().unwrap_or(&o.path).to_string(),
            description: o.description.clone(),
            hidden: true,
            tasks: o
                .tasks
                .iter()
                .map(|t| dishonored_mission::TaskDef {
                    id: t.name.clone(),
                    description: t.description.clone(),
                    initial: match t.initial.as_str() {
                        "DTS_Active" => dishonored_mission::TaskState::Active,
                        "DTS_Completed" => dishonored_mission::TaskState::Completed,
                        "DTS_Failed" => dishonored_mission::TaskState::Failed,
                        _ => dishonored_mission::TaskState::Inactive,
                    },
                    hidden: t.hidden,
                    optional: t.description.starts_with("(Optional)"),
                    target: None,
                })
                .collect(),
        })
        .collect()
}

/// Original dialog graphs a mission references, with VO lengths from the voice banks
/// (`Bk_<GUID>.INT`). Cached under `cache/` (derived from the install; never committed).
pub fn load_mission_conversations(def: &dishonored_mission::MissionDef) -> Result<dishonored_conversation::ConversationDb> {
    let content = Content::locate()?;
    let mut map_pkgs = Vec::new();
    for m in mission_maps(def) {
        map_pkgs.extend(map_packages(&content, &m));
    }
    let pkgs = def.dialog_packages_needed(&map_pkgs);
    load_conversations(&content, &pkgs)
}

/// Dialog graphs of the given cooked packages (cached per package list).
pub fn load_conversations(content: &Content, pkgs: &[String]) -> Result<dishonored_conversation::ConversationDb> {
    // The format version (2: `voice_trees`; 3: `CompareDarknessLevel` nodes; 4: hook node
    // classes, `m_pFallbackTree`, `DisTweaks_ConvSettings`) is part of the key so new fields are
    // filled in.
    let key = format!("v4:{}", pkgs.iter().map(|p| p.to_ascii_lowercase()).collect::<Vec<_>>().join("+"));
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes().chain(env!("CARGO_PKG_VERSION").bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let path = cache_dir().join(format!("dialog_{h:016x}.json"));
    if let Some(db) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<dishonored_conversation::ConversationDb>(&t).ok()) {
        if db.packages.len() == pkgs.len() {
            return Ok(db);
        }
    }
    let mut vo = dishonored_audio::vo::VoResolver::open(&content.root)?;
    let db = dishonored_conversation::load::load_packages(&content.cooked, pkgs, &mut vo)?;
    std::fs::create_dir_all(cache_dir()).ok();
    std::fs::write(&path, serde_json::to_string(&db)?).ok();
    Ok(db)
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
    fn nav(&self) -> Option<&dishonored_world::nav::NavWorld> {
        self.collision.nav.as_deref()
    }
    fn box_contacts(&self, b: &dishonored_world::PawnBox, channels: u8, out: &mut Vec<dishonored_world::Contact>) {
        self.collision.box_contacts(b, channels, out)
    }
    fn sweep_box(&self, b: &dishonored_world::PawnBox, delta: Vec3, channels: u8) -> Option<dishonored_world::SweepHit> {
        self.collision.sweep_box(b, delta, channels)
    }
    fn water_at(&self, p: Vec3) -> Option<dishonored_world::WaterInfo> {
        self.collision.water_at(p)
    }
    fn movers(&self) -> &[dishonored_world::MoverCollision] {
        self.collision.movers()
    }
    fn physics_volumes(&self) -> &[dishonored_world::PhysicsVolume] {
        self.collision.physics_volumes()
    }
    fn climbables(&self) -> &[dishonored_world::Climbable] {
        self.collision.climbables()
    }
    fn hit_package(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<String> {
        self.collision.hit_package(origin, dir, max_dist)
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
