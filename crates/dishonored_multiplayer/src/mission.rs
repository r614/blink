//! Mission mode: a custom mission (`dishonored_mission`) played solo or co-op on the
//! authoritative simulation. NPCs (ambient crowd + mission NPCs) are server-side brains;
//! the mission interpreter sees deaths, detection and player positions and drives NPCs,
//! objectives and the end of the mission. The mission view rides in the match view, so
//! clients, replays and the harness see exactly what the server decided.

use crate::{MatchPhase, MatchView, ScoreEntry};
use dishonored_ai::{Brain, BrainState, Crowd, CrowdConfig, NPC_ID_BASE, NpcSpawn, snap_floor};
use dishonored_mission::runtime::{Command, MissionEvent, MissionState, StepInput};
use dishonored_mission::{Behavior, MissionDef, NpcKind, Place, resolve_actor};
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{PawnKind, PlayerId, WorldState};
use dishonored_sim::{SimConfig, StepOutput, spawn_npc, spawn_player, step_world};
use dishonored_world::WorldQuery;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// First id for mission NPCs (crowd NPCs start at `NPC_ID_BASE`).
pub const MISSION_NPC_BASE: PlayerId = NPC_ID_BASE + 500;

/// A named original actor of the level: (package, name, position, yaw).
pub type LevelActor = (String, String, Vec3, f32);

/// Everything needed to rebuild the mission deterministically (stored in replays).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionSetup {
    pub def: MissionDef,
    /// Resolved markers: id -> (position, radius).
    pub markers: BTreeMap<String, (Vec3, f32)>,
    pub starts: Vec<(Vec3, f32)>,
    /// Mission NPC spawns (id string, spawn).
    pub npcs: Vec<(String, NpcSpawn)>,
    pub crowd: Option<Crowd>,
}

fn resolve_place(p: &Place, def: &MissionDef, actors: &[LevelActor], depth: u32) -> Result<(Vec3, f32), String> {
    if depth > 16 {
        return Err("marker reference cycle".into());
    }
    match p {
        Place::At(x, y, z) => Ok((Vec3::new(*x, *y, *z), 0.0)),
        Place::Actor(a) => {
            let names: Vec<(String, String)> = actors.iter().map(|x| (x.0.clone(), x.1.clone())).collect();
            let i = resolve_actor(a, &names).map_err(|e| e.to_string())?;
            Ok((actors[i].2, actors[i].3))
        }
        Place::Marker(m) => {
            let md = def.markers.iter().find(|x| &x.id == m).ok_or_else(|| format!("unknown marker '{m}'"))?;
            resolve_place(&md.at, def, actors, depth + 1)
        }
        Place::Offset(inner, (x, y, z)) => resolve_place(inner, def, actors, depth + 1).map(|(p, yaw)| (p + Vec3::new(*x, *y, *z), yaw)),
    }
}

