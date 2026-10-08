//! Mission mode: a custom mission (`dishonored_mission`) played solo or co-op on the
//! authoritative simulation. NPCs (ambient crowd + mission NPCs) are server-side brains;
//! the mission interpreter sees deaths, detection and player positions and drives NPCs,
//! objectives and the end of the mission. The mission view rides in the match view, so
//! clients, replays and the harness see exactly what the server decided.
//!
//! Multi-map missions follow the original level travel (research/dishonored/BOYLE_MISSION.md
//! section 1): one persistent map is active at a time; using an original travel door
//! (`DishonoredUsableObject` -> `SetPlayerTravelDestination` -> `ChangeLvl_*`) saves the
//! level state of the map being left (`DisSeqAct_SaveLevelState`: NPCs alive or dead, their
//! brains), swaps the persistent map (`PrepareMapChange`/`CommitMapChange`), restores the
//! destination's saved state or populates it fresh, and places players at the `PlayerStart`
//! with the destination tag (`GotoPlayerTravelDestination`).

use crate::{Look, MatchPhase, MatchView, ScoreEntry};
use dishonored_ai::{Brain, BrainState, Crowd, CrowdConfig, CrowdLayout, NPC_ID_BASE, NpcSpawn, snap_floor};
use dishonored_mission::runtime::{Command, MissionEvent, MissionState, StepInput};
use dishonored_mission::{Behavior, MissionDef, NpcKind, Place, resolve_actor};
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{PawnKind, PlayerId, PlayerState, WorldState};
use dishonored_sim::{SimConfig, StepOutput, spawn_npc, spawn_player, step_world};
use dishonored_world::WorldQuery;
use glam::Vec3;

// [kismet] Original Kismet host (kismet_host.rs), a child module so it can drive the match.
#[path = "kismet_host.rs"]
pub mod kismet_host;
// [save] Saved games and autosaves (save.rs), a child module so it can save the match.
#[path = "save.rs"]
pub mod save;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// First id for mission NPCs (crowd NPCs start at `NPC_ID_BASE`).
pub const MISSION_NPC_BASE: PlayerId = NPC_ID_BASE + 500;
/// First id for NPCs from the levels' original spawners.
pub const ORIGINAL_NPC_BASE: PlayerId = NPC_ID_BASE + 1000;
/// First archetype that indexes `MatchView::appearances` (0..4 are the built-in presentation
/// archetypes).
pub const APPEARANCE_BASE: u8 = 4;

/// One original `DishonoredSpawner` / `DisTravelSpawner` of a mission map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginalNpc {
    pub map: String,
    /// Spawner actor name (`DishonoredSpawner_11`).
    #[serde(default)]
    pub name: String,
    pub spawn: NpcSpawn,
    pub tweak: String,
    pub props: dishonored_ir::spawners::SpawnerProps,
    /// Mesh parts (package, object path) of this pawn's look: body + its picked head.
    #[serde(default)]
    pub parts: Vec<(String, String)>,
    /// Wears a helmet (`m_PotentialNPCArmor` head armour, rolled per pawn): `PlayerState::helmet`.
    #[serde(default)]
    pub helmet: bool,
}

/// A named original actor of a level: (package, name, position, yaw).
pub type LevelActor = (String, String, Vec3, f32);

/// An original travel door, resolved against its destination map's tagged starts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TravelDoor {
    pub map: String,
    /// Door actor name (`DishonoredUsableObject_7`).
    pub door: String,
    pub position: Vec3,
    /// Original interaction prompt (localized `m_InteractText`), e.g. "Press F to go to ...".
    pub prompt: String,
    pub dest_map: String,
    pub dest_tag: String,
    /// The original confirmation (`DisSeqAct_DialogScriptedChoice`): option 1 travels
    /// ("Go to Boyle Mansion Ballroom"), option 2 stays ("Stay in the Garden"). Empty: travel at once.
    #[serde(default)]
    pub choices: Vec<String>,
    /// Sound the "go" option posts (`Door_Metal_Level_Transition`).
    #[serde(default)]
    pub sound: Option<String>,
}

/// A door or usable object the player uses natively (no original Kismet drives it): maps built
/// from a layout (the Playground). `DisDoor` use, decoded (research/dishonored/PLAYGROUND.md,
/// "Doors"): the use handler (vtable slot 242, 0x00A37C10) opens a closed door away from the
/// user (0x00A2D2E0: the side of the user along the door's reference axis picks Open CCW, state
/// 2, or Open CW, state 1) and closes an open one; a locked door cannot be used (0x00A4E2E0).
/// Other usables step to their next stage (wrapping).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsableDef {
    /// `<package>:<actor>` (the name the movers and props use).
    pub actor: String,
    pub map: String,
    /// The actor's `Location`.
    pub location: Vec3,
    /// Where the player aims to use it (the centre of its collision bodies).
    pub center: Vec3,
    /// Doors: the reference axis in world space (the side test of 0x00A2D2E0); None: a usable.
    pub door_axis: Option<Vec3>,
    pub locked: bool,
    /// Initial door state (`m_InitialDoorState`).
    pub initial: u8,
    /// Stage count of a usable (at least 2: a toggle).
    pub stages: i32,
    /// Original interaction text (localized `m_InteractText`), and the text while locked.
    pub prompt: String,
    pub locked_prompt: String,
}

/// State of a natively used door or usable.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct PropState {
    /// `EDisDoorState` (0 closed, 1 opened CW, 2 opened CCW).
    pub door: u8,
    pub locked: bool,
    /// Current stage of a usable.
    pub stage: i32,
}

/// Use a door (0x00A37C10): the next state, or None when it cannot be used (locked, 0x00A4E2E0).
pub fn door_use(state: PropState, location: Vec3, axis: Vec3, user: Vec3) -> Option<u8> {
    if state.locked {
        return None;
    }
    Some(match state.door {
        // 0x00A2D2E0: (user - door) . axis < 0 -> stage Open CCW (state 2), else Open CW (1).
        0 => {
            if (user - location).dot(axis) < 0.0 {
                2
            } else {
                1
            }
        }
        _ => 0,
    })
}

/// Give a player powers and items (`Loadout`): the original's `OnAddPower` / `AddInventoryItem`
/// handlers. Health and mana are topped up to their (possibly new) maxima.
pub fn apply_loadout(p: &mut PlayerState, l: &dishonored_mission::Loadout, cfg: &SimConfig) {
    use dishonored_ir::items::{AmmoType, ElixirType};
    use dishonored_sim::state::power;
    let g = &cfg.gameplay;
    if l.reset {
        p.inv = Default::default();
        dishonored_sim::left_hand::reset_loadout(p, &cfg.tuning);
    }
    if l.all_powers {
        p.inv.power_levels = [2; power::COUNT];
    }
    for (name, level) in &l.powers {
        if let Some(i) = power::NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)) {
            p.inv.power_levels[i] = (*level).min(2);
        }
    }
    if l.full_ammo {
        for t in AmmoType::ALL {
            let cap = dishonored_sim::inventory::capacity(p, t, g);
            dishonored_sim::inventory::add_ammo(p, t, cap, g);
        }
    }
    dishonored_sim::inventory::add_elixirs(p, ElixirType::Health, l.elixirs.0 as i32, g);
    dishonored_sim::inventory::add_elixirs(p, ElixirType::Mana, l.elixirs.1 as i32, g);
    p.inv.runes = p.inv.runes.saturating_add(l.runes);
    p.inv.coins = p.inv.coins.saturating_add(l.coins);
    for u in &l.upgrades {
        dishonored_sim::powers::give_upgrade(p, u, g);
    }
    p.vitals.health = dishonored_sim::powers::health_max(p, g);
    p.vitals.mana = dishonored_sim::powers::mana_max(p, g);
    p.vitals.mana_regen_cap = p.vitals.mana;
    p.vitals.mana_regen_pool = 0.0;
}

/// Everything the setup needs from one persistent map.
pub struct MapInput<'a> {
    pub name: String,
    pub actors: Vec<LevelActor>,
    pub default_starts: Vec<(Vec3, f32)>,
    /// (tag, position, yaw) of tagged `PlayerStart`s.
    pub tagged_starts: Vec<(String, Vec3, f32)>,
    pub doors: Vec<TravelDoor>,
    pub ai_points: &'a [(Vec3, f32)],
    pub guard_spawns: &'a [(Vec3, f32)],
    /// `DishonoredRoute` patrol routes.
    pub routes: &'a [dishonored_ir::ai::PatrolRoute],
    pub world: &'a dyn WorldQuery,
    /// The map's original spawners.
    pub spawners: &'a [dishonored_ir::spawners::Spawner],
    /// [ai] Factions, forbidden zones, alarm bells and probe lights of the map.
    pub ai_level: &'a dishonored_ir::ai::AiLevel,
}

