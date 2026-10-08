//! "Contracts": social-stealth multiplayer (Phase II).
//!
//! Every active player has one target and one pursuer (a directed cycle). The loop is
//! observe -> identify -> stalk -> approach -> assassinate -> escape, while blending,
//! misdirecting and evading one's own pursuer. Faithful mechanics stay in
//! `dishonored_sim`; this module only adds multiplayer semantics: contracts, approximate
//! tracking, exposure, awareness, chases, kill quality, escape and world response.

use crate::{MatchPhase, MatchView, ScoreEntry};
use dishonored_ai::{Crowd, CrowdConfig};
use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::input::{LeftHand, PlayerInput};
use dishonored_sim::state::{MoveMode, PawnKind, PlayerId, PlayerState, SwordPhase, WorldState};
use dishonored_sim::{SimConfig, StepOutput, spawn_npc, spawn_player, step_world};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoreValues {
    pub pistol: i32,
    pub crossbow: i32,
    pub sword: i32,
    pub assassination: i32,
    pub drop_assassination: i32,
    pub windblast: i32,
    pub stealth_unaware: i32,
    pub stealth_suspicious: i32,
    pub proximity: i32,
    /// Per second spent stalking (within stalk range, undetected), capped.
    pub stalk_per_second: f32,
    pub stalk_cap: i32,
    pub escape: i32,
    pub defend_kill: i32,
    pub chase_escape: i32,
    pub wrong_kill_penalty: i32,
    pub civilian_kill_penalty: i32,
}

