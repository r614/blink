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

use crate::{MatchPhase, MatchView, ScoreEntry};
use dishonored_ai::{Brain, BrainState, Crowd, CrowdConfig, CrowdLayout, NPC_ID_BASE, NpcSpawn, snap_floor};
use dishonored_mission::runtime::{Command, MissionEvent, MissionState, StepInput};
use dishonored_mission::{Behavior, MissionDef, NpcKind, Place, resolve_actor};
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{PawnKind, PlayerId, PlayerState, WorldState};
use dishonored_sim::{SimConfig, StepOutput, spawn_npc, spawn_player, step_world};
use dishonored_world::WorldQuery;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// First id for mission NPCs (crowd NPCs start at `NPC_ID_BASE`).
pub const MISSION_NPC_BASE: PlayerId = NPC_ID_BASE + 500;
/// First id for NPCs from the levels' original spawners.
pub const ORIGINAL_NPC_BASE: PlayerId = NPC_ID_BASE + 1000;

/// One original `DishonoredSpawner` / `DisTravelSpawner` of a mission map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginalNpc {
    pub map: String,
    pub spawn: NpcSpawn,
    pub tweak: String,
    pub props: dishonored_ir::spawners::SpawnerProps,
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
    pub world: &'a dyn WorldQuery,
    /// The map's original spawners.
    pub spawners: &'a [dishonored_ir::spawners::Spawner],
}

/// Everything needed to rebuild the mission deterministically (stored in replays).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Original spawners of the maps listed in `MissionDef::original_npcs`.
    #[serde(default)]
    pub original: Vec<OriginalNpc>,
    /// Original dialog graphs the mission references, read from the install by the host
    /// (stored in replays so they play back identically).
    #[serde(default)]
    pub conversations: Option<dishonored_conversation::ConversationDb>,
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
                (Some(Crowd::populate(&cfg, maps[mi].ai_points, maps[mi].guard_spawns, maps[mi].world)), maps[mi].name.clone())
            }
            None => (None, start_map.clone()),
        };
        // Civilian panic destinations per map: the level's `DisFleePointActor`s.
        let flee: BTreeMap<String, Vec<Vec3>> = maps
            .iter()
            .map(|m| (m.name.to_ascii_lowercase(), m.actors.iter().filter(|a| a.1.starts_with("DisFleePointActor")).map(|a| snap_floor(m.world, a.2).unwrap_or(a.2)).collect()))
            .collect();
        let mut original = Vec::new();
        for m in maps.iter().filter(|m| def.original_npcs.iter().any(|o| o.eq_ignore_ascii_case(&m.name))) {
            for s in m.spawners {
                let (kind, archetype) = original_archetype(s);
                let id = ORIGINAL_NPC_BASE + original.len() as PlayerId;
                let p = snap_floor(m.world, s.position).unwrap_or(s.position);
                original.push(OriginalNpc { map: m.name.clone(), spawn: NpcSpawn { id, kind, archetype, position: p, yaw: s.yaw }, tweak: s.tweak.clone(), props: s.props.clone() });
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
        Ok(MissionSetup { def, markers, marker_map, start_map, starts, npcs, npc_map, crowd, crowd_map, maps: maps.iter().map(|m| m.name.clone()).collect(), doors, tagged_starts: tagged, conversations: None, flee, original })
    }
}

/// What a map keeps while players are elsewhere (`DisSeqAct_SaveLevelState`).
#[derive(Debug, Clone, Default)]
struct LevelState {
    npcs: Vec<PlayerState>,
    brains: Vec<Brain>,
    layout: CrowdLayout,
    spawns: Vec<NpcSpawn>,
}

/// What a player is about to interact with.
#[derive(Debug, Clone, PartialEq)]
pub enum Interaction {
    Mission(MissionEvent),
    /// Index into `MissionSetup::doors`.
    Door(usize),
}