/// Everything needed to rebuild the mission deterministically (stored in replays).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MissionSetup {
    pub def: MissionDef,
    /// Resolved markers: id -> (position, radius), and the map each marker is on.
    pub markers: BTreeMap<String, (Vec3, f32)>,
    pub marker_map: BTreeMap<String, String>,
    pub start_map: String,
    pub starts: Vec<(Vec3, f32)>,
    /// Mission NPC spawns (id string, spawn, map).
    pub npcs: Vec<(String, NpcSpawn)>,
    pub npc_map: BTreeMap<String, String>,
    /// Ambient crowd and the map it lives on.
    pub crowd: Option<Crowd>,
    pub crowd_map: String,
    pub maps: Vec<String>,
    pub doors: Vec<TravelDoor>,
    /// (map, tag, position, yaw)
    pub tagged_starts: Vec<(String, String, Vec3, f32)>,
    /// `DisFleePointActor` positions per map (lowercase name), on the floor.
    #[serde(default)]
    pub flee: BTreeMap<String, Vec<Vec3>>,
    /// `DishonoredRoute` patrol routes per map (lowercase name), as each map's crowd uses them.
    #[serde(default)]
    pub routes: BTreeMap<String, Vec<dishonored_ir::ai::PatrolRoute>>,
    /// Original spawners of the maps listed in `MissionDef::original_npcs`.
    #[serde(default)]
    pub original: Vec<OriginalNpc>,
    /// Looks of original NPCs: archetype `APPEARANCE_BASE + i` is `appearances[i]` (mesh parts).
    #[serde(default)]
    pub appearances: Vec<Look>,
    /// Animation setups referenced by `Look::anim`.
    #[serde(default)]
    pub anim_profiles: Vec<dishonored_ir::npc_anim::AnimProfile>,
    /// Original dialog graphs the mission references, read from the install by the host
    /// (stored in replays so they play back identically).
    #[serde(default)]
    pub conversations: Option<dishonored_conversation::ConversationDb>,
    /// [kismet] The original Kismet of the mission maps (`MissionDef::original_kismet`).
    #[serde(default)]
    pub kismet: Option<kismet_host::KismetSetup>,
    /// [ai] AI level data per map (lowercase name).
    #[serde(default)]
    pub ai_levels: BTreeMap<String, dishonored_ir::ai::AiLevel>,
    /// Pickups placed in each mission map (lowercase name), put in the world on a first visit.
    #[serde(default)]
    pub pickups: BTreeMap<String, Vec<dishonored_ir::items::PlacedPickup>>,
    /// Doors and usables used natively (layout maps; original maps use theirs through the Kismet).
    #[serde(default)]
    pub usables: Vec<UsableDef>,
}

/// Resolve a place to (position, yaw, map index).
fn resolve_place(p: &Place, def: &MissionDef, maps: &[MapInput], depth: u32) -> Result<(Vec3, f32, usize), String> {
    if depth > 16 {
        return Err("marker reference cycle".into());
    }
    match p {
        Place::At(x, y, z) => Ok((Vec3::new(*x, *y, *z), 0.0, 0)),
        Place::Actor(a) => {
            // Each map's actors are their own namespace (packages are unique per map, names are
            // not: Ext and Int both have `DisGuardWatchPoint_5`). The start map is searched
            // first, then the others; a package-qualified name finds its own map.
            let mut first_err = None;
            for (mi, m) in maps.iter().enumerate() {
                let names: Vec<(String, String)> = m.actors.iter().map(|x| (x.0.clone(), x.1.clone())).collect();
                match resolve_actor(a, &names) {
                    Ok(i) => {
                        let x = &m.actors[i];
                        return Ok((x.2, x.3, mi));
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("ambiguous") {
                            return Err(msg);
                        }
                        first_err.get_or_insert(msg);
                    }
                }
            }
            Err(first_err.unwrap_or_else(|| format!("actor '{a}' not found")))
        }
        Place::Start(tag) => {
            for (mi, m) in maps.iter().enumerate() {
                if let Some(s) = m.tagged_starts.iter().find(|s| s.0.eq_ignore_ascii_case(tag)) {
                    return Ok((s.1, s.2, mi));
                }
            }
            Err(format!("no PlayerStart tagged '{tag}' in the mission maps"))
        }
        Place::Marker(m) => {
            let md = def.markers.iter().find(|x| &x.id == m).ok_or_else(|| format!("unknown marker '{m}'"))?;
            resolve_place(&md.at, def, maps, depth + 1)
        }
        Place::Offset(inner, (x, y, z)) => resolve_place(inner, def, maps, depth + 1).map(|(p, yaw, mi)| (p + Vec3::new(*x, *y, *z), yaw, mi)),
    }
}

impl MissionSetup {
    /// Resolve markers/NPCs against the levels (the first map is the mission's start map) and
    /// build the ambient crowd.
    pub fn new(def: MissionDef, maps: &[MapInput]) -> Result<Self, String> {
        let errs = def.validate();
        if !errs.is_empty() {
            return Err(errs.join("; "));
        }
        if maps.is_empty() {
            return Err("no maps".into());
        }
        let mut markers = BTreeMap::new();
        let mut marker_map = BTreeMap::new();
        for m in &def.markers {
            let (p, _, mi) = resolve_place(&m.at, &def, maps, 0)?;
            markers.insert(m.id.clone(), (p, m.radius));
            marker_map.insert(m.id.clone(), maps[mi].name.clone());
        }
        let (start_map, starts) = match &def.player_start {
            Some(s) => {
                let md = def.markers.iter().find(|m| &m.id == s).ok_or("player_start marker missing")?;
                let (p, yaw, mi) = resolve_place(&md.at, &def, maps, 0)?;
                (maps[mi].name.clone(), vec![(p, yaw)])
            }
            None => (maps[0].name.clone(), maps[0].default_starts.clone()),
        };
        let mut npcs = Vec::new();
        let mut npc_map = BTreeMap::new();
        for (i, n) in def.npcs.iter().enumerate() {
            let (p, yaw, mi) = resolve_place(&n.at, &def, maps, 0)?;
            let p = snap_floor(maps[mi].world, p).unwrap_or(p);
            let kind = match n.kind {
                NpcKind::Civilian => PawnKind::Civilian,
                NpcKind::Guard => PawnKind::Guard,
            };
            let archetype = n.archetype.unwrap_or(if kind == PawnKind::Guard { 1 } else { 0 });
            npcs.push((n.id.clone(), NpcSpawn { id: MISSION_NPC_BASE + i as PlayerId, kind, archetype, position: p, yaw }));
            npc_map.insert(n.id.clone(), maps[mi].name.clone());
        }
        let (crowd, crowd_map) = match &def.crowd {
            Some(c) => {
                let (center, mi) = match &c.center {
                    Some(p) => {
                        let r = resolve_place(p, &def, maps, 0)?;
                        (r.0, r.2)
                    }
                    None => (starts.first().map(|s| s.0).unwrap_or(Vec3::ZERO), maps.iter().position(|m| m.name == start_map).unwrap_or(0)),
                };
                let cfg = CrowdConfig { civilians: c.civilians, guards: c.guards, center, radius: c.radius, seed: 7, civilian_archetypes: vec![0, 2, 3], guard_archetypes: vec![1] };
                {
                    let mut crowd = Crowd::populate(&cfg, maps[mi].ai_points, maps[mi].guard_spawns, maps[mi].world);
                    crowd.set_routes(maps[mi].routes, maps[mi].world);
                    // [ai] factions (crowd guards and civilians take their spawners'), zones, bells, lights.
                    crowd.set_level(maps[mi].ai_level);
                    crowd.assign_crowd_factions(maps[mi].spawners);
                    (Some(crowd), maps[mi].name.clone())
                }
            }
            None => (None, start_map.clone()),
        };
        // Civilian panic destinations per map: the level's `DisFleePointActor`s.
        let flee: BTreeMap<String, Vec<Vec3>> = maps
            .iter()
            .map(|m| (m.name.to_ascii_lowercase(), m.actors.iter().filter(|a| a.1.starts_with("DisFleePointActor")).map(|a| snap_floor(m.world, a.2).unwrap_or(a.2)).collect()))
            .collect();
        let mut original = Vec::new();
        let mut appearances: Vec<Look> = Vec::new();
        let mut anim_profiles: Vec<dishonored_ir::npc_anim::AnimProfile> = Vec::new();
        for m in maps.iter().filter(|m| def.original_npcs.iter().any(|o| o.eq_ignore_ascii_case(&m.name))) {
            for s in m.spawners {
                let (kind, mut archetype) = original_archetype(s);
                let id = ORIGINAL_NPC_BASE + original.len() as PlayerId;
                // The pawn tweak's own body and head (the head picked per pawn, as the original
                // picks one of `m_RandomHeadMeshes`); the role archetype is the fallback.
                let parts = s.appearance.parts(id);
                if !parts.is_empty() {
                    // Per-pawn material variations (`m_MaterialVariation`, `m_BodyVariationMaterials`)
                    // and the tweak's animation setup ride with the look.
                    let anim = if s.appearance.anim.sets.is_empty() {
                        0
                    } else {
                        let k = anim_profiles.iter().position(|p| *p == s.appearance.anim).unwrap_or_else(|| {
                            anim_profiles.push(s.appearance.anim.clone());
                            anim_profiles.len() - 1
                        });
                        (k + 1) as u16
                    };
                    let look = Look { parts: parts.clone(), materials: s.appearance.materials(id), anim, accessories: s.appearance.accessories(id), tweak: s.tweak.clone() };
                    let i = appearances.iter().position(|a| *a == look).unwrap_or_else(|| {
                        appearances.push(look);
                        appearances.len() - 1
                    });
                    if i + (APPEARANCE_BASE as usize) < 256 {
                        archetype = APPEARANCE_BASE + i as u8;
                    }
                }
                let p = snap_floor(m.world, s.position).unwrap_or(s.position);
                original.push(OriginalNpc { map: m.name.clone(), name: s.name.clone(), spawn: NpcSpawn { id, kind, archetype, position: p, yaw: s.yaw }, tweak: s.tweak.clone(), props: s.props.clone(), parts, helmet: s.appearance.helmet(id) });
            }
        }
        let mut doors = Vec::new();
        let mut tagged = Vec::new();
        for m in maps {
            for (tag, p, yaw) in &m.tagged_starts {
                tagged.push((m.name.clone(), tag.clone(), *p, *yaw));
            }
            // Only doors whose destination is one of the mission's maps are travelable.
            for d in &m.doors {
                if maps.iter().any(|x| x.name.eq_ignore_ascii_case(&d.dest_map)) {
                    doors.push(d.clone());
                }
            }
        }
        Ok(MissionSetup { def, markers, marker_map, start_map, starts, npcs, npc_map, crowd, crowd_map, maps: maps.iter().map(|m| m.name.clone()).collect(), doors, tagged_starts: tagged, conversations: None, flee, original, appearances, pickups: BTreeMap::new(), usables: Vec::new(), anim_profiles, kismet: None, ai_levels: maps.iter().map(|m| (m.name.to_ascii_lowercase(), m.ai_level.clone())).collect(), routes: maps
                .iter()
                .map(|m| {
                    // Snapped to the floor as `Crowd::set_routes` does for the crowd's map.
                    let mut c = Crowd::default();
                    c.set_routes(m.routes, m.world);
                    (m.name.to_ascii_lowercase(), c.routes)
                })
                .collect() })
    }
}