impl MissionSetup {
    /// Resolve markers/NPCs against the level and build the ambient crowd.
    pub fn new(def: MissionDef, actors: &[LevelActor], default_starts: &[(Vec3, f32)], ai_points: &[(Vec3, f32)], guard_spawns: &[(Vec3, f32)], world: &dyn WorldQuery) -> Result<Self, String> {
        let errs = def.validate();
        if !errs.is_empty() {
            return Err(errs.join("; "));
        }
        let mut markers = BTreeMap::new();
        for m in &def.markers {
            let (p, _) = resolve_place(&m.at, &def, actors, 0)?;
            markers.insert(m.id.clone(), (p, m.radius));
        }
        let starts = match &def.player_start {
            Some(s) => {
                let md = def.markers.iter().find(|m| &m.id == s).ok_or("player_start marker missing")?;
                let (p, yaw) = resolve_place(&md.at, &def, actors, 0)?;
                vec![(p, yaw)]
            }
            None => default_starts.to_vec(),
        };
        let mut npcs = Vec::new();
        for (i, n) in def.npcs.iter().enumerate() {
            let (p, yaw) = resolve_place(&n.at, &def, actors, 0)?;
            let p = snap_floor(world, p).unwrap_or(p);
            let kind = match n.kind {
                NpcKind::Civilian => PawnKind::Civilian,
                NpcKind::Guard => PawnKind::Guard,
            };
            let archetype = n.archetype.unwrap_or(if kind == PawnKind::Guard { 1 } else { 0 });
            npcs.push((n.id.clone(), NpcSpawn { id: MISSION_NPC_BASE + i as PlayerId, kind, archetype, position: p, yaw }));
        }
        let crowd = match &def.crowd {
            Some(c) => {
                let center = match &c.center {
                    Some(p) => resolve_place(p, &def, actors, 0)?.0,
                    None => starts.first().map(|s| s.0).unwrap_or(Vec3::ZERO),
                };
                let cfg = CrowdConfig { civilians: c.civilians, guards: c.guards, center, radius: c.radius, seed: 7, civilian_archetypes: vec![0, 2, 3], guard_archetypes: vec![1] };
                Crowd::populate(&cfg, ai_points, guard_spawns, world)
            }
            None => Crowd::default(),
        };
        // Civilian panic destinations: the level's `DisFleePointActor`s.
        let mut crowd = crowd;
        let flee: Vec<Vec3> = actors.iter().filter(|a| a.1.starts_with("DisFleePointActor")).map(|a| a.2).collect();
        crowd.set_flee_points(&flee, world);
        let crowd = Some(crowd);
        Ok(MissionSetup { def, markers, starts, npcs, crowd })
    }
}

pub struct MissionMatch {
    pub world: WorldState,
    pub view: MatchView,
    pub state: MissionState,
    pub crowd: Crowd,
    pub setup: MissionSetup,
    pub last_events: Vec<SimEvent>,
    npc_by_id: BTreeMap<PlayerId, String>,
    id_by_npc: BTreeMap<String, PlayerId>,
    humans: Vec<PlayerId>,
    respawn_queue: Vec<(PlayerId, f32)>,
    /// Guards currently in combat with a human (edge detection for "detected").
    engaged: BTreeSet<PlayerId>,
    alerted: BTreeSet<PlayerId>,
    started: bool,
    use_held: BTreeSet<PlayerId>,
}

impl MissionMatch {
    pub fn new(setup: MissionSetup, cfg: &SimConfig) -> Self {
        let mut world = WorldState::default();
        let mut crowd = setup.crowd.clone().unwrap_or_default();
        crowd.ai = cfg.gameplay.ai.clone();
        for s in crowd.spawns.clone() {
            spawn_npc(&mut world, s.id, s.kind, s.archetype, s.position, s.yaw, cfg);
        }
        let mut npc_by_id = BTreeMap::new();
        let mut id_by_npc = BTreeMap::new();
        for (name, s) in &setup.npcs {
            npc_by_id.insert(s.id, name.clone());
            id_by_npc.insert(name.clone(), s.id);
        }
        let state = MissionState::new(setup.def.clone(), setup.markers.clone());
        let mut m = MissionMatch {
            world,
            view: MatchView { mode: "mission".into(), phase: MatchPhase::Playing, mission: Some(state.view.clone()), ..Default::default() },
            state,
            crowd,
            setup,
            last_events: Vec::new(),
            npc_by_id,
            id_by_npc,
            humans: Vec::new(),
            respawn_queue: Vec::new(),
            engaged: BTreeSet::new(),
            alerted: BTreeSet::new(),
            started: false,
            use_held: BTreeSet::new(),
        };
        for (i, n) in m.setup.def.npcs.clone().iter().enumerate() {
            if n.spawn_at_start {
                m.spawn_mission_npc(i, cfg);
            }
        }
        for c in m.state.initial_commands() {
            m.apply(c, cfg);
        }
        m
    }