pub struct MissionMatch {
    pub world: WorldState,
    pub view: MatchView,
    pub state: MissionState,
    pub crowd: Crowd,
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
    /// Original spawners that have spawned their pawn (ids are unique across maps).
    original_spawned: BTreeSet<PlayerId>,
    /// Players with a door confirmation open (player -> door index).
    choosing: BTreeMap<PlayerId, usize>,
    arrived: Option<String>,
    /// Dialog choice buttons held last tick (edge detection).
    choice_held: BTreeMap<PlayerId, u16>,
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
            view: MatchView { mode: "mission".into(), phase: MatchPhase::Playing, mission: Some(state.view.clone()), map: current_map.clone(), ..Default::default() },
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
            arrived: None,
            choice_held: BTreeMap::new(),
        };
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
            }
        }
        self.crowd.layout.flee_points = self.setup.flee.get(&self.current_map.to_ascii_lowercase()).cloned().unwrap_or_default();
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
    }

    /// Spawn an original spawner's pawn (once). Corpse / knocked-out spawners lie on the floor;
    /// `alert` (or `m_bAwareOfPlayerUponStartup`) starts it hunting the nearest player.
    fn spawn_original(&mut self, index: usize, alert: bool, cfg: &SimConfig) {
        let o = self.setup.original[index].clone();
        if !self.original_spawned.insert(o.spawn.id) {
            return;
        }
        let s = o.spawn;
        spawn_npc(&mut self.world, s.id, s.kind, s.archetype, s.position, s.yaw, cfg);
        if o.props.spawn_dead || o.props.knocked_out {
            if let Some(p) = self.world.player_mut(s.id) {
                p.alive = false;
            }
            return;
        }
        let mut b = Brain::new(s.id, s.kind, s.position, 0x5EED ^ s.id as u64);
        b.stay = !o.props.patrol_on_startup;
        self.crowd.brains.retain(|x| x.id != s.id);
        self.crowd.brains.push(b);
        if alert || o.props.aware_on_startup {
            self.alert_npc(s.id);
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
        let def = &self.setup.def.npcs[index];
        spawn_npc(&mut self.world, s.id, s.kind, s.archetype, s.position, s.yaw, cfg);
        let mut b = Brain::new(s.id, s.kind, s.position, 0x5EED ^ s.id as u64);
        b.stay = matches!(def.behavior, Behavior::Idle);
        self.crowd.brains.retain(|x| x.id != s.id);
        self.crowd.brains.push(b);
    }

    pub fn add_player(&mut self, id: PlayerId, name: &str, archetype: u8, cfg: &SimConfig) {
        let (p, yaw) = self.start_for(self.humans.len());
        spawn_player(&mut self.world, id, p, yaw, cfg);
        if let Some(pl) = self.world.player_mut(id) {
            pl.archetype = archetype;
        }
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
        // Guards investigating, searching or fighting a player: the AI is at least "suspecting",
        // so `DisTravelSpawner`s at the arrival tag send pursuers through the door.
        let pursued = self.crowd.brains.iter().any(|b| b.kind == PawnKind::Guard && matches!(b.state, BrainState::Investigate | BrainState::Search | BrainState::Combat) && b.target.is_some_and(|t| self.humans.contains(&t)));
        if !dest_map.eq_ignore_ascii_case(&self.current_map) {
            // Save the level being left.
            let humans = self.humans.clone();
            let npcs: Vec<PlayerState> = self.world.players.iter().filter(|p| !humans.contains(&p.id)).cloned().collect();
            let fresh = Crowd { ai: self.crowd.ai.clone(), ..Default::default() };
            let crowd = std::mem::replace(&mut self.crowd, fresh);
            self.saved.insert(self.current_map.to_ascii_lowercase(), LevelState { npcs, brains: crowd.brains, layout: crowd.layout, spawns: crowd.spawns });
            self.world.players.retain(|p| humans.contains(&p.id));
            self.world.projectiles.clear();
            self.current_map = self.setup.maps.iter().find(|m| m.eq_ignore_ascii_case(dest_map)).cloned().unwrap_or_else(|| dest_map.to_string());
            match self.saved.remove(&self.current_map.to_ascii_lowercase()) {
                Some(ls) => {
                    self.world.players.extend(ls.npcs);
                    self.crowd = Crowd { brains: ls.brains, layout: ls.layout, spawns: ls.spawns, ai: self.crowd.ai.clone(), ..Default::default() };
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
            Command::Travel(tag) => {
                if let Some(t) = self.setup.tagged_starts.iter().find(|t| t.1.eq_ignore_ascii_case(&tag)).cloned() {
                    self.pending_travel = Some((t.0, t.1));
                }
            }
            Command::Sound(_) => {}
        }
    }

    pub fn step(&mut self, inputs: &[(PlayerId, PlayerInput)], world: &dyn WorldQuery, cfg: &SimConfig) -> StepOutput {
        let dt = dishonored_sim::movement::DT;
        // A player answering a dialog choice does not move or act (the original takes control away).
        let conv_chooser = self.state.view.choice.as_ref().and_then(|c| c.player);
        let mut all: Vec<(PlayerId, PlayerInput)> = inputs
            .iter()
            .map(|(id, i)| {
                if self.choosing.contains_key(id) || conv_chooser == Some(*id) {
                    (*id, PlayerInput { tick: i.tick, yaw: i.yaw, pitch: i.pitch, left_hand: i.left_hand, ..Default::default() })
                } else {
                    (*id, *i)
                }
            })
            .collect();
        all.extend(self.crowd.think(&self.world, &self.last_events, world, dt));
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
        }
        // Detection / alarm / alerted mission NPCs from brain states (edge-triggered).
        for b in &self.crowd.brains {
            let hunting_human = b.state == BrainState::Combat && b.target.is_some_and(|t| self.humans.contains(&t));
            if b.kind == PawnKind::Guard && hunting_human {
                let calm_before = self.engaged.is_empty();
                if self.engaged.insert(b.id) {
                    events.push(MissionEvent::PlayerDetected);
                    // An alarm is the watch going from calm to engaged, not every guard joining.
                    if calm_before {
                        events.push(MissionEvent::AlarmRaised);
                    }
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
        // The alarm brings the map's alarm squads (`m_SpawnAtSuspicionLevel` spawners that do not
        // spawn on begin play, e.g. Boyle's `alarm_guards`), already hunting.
        if events.iter().any(|e| matches!(e, MissionEvent::AlarmRaised)) {
            for i in 0..self.setup.original.len() {
                let o = &self.setup.original[i];
                if o.map.eq_ignore_ascii_case(&self.current_map) && o.props.at_suspicion.is_some() && o.props.travel_tags.is_empty() && !o.props.on_begin_play {
                    self.spawn_original(i, true, cfg);
                }
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
            let (p, yaw) = self.start_for(i);
            let arch = self.world.player(id).map(|p| p.archetype).unwrap_or(0);
            spawn_player(&mut self.world, id, p, yaw, cfg);
            if let Some(pl) = self.world.player_mut(id) {
                pl.archetype = arch;
            }
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
            let cmds = self.state.step(&StepInput { dt, map: &map, players: &players, npc_positions: &npc_positions, events: &events, total_npcs: total });
            for c in cmds {
                self.apply(c, cfg);
            }
            if self.state.outcome().is_some() && self.view.phase != MatchPhase::Ended {
                self.view.phase = MatchPhase::Ended;
            }
            self.view.time_left = if self.setup.def.settings.time_limit > 0.0 { (self.setup.def.settings.time_limit - self.state.view.stats.time).max(0.0) } else { 0.0 };
        }
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
        self.view.mission = Some(mv);
        self.view.map = self.current_map.clone();
        self.last_events = out.events.clone();
        out
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