/// What a map keeps while players are elsewhere (`DisSeqAct_SaveLevelState`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LevelState {
    npcs: Vec<PlayerState>,
    pickups: Vec<dishonored_sim::state::PickupState>,
    brains: Vec<Brain>,
    layout: CrowdLayout,
    spawns: Vec<NpcSpawn>,
    /// [ai] faction table (brains index it), forbidden zones, alarm bells, lights, overrides.
    level: Crowd,
}

/// What a player is about to interact with.
#[derive(Debug, Clone, PartialEq)]
pub enum Interaction {
    Mission(MissionEvent),
    /// Index into `MissionSetup::doors`.
    Door(usize),
    /// [kismet] Index into `kismet_host::usables` of this step.
    Kismet(usize),
    /// Index into `MissionSetup::usables` (a native door or usable).
    Prop(usize),
}

/// The whole match state serializes for saved games (`save.rs`) except the setup, which the
/// loader rebuilds from the mission and re-attaches. New fields need serde
/// (`#[serde(default)]` keeps older saves loading).
#[derive(Serialize, Deserialize)]
pub struct MissionMatch {
    pub world: WorldState,
    pub view: MatchView,
    pub state: MissionState,
    pub crowd: Crowd,
    #[serde(skip)]
    pub setup: MissionSetup,
    pub last_events: Vec<SimEvent>,
    /// Active persistent map.
    pub current_map: String,
    saved: BTreeMap<String, LevelState>,
    npc_by_id: BTreeMap<PlayerId, String>,
    id_by_npc: BTreeMap<String, PlayerId>,
    humans: Vec<PlayerId>,
    respawn_queue: Vec<(PlayerId, f32)>,
    /// Guards currently in combat with a human (edge detection for "detected").
    engaged: BTreeSet<PlayerId>,
    alerted: BTreeSet<PlayerId>,
    started: bool,
    use_held: BTreeSet<PlayerId>,
    pending_travel: Option<(String, String)>,
    /// Each NPC's own archetype before material swaps (`ClearMaterials` restores it).
    own_look: BTreeMap<PlayerId, u8>,
    /// Current look of NPCs whose materials a mission changed (also for ones not spawned yet).
    look_override: BTreeMap<PlayerId, u8>,
    /// Original spawners that have spawned their pawn (ids are unique across maps).
    original_spawned: BTreeSet<PlayerId>,
    /// Players with a door confirmation open (player -> door index).
    choosing: BTreeMap<PlayerId, usize>,
    arrived: Option<String>,
    /// Dialog choice buttons held last tick (edge detection).
    choice_held: BTreeMap<PlayerId, u16>,
    /// [kismet] Original Kismet runtime, and original spawners spawned since it last looked.
    pub kismet: Option<kismet_host::KismetRun>,
    spawn_log: Vec<usize>,
    /// Pickups carried by spawners whose pawn has not spawned yet: (pickup id, spawner name).
    pending_stealables: Vec<(u32, String)>,
    /// [save] Pending autosaves and the saves taken this step.
    #[serde(default)]
    pub autosave: save::AutosaveState,
    /// States of the natively used doors and usables (`MissionSetup::usables`), by actor.
    #[serde(default)]
    pub props: BTreeMap<String, PropState>,
    /// Mover collision placed where the movers are: (actor, delta) and, in the same order, the
    /// placed collision (rebuilt when a mover moves).
    #[serde(skip)]
    mover_keys: Vec<(String, glam::Mat4, u8)>,
    #[serde(skip)]
    mover_worlds: Vec<dishonored_world::CollisionWorld>,
}

impl MissionMatch {
    pub fn new(setup: MissionSetup, cfg: &SimConfig) -> Self {
        let mut npc_by_id = BTreeMap::new();
        let mut id_by_npc = BTreeMap::new();
        for (name, s) in &setup.npcs {
            npc_by_id.insert(s.id, name.clone());
            id_by_npc.insert(name.clone(), s.id);
        }
        let mut state = MissionState::new(setup.def.clone(), setup.markers.clone());
        state.marker_map = setup.marker_map.clone();
        let current_map = setup.start_map.clone();
        if let Some(db) = &setup.conversations {
            state.set_conversations(std::sync::Arc::new(db.clone()));
        }
        let mut m = MissionMatch {
            world: WorldState::default(),
            view: MatchView { mode: "mission".into(), phase: MatchPhase::Playing, mission: Some(state.view.clone()), map: current_map.clone(), appearances: setup.appearances.clone(), anim_profiles: setup.anim_profiles.clone(), ..Default::default() },
            state,
            crowd: Crowd { ai: cfg.gameplay.ai.clone(), ..Default::default() },
            setup,
            last_events: Vec::new(),
            current_map,
            saved: BTreeMap::new(),
            npc_by_id,
            id_by_npc,
            humans: Vec::new(),
            respawn_queue: Vec::new(),
            engaged: BTreeSet::new(),
            alerted: BTreeSet::new(),
            started: false,
            use_held: BTreeSet::new(),
            pending_travel: None,
            choosing: BTreeMap::new(),
            original_spawned: BTreeSet::new(),
            own_look: BTreeMap::new(),
            look_override: BTreeMap::new(),
            arrived: None,
            choice_held: BTreeMap::new(),
            kismet: None,
            spawn_log: Vec::new(),
            pending_stealables: Vec::new(),
            autosave: Default::default(),
            props: BTreeMap::new(),
            mover_keys: Vec::new(),
            mover_worlds: Vec::new(),
        };
        m.props = m.setup.usables.iter().map(|u| (u.actor.clone(), PropState { door: u.initial, locked: u.locked, stage: 0 })).collect();
        // [kismet]
        m.kismet = m.setup.kismet.as_ref().map(|k| kismet_host::KismetRun::new(k, &m.setup));
        m.populate_fresh(cfg);
        for c in m.state.initial_commands() {
            m.apply(c, cfg);
        }
        m
    }

