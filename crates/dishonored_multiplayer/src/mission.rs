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
// [ai] Door- and Kismet-driven audio portal occlusion (audio_portals.rs).
#[path = "audio_portals.rs"]
pub mod audio_portals;
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
    /// The pawn tweak's NPC class (`AiLevel::npc_class`; `None`: brain not resolved).
    #[serde(default)]
    pub class: Option<dishonored_ir::ai::NpcClass>,
    pub tweak: String,
    pub props: dishonored_ir::spawners::SpawnerProps,
    /// Mesh parts (package, object path) of this pawn's look: body + its picked head.
    #[serde(default)]
    pub parts: Vec<(String, String)>,
    /// Wears a helmet (`m_PotentialNPCArmor` head armour, rolled per pawn): `PlayerState::helmet`.
    #[serde(default)]
    pub helmet: bool,
    /// The pawn tweak and the tweaks it inherits from (`Spawner::lineage`): the chaos deeds'
    /// victim filter.
    #[serde(default)]
    pub lineage: Vec<String>,
    /// The level package that places the spawner.
    #[serde(default)]
    pub package: String,
}

/// The victim tweak lineage of a mission NPC, which has no pawn tweak of its own: the base
/// tweak of its role archetype (civilians `Pwn_Civ_Base`, guards `Pwn_Guard_Base`, the tweaks the
/// chaos deeds name for those classes).
pub fn role_lineage(kind: PawnKind) -> Vec<String> {
    vec![if kind == PawnKind::Guard { "Pwn_Guard_Base.Pwn_Guard_Base" } else { "Pwn_Civ_Base.Pwn_Civ_Base" }.to_string()]
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
    /// Breakables, whale oil batteries and receptacles of each mission map (lowercase name).
    #[serde(default)]
    pub world_actors: BTreeMap<String, dishonored_ir::world_actors::WorldActors>,
    /// Door keyholes and tripwires of each mission map (lowercase name).
    #[serde(default)]
    pub interactables: BTreeMap<String, dishonored_ir::interactables::Interactables>,
    /// [streaming] Each mission map's (lowercase name) Kismet-streamed sublevels
    /// (`LevelStreamingKismet` packages): not in the world until the Kismet streams them in.
    #[serde(default)]
    pub streamed_levels: BTreeMap<String, Vec<String>>,
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
                // [ai] The pawn tweak's NPC class (its brain's Combat-slot behaviour): a pawn with a
                // combat behaviour (tallboys, hounds, weepers, overseers) is a fighter.
                let class = m.ai_level.npc_class(&s.tweak);
                let (kind, mut archetype) = original_archetype(s, class);
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
                original.push(OriginalNpc { map: m.name.clone(), name: s.name.clone(), spawn: NpcSpawn { id, kind, archetype, position: p, yaw: s.yaw }, class, tweak: s.tweak.clone(), props: s.props.clone(), parts, helmet: s.appearance.helmet(id), lineage: if s.lineage.is_empty() { vec![s.tweak.clone()] } else { s.lineage.clone() }, package: s.package.clone() });
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
        Ok(MissionSetup { def, markers, marker_map, start_map, starts, npcs, npc_map, crowd, crowd_map, maps: maps.iter().map(|m| m.name.clone()).collect(), doors, tagged_starts: tagged, conversations: None, flee, original, appearances, pickups: BTreeMap::new(), usables: Vec::new(), world_actors: BTreeMap::new(), interactables: BTreeMap::new(), streamed_levels: BTreeMap::new(), anim_profiles, kismet: None, ai_levels: maps.iter().map(|m| (m.name.to_ascii_lowercase(), m.ai_level.clone())).collect(), routes: maps
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
    /// The physics props (picked up, thrown or broken ones stay so).
    #[serde(default)]
    movables: Vec<dishonored_sim::state::MovableState>,
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
    /// [world] Index into the active map's `Interactables::launchers`: the launcher in view
    /// (disarm and harvest, `dishonored_sim::launchers`).
    Launcher(usize),
}

/// The touch filter's faction questions (`dishonored_sim::walls::WallSocial`) answered by the
/// crowd's social state: a pawn's faction along its fallback chain (0x00C6F360), and the player's
/// relationship to it.
struct WallSocial<'a> {
    soc: &'a dishonored_ai::Social,
    player: Option<PlayerId>,
}

impl dishonored_sim::walls::WallSocial for WallSocial<'_> {
    fn in_factions(&self, pawn: PlayerId, factions: &[String]) -> bool {
        let (Some(t), Some(&f)) = (self.soc.factions.as_ref(), self.soc.pawn_faction.get(&pawn)) else { return false };
        factions.iter().filter_map(|n| t.index(n)).any(|x| t.isa(f, x))
    }

    fn player_enemy(&self, pawn: PlayerId) -> bool {
        let Some(&mine) = self.player.and_then(|p| self.soc.pawn_faction.get(&p)) else { return false };
        self.soc.faction_relation(mine, pawn) == dishonored_ir::ai::Relationship::Enemy
    }
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
    /// The host interactables the last step offered the crosshair, by `HostTarget::key` (what a
    /// stored `TargetRef::Host` means). Rebuilt every step; not saved.
    #[serde(skip)]
    host_list: Vec<Interaction>,
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
    /// The game's chaos (`DisDarknessManager`): one score for the match, saved with it.
    #[serde(default)]
    pub darkness: dishonored_sim::darkness::Darkness,
    /// Corpses some NPC has discovered (0x00A98950 keeps the set: a corpse counts once).
    #[serde(default)]
    pub corpses_discovered: BTreeSet<PlayerId>,
    /// [ai] Generic dialog voice (`DisDialogVoiceData` path) of pawns the host gave one (mission
    /// NPCs placed from a spawner or tweak; original spawns take theirs from their tweak).
    #[serde(default)]
    pub bark_voices: BTreeMap<PlayerId, String>,
    /// [world] Breakables, whale oil batteries and receptacles of each visited map (lowercase
    /// name), saved with the match.
    #[serde(default)]
    pub world_actors: BTreeMap<String, dishonored_sim::world_actors::WorldActorsState>,
    /// [world] What they reported this step (the Kismet host fires the events).
    #[serde(skip)]
    pub world_events: Vec<dishonored_sim::world_actors::WorldEvent>,
    /// [world] Keyhole peeks, hold times and tripped tripwires of each visited map (lowercase
    /// name), saved with the match.
    #[serde(default)]
    pub interactables: BTreeMap<String, dishonored_sim::interactables::InteractablesState>,
    /// [world] What they reported this step (the Kismet host fires the events).
    #[serde(skip)]
    pub interact_events: Vec<dishonored_sim::interactables::InteractEvent>,
    /// [world] Projectile launchers of each visited map (lowercase name): armed / harvested and
    /// the shots in flight (research/dishonored/systems/interactables.md section 2), saved with
    /// the match.
    #[serde(default)]
    pub launchers: BTreeMap<String, dishonored_sim::launchers::LaunchersState>,
    /// [watchtowers] Each map's watch towers (`dishonored_ai::watchtower`), by map.
    #[serde(default)]
    pub towers: BTreeMap<String, dishonored_ai::watchtower::WatchTowersState>,
    /// [watchtowers] What the towers reported this step, for the Kismet (`DisSeqEvent_WatchTower`).
    #[serde(skip)]
    pub tower_events: Vec<dishonored_ai::watchtower::TowerEvent>,
    /// [world] Events of shots the Kismet's update fired (`SimEvent::Fired`), delivered with the
    /// step's own after the simulation (`step_launchers`). In a mission running the original
    /// Kismet it is empty between steps, like the other event lists here (the Kismet's update
    /// comes first in the step and its feed takes the reports at the end), so a save has nothing
    /// to keep of it.
    #[serde(skip)]
    pub launcher_events: Vec<SimEvent>,
    /// Mover collision placed where the movers are: (actor, delta) and, in the same order, the
    /// placed collision (rebuilt when a mover moves).
    #[serde(skip)]
    mover_keys: Vec<(String, glam::Mat4, u8)>,
    #[serde(skip)]
    mover_worlds: Vec<dishonored_world::CollisionWorld>,
    /// [streaming] The Kismet-streamed sublevels visible now (lowercase package names; the
    /// streaming ops and map changes of the original Kismet set them).
    #[serde(default)]
    pub levels_visible: BTreeSet<String>,
    /// [ai] What the current map's audio portals were last set from (`audio_portals.rs`).
    #[serde(default)]
    pub portal_sync: audio_portals::PortalSync,
    /// [dialog] The blocking conversations running: instance -> the NPC speakers held in
    /// `DisBehaviorBlockingDialog`.
    #[serde(default)]
    pub dialog_blocks: BTreeMap<u32, Vec<PlayerId>>,
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
            host_list: Vec::new(),
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
            darkness: Default::default(),
            corpses_discovered: BTreeSet::new(),
            bark_voices: BTreeMap::new(),
            world_actors: BTreeMap::new(),
            world_events: Vec::new(),
            interactables: BTreeMap::new(),
            interact_events: Vec::new(),
            launchers: BTreeMap::new(),
            towers: BTreeMap::new(),
            tower_events: Vec::new(),
            launcher_events: Vec::new(),
            mover_keys: Vec::new(),
            mover_worlds: Vec::new(),
            levels_visible: BTreeSet::new(),
            portal_sync: Default::default(),
            dialog_blocks: BTreeMap::new(),
        };
        // The score carried in from the previous missions (the manager's score persists).
        m.darkness.add(m.setup.def.settings.darkness);
        m.sync_darkness(cfg);
        m.props = m.setup.usables.iter().map(|u| (u.actor.clone(), PropState { door: u.initial, locked: u.locked, stage: 0 })).collect();
        // [kismet]
        m.kismet = m.setup.kismet.as_ref().map(|k| kismet_host::KismetRun::new(k, &m.setup));
        m.populate_fresh(cfg);
        for c in m.state.initial_commands() {
            m.apply(c, cfg);
        }
        m
    }

    /// Hand the chaos score and level to the readers: the mission interpreter (conversations'
    /// `DisConv_CompareDarknessLevel`, the HUD) and the Kismet (`DisSeqVar_Darkness*`).
    pub fn sync_darkness(&mut self, cfg: &SimConfig) {
        let level = self.darkness.level(&cfg.gameplay.items.darkness);
        self.state.darkness_score = self.darkness.score;
        self.state.darkness_level = level;
        if let Some(k) = self.kismet.as_mut() {
            k.k.darkness = (self.darkness.score, level);
        }
    }

    /// A player statistic was raised (the stat increment 0x00AB9560 ends with the deed check
    /// 0x00C4D2C0): the chaos deeds on that stat count it.
    pub fn player_stat(&mut self, stat: &str, victim: Option<&[String]>, amount: i32, cfg: &SimConfig) {
        // The mission's tracked stats (0x00AA97E0, after the deeds): the stats screen reads them.
        self.state.view.add_player_stat(stat, amount as f32);
        if self.darkness.notify_deed(&cfg.gameplay.items.darkness, stat, victim, amount).is_some() {
            self.sync_darkness(cfg);
        }
    }

    /// The pawn tweak lineage of an NPC (its spawner's tweak, else its role's base tweak).
    pub fn victim_lineage(&self, id: PlayerId, kind: PawnKind) -> Vec<String> {
        match self.setup.original.iter().find(|o| o.spawn.id == id) {
            Some(o) => o.lineage.clone(),
            None => role_lineage(kind),
        }
    }

    /// [streaming] Whether a level package is a Kismet-streamed sublevel of some mission map.
    pub fn is_streamed_level(&self, package: &str) -> bool {
        self.setup.streamed_levels.values().flatten().any(|p| p.eq_ignore_ascii_case(package))
    }

    /// [streaming] Whether a level package's actors are in the world: always-loaded levels
    /// always, Kismet-streamed ones while visible.
    pub fn level_present(&self, package: &str) -> bool {
        package.is_empty() || !self.is_streamed_level(package) || self.levels_visible.contains(&package.to_ascii_lowercase())
    }

    /// [streaming] The active map's Kismet-streamed sublevels that are visible / hidden now.
    pub fn visible_levels(&self) -> Vec<String> {
        self.setup.streamed_levels.get(&self.current_map.to_ascii_lowercase()).map(|l| l.iter().filter(|p| self.levels_visible.contains(&p.to_ascii_lowercase())).cloned().collect()).unwrap_or_default()
    }
    pub fn hidden_levels(&self) -> Vec<String> {
        self.setup.streamed_levels.get(&self.current_map.to_ascii_lowercase()).map(|l| l.iter().filter(|p| !self.levels_visible.contains(&p.to_ascii_lowercase())).cloned().collect()).unwrap_or_default()
    }

    /// [streaming] Make a Kismet-streamed sublevel visible (its actors join the world: the
    /// begin-play spawners spawn, the placed pickups appear) or hidden (its actors leave the
    /// world: its spawners' pawns and its pickups are removed; UE3 removes a hidden level's
    /// actors from the world). Collision follows through [`dishonored_world::WorldQuery::set_visible_levels`].
    pub fn set_level_visible(&mut self, package: &str, visible: bool, cfg: &SimConfig) {
        let key = package.to_ascii_lowercase();
        if visible == self.levels_visible.contains(&key) {
            return;
        }
        let here = self.setup.streamed_levels.get(&self.current_map.to_ascii_lowercase()).is_some_and(|l| l.iter().any(|p| p.eq_ignore_ascii_case(package)));
        if visible {
            self.levels_visible.insert(key);
            if !here {
                return;
            }
            for i in 0..self.setup.original.len() {
                let o = &self.setup.original[i];
                if o.package.eq_ignore_ascii_case(package) && o.map.eq_ignore_ascii_case(&self.current_map) && o.props.on_begin_play && o.props.travel_tags.is_empty() && o.props.at_suspicion.is_none() {
                    self.spawn_original(i, false, cfg);
                }
            }
            if let Some(placed) = self.setup.pickups.get(&self.current_map.to_ascii_lowercase()) {
                let scripted = kismet_host::scripted_pickups(self);
                let placed: Vec<_> = placed.iter().filter(|p| p.package.eq_ignore_ascii_case(package) && !scripted.iter().any(|(pk, n)| n.eq_ignore_ascii_case(&p.name) && pk.eq_ignore_ascii_case(&p.package))).cloned().collect();
                let seed = self.setup.def.title.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
                dishonored_sim::inventory::spawn_pickups(&mut self.world, &placed, seed, &|_: &str| None);
            }
            self.spawn_movables(Some(package));
        } else {
            self.levels_visible.remove(&key);
            if !here {
                return;
            }
            let ids: Vec<PlayerId> = self.setup.original.iter().filter(|o| o.package.eq_ignore_ascii_case(package)).map(|o| o.spawn.id).collect();
            self.world.players.retain(|p| !ids.contains(&p.id));
            self.crowd.brains.retain(|b| !ids.contains(&b.id));
            for id in ids {
                self.original_spawned.remove(&id);
            }
            self.world.pickups.retain(|p| !p.package.eq_ignore_ascii_case(package));
            dishonored_sim::movables::remove_package(&mut self.world, package);
        }
    }

    /// The map's placed movable props of the levels that are present.
    fn spawn_movables(&mut self, only_package: Option<&str>) {
        let Some(defs) = self.setup.world_actors.get(&self.current_map.to_ascii_lowercase()) else { return };
        let defs: Vec<_> = defs.movables.iter().filter(|m| only_package.is_none_or(|p| m.package.eq_ignore_ascii_case(p))).cloned().collect();
        for m in defs {
            if self.level_present(&m.package) {
                dishonored_sim::movables::spawn(&mut self.world, &m);
            }
        }
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
            if o.map.eq_ignore_ascii_case(&self.current_map) && o.props.on_begin_play && o.props.travel_tags.is_empty() && o.props.at_suspicion.is_none() && self.level_present(&o.package) {
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
        // The map's physics props (`DishonoredMovable`).
        self.spawn_movables(None);
        // The map's placed pickups; a spawner's `m_pStealablePickup` rides on its pawn.
        if let Some(mut placed) = self.setup.pickups.get(&self.current_map.to_ascii_lowercase()).cloned() {
            // [kismet] Pickups with a `DisSeqEvent_PickupPickedUp` stay Kismet usables.
            let scripted = kismet_host::scripted_pickups(self);
            placed.retain(|p| !scripted.iter().any(|(pk, n)| n.eq_ignore_ascii_case(&p.name) && pk.eq_ignore_ascii_case(&p.package)));
            // [streaming] Not the pickups of hidden Kismet-streamed sublevels.
            placed.retain(|p| self.level_present(&p.package));
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
        // [ai] The pawn tweak's own `Attribute_HealthMax` (civilians: aristocrats 11, ...).
        let tweak_health = self.crowd.tweak_health_max.get(&o.tweak).copied();
        if let Some(p) = self.world.player_mut(s.id) {
            p.helmet = o.helmet;
            if let Some(h) = tweak_health.filter(|_| s.kind == PawnKind::Civilian) {
                p.vitals.health = h;
            }
            if let Some(c) = o.class {
                // [ai] The special classes' `Attribute_HealthMax` (tallboys 150, weepers 30,
                // music-box Overseers 40) and the weepers' plague (`m_bHasPlague`).
                if let Some(h) = cfg.gameplay.ai.special.health_max(c) {
                    p.vitals.health = h;
                }
                p.plague = c == dishonored_ir::ai::NpcClass::Weeper;
            }
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
        if let Some(c) = o.class {
            // [ai] The Combat slot's behaviour of the pawn's brain (`dishonored_ai::special`).
            b.class = c;
        }
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
        // [distraction] The pawn tweak's lineage (the distractors' tweak filters) and its
        // `m_fDistractionCooldownTime`.
        b.lineage = o.lineage.clone();
        b.distraction_cooldown_time = self.crowd.tweak_distraction_cooldown.get(&o.tweak).copied().unwrap_or(0.0);
        // [tether] The spawner's tether volumes (`m_TetherVolumes`, actors of its package).
        b.tether = o.props.tether_volumes.iter().filter_map(|n| self.crowd.tether_volumes.iter().position(|v| v.name.eq_ignore_ascii_case(n) && v.package.eq_ignore_ascii_case(&o.package)).or_else(|| self.crowd.tether_volumes.iter().position(|v| v.name.eq_ignore_ascii_case(n)))).map(|i| i as u16).collect();
        // [rats] The brain's `DisBehaviorCombatRatSwarm` and the pawn's call for help.
        b.rat_combat = self.crowd.tweak_rat_combat.get(&o.tweak).cloned();
        b.rat_call_radius = self.crowd.tweak_rat_call.get(&o.tweak).copied().unwrap_or(0.0);
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

    /// An NPC plays the ambient idle `seq` (`Empty_Idle_03`), already `age` seconds in. A
    /// negative `age` counts down to its start. The pick is pinned to it.
    pub fn start_ambient_idle(&mut self, id: PlayerId, seq: &str, age: f32) {
        let ai = self.crowd.ai.clone();
        if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == id) {
            b.start_ambient(&ai, seq, age);
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
            // A pawn without a brain reads as `EAIAwareness` 0, Unaware (0x00B5E250: no brain
            // -> 0), which the assassination table allows (0x00C09060).
            if let Some(p) = self.world.player_mut(s.id) {
                p.ai_awareness = dishonored_sim::state::awareness::UNAWARE;
            }
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

    /// [dialog] The NPC a conversation speaker name stands for: a mission NPC, else an original
    /// spawner of the current map that has spawned.
    fn speaker_id(&self, name: &str) -> Option<PlayerId> {
        self.id_by_npc.get(name).copied().or_else(|| self.setup.original.iter().find(|o| o.name.eq_ignore_ascii_case(name) && o.map.eq_ignore_ascii_case(&self.current_map) && self.original_spawned.contains(&o.spawn.id)).map(|o| o.spawn.id))
    }

    /// [dialog] `EAIStimID_BlockingDialogRequest` / `End` (the pawn's dialog component, 0x00b7fe50
    /// / 0x00b7ff80): the bound NPC speakers of each blocking conversation (`m_bBlocking`) stand in
    /// `DisBehaviorBlockingDialog` while it runs, facing who talks (0x00adac60: the speaker of the
    /// current line, else the initiator). The request's priority flag is the conversation's low
    /// priority (LIKELY: the flag the dialog component passes was not traced to its source).
    fn blocking_dialogs(&mut self) {
        let Some(db) = self.setup.conversations.as_ref() else { return };
        type Live = (u32, bool, Vec<PlayerId>, Option<PlayerId>, Option<(Option<String>, bool)>);
        let mut live: Vec<Live> = Vec::new();
        if let Some(r) = self.state.conv.as_ref() {
            for inst in r.active() {
                let Some(c) = inst.conversation.and_then(|c| db.conversations.get(c as usize)) else { continue };
                if !c.blocking {
                    continue;
                }
                let npcs: Vec<PlayerId> = inst.bindings.iter().flatten().filter_map(|n| self.speaker_id(n)).collect();
                let line = inst.line.as_ref().map(|l| (l.speaker.npc.clone(), l.speaker.player));
                live.push((inst.id, c.priority == dishonored_conversation::model::Priority::Low, npcs, inst.initiator, line));
            }
        }
        let ended: Vec<u32> = self.dialog_blocks.keys().filter(|id| !live.iter().any(|l| l.0 == **id)).copied().collect();
        for id in ended {
            for p in self.dialog_blocks.remove(&id).unwrap_or_default() {
                if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == p) {
                    b.blocking_dialog_end(id);
                }
            }
        }
        let mut looks: Vec<(PlayerId, Vec3)> = Vec::new();
        for (id, low, npcs, initiator, line) in &live {
            if !self.dialog_blocks.contains_key(id) {
                for p in npcs {
                    if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == *p) {
                        b.blocking_dialog_start(*id, *low);
                    }
                }
                self.dialog_blocks.insert(*id, npcs.clone());
            }
            let talker = match line {
                Some((Some(n), _)) => self.speaker_id(n),
                Some((None, true)) => *initiator,
                _ => None,
            };
            for p in npcs {
                let target = talker.filter(|t| t != p).or(*initiator).or_else(|| npcs.iter().copied().find(|q| q != p));
                if let Some(pos) = target.and_then(|t| self.world.player(t)).map(|x| x.position) {
                    looks.push((*p, pos));
                }
            }
        }
        for (p, pos) in looks {
            if let Some(b) = self.crowd.brains.iter_mut().find(|b| b.id == p) {
                b.dialog_look = Some(pos);
            }
        }
    }

    /// [ai] The plot settings' game-over rules of an original NPC (its spawner's pawn tweak).
    fn plot_game_over(&self, id: PlayerId) -> Option<dishonored_ir::ai::PlotGameOver> {
        let o = self.setup.original.iter().find(|o| o.spawn.id == id)?;
        self.setup.ai_levels.get(&o.map.to_ascii_lowercase())?.tweak_plot.get(&o.tweak).cloned()
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
            // A prop in a player's hands goes with him (it is dropped where the level is left).
            for h in &humans {
                if let Some(p) = self.world.player_mut(*h) {
                    p.holding = None;
                }
            }
            for m in self.world.movables.iter_mut().filter(|m| m.phase == dishonored_sim::state::MovablePhase::Held) {
                m.phase = dishonored_sim::state::MovablePhase::Rest;
                m.holder = None;
            }
            self.saved.insert(self.current_map.to_ascii_lowercase(), LevelState { npcs, pickups: std::mem::take(&mut self.world.pickups), movables: std::mem::take(&mut self.world.movables), brains: crowd.brains.clone(), layout: crowd.layout.clone(), spawns: crowd.spawns.clone(), level: Crowd { brains: Vec::new(), ..crowd } });
            self.world.players.retain(|p| humans.contains(&p.id));
            self.world.projectiles.clear();
            // [streaming] The persistent map change drops the old map's streamed sublevels.
            if let Some(old) = self.setup.streamed_levels.get(&self.current_map.to_ascii_lowercase()) {
                for p in old {
                    self.levels_visible.remove(&p.to_ascii_lowercase());
                }
            }
            self.current_map = self.setup.maps.iter().find(|m| m.eq_ignore_ascii_case(dest_map)).cloned().unwrap_or_else(|| dest_map.to_string());
            match self.saved.remove(&self.current_map.to_ascii_lowercase()) {
                Some(ls) => {
                    self.world.players.extend(ls.npcs);
                    self.world.pickups = ls.pickups;
                    self.world.movables = ls.movables;
                    let routes = self.setup.routes.get(&self.current_map.to_ascii_lowercase()).cloned().unwrap_or_default();
                    self.crowd = Crowd { brains: ls.brains, layout: ls.layout, spawns: ls.spawns, ai: self.crowd.ai.clone(), routes, factions: ls.level.factions, player_faction: ls.level.player_faction, tweak_factions: ls.level.tweak_factions, zones: ls.level.zones, overrides: ls.level.overrides, alarms: ls.level.alarms, lights: ls.level.lights, distractors: ls.level.distractors, tweak_distraction_cooldown: ls.level.tweak_distraction_cooldown, tweak_health_max: ls.level.tweak_health_max, ..Default::default() };
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

    /// The interactables this layer owns, offered to the sim's crosshair rule
    /// (`dishonored_sim::crosshair`) beside the sim's own: a mission's usable markers, talkable
    /// NPCs, travel doors, Kismet usables, launchers and native props. Each is the actor's
    /// collision as far as it is known (a launcher's bounds; a point for the rest, whose
    /// collision the port has not extracted: U-981) and the `Interaction` its Use runs.
    fn host_targets(&self, cfg: &SimConfig) -> Vec<(dishonored_sim::crosshair::HostTarget, Interaction)> {
        use dishonored_sim::crosshair::{CrosshairStatus as St, HostTarget};
        let reach = cfg.gameplay.items.inventory.interact_distance;
        let mut out: Vec<(HostTarget, Interaction)> = Vec::new();
        let mut push = |origin: Vec3, bounds: Option<(Vec3, Vec3)>, max_distance: f32, skeletal: bool, status: St, pawn: Option<PlayerId>, it: Interaction| {
            let (lo, hi) = bounds.unwrap_or((origin, origin));
            out.push((HostTarget { key: out.len() as u32, origin, lo, hi, max_distance, skeletal, usable: true, status, pawn, actor: None, shift: Vec3::ZERO, oriented: None }, it));
        };
        for m in self.state.usable.keys() {
            if self.setup.marker_map.get(m).is_some_and(|mm| !mm.eq_ignore_ascii_case(&self.current_map)) {
                continue;
            }
            if let Some((pos, _)) = self.state.markers.get(m) {
                push(*pos, None, reach, true, St::Usable, None, Interaction::Mission(MissionEvent::Used(m.clone())));
            }
        }
        for n in self.state.talk.keys() {
            if let Some(id) = self.id_by_npc.get(n).copied() {
                if let Some(np) = self.world.player(id).filter(|x| x.alive) {
                    push(np.position, None, reach, true, St::Usable, Some(id), Interaction::Mission(MissionEvent::Talked(n.clone())));
                }
            }
        }
        for (i, d) in self.setup.doors.iter().enumerate() {
            if d.map.eq_ignore_ascii_case(&self.current_map) {
                let status = if d.dest_map.is_empty() { St::Usable } else { St::DoorLevelTransition };
                push(d.position + Vec3::Z * 60.0, None, reach, true, status, None, Interaction::Door(i));
            }
        }
        // [kismet]
        for (i, (u, pos, _)) in kismet_host::usables(self).into_iter().enumerate() {
            let (status, pawn, skeletal) = match &u {
                kismet_host::KUse::Pickup(_) => (St::Pickup, None, false),
                kismet_host::KUse::Talk { pawn, .. } => (St::Usable, Some(*pawn), true),
                _ => (St::Usable, None, true),
            };
            push(pos, None, reach, skeletal, status, pawn, Interaction::Kismet(i));
        }
        // [world] A launcher: its tripod's collision bounds, its interactable's `m_fMaxDistance`.
        if let Some(defs) = self.setup.interactables.get(&self.current_map.to_ascii_lowercase()) {
            let hidden = self.hidden_levels();
            for (i, l) in defs.launchers.iter().enumerate() {
                if hidden.iter().any(|h| h.eq_ignore_ascii_case(&l.package)) {
                    continue;
                }
                push(l.position, Some((l.bounds_min, l.bounds_max)), l.max_distance, false, St::Usable, None, Interaction::Launcher(i));
            }
        }
        for (i, u) in self.setup.usables.iter().enumerate() {
            if u.map.eq_ignore_ascii_case(&self.current_map) {
                let locked = self.props.get(&u.actor).map_or(u.locked, |s| s.locked);
                let status = if u.door_axis.is_some() && locked { St::DoorLockedUnbreakable } else { St::Usable };
                push(u.center, None, reach, true, status, None, Interaction::Prop(i));
            }
        }
        // The placed actors the world holds the collision of: a Kismet usable's, a native
        // usable's (`<package>:<actor>`).
        let kismet = kismet_host::usables(self);
        for (h, it) in out.iter_mut() {
            if let Interaction::Kismet(i) = it {
                if let Some(k) = kismet.get(*i) {
                    let (bounds, shift, oriented) = kismet_host::usable_box(self, &k.0);
                    if let Some((lo, hi)) = bounds {
                        h.lo = lo;
                        h.hi = hi;
                    }
                    h.shift = shift;
                    h.oriented = oriented;
                }
            }
            h.actor = match it {
                Interaction::Kismet(i) => kismet.get(*i).and_then(|k| kismet_host::usable_actor(self, &k.0)),
                Interaction::Prop(i) => self.setup.usables.get(*i).and_then(|u| u.actor.split_once(':')).map(|(p, a)| (p.to_string(), a.to_string())),
                // A travel door: its actor by name (any package of the map).
                Interaction::Door(i) => self.setup.doors.get(*i).map(|d| (String::new(), d.door.clone())).filter(|(_, a)| !a.is_empty()),
                _ => None,
            };
        }
        out
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
        // The mission runs once someone is in the game.
        if !self.humans.is_empty() {
            self.started = true;
        }
        // [kismet] The Kismet first: the original's world tick runs every game sequence's
        // `UpdateOp` before its actors (0x006552C0 slot 92, then the actor loop 0x00651A80;
        // systems/tick_order.md section 1), so the events the last step's gameplay raised run
        // now, and what they change is in the world before the simulation and the AI read it.
        if self.started {
            kismet_host::update(self, dt, cfg);
            // The update's AI noises (`DisSeqAct_AINoise`) are heard by this step's AI.
            if let Some(k) = &mut self.kismet {
                self.last_events.append(&mut k.st.noises);
            }
        }
        // [streaming] The visible Kismet-streamed sublevels collide; the hidden ones do not.
        world.set_visible_levels(&self.visible_levels());
        self.view.hidden_levels = self.hidden_levels();
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
        kismet_host::journal_inputs(self, inputs); // [kismet] DisSeqEvent_JournalViewed
        kismet_host::cinematic_inputs(self, &mut all); // [kismet] SeqAct_ToggleCinematicMode
        kismet_host::track_views(self, &mut all, dt); // [kismet] DisSeqAct_PlayerTrackTarget
        self.lock_peeking_inputs(&mut all); // [world] a pawn at a keyhole
        // Movers (`InterpActor`s) collide where the Kismet has moved them.
        self.place_movers(world);
        let mut placed = std::mem::take(&mut self.mover_worlds);
        // [world] Walls of light that collide (powered, time running) answer the projectile and
        // melee checks, and the sim's projectiles and Windblast see them (`dishonored_sim::walls`).
        let wall_key = self.current_map.to_ascii_lowercase();
        let blockers = match (self.setup.world_actors.get(&wall_key), self.world_actors.get(&wall_key)) {
            (Some(d), Some(st)) if !d.walls_of_light.is_empty() => dishonored_sim::walls::blockers(d, st),
            _ => Vec::new(),
        };
        let wall_layer = !blockers.is_empty();
        if wall_layer {
            placed.push(dishonored_sim::walls::layer(&blockers));
        }
        self.world.wall_blockers = blockers;
        let layered = dishonored_world::Layered { base: world, layers: &placed };
        let world: &dyn WorldQuery = if placed.is_empty() { world } else { &layered };
        // [ai] Doors and the Kismet set the audio portals' occlusion for hearing.
        audio_portals::update(self);
        // [world] Broken nav blockers open their path objects (0x00A305A0).
        let nav_blocks = match (self.setup.world_actors.get(&wall_key), self.world_actors.get(&wall_key)) {
            (Some(d), Some(st)) => st.nav_blocks(d),
            _ => Vec::new(),
        };
        if let Some(k) = self.kismet.as_mut() {
            k.set_nav_blocks(nav_blocks);
        }
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
        // The brains' requests to their pawns (item contexts, GenericAction chains, locomotion
        // speeds: `dishonored_sim::pawn_states`).
        self.world.requests = self.crowd.requests();
        self.crowd.write_awareness(&mut self.world);
        // The Use of this tick acts on what the last tick's camera update chose (the stored
        // crosshair), so its host target is read from the list that update saw; this step's
        // update then chooses among the list built now.
        let focus_before: Vec<(PlayerId, Option<dishonored_sim::crosshair::Crosshair>)> = self.world.players.iter().map(|p| (p.id, p.crosshair)).collect();
        let used_list = std::mem::take(&mut self.host_list);
        let host = self.host_targets(cfg);
        self.world.host_targets = host.iter().map(|h| h.0.clone()).collect();
        self.host_list = host.into_iter().map(|h| h.1).collect();
        let mut out = step_world(&mut self.world, &all, world, cfg);
        // [interp] The pawns the matinees placed in this step's Kismet update are
        // `PHYS_Interpolating`: their physics does not move them.
        kismet_host::cutscene::hold(self, cfg);
        // The HUD's focus: the target this step's camera update chose, from this list.
        self.view.focus = self.focus_views(world, cfg);
        self.world.host_targets.clear();
        // [world] The launchers' shots fly, strike and burst (before the tripwires so that a burst
        // that damages a wire trips it in the same step).
        self.step_towers(world, cfg, &mut out.events);
        self.step_launchers(world, cfg, &mut out.events);
        self.world.wall_blockers.clear();
        // [world] Tripwires touched or damaged, keyhole holds and peeks.
        self.step_interactables(&all, &out.events, cfg, dt);
        // [world] Breakables, batteries and receptacles of the active map: explosions damage the
        // breakables, the receptacles plug their placed batteries, steps break.
        let key = self.current_map.to_ascii_lowercase();
        if let Some(defs) = self.setup.world_actors.get(&key) {
            let soc = (!defs.walls_of_light.is_empty()).then(|| self.crowd.social(&self.world));
            let st = self.world_actors.entry(key).or_insert_with(|| dishonored_sim::world_actors::WorldActorsState::new(defs));
            let mut evs = st.step(defs, &out.events, &cfg.gameplay, dt);
            // [world] Walls of light: the projectiles and Windblasts that reached one this step
            // (their damage and vslot 223), then power, shocks and burns (`dishonored_sim::walls`),
            // at the world's rate of time.
            if !defs.walls_of_light.is_empty() {
                let hits: Vec<dishonored_sim::events::SimEvent> = out.events.iter().filter(|e| matches!(e, dishonored_sim::events::SimEvent::WallOfLightHit { .. } | dishonored_sim::events::SimEvent::WallOfLightBlast { .. })).copied().collect();
                if !hits.is_empty() {
                    evs.extend(dishonored_sim::walls::apply_hits(defs, st, &mut self.world, &hits, &cfg.gameplay, &mut out.events));
                }
            }
            if let Some(soc) = soc {
                let player = self.world.players.iter().find(|p| p.kind == dishonored_sim::state::PawnKind::Human).map(|p| p.id);
                let ws = WallSocial { soc: &soc, player };
                let wdt = dishonored_sim::movement::DT * dishonored_sim::powers::world_time_scale(&self.world, &cfg.gameplay, false);
                evs.extend(dishonored_sim::walls::step(defs, st, &mut self.world, &ws, player, &cfg.gameplay, wdt, &mut out.events));
            }
            self.view.walls = defs.walls_of_light.iter().zip(&st.walls).map(|(d, w)| crate::WallView { name: d.name.clone(), position: d.position.to_array(), powered: w.powered, effect: (w.activation > 0.0).then(|| w.effect_point.to_array()) }).collect();
            // A movable whose breakable reached its last step is gone (whoever holds it lets go).
            for e in &evs {
                if let dishonored_sim::world_actors::WorldEvent::Broken { breakable, output: 0 } = e {
                    dishonored_sim::movables::on_broken(&mut self.world, *breakable as u32);
                }
            }
            self.world_events.extend(evs);
        }
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
            // At a keyhole Use is the exit (Dis_ExitKeyhole), not a use of what is nearby.
            if self.is_peeking(*id) {
                if pressed {
                    self.use_held.insert(*id);
                } else {
                    self.use_held.remove(id);
                }
                continue;
            }
            // A Use that took a prop in the hands or let go of one did nothing else (the Use handler
            // 0x00AD5BA0 returns after the grab / drop).
            let prop_use = out.events.iter().any(|e| matches!(e, SimEvent::MovablePickedUp { id: p, .. } | SimEvent::MovableDropped { id: p, .. } if p == id));
            if pressed && self.use_held.insert(*id) {
                if prop_use {
                    continue;
                }
                let focused = focus_before.iter().find(|f| f.0 == *id).and_then(|f| f.1).filter(|c| c.usable);
                let it = match focused.map(|c| c.target) {
                    Some(dishonored_sim::crosshair::TargetRef::Host(k)) => used_list.get(k as usize).cloned(),
                    _ => None,
                };
                match it {
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
                    Some(Interaction::Launcher(i)) => self.interact_launcher(*id, i, cfg),
                    None => {}
                }
            } else if !pressed {
                self.use_held.remove(id);
            }
        }
        let mut found_stats: Vec<(&'static str, i32)> = Vec::new();
        for e in &out.events {
            // [ai] Possessing a plot NPC ends the game when the possession ends
            // (`m_GameOverWhenPossessedByPlayer`, 0x00B7D2F0).
            if let SimEvent::PossessEnd { host, .. } = e {
                if let Some((delay, msg)) = self.plot_game_over(*host).and_then(|g| g.when_possessed) {
                    self.state.schedule_game_over(if msg.is_empty() { "Game over".into() } else { msg }, delay);
                }
            }
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
                    // [ai] A plot NPC's death ends the game (`m_GameOverWhenDead`, 0x00B7BF50).
                    if let Some((delay, msg)) = self.plot_game_over(*victim).and_then(|g| g.when_dead) {
                        self.state.schedule_game_over(if msg.is_empty() { "Game over".into() } else { msg }, delay);
                    }
                    // The kill report (0x00B73990): a kill credited to the player raises
                    // `NumKills` with the victim's tweak, which the chaos deeds filter by class
                    // (the exact player-side test is U-W2; a knock-out reports only the sleep
                    // stats, 0x00B738F0).
                    if by_player && *killer != Some(*victim) && k != PawnKind::Rat {
                        let lineage = self.victim_lineage(*victim, k);
                        self.player_stat(dishonored_sim::darkness::stat::NUM_KILLS, Some(&lineage), 1, cfg);
                        // Then `HostileKill` / `CivilianKill` by `Twk_PlayerStats`' NPC types.
                        if let Some(s) = dishonored_sim::player_stats::kill_stat(&lineage) {
                            self.player_stat(s, Some(&lineage), 1, cfg);
                        }
                    }
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
                        // Found pickups (0x00C4BD60: coins / runes by the quantity given; 0x00ABBA90:
                        // a bone charm).
                        let found = match &pk.kind {
                            dishonored_ir::items::PickupKind::Abstract { item, quantity, .. } => dishonored_sim::player_stats::abstract_pickup_stat(item).map(|s| (s, *quantity)),
                            dishonored_ir::items::PickupKind::BoneCharm(_) => Some((dishonored_sim::player_stats::BONE_CHARM_FOUND, 1)),
                            dishonored_ir::items::PickupKind::CrackedCharm(_) => Some((dishonored_sim::player_stats::CRACKED_CHARM_FOUND, 1)),
                            _ => None,
                        };
                        events.push(MissionEvent::PickedUp { player: *id, pickup: pk.name.clone(), item });
                        if let Some((s, n)) = found.filter(|_| self.humans.contains(id)) {
                            found_stats.push((s, n));
                        }
                    }
                    for (s, n) in found_stats.drain(..) {
                        self.player_stat(s, None, n, cfg);
                    }
                }
                SimEvent::KnockedOut { victim, by, .. } if !self.humans.contains(victim) => {
                    let by_player = by.is_some_and(|k| self.humans.contains(&k));
                    if by_player {
                        // `HostilePutToSleep` / `CivilianPutToSleep` (0x00B738F0).
                        let vk = self.world.player(*victim).map(|p| p.kind).unwrap_or(PawnKind::Civilian);
                        let lineage = self.victim_lineage(*victim, vk);
                        if let Some(s) = dishonored_sim::player_stats::sleep_stat(&lineage) {
                            self.player_stat(s, Some(&lineage), 1, cfg);
                        }
                    }
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
                dishonored_ai::AiEvent::AlarmRang { .. } => {
                    events.push(MissionEvent::AlarmRaised);
                    // The bell's trigger (`DisAlarmBell` slot 244, 0x00A3D080) raises
                    // `NumAlarmsTriggered`.
                    self.player_stat(dishonored_sim::darkness::stat::NUM_ALARMS_TRIGGERED, None, 1, cfg);
                }
                dishonored_ai::AiEvent::CorpseFound { corpse, .. } => {
                    // 0x00A98950: `NumCorpsesDiscovered` the first time a given corpse is found.
                    if self.corpses_discovered.insert(*corpse) {
                        self.player_stat(dishonored_sim::darkness::stat::NUM_CORPSES_DISCOVERED, None, 1, cfg);
                    }
                }
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
                // A corpse the rats finished eating (0x00C6D760, `eDisEatenState_CorpseConsumed`)
                // leaves no body.
                dishonored_ai::AiEvent::RatEat { corpse, devoured: true, .. } => {
                    if let Some(p) = self.world.player_mut(*corpse) {
                        p.no_body = true;
                    }
                }
                // [distraction] The Kismet host fires `DisSeqEvent_Distracted`.
                // The Kismet host fires these (`kismet_host::ai_events`).
                dishonored_ai::AiEvent::AlarmEnded { .. }
                | dishonored_ai::AiEvent::RatEat { .. }
                | dishonored_ai::AiEvent::Distracted { .. }
                | dishonored_ai::AiEvent::BehaviorStarted { .. }
                | dishonored_ai::AiEvent::Combat { .. }
                | dishonored_ai::AiEvent::Investigate { .. }
                | dishonored_ai::AiEvent::PatrolPointReached { .. }
                | dishonored_ai::AiEvent::FleepointReached { .. } => {}
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
            self.bark_speakers(); // [ai] who can bark, where, and what their hook nodes read
            kismet_host::filter_events(self, &mut events); // [kismet] scripted choices
            self.sync_darkness(cfg);
            let cmds = self.state.step(&StepInput { dt, map: &map, players: &players, npc_positions: &npc_positions, events: &events, total_npcs: total });
            for c in cmds {
                self.apply(c, cfg);
            }
            self.sync_darkness(cfg);
            let climbables: Vec<String> = world.climbables().iter().map(|c| c.name.clone()).collect();
            kismet_host::presentation::rain_cover(self, world, cfg); // [kismet] the camera's rain cover test
            // [kismet] The step's gameplay activates its Kismet events (the next step's update runs
            // them) and the map change the update committed travels.
            kismet_host::raise(self, &out.events, cfg, &climbables);
            self.blocking_dialogs(); // [dialog] speakers stand while their conversation runs
            self.sync_darkness(cfg);
            save::tick(self, dt, &cfg.gameplay.items.combat_save); // [save] autosaves
            if self.state.outcome().is_some() && self.view.phase != MatchPhase::Ended {
                self.view.phase = MatchPhase::Ended;
            }
            self.view.time_left = if self.setup.def.settings.time_limit > 0.0 { (self.setup.def.settings.time_limit - self.state.view.stats.time).max(0.0) } else { 0.0 };
        }
        self.view.npc_poses = crate::npc_poses(&self.crowd.brains);
        self.view.npc_actions = crate::npc_actions(&self.crowd.brains);
        self.view.npc_heads = crate::npc_heads(&self.crowd.brains, &self.crowd.ai, &self.world.players);
        // Level travel happens at the end of the step (the host swaps collision before the next).
        if let Some((m, tag)) = self.pending_travel.take() {
            self.travel(&m, &tag, cfg);
        }
        let mut mv = self.state.view.clone();
        // Chaos as of the end of the step (the Kismet may have added to it after the view).
        mv.darkness_score = self.state.darkness_score;
        mv.darkness_level = self.state.darkness_level;
        mv.high_chaos = self.state.chaos_threshold.or(self.setup.def.settings.chaos_level_threshold).is_some_and(|t| self.state.darkness_level >= t);
        mv.npc_ids =self.id_by_npc.iter().map(|(n, i)| (n.clone(), *i)).collect();
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
        // Awareness markers: each living NPC's attention entry toward a human (`Attn::level`,
        // the brain's `DisAttentionInfo` level) with the pawn's `EAIAwareness`, within the
        // marker range (`m_fDisplay_NPCMaxDist_*` 10000 uu) of that human.
        self.view.marks.clear();
        for b in &self.crowd.brains {
            let Some(p) = self.world.players.iter().find(|p| p.id == b.id && p.alive) else { continue };
            for a in b.attn.iter().filter(|a| a.level > 0 && self.humans.contains(&a.who)) {
                let Some(h) = self.world.players.iter().find(|h| h.id == a.who && h.alive) else { continue };
                if h.position.distance(p.position) > 10_000.0 {
                    continue;
                }
                self.view.marks.push(crate::AwarenessMark { id: b.id, who: a.who, level: a.level.min(4), awareness: p.ai_awareness, proxy: a.proxy.to_array() });
            }
        }
        self.last_events = out.events.clone();
        // [kismet] AI noises the Kismet made since the update (the world actors' events it fed)
        // are heard next step.
        if let Some(k) = &mut self.kismet {
            self.last_events.append(&mut k.st.noises);
        }
        // [world] What no Kismet took this step: the AI noises are heard next step.
        for e in std::mem::take(&mut self.world_events) {
            if let dishonored_sim::world_actors::WorldEvent::Noise(s) = e {
                self.last_events.push(s);
            }
        }
        if wall_layer {
            placed.pop();
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
        let voice = self.bark_voices.get(&npc).cloned().or_else(|| {
            self.setup.original.iter().find(|o| o.spawn.id == npc).and_then(|o| {
                let list = self.setup.ai_levels.get(&o.map.to_ascii_lowercase())?.generic_trees.get(&o.tweak)?;
                (!list.is_empty()).then(|| list[npc as usize % list.len()].clone())
            })
        });
        if let Some((name, actor)) = named {
            self.state.ai_hooks.push((name, actor, hook.to_string(), initiator, voice, Some(npc)));
        }
    }

    /// [ai] The bark speakers for the conversation runner: every mission NPC and original spawn
    /// by its bark name (as [`Self::request_bark`] names it), where it stands, the dead ones,
    /// and the AI facts the hook node classes read (R5): the brain's suspicion level, knocked
    /// out, and how it perceives its current stimulus (seen 0, heard 1, heard danger 2).
    fn bark_speakers(&mut self) {
        let mut pos = BTreeMap::new();
        let mut dead = BTreeSet::new();
        let mut facts = BTreeMap::new();
        let names: Vec<(PlayerId, String)> = self.npc_by_id.iter().map(|(id, n)| (*id, n.clone())).chain(self.setup.original.iter().map(|o| (o.spawn.id, o.name.clone()))).collect();
        for (id, name) in names {
            let Some(p) = self.world.player(id) else { continue };
            if !p.alive {
                dead.insert(name.clone());
            } else {
                pos.entry(name.clone()).or_insert(p.position);
            }
            let brain = self.crowd.brains.iter().find(|b| b.id == id);
            let f = dishonored_conversation::HookFacts {
                suspicion: brain.map(|b| b.suspicion_level() as u8).unwrap_or(0),
                knocked_out: p.condition == dishonored_sim::state::Condition::Unconscious,
                notice: brain.and_then(dishonored_ai::Brain::notice_kind),
                investigate: 0,
            };
            facts.entry(name).or_insert(f);
        }
        self.state.speaker_pos = pos;
        self.state.dead_speakers = dead;
        self.state.hook_facts = facts;
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
            // [streaming] A mover of a hidden Kismet-streamed sublevel is not in the world, nor is
            // a nav blocker a step took out ([world]).
            if !self.level_present(dishonored_world::source_package(&m.actor).unwrap_or("")) || self.mover_gone(&m.actor) {
                match self.mover_keys.get(i) {
                    Some(k) if k.2 == u8::MAX => {}
                    Some(_) => {
                        self.mover_keys[i].2 = u8::MAX;
                        self.mover_worlds[i] = dishonored_world::CollisionWorld::from_triangles(Vec::new(), Vec::new());
                    }
                    None => {
                        self.mover_keys.push((m.actor.clone(), glam::Mat4::IDENTITY, u8::MAX));
                        self.mover_worlds.push(dishonored_world::CollisionWorld::from_triangles(Vec::new(), Vec::new()));
                    }
                }
                continue;
            }
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

    /// [world] A breakable nav blocker (`<package>:<name>`) taken out by a step without mesh or
    /// materials: its collision is gone (0x00A3A5F0).
    fn mover_gone(&self, actor: &str) -> bool {
        let key = self.current_map.to_ascii_lowercase();
        match (self.setup.world_actors.get(&key), self.world_actors.get(&key)) {
            (Some(d), Some(st)) => st.gone(d, actor),
            _ => false,
        }
    }

    /// What the player's crosshair is on that is the mission layer's to use (the host target the
    /// sim's last camera update chose, `dishonored_sim::crosshair`): what a Use now would
    /// interact with, and what the HUD's prompt shows. A sim-owned target (a prop, a pickup, a
    /// body) is the sim's own Use, not an `Interaction`.
    pub fn interaction(&self, id: PlayerId) -> Option<Interaction> {
        let c = self.world.player(id)?.crosshair.filter(|c| c.usable)?;
        match c.target {
            dishonored_sim::crosshair::TargetRef::Host(k) => self.host_list.get(k as usize).cloned(),
            _ => None,
        }
    }

    /// What each human's crosshair is on and what the HUD shows for it ([`crate::FocusView`]),
    /// read right after the sim step while `world.host_targets` still holds the list its camera
    /// update chose from (`Host(k)` indexes `host_list`).
    fn focus_views(&self, world: &dyn WorldQuery, cfg: &SimConfig) -> Vec<crate::FocusView> {
        use dishonored_sim::crosshair::TargetRef;
        let g = &cfg.gameplay;
        let npc_name = |id: PlayerId| self.npc_by_id.get(&id).and_then(|n| self.state.view.npc_names.iter().find(|x| &x.0 == n)).map(|x| x.1.clone()).unwrap_or_default();
        let mut out = Vec::new();
        for p in self.world.players.iter().filter(|p| self.humans.contains(&p.id)) {
            let mut f = crate::FocusView { player: p.id, crosshair: p.crosshair.filter(|c| c.usable), choke: dishonored_sim::crosshair::choke_target(&self.world, p, cfg).is_some(), assassinate: dishonored_sim::combat::can_assassinate(p, &self.world, world, &cfg.gameplay.movement, &cfg.tuning), drop_assassinate: dishonored_sim::combat::can_drop_assassinate(p, &self.world, world, &cfg.gameplay.movement, &cfg.tuning), ..Default::default() };
            match f.crosshair.map(|c| c.target) {
                Some(TargetRef::Prop(id)) => {
                    if let Some(m) = self.world.movables.iter().find(|m| m.id == id) {
                        f.text = m.prompt.clone();
                        f.point = Some(m.center().to_array());
                    }
                }
                Some(TargetRef::Pickup(id)) => {
                    if let Some(pk) = self.world.pickups.iter().find(|x| x.id == id) {
                        f.point = Some(dishonored_sim::inventory::pickup_position(&self.world, pk, &g.movement).to_array());
                    }
                }
                Some(TargetRef::Body(id)) => f.point = self.world.player(id).map(|v| v.position.to_array()),
                Some(TargetRef::Pawn(id)) => {
                    f.point = self.world.player(id).map(|v| (v.position + Vec3::Z * 40.0).to_array());
                    f.name = npc_name(id);
                }
                Some(TargetRef::Host(k)) => {
                    if let Some(it) = self.host_list.get(k as usize) {
                        let (text, name, point) = self.interaction_prompt(it);
                        f.text = text;
                        f.name = name;
                        f.point = point.map(|v| v.to_array());
                    }
                }
                // A climbable chain is not among the crosshair's candidates: a Use with nothing
                // else in focus grabs the one in view (`climb::try_use_grab`), with its
                // interactable's text.
                None => {
                    if let Some((_, q)) = dishonored_sim::climb::grab_target(p, world, &g.movement, g.items.inventory.interact_distance, dishonored_sim::inventory::INTERACT_COS) {
                        f.text = "`GBA_Use` Grab".to_string();
                        f.point = Some(q.to_array());
                    }
                }
            }
            out.push(f);
        }
        out
    }

    /// A host interaction's prompt (`m_InteractText`, with its key tag when it has one), the
    /// name its window shows and the point the crosshair moves to.
    fn interaction_prompt(&self, it: &Interaction) -> (String, String, Option<Vec3>) {
        let npc_name = |n: &str| self.state.view.npc_names.iter().find(|x| x.0 == n).map(|x| x.1.clone()).unwrap_or_default();
        match it {
            Interaction::Mission(MissionEvent::Used(m)) => (self.state.usable.get(m).cloned().unwrap_or_default(), String::new(), self.state.markers.get(m).map(|x| x.0)),
            Interaction::Mission(MissionEvent::Talked(n)) => {
                let pos = self.id_by_npc.get(n).and_then(|id| self.world.player(*id)).map(|v| v.position + Vec3::Z * 40.0);
                (self.state.talk.get(n).cloned().unwrap_or_default(), npc_name(n), pos)
            }
            Interaction::Mission(_) => Default::default(),
            Interaction::Door(i) => self.setup.doors.get(*i).map(|d| (d.prompt.clone(), String::new(), Some(d.position + Vec3::Z * 60.0))).unwrap_or_default(),
            Interaction::Kismet(i) => match kismet_host::usables(self).into_iter().nth(*i) {
                Some((u, p, prompt)) => {
                    let name = match u {
                        kismet_host::KUse::Talk { pawn, .. } => self.npc_by_id.get(&pawn).map(|n| npc_name(n)).unwrap_or_default(),
                        _ => String::new(),
                    };
                    (prompt, name, Some(p + Vec3::Z * 20.0))
                }
                None => Default::default(),
            },
            Interaction::Prop(i) => match self.setup.usables.get(*i) {
                Some(u) => {
                    let locked = self.props.get(&u.actor).map_or(u.locked, |s| s.locked);
                    let prompt = if u.door_axis.is_some() && locked { u.locked_prompt.clone() } else { u.prompt.clone() };
                    (prompt, String::new(), Some(u.center))
                }
                None => Default::default(),
            },
            Interaction::Launcher(i) => match self.setup.interactables.get(&self.current_map.to_ascii_lowercase()).and_then(|d| d.launchers.get(*i)) {
                Some(l) => (l.prompt.clone(), l.label.clone(), Some((l.bounds_min + l.bounds_max) * 0.5)),
                None => Default::default(),
            },
        }
    }

    /// The name of the actor behind a host interaction (what the original's `m_pCrosshairActor`
    /// is called): the Kismet usable's actor, the native usable's, the launcher's, the door's, the
    /// marker's or the NPC's id.
    pub fn interaction_name(&self, it: &Interaction) -> String {
        match it {
            Interaction::Mission(MissionEvent::Used(m)) => m.clone(),
            Interaction::Mission(MissionEvent::Talked(n)) => n.clone(),
            Interaction::Mission(_) => String::new(),
            Interaction::Door(i) => self.setup.doors.get(*i).map(|d| d.door.clone()).unwrap_or_default(),
            Interaction::Kismet(i) => {
                let list = kismet_host::usables(self);
                match list.get(*i) {
                    Some((u, _, _)) => match (kismet_host::usable_actor(self, u), u) {
                        (Some((_, name)), _) => name,
                        (None, kismet_host::KUse::Talk { pawn, .. }) => self.npc_by_id.get(pawn).cloned().unwrap_or_else(|| format!("#{pawn}")),
                        _ => String::new(),
                    },
                    None => String::new(),
                }
            }
            Interaction::Prop(i) => self.setup.usables.get(*i).map(|u| u.actor.rsplit(':').next().unwrap_or(&u.actor).to_string()).unwrap_or_default(),
            Interaction::Launcher(i) => self.setup.interactables.get(&self.current_map.to_ascii_lowercase()).and_then(|d| d.launchers.get(*i)).map(|l| l.name.clone()).unwrap_or_default(),
        }
    }

    pub fn respawn_time_left(&self, id: PlayerId) -> Option<f32> {
        self.respawn_queue.iter().find(|(p, _)| *p == id).map(|(_, t)| *t)
    }
}

/// Pawn kind and presentation archetype for an original spawner. The pawn tweak decides the
/// real model; until tweak -> mesh resolution exists, guards use the elite guard and civilians
/// the party models (female tweaks the female aristocrat).
fn original_archetype(s: &dishonored_ir::spawners::Spawner, class: Option<dishonored_ir::ai::NpcClass>) -> (PawnKind, u8) {
    let t = s.tweak.to_ascii_lowercase();
    match s.guard || class.is_some_and(|c| c.fights()) {
        true => (PawnKind::Guard, 1),
        _ if t.contains("_f_") || t.contains("female") || t.contains("lady") || t.contains("servant_f") => (PawnKind::Civilian, 3),
        _ => (PawnKind::Civilian, if s.position.x as i64 % 2 == 0 { 0 } else { 2 }),
    }
}