impl Default for ScoreValues {
    fn default() -> Self {
        Self {
            pistol: 100,
            crossbow: 150,
            sword: 250,
            assassination: 400,
            drop_assassination: 450,
            windblast: 100,
            stealth_unaware: 150,
            stealth_suspicious: 60,
            proximity: 50,
            stalk_per_second: 5.0,
            stalk_cap: 100,
            escape: 150,
            defend_kill: 100,
            chase_escape: 100,
            wrong_kill_penalty: -100,
            civilian_kill_penalty: -150,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractsRules {
    pub max_players: usize,
    pub time_limit: f32,
    pub respawn_delay: f32,
    /// Grace period after spawning/reassignment before tracking info is shown.
    pub reacquire_delay: f32,
    pub warmup_time: f32,
    pub post_match_time: f32,
    pub min_players: usize,
    pub escape_window: f32,
    pub escape_distance: f32,
    pub stalk_range: f32,
    pub chase_lose_time: f32,
    pub scores: ScoreValues,
    pub crowd: Option<CrowdConfig>,
}

impl Default for ContractsRules {
    fn default() -> Self {
        Self {
            max_players: 8,
            time_limit: 720.0,
            respawn_delay: 5.0,
            reacquire_delay: 4.0,
            warmup_time: 5.0,
            post_match_time: 12.0,
            min_players: 2,
            escape_window: 15.0,
            escape_distance: 900.0,
            stalk_range: 1500.0,
            chase_lose_time: 8.0,
            scores: ScoreValues::default(),
            crowd: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, PartialOrd, Ord)]
pub enum PursuitAwareness {
    #[default]
    Unaware,
    Suspicious,
    Aware,
    Pursuit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum ExposureEvent {
    Sprint,
    PowerUse,
    WeaponDraw,
    Attack,
    Parkour,
    RestrictedMovement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Proximity {
    Far,
    Medium,
    Near,
    VeryNear,
}

/// What a player is told about their target. Never exact coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TargetInfo {
    pub target: Option<PlayerId>,
    /// Visual archetype of the target body (appearance hint).
    pub appearance: Option<u8>,
    /// 0..7 compass octant (0 = +X / east in world space), None during reacquire.
    pub bearing_octant: Option<u8>,
    pub proximity: Option<Proximity>,
    /// Your target's awareness of you (hidden from the target itself).
    pub target_awareness: PursuitAwareness,
    /// Whether you are currently being chased by your pursuer.
    pub being_chased: bool,
    /// Weak cue: your pursuer is near (<= stalk range) — direction unknown.
    pub pursuer_near: bool,
    pub exposure: f32,
    pub escaping: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct KillScore {
    pub base: u32,
    pub stealth_bonus: u32,
    pub proximity_bonus: u32,
    pub method_bonus: u32,
    pub stalk_bonus: u32,
    pub escape_bonus: u32,
}

impl KillScore {
    pub fn total(&self) -> i32 {
        (self.base + self.stealth_bonus + self.proximity_bonus + self.method_bonus + self.stalk_bonus + self.escape_bonus) as i32
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayerContract {
    pub id: PlayerId,
    pub target: Option<PlayerId>,
    pub pursuer: Option<PlayerId>,
    pub active: bool,
    pub reacquire: f32,
    /// This player's awareness of their pursuer.
    pub awareness: PursuitAwareness,
    pub awareness_level: f32,
    pub exposure: f32,
    pub stalk_time: f32,
    pub chase_unseen: f32,
    /// Pending escape after a contract kill: (seconds left, kill position, score so far).
    pub escape: Option<(f32, Vec3, KillScore)>,
    pub kills_log: Vec<KillScore>,
}

pub struct ContractsMatch {
    pub rules: ContractsRules,
    pub world: WorldState,
    pub view: MatchView,
    pub spawns: Vec<(Vec3, f32)>,
    pub contracts: Vec<PlayerContract>,
    pub crowd: Option<Crowd>,
    pub last_events: Vec<SimEvent>,
    phase_timer: f32,
    respawn_queue: Vec<(PlayerId, f32)>,
    rng: u64,
    /// Movers and doors as the map starts them.
    start: dishonored_world::StartLayers,
    /// Spawn candidates a pawn can walk between (built on the first spawn, see `candidates`).
    candidates: Option<Vec<(Vec3, f32)>>,
}

fn rng_next(s: &mut u64) -> f32 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 40) as f32 / (1u64 << 24) as f32
}

fn sees(world: &dyn WorldQuery, a: &PlayerState, b: &PlayerState, range: f32, fov_deg: f32) -> bool {
    let eye = a.position + Vec3::Z * 85.0;
    let to = b.position + Vec3::Z * 40.0 - eye;
    let d = to.length();
    if d > range || d < 1.0 {
        return false;
    }
    let fwd = dishonored_sim::movement::view_vector(a.yaw, a.pitch * 0.5);
    if fwd.dot(to / d) < (fov_deg * 0.5).to_radians().cos() {
        return false;
    }
    world.raycast(eye, to, d, channel::VISION).is_none_or(|h| h.distance >= d - 40.0)
}

impl ContractsMatch {
    pub fn new(rules: ContractsRules, spawns: Vec<(Vec3, f32)>, crowd: Option<Crowd>, cfg: &SimConfig) -> Self {
        let view = MatchView { mode: "contracts".into(), phase: MatchPhase::Warmup, time_left: rules.time_limit, score_limit: 0, ..Default::default() };
        let mut m = Self { rules, world: WorldState::default(), view, spawns, contracts: Vec::new(), crowd, last_events: Vec::new(), phase_timer: 0.0, respawn_queue: Vec::new(), rng: 0xC0FF_EE11, start: Default::default(), candidates: None };
        if let Some(c) = m.crowd.as_mut() {
            // Original AI tuning (perception, thresholds, search, panic) from the install.
            c.ai = cfg.gameplay.ai.clone();
            for s in c.spawns.clone() {
                spawn_npc(&mut m.world, s.id, s.kind, s.archetype, s.position, s.yaw, cfg);
            }
        }
        m
    }

    fn contract(&self, id: PlayerId) -> Option<&PlayerContract> {
        self.contracts.iter().find(|c| c.id == id)
    }
    fn contract_mut(&mut self, id: PlayerId) -> Option<&mut PlayerContract> {
        self.contracts.iter_mut().find(|c| c.id == id)
    }

    /// Spawn candidates: the match spawns and the crowd's AI points that stand on the original
    /// navmesh in the largest group a pawn can walk between (as the free-for-all spawns: Boyle
    /// Ext's canal drop-off, roof patio and river grate starts are left out, else a player
    /// spawned there can never reach anyone). Without a navmesh every candidate is kept.
    fn walkable_candidates(&self, world: &dyn WorldQuery) -> Vec<(Vec3, f32)> {
        let mut all: Vec<(Vec3, f32)> = self.spawns.clone();
        if let Some(c) = self.crowd.as_ref() {
            all.extend(c.layout.points.iter().map(|p| (*p, 0.0)));
        }
        let Some(nav) = world.nav().filter(|n| !n.polys.is_empty()) else { return all };
        let on: Vec<usize> = (0..all.len()).filter(|&i| nav.locate(all[i].0, 60.0, 260.0).is_some()).collect();
        // Group the candidates by reachability from each unassigned one (largest group wins).
        let mut group_of: Vec<Option<usize>> = vec![None; all.len()];
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for &i in &on {
            if group_of[i].is_some() {
                continue;
            }
            let g = groups.len();
            let mut members = vec![i];
            group_of[i] = Some(g);
            for &j in &on {
                if group_of[j].is_none() && nav.find_path(all[i].0, all[j].0, &Default::default()).is_some() {
                    group_of[j] = Some(g);
                    members.push(j);
                }
            }
            groups.push(members);
        }
        match groups.into_iter().max_by(|a, b| a.len().cmp(&b.len()).then(b[0].cmp(&a[0]))) {
            Some(mut g) if g.len() >= 2 => {
                g.sort_unstable();
                g.into_iter().map(|i| all[i]).collect()
            }
            _ => all,
        }
    }

    /// Spawn rule (brief): not in direct view of enemies, not next to target/pursuer,
    /// not in combat; prefer NPC-populated places. Deterministic scoring.
    fn pick_spawn(&mut self, for_id: PlayerId, world: &dyn WorldQuery) -> (Vec3, f32) {
        if self.candidates.is_none() {
            self.candidates = Some(self.walkable_candidates(world));
        }
        let candidates = self.candidates.clone().unwrap_or_default();
        if candidates.is_empty() {
            return (Vec3::new(0.0, 0.0, 200.0), 0.0);
        }
        let humans: Vec<PlayerState> = self.world.players.iter().filter(|p| p.alive && p.kind == PawnKind::Human && p.id != for_id).copied().collect();
        let npcs: Vec<Vec3> = self.world.players.iter().filter(|p| p.alive && p.kind == PawnKind::Civilian).map(|p| p.position).collect();
        let mut best = (f32::MIN, candidates[0]);
        let jitter = rng_next(&mut self.rng);
        for (i, (pos, yaw)) in candidates.iter().enumerate() {
            let probe = PlayerState::new(0, *pos, *yaw);
            let mut s = 0.0f32;
            for h in &humans {
                let d = h.position.distance(*pos);
                s += (d.min(4000.0)) / 4000.0;
                if d < 1200.0 {
                    s -= 3.0;
                }
                if sees(world, h, &probe, 3000.0, 110.0) {
                    s -= 4.0;
                }
            }
            let crowd_near = npcs.iter().filter(|n| n.distance(*pos) < 700.0).count() as f32;
            s += crowd_near.min(5.0) * 0.4;
            s += ((i as f32 * 0.618 + jitter).fract()) * 0.3; // deterministic tiebreak
            if s > best.0 {
                best = (s, (*pos, *yaw));
            }
        }
        best.1
    }

    pub fn add_player(&mut self, id: PlayerId, name: &str, archetype: u8, world: &dyn WorldQuery, cfg: &SimConfig) {
        if self.view.scores.iter().any(|s| s.id == id) {
            return;
        }
        self.view.scores.push(ScoreEntry { id, name: name.to_string(), ..Default::default() });
        let (p, y) = self.pick_spawn(id, world);
        spawn_player(&mut self.world, id, p, y, cfg);
        if let Some(pl) = self.world.player_mut(id) {
            pl.archetype = archetype;
        }
        self.contracts.push(PlayerContract { id, active: true, reacquire: self.rules.reacquire_delay, ..Default::default() });
        self.rebuild_cycle();
    }

    pub fn remove_player(&mut self, id: PlayerId) {
        self.view.scores.retain(|s| s.id != id);
        self.contracts.retain(|c| c.id != id);
        self.respawn_queue.retain(|(p, _)| *p != id);
        dishonored_sim::remove_player(&mut self.world, id);
        self.rebuild_cycle();
    }

    /// Rebuild a single directed cycle over active players, preserving existing order
    /// where possible (sorted by id with a rotation that changes per rebuild).
    pub fn rebuild_cycle(&mut self) {
        let delay = self.rules.reacquire_delay;
        let mut active: Vec<PlayerId> = self.contracts.iter().filter(|c| c.active).map(|c| c.id).collect();
        active.sort();
        // Rotate deterministically so assignments vary across a match.
        if active.len() > 2 {
            let r = (rng_next(&mut self.rng) * active.len() as f32) as usize % active.len();
            active.rotate_left(r);
            // Shuffle interior deterministically (Fisher-Yates with match RNG).
            for i in (1..active.len()).rev() {
                let j = (rng_next(&mut self.rng) * (i + 1) as f32) as usize % (i + 1);
                active.swap(i, j);
            }
        }
        for c in self.contracts.iter_mut() {
            c.target = None;
            c.pursuer = None;
        }
        let n = active.len();
        if n >= 2 {
            for i in 0..n {
                let a = active[i];
                let b = active[(i + 1) % n];
                let old_target = self.contract(a).and_then(|c| c.target);
                if let Some(c) = self.contract_mut(a) {
                    c.target = Some(b);
                    if old_target != Some(b) {
                        c.reacquire = c.reacquire.max(delay);
                        c.stalk_time = 0.0;
                    }
                }
                if let Some(c) = self.contract_mut(b) {
                    c.pursuer = Some(a);
                }
            }
        }
    }

    /// Repair after a death: the victim leaves the cycle; its pursuer inherits its target.
    fn repair_after_death(&mut self, victim: PlayerId) {
        let delay = self.rules.reacquire_delay;
        let (vt, vp) = match self.contract(victim) {
            Some(c) => (c.target, c.pursuer),
            None => return,
        };
        if let Some(c) = self.contract_mut(victim) {
            c.active = false;
            c.target = None;
            c.pursuer = None;
            c.awareness = PursuitAwareness::Unaware;
            c.awareness_level = 0.0;
        }
        match (vp, vt) {
            (Some(p), Some(t)) if p != t => {
                if let Some(c) = self.contract_mut(p) {
                    c.target = Some(t);
                    c.reacquire = delay;
                    c.stalk_time = 0.0;
                }
                if let Some(c) = self.contract_mut(t) {
                    c.pursuer = Some(p);
                    c.awareness = PursuitAwareness::Unaware;
                    c.awareness_level = 0.0;
                }
            }
            (Some(p), Some(t)) if p == t => {
                // Two-player cycle collapses; both wait for the victim's return.
                if let Some(c) = self.contract_mut(p) {
                    c.target = None;
                    c.pursuer = None;
                }
            }
            _ => {}
        }
    }

    fn award(&mut self, id: PlayerId, pts: i32, msg: String) {
        if let Some(s) = self.view.scores.iter_mut().find(|s| s.id == id) {
            s.score += pts;
        }
        let _ = msg;
    }

    /// Exposure generated by a pawn this tick (multiplayer-only semantics).
    fn exposure_events(p: &PlayerState) -> Vec<ExposureEvent> {
        let mut e = Vec::new();
        let i = &p.last_input;
        if i.pressed(dishonored_sim::input::buttons::SPRINT) && p.velocity.truncate().length() > 450.0 {
            e.push(ExposureEvent::Sprint);
        }
        if p.mode == MoveMode::Blinking || p.vitals.dark_vision_remaining > 0.0 && p.vitals.dark_vision_remaining > 29.9 {
            e.push(ExposureEvent::PowerUse);
        }
        if matches!(p.left.item, LeftHand::Pistol | LeftHand::Crossbow) {
            e.push(ExposureEvent::WeaponDraw);
        }
        if matches!(p.sword.phase, SwordPhase::Startup | SwordPhase::Active) || p.left.phase == dishonored_sim::state::LeftPhase::Recovering {
            e.push(ExposureEvent::Attack);
        }
        if matches!(p.mode, MoveMode::Mantling) || (p.mode == MoveMode::Falling && p.velocity.z > 300.0) {
            e.push(ExposureEvent::Parkour);
        }
        e
    }

    /// One simulation step. The level's Kismet does not run in this mode: its movers collide
    /// where the map places them and its doors as they start ([`dishonored_world::StartLayers`]).
    pub fn step(&mut self, inputs: &[(PlayerId, PlayerInput)], world: &dyn WorldQuery, cfg: &SimConfig) -> StepOutput {
        self.start.update(world);
        let layers = std::mem::take(&mut self.start.layers);
        let layered = dishonored_world::Layered { base: world, layers: &layers };
        let out = self.step_in(inputs, if layers.is_empty() { world } else { &layered }, cfg);
        self.start.layers = layers;
        out
    }

    fn step_in(&mut self, inputs: &[(PlayerId, PlayerInput)], world: &dyn WorldQuery, cfg: &SimConfig) -> StepOutput {
        let dt = dishonored_sim::movement::DT;
        let mut all_inputs = inputs.to_vec();
        if let Some(crowd) = self.crowd.as_mut() {
            all_inputs.extend(crowd.think(&self.world, &self.last_events, world, dt));
            crowd.write_awareness(&mut self.world);
        }
        let out = step_world(&mut self.world, &all_inputs, world, cfg);
        self.phase_timer += dt;

        // NPC respawns.
        if let Some(crowd) = self.crowd.as_mut() {
            for s in crowd.tick_respawns(&out.events, dt) {
                spawn_npc(&mut self.world, s.id, s.kind, s.archetype, s.position, s.yaw, cfg);
            }
            self.view.npc_poses = crate::npc_poses(&crowd.brains);
        }

        match self.view.phase {
            MatchPhase::Warmup => {
                let humans = self.contracts.len();
                if humans >= self.rules.min_players && self.phase_timer >= self.rules.warmup_time {
                    self.view.phase = MatchPhase::Playing;
                    self.phase_timer = 0.0;
                    self.view.time_left = self.rules.time_limit;
                    for s in self.view.scores.iter_mut() {
                        s.kills = 0;
                        s.deaths = 0;
                        s.score = 0;
                    }
                    self.rebuild_cycle();
                }
            }
            MatchPhase::Playing => {
                self.view.time_left = (self.view.time_left - dt).max(0.0);
                self.update_awareness_and_exposure(world, dt);
                self.resolve_events(&out.events, world);
                self.update_escapes(dt);
                if self.view.time_left <= 0.0 {
                    self.view.phase = MatchPhase::Ended;
                    self.view.winner = self.view.scores.iter().max_by_key(|s| (s.score, -s.deaths)).map(|s| s.id);
                    self.phase_timer = 0.0;
                }
            }
            MatchPhase::Ended => {
                if self.phase_timer >= self.rules.post_match_time {
                    self.view.phase = MatchPhase::Warmup;
                    self.view.match_number += 1;
                    self.view.winner = None;
                    self.phase_timer = self.rules.warmup_time;
                }
            }
        }

        // Player respawns + reacquire timers.
        for e in &out.events {
            if let SimEvent::Death { victim, .. } = e {
                if self.contract(*victim).is_some() && !self.respawn_queue.iter().any(|(p, _)| p == victim) {
                    self.respawn_queue.push((*victim, self.rules.respawn_delay));
                }
            }
        }
        let mut due = Vec::new();
        for (id, t) in self.respawn_queue.iter_mut() {
            *t -= dt;
            if *t <= 0.0 {
                due.push(*id);
            }
        }
        self.respawn_queue.retain(|(_, t)| *t > 0.0);
        for id in due {
            let (p, y) = self.pick_spawn(id, world);
            let arch = self.world.player(id).map(|p| p.archetype).unwrap_or(0);
            spawn_player(&mut self.world, id, p, y, cfg);
            if let Some(pl) = self.world.player_mut(id) {
                pl.archetype = arch;
            }
            // A new pawn: the guards' grudges and attention were held against the old one.
            if let Some(c) = self.crowd.as_mut() {
                c.forget(id);
            }
            let delay = self.rules.reacquire_delay;
            if let Some(c) = self.contract_mut(id) {
                c.active = true;
                c.reacquire = delay;
                c.escape = None;
            }
            self.rebuild_cycle();
        }
        for c in self.contracts.iter_mut() {
            c.reacquire = (c.reacquire - dt).max(0.0);
        }
        self.view.scores.sort_by(|a, b| b.score.cmp(&a.score).then(a.deaths.cmp(&b.deaths)).then(a.id.cmp(&b.id)));
        self.last_events = out.events.clone();
        out
    }

    fn update_awareness_and_exposure(&mut self, world: &dyn WorldQuery, dt: f32) {
        let snapshot: Vec<PlayerState> = self.world.players.clone();
        let find = |id: PlayerId| snapshot.iter().find(|p| p.id == id).copied();
        for c in self.contracts.iter_mut() {
            let Some(me) = find(c.id).filter(|p| p.alive) else { continue };
            // Exposure decays; events add.
            let ev = Self::exposure_events(&me);
            c.exposure = (c.exposure - dt * 0.35).max(0.0);
            for e in ev {
                c.exposure += match e {
                    ExposureEvent::Sprint => 0.6,
                    ExposureEvent::PowerUse => 1.5,
                    ExposureEvent::WeaponDraw => 0.4,
                    ExposureEvent::Attack => 2.0,
                    ExposureEvent::Parkour => 0.8,
                    ExposureEvent::RestrictedMovement => 1.0,
                } * dt * 2.0;
            }
            c.exposure = c.exposure.min(3.0);
            // Stalking credit toward own target.
            if let Some(t) = c.target.and_then(find) {
                if t.alive && t.position.distance(me.position) <= self.rules.stalk_range {
                    c.stalk_time += dt;
                }
            }
        }
        // Awareness: each target's perception of their pursuer.
        let exposures: Vec<(PlayerId, f32)> = self.contracts.iter().map(|c| (c.id, c.exposure)).collect();
        let lose = self.rules.chase_lose_time;
        for c in self.contracts.iter_mut() {
            let Some(me) = find(c.id).filter(|p| p.alive) else { continue };
            let Some(hunter) = c.pursuer.and_then(find).filter(|p| p.alive) else {
                c.awareness_level = (c.awareness_level - dt).max(0.0);
                continue;
            };
            let hunter_exposure = exposures.iter().find(|e| e.0 == hunter.id).map(|e| e.1).unwrap_or(0.0);
            let d = hunter.position.distance(me.position);
            let visible = sees(world, &me, &hunter, 2500.0, 120.0);
            if visible {
                // Close + exposed hunters are noticed quickly; blending hunters slowly.
                let closeness = (1.0 - d / 2500.0).max(0.0);
                c.awareness_level += dt * (0.15 + hunter_exposure * 0.6) * (0.4 + closeness * 1.6);
                c.chase_unseen = 0.0;
            } else {
                c.awareness_level -= dt * 0.08;
                c.chase_unseen += dt;
            }
            c.awareness_level = c.awareness_level.clamp(0.0, 3.0);
            let new = if c.awareness == PursuitAwareness::Pursuit {
                if c.chase_unseen > lose { PursuitAwareness::Suspicious } else { PursuitAwareness::Pursuit }
            } else if c.awareness_level >= 2.0 && visible && d < 1200.0 && (hunter_exposure > 1.0 || d < 400.0) {
                PursuitAwareness::Pursuit
            } else if c.awareness_level >= 1.5 {
                PursuitAwareness::Aware
            } else if c.awareness_level >= 0.6 {
                PursuitAwareness::Suspicious
            } else {
                PursuitAwareness::Unaware
            };
            // Chase ended by escape -> target scores (handled by caller via flag below).
            if c.awareness == PursuitAwareness::Pursuit && new != PursuitAwareness::Pursuit {
                c.kills_log.push(KillScore { escape_bonus: 0, ..Default::default() });
                c.escape = c.escape.take();
                c.awareness_level = 1.0;
            }
            c.awareness = new;
        }
        // Chase escapes award points to the escaping target.
        let escaped: Vec<PlayerId> = self.contracts.iter_mut().filter_map(|c| {
            if c.kills_log.last().is_some_and(|k| k.total() == 0) {
                c.kills_log.pop();
                Some(c.id)
            } else {
                None
            }
        }).collect();
        for id in escaped {
            let pts = self.rules.scores.chase_escape;
            self.award(id, pts, "escaped a chase".into());
        }
        // Chase support (multiplayer affordance using faithful mechanics): a target
        // in pursuit regains mana faster so Blink escapes are possible.
        for c in self.contracts.iter() {
            if c.awareness == PursuitAwareness::Pursuit {
                if let Some(p) = self.world.player_mut(c.id) {
                    p.vitals.mana = (p.vitals.mana + 6.0 * dt).min(100.0);
                }
            }
        }
    }

    fn resolve_events(&mut self, events: &[SimEvent], world: &dyn WorldQuery) {
        let _ = world;
        // Being hurt by your pursuer reveals them.
        for e in events {
            if let SimEvent::Damage { victim, attacker: Some(a), .. } = *e {
                if let Some(c) = self.contract_mut(victim) {
                    if c.pursuer == Some(a) && c.awareness < PursuitAwareness::Aware {
                        c.awareness = PursuitAwareness::Aware;
                        c.awareness_level = c.awareness_level.max(1.6);
                    }
                }
            }
        }
        for e in events {
            let SimEvent::Death { victim, killer, cause } = *e else { continue };
            let victim_kind = self.world.player(victim).map(|p| p.kind).unwrap_or(PawnKind::Human);
            if let Some(s) = self.view.scores.iter_mut().find(|s| s.id == victim) {
                s.deaths += 1;
            }
            let Some(k) = killer else {
                if self.contract(victim).is_some() {
                    self.repair_after_death(victim);
                }
                continue;
            };
            let killer_is_player = self.contract(k).is_some();
            if !killer_is_player {
                // NPC (guard) kill: victim just leaves the cycle.
                if self.contract(victim).is_some() {
                    self.repair_after_death(victim);
                }
                continue;
            }
            if victim_kind != PawnKind::Human {
                let pts = self.rules.scores.civilian_kill_penalty;
                self.award(k, pts, "killed an NPC".into());
                if let Some(c) = self.contract_mut(k) {
                    c.exposure = 3.0;
                }
                continue;
            }
            let kc = self.contract(k).cloned().unwrap_or_default();
            let vc = self.contract(victim).cloned().unwrap_or_default();
            if kc.target == Some(victim) {
                // Contract kill: quality-scored.
                let sv = &self.rules.scores;
                let base = match cause {
                    DamageCause::Pistol | DamageCause::PistolHeadshot => sv.pistol,
                    DamageCause::Crossbow | DamageCause::CrossbowHeadshot => sv.crossbow,
                    DamageCause::Assassination => sv.assassination,
                    DamageCause::DropAssassination => sv.drop_assassination,
                    DamageCause::Windblast => sv.windblast,
                    _ => sv.sword,
                } as u32;
                let stealth = match vc.awareness_of(k) {
                    PursuitAwareness::Unaware => sv.stealth_unaware,
                    PursuitAwareness::Suspicious => sv.stealth_suspicious,
                    _ => 0,
                } as u32;
                let close = matches!(cause, DamageCause::Sword | DamageCause::SwordCounter | DamageCause::Assassination | DamageCause::DropAssassination);
                let quiet_ranged = matches!(cause, DamageCause::Crossbow | DamageCause::CrossbowHeadshot);
                // Loud weapons carry an information cost, not a damage penalty: no stealth
                // or stalk credit for gunfire; half credit for the comparatively quiet crossbow.
                let stealth_scale = if close { 1.0 } else if quiet_ranged { 0.5 } else { 0.0 };
                let stalk_scale = if close { 1.0 } else { 0.0 };
                let stealth = (stealth as f32 * stealth_scale) as u32;
                let ks = KillScore {
                    base,
                    stealth_bonus: stealth,
                    proximity_bonus: if close { sv.proximity as u32 } else { 0 },
                    method_bonus: 0,
                    stalk_bonus: (((kc.stalk_time * sv.stalk_per_second) as i32).min(sv.stalk_cap).max(0) as f32 * stalk_scale) as u32,
                    escape_bonus: 0,
                };
                let kpos = self.world.player(k).map(|p| p.position).unwrap_or(Vec3::ZERO);
                let pts = ks.total();
                self.award(k, pts, format!("contract kill {ks:?}"));
                if let Some(s) = self.view.scores.iter_mut().find(|s| s.id == k) {
                    s.kills += 1;
                }
                let window = self.rules.escape_window;
                if let Some(c) = self.contract_mut(k) {
                    c.escape = Some((window, kpos, ks));
                    c.kills_log.push(ks);
                    c.exposure = c.exposure.max(2.0);
                }
            } else if vc.target == Some(k) {
                // The target killed its hunter: defensive kill.
                let pts = self.rules.scores.defend_kill;
                self.award(k, pts, "defended against pursuer".into());
            } else {
                let pts = self.rules.scores.wrong_kill_penalty;
                self.award(k, pts, "unauthorised kill".into());
            }
            self.repair_after_death(victim);
        }
    }

    fn update_escapes(&mut self, dt: f32) {
        let guards_hunting: Vec<(PlayerId, Vec3)> = Vec::new();
        let _ = guards_hunting;
        let mut bonuses = Vec::new();
        let positions: Vec<(PlayerId, Vec3, bool)> = self.world.players.iter().map(|p| (p.id, p.position, p.alive)).collect();
        let escape_d = self.rules.escape_distance;
        let bonus = self.rules.scores.escape as u32;
        for c in self.contracts.iter_mut() {
            if let Some((t, pos, ks)) = c.escape.as_mut() {
                *t -= dt;
                if *t <= 0.0 {
                    let alive_far = positions.iter().find(|p| p.0 == c.id).is_some_and(|p| p.2 && p.1.distance(*pos) >= escape_d);
                    if alive_far && c.exposure < 1.0 {
                        ks.escape_bonus = bonus;
                        bonuses.push((c.id, bonus as i32));
                    }
                    c.escape = None;
                }
            }
        }
        for (id, b) in bonuses {
            self.award(id, b, "escaped after kill".into());
        }
    }

    /// Private per-player info (sent only to that player).
    pub fn target_info(&self, id: PlayerId) -> TargetInfo {
        let Some(c) = self.contract(id) else { return TargetInfo::default() };
        let me = self.world.player(id);
        let mut info = TargetInfo { target: c.target, exposure: c.exposure, being_chased: c.awareness == PursuitAwareness::Pursuit, escaping: c.escape.map(|e| e.0), ..Default::default() };
        if let (Some(me), Some(t)) = (me, c.target.and_then(|t| self.world.player(t))) {
            info.appearance = Some(t.archetype);
            let tc = self.contract(t.id);
            info.target_awareness = tc.map(|x| x.awareness).unwrap_or_default();
            let in_chase = info.target_awareness == PursuitAwareness::Pursuit;
            if c.reacquire <= 0.0 || in_chase {
                let d = t.position - me.position;
                let ang = d.y.atan2(d.x).to_degrees().rem_euclid(360.0);
                info.bearing_octant = Some((((ang + 22.5) / 45.0) as u8) % 8);
                let dist = d.length();
                info.proximity = Some(if dist > 3000.0 {
                    Proximity::Far
                } else if dist > 1500.0 {
                    Proximity::Medium
                } else if dist > 600.0 {
                    Proximity::Near
                } else {
                    Proximity::VeryNear
                });
            }
        }
        if let (Some(me), Some(h)) = (me, c.pursuer.and_then(|p| self.world.player(p))) {
            info.pursuer_near = h.position.distance(me.position) <= self.rules.stalk_range && c.awareness >= PursuitAwareness::Suspicious;
        }
        info
    }
}

impl PlayerContract {
    /// The victim's awareness of a specific hunter (only meaningful for its pursuer).
    pub fn awareness_of(&self, hunter: PlayerId) -> PursuitAwareness {
        if self.pursuer == Some(hunter) { self.awareness } else { PursuitAwareness::Unaware }
    }
}

/// Graph invariants: every active player has exactly one target and one pursuer, the
/// assignment forms a single cycle, and (for >= 3 players) no reciprocal pairs.
pub fn validate_cycle(contracts: &[PlayerContract]) -> Result<(), String> {
    let active: Vec<&PlayerContract> = contracts.iter().filter(|c| c.active).collect();
    if active.len() < 2 {
        return Ok(());
    }
    for c in &active {
        let t = c.target.ok_or(format!("{} has no target", c.id))?;
        let tc = active.iter().find(|x| x.id == t).ok_or(format!("{}'s target {t} inactive", c.id))?;
        if tc.pursuer != Some(c.id) {
            return Err(format!("{t}'s pursuer is {:?}, expected {}", tc.pursuer, c.id));
        }
        if active.len() >= 3 && tc.target == Some(c.id) {
            return Err(format!("reciprocal pair {} <-> {t}", c.id));
        }
    }
    // Single cycle.
    let start = active[0].id;
    let mut cur = start;
    for _ in 0..active.len() {
        cur = active.iter().find(|x| x.id == cur).and_then(|x| x.target).ok_or("broken chain")?;
    }
    if cur != start {
        return Err("assignments do not form a single cycle".into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut cur = start;
    for _ in 0..active.len() {
        seen.insert(cur);
        cur = active.iter().find(|x| x.id == cur).and_then(|x| x.target).unwrap();
    }
    if seen.len() != active.len() {
        return Err(format!("cycle covers {} of {} players", seen.len(), active.len()));
    }
    Ok(())
}