    /// First visit of the active map: its share of the crowd and of the mission NPCs.
    fn populate_fresh(&mut self, cfg: &SimConfig) {
        if self.setup.crowd_map.eq_ignore_ascii_case(&self.current_map) {
            if let Some(c) = self.setup.crowd.clone() {
                for s in &c.spawns {
                    spawn_npc(&mut self.world, s.id, s.kind, s.archetype, s.position, s.yaw, cfg);
                }
                self.crowd.brains.extend(c.brains);
                self.crowd.layout = c.layout;
                self.crowd.spawns = c.spawns;
                // [ai] the crowd's faction table (its brains index it) and level data.
                self.crowd.factions = c.factions;
                self.crowd.player_faction = c.player_faction;
                self.crowd.tweak_factions = c.tweak_factions;
                self.crowd.zones = c.zones;
                self.crowd.alarms = c.alarms;
                self.crowd.lights = c.lights;
            }
        }
        self.crowd.layout.flee_points = self.setup.flee.get(&self.current_map.to_ascii_lowercase()).cloned().unwrap_or_default();
        // [ai] the map's factions, forbidden zones, alarm bells and probe lights.
        if self.crowd.player_faction.is_none() {
            if let Some(l) = self.setup.ai_levels.get(&self.current_map.to_ascii_lowercase()) {
                self.crowd.set_level(l);
            }
        }
        // The map's own patrol routes (the crowd of the start map already has them).
        if self.crowd.routes.is_empty() {
            self.crowd.routes = self.setup.routes.get(&self.current_map.to_ascii_lowercase()).cloned().unwrap_or_default();
        }
        // Original spawners that spawn on begin play (not alarm squads or travel pursuers).
        for i in 0..self.setup.original.len() {
            let o = &self.setup.original[i];
            if o.map.eq_ignore_ascii_case(&self.current_map) && o.props.on_begin_play && o.props.travel_tags.is_empty() && o.props.at_suspicion.is_none() {
                self.spawn_original(i, false, cfg);
            }
        }
        for i in 0..self.setup.npcs.len() {
            let name = self.setup.npcs[i].0.clone();
            let here = self.setup.npc_map.get(&name).is_some_and(|m| m.eq_ignore_ascii_case(&self.current_map));
            if here && self.state.npc_alive.contains(&name) {
                self.spawn_mission_npc(i, cfg);
            }
        }
        // The map's placed pickups; a spawner's `m_pStealablePickup` rides on its pawn.
        if let Some(mut placed) = self.setup.pickups.get(&self.current_map.to_ascii_lowercase()).cloned() {
            // [kismet] Pickups with a `DisSeqEvent_PickupPickedUp` stay Kismet usables.
            let scripted = kismet_host::scripted_pickups(self);
            placed.retain(|p| !scripted.iter().any(|(pk, n)| n.eq_ignore_ascii_case(&p.name) && pk.eq_ignore_ascii_case(&p.package)));
            let owners: BTreeMap<String, PlayerId> = self.setup.original.iter().filter(|o| o.map.eq_ignore_ascii_case(&self.current_map) && self.original_spawned.contains(&o.spawn.id)).map(|o| (o.name.to_ascii_lowercase(), o.spawn.id)).collect();
            let seed = self.setup.def.title.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
            let first = self.world.pickups.len();
            dishonored_sim::inventory::spawn_pickups(&mut self.world, &placed, seed, &|s: &str| owners.get(&s.to_ascii_lowercase()).copied());
            // Carried by a spawner whose pawn has not spawned yet: attach when it does.
            for (k, pp) in placed.iter().enumerate() {
                if let (Some(s), Some(pk)) = (&pp.stealable_from, self.world.pickups.get(first + k)) {
                    if pk.carried_by.is_none() {
                        self.pending_stealables.push((pk.id, s.clone()));
                    }
                }
            }
        }
    }