    fn spawn_mission_npc(&mut self, index: usize, cfg: &SimConfig) {
        let (_, s) = self.setup.npcs[index].clone();
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

    fn start_for(&self, i: usize) -> (Vec3, f32) {
        let s = self.setup.starts.get(i % self.setup.starts.len().max(1)).copied().unwrap_or((Vec3::ZERO, 0.0));
        // Co-op players fan out around the start.
        let off = if i == 0 { Vec3::ZERO } else { Vec3::new((i as f32 * 2.1).cos(), (i as f32 * 2.1).sin(), 0.0) * 120.0 };
        (s.0 + off, s.1)
    }

    fn nearest_human(&self, from: Vec3) -> Option<PlayerId> {
        self.humans.iter().filter_map(|h| self.world.player(*h)).filter(|p| p.alive).min_by(|a, b| a.position.distance(from).total_cmp(&b.position.distance(from))).map(|p| p.id)
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
            Command::Sound(_) => {}
        }
    }

    pub fn step(&mut self, inputs: &[(PlayerId, PlayerInput)], world: &dyn WorldQuery, cfg: &SimConfig) -> StepOutput {
        let dt = dishonored_sim::movement::DT;
        let mut all = inputs.to_vec();
        all.extend(self.crowd.think(&self.world, &self.last_events, world, dt));
        let out = step_world(&mut self.world, &all, world, cfg);
        let mut events = Vec::new();
        // F (use): on press, interact with what the player is looking at.
        for (id, inp) in inputs {
            let pressed = inp.buttons & dishonored_sim::input::buttons::USE != 0;
            if pressed && self.use_held.insert(*id) {
                if let Some(e) = self.interaction(*id) {
                    events.push(e);
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
            let total = (self.crowd.spawns.len() + self.setup.npcs.len()) as u32;
            let cmds = self.state.step(&StepInput { dt, players: &players, npc_positions: &npc_positions, events: &events, total_npcs: total });
            for c in cmds {
                self.apply(c, cfg);
            }
            if self.state.outcome().is_some() && self.view.phase != MatchPhase::Ended {
                self.view.phase = MatchPhase::Ended;
            }
            self.view.time_left = if self.setup.def.settings.time_limit > 0.0 { (self.setup.def.settings.time_limit - self.state.view.stats.time).max(0.0) } else { 0.0 };
        }
        let mut mv = self.state.view.clone();
        mv.npc_ids = self.id_by_npc.iter().map(|(n, i)| (n.clone(), *i)).collect();
        self.view.mission = Some(mv);
        self.last_events = out.events.clone();
        out
    }

    /// What a player would interact with right now (nearest usable marker or talkable NPC
    /// within reach, in front of the view).
    pub fn interaction(&self, id: PlayerId) -> Option<MissionEvent> {
        let p = self.world.player(id).filter(|p| p.alive)?;
        let eye = p.position + Vec3::Z * 64.0;
        let fwd = dishonored_sim::movement::view_vector(p.yaw, p.pitch);
        let mut best: Option<(f32, MissionEvent)> = None;
        let mut consider = |pos: Vec3, reach: f32, ev: MissionEvent| {
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
                best = Some((score, ev));
            }
        };
        for m in self.state.usable.keys() {
            if let Some((pos, _)) = self.state.markers.get(m) {
                consider(*pos, 220.0, MissionEvent::Used(m.clone()));
            }
        }
        for n in self.state.talk.keys() {
            if let Some(np) = self.id_by_npc.get(n).and_then(|i| self.world.player(*i)).filter(|x| x.alive) {
                consider(np.position + Vec3::Z * 40.0, 200.0, MissionEvent::Talked(n.clone()));
            }
        }
        best.map(|b| b.1)
    }

    pub fn respawn_time_left(&self, id: PlayerId) -> Option<f32> {
        self.respawn_queue.iter().find(|(p, _)| *p == id).map(|(_, t)| *t)
    }
}
