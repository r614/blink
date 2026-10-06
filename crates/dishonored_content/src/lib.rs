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
    let path = cache_dir().join("gameplay_config.v7.json");
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

pub fn load_match(map: &str) -> Result<MatchContent> {
    let content = Content::locate()?;
    let sim = load_sim_config(&content).context("gameplay config")?;
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).ok();
    let coll_path = dir.join(format!("{map}.collision2.bin"));
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
    collision.water = Some(std::sync::Arc::new(load_water(&content, map)));
    let curated = load_spawns(map, None);
    let spawns = if curated.is_empty() { spawns } else { curated };
    let npc_path = dir.join(format!("{map}.npcs6.json"));
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
    let look_path = dir.join(format!("{map}.appearance2.json"));
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
    let spawners = npcs.spawns.iter().map(|s| dishonored_ir::spawners::Spawner { name: s.source.rsplit('.').next().unwrap_or("").to_string(), position: s.position, yaw: s.yaw, tweak: s.tweak.clone(), guard: s.role == dishonored_assets::npcs::NpcRole::Guard, props: s.spawner.clone(), appearance: looks.get(&s.tweak).cloned().unwrap_or_default() }).collect();
    let level_pkgs = || {
        let mut pkgs = map_packages(&content, map);
        if map.starts_with("L_Boyle_Ext") {
            pkgs.push("L_BoyleStreet_Ext_Script".into());
        }
        pkgs
    };
    // AI level data: factions, forbidden zones, alarm bells, probe lights (dishonored_ai).
    let ai_path = dir.join(format!("{map}.ailevel2.json"));
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
    let pickup_path = dir.join(format!("{map}.pickups1.json"));
    let pickups: Vec<dishonored_ir::items::PlacedPickup> = match std::fs::read_to_string(&pickup_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(p) => p,
        None => {
            let pkgs = level_pkgs();
            let p = dishonored_assets::pickups::extract_pickups(&content, &pkgs).unwrap_or_default();
            std::fs::write(&pickup_path, serde_json::to_string(&p)?).ok();
            p
        }
    };
    Ok(MatchContent { map_name: map.to_string(), collision, spawns, sim, ai_points, guard_spawns, markers, travel, tagged_starts, flee_points, spawners, routes: npcs.routes.clone(), ai_level, pickups })
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
    Ok(dishonored_multiplayer::mission::kismet_host::KismetSetup { db, start_event: ok.start_event.clone(), seed: ok.seed, choices, prompts })
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
    let key = pkgs.iter().map(|p| p.to_ascii_lowercase()).collect::<Vec<_>>().join("+");
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