    /// Spawn an original spawner's pawn (once). Corpse / knocked-out spawners lie on the floor;
    /// `alert` (or `m_bAwareOfPlayerUponStartup`) starts it hunting the nearest player.
    fn spawn_original(&mut self, index: usize, alert: bool, cfg: &SimConfig) {
        let o = self.setup.original[index].clone();
        if !self.original_spawned.insert(o.spawn.id) {
            return;
        }
        if self.setup.kismet.is_some() {
            self.spawn_log.push(index); // [kismet] DisSeqEvent_Spawned
        }
        let s = o.spawn;
        let arch = self.look_override.get(&s.id).copied().unwrap_or(s.archetype);
        spawn_npc(&mut self.world, s.id, s.kind, arch, s.position, s.yaw, cfg);
        if let Some(p) = self.world.player_mut(s.id) {
            p.helmet = o.helmet;
        }
        if o.props.spawn_dead || o.props.knocked_out {
            if let Some(p) = self.world.player_mut(s.id) {
                // `m_bTreatAsKnockedOut`: unconscious (non-lethal, can be carried), not dead.
                if o.props.spawn_dead {
                    p.alive = false;
                } else {
                    p.condition = dishonored_sim::state::Condition::Unconscious;
                }
            }
            return;
        }
        let mut b = Brain::new(s.id, s.kind, s.position, 0x5EED ^ s.id as u64);
        b.stay = !o.props.patrol_on_startup;
        if o.props.patrol_on_startup {
            // [ai] `EAIStimID_PatrolRequest` at startup: `DisBehaviorPatrol` adopts a route
            // (squad, capacity, adoption range) and walks it.
            b.request_patrol();
        }
        // [ai] The pawn tweak's locomotion config (`m_pComponentLocomotionConfig`): its NPC
        // moves use that config's authored loop speeds.
        b.loco = arch
            .checked_sub(APPEARANCE_BASE)
            .and_then(|i| self.view.appearances.get(i as usize))
            .and_then(|l| l.anim.checked_sub(1))
            .and_then(|k| self.setup.anim_profiles.get(k as usize))
            .and_then(|p| p.locomotion.as_ref())
            .and_then(|(_, path)| self.crowd.ai.locomotion.iter().position(|c| c.name.eq_ignore_ascii_case(path)))
            .map(|i| i as u16);
        // [ai] DisSpawnerInfo: faction override (else the pawn tweak's), squad, and the
        // spawner's m_SpawnAtSuspicionLevel.
        b.faction = self.crowd.spawner_faction(&o.props.faction, &o.tweak);
        b.squad = o.props.squad.clone();
        b.suspecting = o.props.at_suspicion.as_deref() == Some("DAISL_Suspecting");
        self.crowd.brains.retain(|x| x.id != s.id);
        self.crowd.brains.push(b);
        if alert {
            self.alert_npc(s.id);
        } else if o.props.aware_on_startup {
            // [ai] m_bAwareOfPlayerUponStartup: max attention (Psychic) on the player.
            if let Some(t) = self.nearest_human(s.position) {
                let at = self.world.player(t).map(|p| p.position).unwrap_or(s.position);
                let ai = self.crowd.ai.clone();
                if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == s.id) {
                    b.aware_of(&ai, t, at);
                }
            }
        }
    }

    /// A mission NPC id or an original spawner name -> pawn id.
    fn npc_pawn(&self, name: &str) -> Option<PlayerId> {
        self.id_by_npc.get(name).copied().or_else(|| self.setup.original.iter().find(|o| o.name.eq_ignore_ascii_case(name)).map(|o| o.spawn.id))
    }

    /// `DisSeqAct_NPCSetMaterials`: give the NPC a look with these materials over its own (Set), or
    /// its own look back (Clear). Looks are shared in `MatchView::appearances`.
    pub fn set_materials(&mut self, npc: &str, mats: Option<Vec<(u8, u32, String)>>) {
        let Some(id) = self.npc_pawn(npc) else { return };
        self.set_materials_id(id, mats);
    }

    /// [kismet] `set_materials` by pawn id (original spawner names repeat across sublevels).
    pub fn set_materials_id(&mut self, id: PlayerId, mats: Option<Vec<(u8, u32, String)>>) {
        let base = APPEARANCE_BASE;
        // Not spawned yet (a map not visited): its spawner's look, applied when it spawns.
        let Some(cur) = self
            .world
            .player(id)
            .map(|p| p.archetype)
            .or_else(|| self.saved.values().flat_map(|l| l.npcs.iter()).find(|p| p.id == id).map(|p| p.archetype))
            .or_else(|| self.look_override.get(&id).copied())
            .or_else(|| self.setup.original.iter().find(|o| o.spawn.id == id).map(|o| o.spawn.archetype))
        else {
            return;
        };
        let own = *self.own_look.entry(id).or_insert(cur);
        let new = match mats {
            None => own,
            Some(m) => {
                // Only tweak-resolved looks have the parts to re-material.
                let Some(look) = own.checked_sub(base).and_then(|i| self.view.appearances.get(i as usize)).cloned() else { return };
                let mut materials = look.materials.clone();
                for (p, i, path) in m {
                    materials.retain(|x| !(x.0 == p && x.1 == i));
                    materials.push((p, i, path));
                }
                let want = Look { parts: look.parts, materials, anim: look.anim, accessories: look.accessories, tweak: look.tweak };
                let idx = match self.view.appearances.iter().position(|l| *l == want) {
                    Some(i) => i,
                    None => {
                        self.view.appearances.push(want);
                        self.view.appearances.len() - 1
                    }
                };
                if idx + base as usize >= 256 {
                    return;
                }
                base + idx as u8
            }
        };
        self.look_override.insert(id, new);
        if let Some(p) = self.world.player_mut(id) {
            p.archetype = new;
        }
        for l in self.saved.values_mut() {
            if let Some(p) = l.npcs.iter_mut().find(|p| p.id == id) {
                p.archetype = new;
            }
        }
    }

    /// Put an NPC brain into combat (guards) or flight (civilians) against the nearest player.
    pub fn alert_npc(&mut self, id: PlayerId) {
        let pos = self.world.player(id).map(|p| p.position).unwrap_or(Vec3::ZERO);
        let target = self.nearest_human(pos);
        let threat = target.and_then(|t| self.world.player(t)).map(|p| p.position);
        let ai = self.crowd.ai.clone();
        if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == id) {
            // Like `DisSeqAct_SetDisposition` / `AISetSuspicionLevel`: the human becomes an enemy
            // at full attention, so perception keeps it alerted.
            if let (Some(t), Some(p)) = (target, threat) {
                b.alert_to(&ai, t, p);
            }
            b.target = target;
            b.state = if b.kind == PawnKind::Guard { BrainState::Combat } else { BrainState::Flee };
            b.threat_pos = threat;
            b.timer = 0.0;
        }
    }

    fn spawn_mission_npc(&mut self, index: usize, cfg: &SimConfig) {
        let (name, s) = self.setup.npcs[index].clone();
        // NPCs of another map appear when players travel there (state.npc_alive tracks them).
        if !self.setup.npc_map.get(&name).is_some_and(|m| m.eq_ignore_ascii_case(&self.current_map)) {
            return;
        }
        let def = self.setup.def.npcs[index].clone();
        spawn_npc(&mut self.world, s.id, s.kind, s.archetype, s.position, s.yaw, cfg);
        // A passive NPC (a training dummy) has no brain: it stands and takes hits.
        if def.passive {
            self.crowd.brains.retain(|x| x.id != s.id);
            return;
        }
        let mut b = Brain::new(s.id, s.kind, s.position, 0x5EED ^ s.id as u64);
        b.stay = matches!(def.behavior, Behavior::Idle);
        if let Some(f) = &def.faction {
            b.faction = Some(self.crowd.factions.intern(f));
        }
        self.crowd.brains.retain(|x| x.id != s.id);
        self.crowd.brains.push(b);
    }

    pub fn add_player(&mut self, id: PlayerId, name: &str, archetype: u8, cfg: &SimConfig) {
        let at = self.start_for(self.humans.len());
        self.spawn_human(id, at, archetype, cfg);
        if !self.humans.contains(&id) {
            self.humans.push(id);
            self.view.scores.push(ScoreEntry { id, name: name.to_string(), kills: 0, deaths: 0, score: 0 });
        }
    }

    pub fn remove_player(&mut self, id: PlayerId) {
        self.humans.retain(|h| *h != id);
        self.world.players.retain(|p| p.id != id);
        self.view.scores.retain(|s| s.id != id);
    }

    fn fan(i: usize, s: (Vec3, f32)) -> (Vec3, f32) {
        // Co-op players fan out around a start.
        let off = if i == 0 { Vec3::ZERO } else { Vec3::new((i as f32 * 2.1).cos(), (i as f32 * 2.1).sin(), 0.0) * 120.0 };
        (s.0 + off, s.1)
    }

    fn start_for(&self, i: usize) -> (Vec3, f32) {
        if !self.current_map.eq_ignore_ascii_case(&self.setup.start_map) {
            // Joining after travel: the arrival start of the current map.
            if let Some(t) = self.setup.tagged_starts.iter().find(|t| t.0.eq_ignore_ascii_case(&self.current_map)) {
                return Self::fan(i, (t.2, t.3));
            }
        }
        let s = self.setup.starts.get(i % self.setup.starts.len().max(1)).copied().unwrap_or((Vec3::ZERO, 0.0));
        Self::fan(i, s)
    }

    fn nearest_human(&self, from: Vec3) -> Option<PlayerId> {
        self.humans.iter().filter_map(|h| self.world.player(*h)).filter(|p| p.alive).min_by(|a, b| a.position.distance(from).total_cmp(&b.position.distance(from))).map(|p| p.id)
    }

    /// The persistent map clients should show and hosts should collide against.
    pub fn current_map(&self) -> &str {
        &self.current_map
    }

    /// Level travel (see module docs). Runs between steps' simulation and next think.
    fn travel(&mut self, dest_map: &str, tag: &str, cfg: &SimConfig) {
        let Some((_, _, pos, yaw)) = self.setup.tagged_starts.iter().find(|t| t.0.eq_ignore_ascii_case(dest_map) && t.1.eq_ignore_ascii_case(tag)).cloned() else { return };
        // A guard at `DAISL_Suspecting` or above (or investigating, searching or fighting):
        // so `DisTravelSpawner`s at the arrival tag send pursuers through the door.
        let pursued = self.crowd.brains.iter().any(|b| b.kind == PawnKind::Guard && (b.suspicion_level() >= 1 || matches!(b.state, BrainState::Investigate | BrainState::Search | BrainState::Combat)));
        if !dest_map.eq_ignore_ascii_case(&self.current_map) {
            // Save the level being left.
            let humans = self.humans.clone();
            let npcs: Vec<PlayerState> = self.world.players.iter().filter(|p| !humans.contains(&p.id)).cloned().collect();
            let fresh = Crowd { ai: self.crowd.ai.clone(), ..Default::default() };
            let crowd = std::mem::replace(&mut self.crowd, fresh);
            self.saved.insert(self.current_map.to_ascii_lowercase(), LevelState { npcs, pickups: std::mem::take(&mut self.world.pickups), brains: crowd.brains.clone(), layout: crowd.layout.clone(), spawns: crowd.spawns.clone(), level: Crowd { brains: Vec::new(), ..crowd } });
            self.world.players.retain(|p| humans.contains(&p.id));
            self.world.projectiles.clear();
            self.current_map = self.setup.maps.iter().find(|m| m.eq_ignore_ascii_case(dest_map)).cloned().unwrap_or_else(|| dest_map.to_string());
            match self.saved.remove(&self.current_map.to_ascii_lowercase()) {
                Some(ls) => {
                    self.world.players.extend(ls.npcs);
                    self.world.pickups = ls.pickups;
                    let routes = self.setup.routes.get(&self.current_map.to_ascii_lowercase()).cloned().unwrap_or_default();
                    self.crowd = Crowd { brains: ls.brains, layout: ls.layout, spawns: ls.spawns, ai: self.crowd.ai.clone(), routes, factions: ls.level.factions, player_faction: ls.level.player_faction, tweak_factions: ls.level.tweak_factions, zones: ls.level.zones, overrides: ls.level.overrides, alarms: ls.level.alarms, lights: ls.level.lights, ..Default::default() };
                }
                None => self.populate_fresh(cfg),
            }
            self.engaged.clear();
            self.alerted.clear();
            self.choosing.clear();
        }
        for (i, h) in self.humans.clone().into_iter().enumerate() {
            let (p, y) = Self::fan(i, (pos, yaw));
            if let Some(pl) = self.world.player_mut(h) {
                pl.position = p;
                pl.yaw = y;
                // m_bUseDestinationTargetRotation defaults to true: the PlayerStart's rotation.
                pl.pitch = 0.0;
                pl.velocity = Vec3::ZERO;
            }
        }
        if pursued {
            for i in 0..self.setup.original.len() {
                let o = &self.setup.original[i];
                if o.map.eq_ignore_ascii_case(&self.current_map) && o.props.at_suspicion.is_some() && o.props.travel_tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                    self.spawn_original(i, true, cfg);
                }
            }
        }
        self.arrived = Some(self.current_map.clone());
        self.view.map = self.current_map.clone();
    }

    fn apply(&mut self, c: Command, cfg: &SimConfig) {
        let brain = |s: &mut Self, n: &str| s.id_by_npc.get(n).copied();
        match c {
            Command::Spawn(n) => {
                if let Some(i) = self.setup.def.npcs.iter().position(|x| x.id == n) {
                    self.spawn_mission_npc(i, cfg);
                }
            }
            Command::Despawn(n) => {
                if let Some(id) = brain(self, &n) {
                    self.world.players.retain(|p| p.id != id);
                    self.crowd.brains.retain(|b| b.id != id);
                    for ls in self.saved.values_mut() {
                        ls.npcs.retain(|p| p.id != id);
                        ls.brains.retain(|b| b.id != id);
                    }
                }
            }
            Command::Move { npc, to, run } => {
                if let Some(id) = brain(self, &npc) {
                    if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == id) {
                        b.goal = Some(to);
                        b.run = run;
                        b.spot = None;
                        b.stay = true;
                        b.state = BrainState::WalkToPoint;
                    }
                }
            }
            Command::Patrol { npc, route } => {
                if let Some(id) = brain(self, &npc) {
                    if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == id) {
                        b.route = route;
                        b.route_i = 0;
                        b.stay = false;
                        b.state = BrainState::Patrol;
                    }
                }
            }
            Command::Alert(npc) => {
                if let Some(id) = brain(self, &npc) {
                    let pos = self.world.player(id).map(|p| p.position).unwrap_or(Vec3::ZERO);
                    let target = self.nearest_human(pos);
                    let ai = self.crowd.ai.clone();
                    if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == id) {
                        let tpos = target.and_then(|t| self.world.player(t)).map(|p| p.position);
                        // Like `DisSeqAct_SetDisposition` / `AISetSuspicionLevel`: the human
                        // becomes an enemy at full attention, so perception keeps it alerted.
                        if let (Some(t), Some(p)) = (target, tpos) {
                            b.alert_to(&ai, t, p);
                        }
                        b.target = target;
                        b.state = if b.kind == PawnKind::Guard { BrainState::Combat } else { BrainState::Flee };
                        b.threat_pos = tpos;
                        b.timer = 0.0;
                    }
                }
            }
            Command::Calm(npc) => {
                if let Some(id) = brain(self, &npc) {
                    if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == id) {
                        b.calm();
                        b.state = BrainState::Idle;
                    }
                }
            }
            Command::SetMaterials { npc, body, head } => {
                let mats: Vec<(u8, u32, String)> = body.iter().map(|(i, m)| (0u8, *i, m.clone())).chain(head.iter().map(|(i, m)| (1u8, *i, m.clone()))).collect();
                self.set_materials(&npc, Some(mats));
            }
            Command::ClearMaterials(npc) => self.set_materials(&npc, None),
            Command::Travel(tag) => {
                if let Some(t) = self.setup.tagged_starts.iter().find(|t| t.1.eq_ignore_ascii_case(&tag)).cloned() {
                    self.pending_travel = Some((t.0, t.1));
                }
            }
            Command::Sound(_) => {}
            Command::Equip(l) => {
                for h in self.humans.clone() {
                    if let Some(p) = self.world.player_mut(h) {
                        apply_loadout(p, &l, cfg);
                    }
                }
            }
        }
    }

    /// Spawn (or respawn) a human with the mission's loadout.
    fn spawn_human(&mut self, id: PlayerId, at: (Vec3, f32), archetype: u8, cfg: &SimConfig) {
        spawn_player(&mut self.world, id, at.0, at.1, cfg);
        let loadout = self.setup.def.loadout.clone();
        if let Some(pl) = self.world.player_mut(id) {
            pl.archetype = archetype;
            if let Some(l) = &loadout {
                apply_loadout(pl, l, cfg);
            }
        }
    }

    /// A player uses a native door or usable (`MissionSetup::usables[i]`).
    fn use_prop(&mut self, player: PlayerId, i: usize) {
        let Some(u) = self.setup.usables.get(i).cloned() else { return };
        let Some(user) = self.world.player(player).map(|p| p.position) else { return };
        let st = self.props.entry(u.actor.clone()).or_insert(PropState { door: u.initial, locked: u.locked, stage: 0 });
        match u.door_axis {
            Some(axis) => match door_use(*st, u.location, axis, user) {
                Some(next) => st.door = next,
                None => {
                    // The original's "used while locked" output: a HUD line.
                    let _ = self.state.apply(&[dishonored_mission::Action::Message(u.locked_prompt.clone())], &BTreeMap::new());
                }
            },
            None => st.stage = (st.stage + 1) % u.stages.max(2),
        }
    }

    pub fn step(&mut self, inputs: &[(PlayerId, PlayerInput)], world: &dyn WorldQuery, cfg: &SimConfig) -> StepOutput {
        let dt = dishonored_sim::movement::DT;
        // A player answering a dialog choice does not move or act (the original takes control away).
        let conv_chooser = self.state.view.choice.as_ref().and_then(|c| c.player);
        let mut all: Vec<(PlayerId, PlayerInput)> = inputs
            .iter()
            .map(|(id, i)| {
                if self.choosing.contains_key(id) || conv_chooser == Some(*id) || self.kismet.as_ref().is_some_and(|k| k.choosing(*id) || k.locked(*id)) /* [interp] */ {
                    (*id, PlayerInput { tick: i.tick, yaw: i.yaw, pitch: i.pitch, left_hand: i.left_hand, ..Default::default() })
                } else {
                    (*id, *i)
                }
            })
            .collect();
        kismet_host::track_views(self, &mut all, dt); // [kismet] DisSeqAct_PlayerTrackTarget
        // Movers (`InterpActor`s) collide where the Kismet has moved them.
        self.place_movers(world);
        let placed = std::mem::take(&mut self.mover_worlds);
        let layered = dishonored_world::Layered { base: world, layers: &placed };
        let world: &dyn WorldQuery = if placed.is_empty() { world } else { &layered };
        // [kismet] Doors the original Kismet locked or unlocked change the AI's paths.
        let door_nav = self.kismet.as_mut().and_then(|k| k.door_nav(&self.current_map, world.nav()));
        let overlay = door_nav.as_deref().map(|nav| kismet_host::NavOverlay { inner: world, nav });
        let ai_world: &dyn WorldQuery = match &overlay {
            Some(o) => o,
            None => world,
        };
        let thought = self.crowd.think(&self.world, &self.last_events, ai_world, dt);
        // NPCs walking a navmesh path move on the navmesh (`m_bNewNPCPhysWalking`,
        // `movement::nav_walk_move`; research/dishonored/AI.md "Go-to completion").
        for b in &self.crowd.brains {
            let moving = thought.iter().any(|(id, i)| *id == b.id && (i.forward != 0 || i.right != 0));
            if let Some(p) = self.world.player_mut(b.id) {
                p.nav_walk = moving && !b.follow.path.is_empty();
                // A scripted go-to stops on its path's end (accurate stop, 0.1 uu).
                p.nav_end = b.accurate_stop();
            }
        }
        all.extend(thought);
        self.crowd.write_awareness(&mut self.world);
        let out = step_world(&mut self.world, &all, world, cfg);
        let mut events = Vec::new();
        if let Some(m) = self.arrived.take() {
            events.push(MissionEvent::Arrived(m));
        }
        // Open door confirmations: option 1 travels, any other option stays.
        for (id, inp) in inputs {
            let Some(&door) = self.choosing.get(id) else { continue };
            let picked = dishonored_sim::input::buttons::CHOICES.iter().position(|b| inp.buttons & b != 0);
            if let Some(n) = picked {
                self.choosing.remove(id);
                self.use_held.insert(*id);
                if n == 0 {
                    let d = &self.setup.doors[door];
                    if let Some(s) = d.sound.clone() {
                        self.state.post_sound(&s);
                    }
                    self.pending_travel = Some((d.dest_map.clone(), d.dest_tag.clone()));
                }
            }
        }
        // F (use): on press, interact with what the player is looking at.
        for (id, inp) in inputs {
            let held = inp.buttons & dishonored_sim::input::buttons::CHOICES.iter().fold(0, |a, b| a | b);
            let prev = self.choice_held.insert(*id, held).unwrap_or(0);
            if self.choosing.contains_key(id) {
                continue;
            }
            // Conversation choices (`DisConv_PlayerChoice`), on the CHOICE_n press edge.
            if let Some(k) = dishonored_sim::input::buttons::CHOICES.iter().position(|b| held & b != 0 && prev & b == 0) {
                events.push(MissionEvent::Choice { player: *id, index: k });
            }
            let pressed = inp.buttons & dishonored_sim::input::buttons::USE != 0;
            if pressed && self.use_held.insert(*id) {
                match self.interaction(*id) {
                    Some(Interaction::Mission(MissionEvent::Talked(npc))) => events.push(MissionEvent::TalkedBy { npc, player: *id }),
                    Some(Interaction::Mission(e)) => events.push(e),
                    Some(Interaction::Door(i)) => {
                        let d = &self.setup.doors[i];
                        if d.choices.is_empty() {
                            self.pending_travel = Some((d.dest_map.clone(), d.dest_tag.clone()));
                        } else {
                            self.choosing.insert(*id, i);
                        }
                    }
                    Some(Interaction::Kismet(i)) => kismet_host::use_usable(self, *id, i),
                    Some(Interaction::Prop(i)) => self.use_prop(*id, i),
                    None => {}
                }
            } else if !pressed {
                self.use_held.remove(id);
            }
        }
        for e in &out.events {
            if let SimEvent::Death { victim, killer, .. } = e {
                let vk = self.world.player(*victim).map(|p| p.kind);
                if self.humans.contains(victim) {
                    events.push(MissionEvent::PlayerKilled(*victim));
                    if let Some(s) = self.view.scores.iter_mut().find(|s| s.id == *victim) {
                        s.deaths += 1;
                    }
                    if self.setup.def.settings.respawn {
                        self.respawn_queue.push((*victim, self.setup.def.settings.respawn_delay));
                    }
                } else if let Some(k) = vk {
                    let by_player = killer.is_some_and(|k| self.humans.contains(&k));
                    if by_player {
                        if let Some(s) = self.view.scores.iter_mut().find(|s| Some(s.id) == *killer) {
                            s.kills += 1;
                        }
                    }
                    let kind = if k == PawnKind::Guard { NpcKind::Guard } else { NpcKind::Civilian };
                    events.push(MissionEvent::NpcKilled { npc: self.npc_by_id.get(victim).cloned(), by_player, kind });
                    self.crowd.brains.retain(|b| b.id != *victim);
                }
            }
            // Inventory and non-lethal takedowns.
            match e {
                SimEvent::PickedUp { id, pickup } => {
                    if let Some(pk) = self.world.pickups.iter().find(|p| p.id == *pickup) {
                        let item = match &pk.kind {
                            dishonored_ir::items::PickupKind::Abstract { item, .. } | dishonored_ir::items::PickupKind::Inventory { item } => Some(item.clone()),
                            _ => None,
                        };
                        events.push(MissionEvent::PickedUp { player: *id, pickup: pk.name.clone(), item });
                    }
                }
                SimEvent::KnockedOut { victim, by, .. } if !self.humans.contains(victim) => {
                    let by_player = by.is_some_and(|k| self.humans.contains(&k));
                    let kind = if self.world.player(*victim).is_some_and(|p| p.kind == PawnKind::Guard) { NpcKind::Guard } else { NpcKind::Civilian };
                    events.push(MissionEvent::NpcKnockedOut { npc: self.npc_by_id.get(victim).cloned(), by_player, kind });
                    // Unconscious for good: its brain stops (the sim ignores its input anyway).
                    self.crowd.brains.retain(|b| b.id != *victim);
                }
                _ => {}
            }
        }
        // Stealable pickups whose carrier has spawned since.
        if !self.pending_stealables.is_empty() {
            let pend = std::mem::take(&mut self.pending_stealables);
            for (pid, spawner) in pend {
                let owner = self.setup.original.iter().find(|o| o.name.eq_ignore_ascii_case(&spawner) && self.original_spawned.contains(&o.spawn.id)).map(|o| o.spawn.id);
                match (owner, self.world.pickups.iter_mut().find(|p| p.id == pid)) {
                    (Some(o), Some(pk)) if pk.active => pk.carried_by = Some(o),
                    (None, Some(pk)) if pk.active => self.pending_stealables.push((pid, spawner)),
                    _ => {}
                }
            }
        }
        // Detection / alarm / alerted mission NPCs from brain states (edge-triggered).
        for b in &self.crowd.brains {
            let hunting_human = b.state == BrainState::Combat && b.target.is_some_and(|t| self.humans.contains(&t));
            if b.kind == PawnKind::Guard && hunting_human {
                let calm_before = self.engaged.is_empty();
                if self.engaged.insert(b.id) {
                    events.push(MissionEvent::PlayerDetected);
                    let _ = calm_before;
                }
            }
            if !hunting_human {
                self.engaged.remove(&b.id);
            }
            if let Some(n) = self.npc_by_id.get(&b.id) {
                let alerted = matches!(b.state, BrainState::Combat | BrainState::Flee | BrainState::React | BrainState::Investigate);
                if alerted && self.alerted.insert(b.id) {
                    events.push(MissionEvent::NpcAlerted(n.clone()));
                } else if !alerted {
                    self.alerted.remove(&b.id);
                }
            }
        }
        // [ai] The alarm comes from the original mechanism: a guard rang a `DisAlarmBell`
        // (AlarmRaised); after the bell's ring delay its `m_TriggeredSpawners` spawn, aware of
        // the alarm's enemy (`alarm_spawn_choice`).
        let ai_events = self.crowd.events.clone();
        for e in &ai_events {
            match e {
                dishonored_ai::AiEvent::AlarmRang { .. } => events.push(MissionEvent::AlarmRaised),
                dishonored_ai::AiEvent::AlarmSpawn { bell, enemy_pos, .. } => {
                    let Some(def) = self.crowd.alarms.get(*bell).map(|a| a.def.clone()) else { continue };
                    let here: Vec<(String, Vec3)> = self.setup.original.iter().filter(|o| o.map.eq_ignore_ascii_case(&self.current_map)).map(|o| (o.name.clone(), o.spawn.position)).collect();
                    for name in dishonored_ai::alarm_spawn_choice(&def, &here, *enemy_pos) {
                        if let Some(i) = self.setup.original.iter().position(|o| o.map.eq_ignore_ascii_case(&self.current_map) && o.name.eq_ignore_ascii_case(&name)) {
                            self.spawn_original(i, true, cfg);
                        }
                    }
                }
                dishonored_ai::AiEvent::Bark { npc, hook, initiator } => self.request_bark(*npc, hook, *initiator),
                dishonored_ai::AiEvent::AlarmEnded { .. } | dishonored_ai::AiEvent::RatEat { .. } => {}
            }
        }
        // Respawns (if the mission allows them).
        let mut due = Vec::new();
        for (id, t) in self.respawn_queue.iter_mut() {
            *t -= dt;
            if *t <= 0.0 {
                due.push(*id);
            }
        }
        self.respawn_queue.retain(|(_, t)| *t > 0.0);
        for id in due {
            let i = self.humans.iter().position(|h| *h == id).unwrap_or(0);
            let at = self.start_for(i);
            let arch = self.world.player(id).map(|p| p.archetype).unwrap_or(0);
            self.spawn_human(id, at, arch, cfg);
        }
        // Mission interpreter (only once someone is in the game).
        if !self.humans.is_empty() {
            self.started = true;
        }
        if self.started {
            let players: Vec<(u32, Vec3, bool)> = self.humans.iter().filter_map(|h| self.world.player(*h)).map(|p| (p.id, p.position, p.alive)).collect();
            let npc_positions: BTreeMap<String, Vec3> = self.npc_by_id.iter().filter_map(|(id, n)| self.world.player(*id).filter(|p| p.alive).map(|p| (n.clone(), p.position))).collect();
            let total = (self.setup.crowd.as_ref().map(|c| c.spawns.len()).unwrap_or(0) + self.setup.npcs.len()) as u32;
            let map = self.current_map.clone();
            // Speaker gating for conversations (m_bNotWhileInCombat / Fearful / Suspicious).
            self.state.npc_suspicion = self
                .crowd
                .brains
                .iter()
                .filter_map(|b| {
                    let n = self.npc_by_id.get(&b.id)?;
                    let level = match b.state {
                        BrainState::Combat | BrainState::Flee => 2,
                        BrainState::React | BrainState::Investigate => 1,
                        _ => 0,
                    };
                    Some((n.clone(), level))
                })
                .collect();
            kismet_host::filter_events(self, &mut events); // [kismet] scripted choices
            let cmds = self.state.step(&StepInput { dt, map: &map, players: &players, npc_positions: &npc_positions, events: &events, total_npcs: total });
            for c in cmds {
                self.apply(c, cfg);
            }
            kismet_host::step(self, &out.events, dt, cfg); // [kismet]
            save::tick(self, dt); // [save] autosaves
            if self.state.outcome().is_some() && self.view.phase != MatchPhase::Ended {
                self.view.phase = MatchPhase::Ended;
            }
            self.view.time_left = if self.setup.def.settings.time_limit > 0.0 { (self.setup.def.settings.time_limit - self.state.view.stats.time).max(0.0) } else { 0.0 };
        }
        self.view.npc_poses = crate::npc_poses(&self.crowd.brains);
        // Level travel happens at the end of the step (the host swaps collision before the next).
        if let Some((m, tag)) = self.pending_travel.take() {
            self.travel(&m, &tag, cfg);
        }
        let mut mv = self.state.view.clone();
        mv.npc_ids = self.id_by_npc.iter().map(|(n, i)| (n.clone(), *i)).collect();
        // Travel doors of the active map are usable with their original prompts.
        for d in self.setup.doors.iter().filter(|d| d.map.eq_ignore_ascii_case(&self.current_map)) {
            mv.usables.push(((d.position + Vec3::Z * 60.0).to_array(), d.prompt.clone()));
        }
        // Climbable chains: their interactable text (`m_InteractText` "`GBA_Use` Grab") at every
        // bone below the anchor; the sim grabs on Use (`dishonored_sim::climb`).
        for c in world.climbables() {
            for b in c.bones.iter().filter(|b| b.z <= c.anchor.z + 0.5) {
                mv.usables.push((b.to_array(), "Grab".to_string()));
            }
        }
        mv.choices = self.choosing.iter().map(|(p, d)| (*p, self.setup.doors[*d].choices.clone())).collect();
        // A conversation choice joins the same list (one input path for both).
        if let Some(c) = &mv.choice {
            let who: Vec<PlayerId> = match c.player {
                Some(p) => vec![p],
                None => self.humans.clone(),
            };
            for p in who {
                if !mv.choices.iter().any(|x| x.0 == p) {
                    mv.choices.push((p, c.options.clone()));
                }
            }
        }
        kismet_host::view(self, &mut mv); // [kismet] usables, scripted choice, task target
        // Native doors and usables: their prompts and the doors' states.
        for u in self.setup.usables.iter().filter(|u| u.map.eq_ignore_ascii_case(&self.current_map)) {
            let st = self.props.get(&u.actor).copied().unwrap_or_default();
            let prompt = if u.door_axis.is_some() && st.locked { u.locked_prompt.clone() } else { u.prompt.clone() };
            mv.usables.push((u.center.to_array(), prompt));
            if u.door_axis.is_some() && st.door != u.initial {
                mv.doors.push((u.actor.clone(), st.door));
            }
        }
        self.view.mission = Some(mv);
        self.view.map = self.current_map.clone();
        // Presentation: NPCs within music range (4500 uu, `m_fEnemyRangeForChase[1]`) of a human.
        let humans: Vec<Vec3> = self.world.players.iter().filter(|p| self.humans.contains(&p.id) && p.alive).map(|p| p.position).collect();
        self.view.ai = self
            .crowd
            .brains
            .iter()
            .filter_map(|b| {
                let p = self.world.players.iter().find(|p| p.id == b.id && p.alive)?;
                humans.iter().any(|h| h.distance(p.position) < 5000.0).then_some(())?;
                let combat = b.state == BrainState::Combat && b.target.is_some_and(|t| self.humans.contains(&t));
                let hostile = b.kind == PawnKind::Guard || b.hostile.iter().any(|h| self.humans.contains(h));
                Some(crate::AiPresence { id: b.id, hostile, combat, suspicion: b.suspicion_level(), music_type: if b.kind == PawnKind::Guard { 4 } else { 0 } })
            })
            .collect();
        self.last_events = out.events.clone();
        // [kismet] AI noises made by the Kismet (`DisSeqAct_AINoise`) are heard next step.
        if let Some(k) = &mut self.kismet {
            self.last_events.append(&mut k.st.noises);
        }
        self.mover_worlds = placed;
        out
    }

    /// [ai] A dialog hook an NPC fires (a bark, `FUN_00af00f0`): queued for the conversation
    /// runner with the NPC's speaker and generic voice. Mission NPCs bark as their speaker
    /// actor, original NPCs as their spawner; an original NPC's voice is one of its pawn tweak's
    /// `m_GenericDialogData` (the original picks at random, 0x00C8D2C0; the port takes a
    /// per-pawn index so a pawn keeps its voice).
    pub fn request_bark(&mut self, npc: PlayerId, hook: &str, initiator: Option<PlayerId>) {
        let named = self.npc_by_id.get(&npc).and_then(|n| self.setup.def.npcs.iter().find(|d| &d.id == n).map(|d| (n.clone(), d.speaker_actor().unwrap_or(n).to_string())));
        let named = named.or_else(|| self.setup.original.iter().find(|o| o.spawn.id == npc).map(|o| (o.name.clone(), o.name.clone())));
        let voice = self.setup.original.iter().find(|o| o.spawn.id == npc).and_then(|o| {
            let list = self.setup.ai_levels.get(&o.map.to_ascii_lowercase())?.generic_trees.get(&o.tweak)?;
            (!list.is_empty()).then(|| list[npc as usize % list.len()].clone())
        });
        if let Some((name, actor)) = named {
            self.state.ai_hooks.push((name, actor, hook.to_string(), initiator, voice));
        }
    }

    /// Place the map's movers where their actors are now (identity for movers the Kismet has
    /// not moved) and pose its doors for their state (the Kismet's, else `m_InitialDoorState`),
    /// rebuilding only the ones that changed since the last step.
    fn place_movers(&mut self, world: &dyn WorldQuery) {
        let movers = world.movers();
        if self.mover_keys.len() != movers.len() || self.mover_keys.iter().zip(movers).any(|(k, m)| k.0 != m.actor) {
            self.mover_keys.clear();
            self.mover_worlds.clear();
        }
        for (i, m) in movers.iter().enumerate() {
            let delta = self.kismet.as_ref().and_then(|k| k.mover_delta(&m.actor, m.actor_to_world)).unwrap_or(glam::Mat4::IDENTITY);
            let state = if m.is_door() { self.kismet.as_ref().and_then(|k| k.door_state(&m.actor)).or_else(|| self.props.get(&m.actor).map(|s| s.door)).unwrap_or(m.initial) } else { 0 };
            match self.mover_keys.get(i) {
                Some(k) if k.1.abs_diff_eq(delta, 1e-4) && k.2 == state => {}
                Some(_) => {
                    self.mover_keys[i].1 = delta;
                    self.mover_keys[i].2 = state;
                    self.mover_worlds[i] = m.placed_in(state, delta);
                }
                None => {
                    self.mover_keys.push((m.actor.clone(), delta, state));
                    self.mover_worlds.push(m.placed_in(state, delta));
                }
            }
        }
    }

    /// What a player would interact with right now (nearest usable marker, talkable NPC or
    /// travel door within reach, in front of the view).
    pub fn interaction(&self, id: PlayerId) -> Option<Interaction> {
        let p = self.world.player(id).filter(|p| p.alive)?;
        let eye = p.position + Vec3::Z * 64.0;
        let fwd = dishonored_sim::movement::view_vector(p.yaw, p.pitch);
        let mut best: Option<(f32, Interaction)> = None;
        let mut consider = |pos: Vec3, reach: f32, it: Interaction| {
            let d = pos - eye;
            let dist = d.length();
            if dist > reach || dist < 1.0 {
                return;
            }
            let facing = d.normalize().dot(fwd);
            if facing < 0.6 {
                return;
            }
            let score = dist * (2.0 - facing);
            if best.as_ref().is_none_or(|b| score < b.0) {
                best = Some((score, it));
            }
        };
        for m in self.state.usable.keys() {
            if self.setup.marker_map.get(m).is_some_and(|mm| !mm.eq_ignore_ascii_case(&self.current_map)) {
                continue;
            }
            if let Some((pos, _)) = self.state.markers.get(m) {
                consider(*pos, 220.0, Interaction::Mission(MissionEvent::Used(m.clone())));
            }
        }
        for n in self.state.talk.keys() {
            if let Some(np) = self.id_by_npc.get(n).and_then(|i| self.world.player(*i)).filter(|x| x.alive) {
                consider(np.position + Vec3::Z * 40.0, 200.0, Interaction::Mission(MissionEvent::Talked(n.clone())));
            }
        }
        for (i, d) in self.setup.doors.iter().enumerate() {
            if d.map.eq_ignore_ascii_case(&self.current_map) {
                consider(d.position + Vec3::Z * 60.0, 220.0, Interaction::Door(i));
            }
        }
        for (pos, reach, it) in kismet_host::interactions(self) {
            consider(pos, reach, it); // [kismet]
        }
        for (i, u) in self.setup.usables.iter().enumerate() {
            if u.map.eq_ignore_ascii_case(&self.current_map) {
                consider(u.center, 220.0, Interaction::Prop(i));
            }
        }
        best.map(|b| b.1)
    }

    pub fn respawn_time_left(&self, id: PlayerId) -> Option<f32> {
        self.respawn_queue.iter().find(|(p, _)| *p == id).map(|(_, t)| *t)
    }
}

/// Pawn kind and presentation archetype for an original spawner. The pawn tweak decides the
/// real model; until tweak -> mesh resolution exists, guards use the elite guard and civilians
/// the party models (female tweaks the female aristocrat).
fn original_archetype(s: &dishonored_ir::spawners::Spawner) -> (PawnKind, u8) {
    let t = s.tweak.to_ascii_lowercase();
    match s.guard {
        true => (PawnKind::Guard, 1),
        _ if t.contains("_f_") || t.contains("female") || t.contains("lady") || t.contains("servant_f") => (PawnKind::Civilian, 3),
        _ => (PawnKind::Civilian, if s.position.x as i64 % 2 == 0 { 0 } else { 2 }),
    }
}
