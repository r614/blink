//! FFA debug mode (M15): a systems-validation mode, not the product.
//! spawn -> death -> kill credit -> score -> respawn -> timer/score limit -> winner -> reset.

use crate::{MatchPhase, MatchView, ScoreEntry};
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{PlayerId, WorldState};
use dishonored_sim::{SimConfig, StepOutput, spawn_player, step_world};
use dishonored_world::WorldQuery;
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FfaRules {
    pub max_players: usize,
    pub score_limit: i32,
    pub time_limit: f32,
    pub respawn_delay: f32,
    pub warmup_time: f32,
    pub post_match_time: f32,
    /// Minimum players to leave warmup.
    pub min_players: usize,
}

impl Default for FfaRules {
    fn default() -> Self {
        Self { max_players: 8, score_limit: 20, time_limit: 600.0, respawn_delay: 3.0, warmup_time: 5.0, post_match_time: 10.0, min_players: 2 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SpawnPoint {
    pub position: Vec3,
    pub yaw: f32,
}

pub struct FfaMatch {
    pub rules: FfaRules,
    pub world: WorldState,
    pub view: MatchView,
    pub spawns: Vec<SpawnPoint>,
    phase_timer: f32,
    /// Players waiting to respawn: (id, seconds left).
    respawn_queue: Vec<(PlayerId, f32)>,
    spawn_cursor: usize,
    /// Movers and doors as the map starts them.
    start: dishonored_world::StartLayers,
}

impl FfaMatch {
    pub fn new(rules: FfaRules, spawns: Vec<SpawnPoint>) -> Self {
        let view = MatchView { mode: "ffa".into(), phase: MatchPhase::Warmup, time_left: rules.time_limit, score_limit: rules.score_limit, ..Default::default() };
        Self { rules, world: WorldState::default(), view, spawns, phase_timer: 0.0, respawn_queue: Vec::new(), spawn_cursor: 0, start: Default::default() }
    }

    pub fn is_full(&self) -> bool {
        self.view.scores.len() >= self.rules.max_players
    }

    /// Choose the spawn farthest from living players (round-robin tiebreak for determinism).
    fn pick_spawn(&mut self) -> SpawnPoint {
        if self.spawns.is_empty() {
            return SpawnPoint { position: Vec3::new(0.0, 0.0, 200.0), yaw: 0.0 };
        }
        let alive: Vec<Vec3> = self.world.players.iter().filter(|p| p.alive).map(|p| p.position).collect();
        let n = self.spawns.len();
        let mut best = self.spawn_cursor % n;
        let mut best_d = -1.0f32;
        for k in 0..n {
            let i = (self.spawn_cursor + k) % n;
            let d = alive.iter().map(|a| a.distance(self.spawns[i].position)).fold(f32::MAX, f32::min);
            if d > best_d + 1.0 {
                best_d = d;
                best = i;
            }
        }
        self.spawn_cursor = (best + 1) % n;
        self.spawns[best]
    }

    pub fn add_player(&mut self, id: PlayerId, name: &str, cfg: &SimConfig) {
        if self.view.scores.iter().any(|s| s.id == id) {
            return;
        }
        self.view.scores.push(ScoreEntry { id, name: name.to_string(), ..Default::default() });
        let sp = self.pick_spawn();
        spawn_player(&mut self.world, id, sp.position, sp.yaw, cfg);
    }

    pub fn remove_player(&mut self, id: PlayerId) {
        self.view.scores.retain(|s| s.id != id);
        self.respawn_queue.retain(|(p, _)| *p != id);
        dishonored_sim::remove_player(&mut self.world, id);
    }

    fn reset(&mut self, cfg: &SimConfig) {
        for s in self.view.scores.iter_mut() {
            s.kills = 0;
            s.deaths = 0;
            s.score = 0;
        }
        self.view.winner = None;
        self.view.time_left = self.rules.time_limit;
        self.view.match_number += 1;
        self.respawn_queue.clear();
        let ids: Vec<PlayerId> = self.view.scores.iter().map(|s| s.id).collect();
        for id in ids {
            let sp = self.pick_spawn();
            spawn_player(&mut self.world, id, sp.position, sp.yaw, cfg);
        }
        self.world.projectiles.clear();
    }

    /// Advance the match one tick: simulation, then rules.
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
        let out = step_world(&mut self.world, inputs, world, cfg);
        self.phase_timer += dt;
        match self.view.phase {
            MatchPhase::Warmup => {
                if self.view.scores.len() >= self.rules.min_players && self.phase_timer >= self.rules.warmup_time {
                    self.view.phase = MatchPhase::Playing;
                    self.phase_timer = 0.0;
                    self.reset(cfg);
                }
            }
            MatchPhase::Playing => {
                self.view.time_left = (self.view.time_left - dt).max(0.0);
                for e in &out.events {
                    if let SimEvent::Death { victim, killer, .. } = e {
                        if let Some(s) = self.view.scores.iter_mut().find(|s| s.id == *victim) {
                            s.deaths += 1;
                        }
                        match killer {
                            Some(k) if k != victim => {
                                if let Some(s) = self.view.scores.iter_mut().find(|s| s.id == *k) {
                                    s.kills += 1;
                                    s.score += 1;
                                }
                            }
                            _ => {
                                // Suicide / world death costs a point.
                                if let Some(s) = self.view.scores.iter_mut().find(|s| s.id == *victim) {
                                    s.score -= 1;
                                }
                            }
                        }
                    }
                }
                let leader = self.view.scores.iter().max_by_key(|s| (s.score, -s.deaths)).cloned();
                let limit_hit = leader.as_ref().is_some_and(|l| l.score >= self.rules.score_limit);
                if limit_hit || self.view.time_left <= 0.0 {
                    self.view.phase = MatchPhase::Ended;
                    self.view.winner = leader.map(|l| l.id);
                    self.phase_timer = 0.0;
                }
            }
            MatchPhase::Ended => {
                if self.phase_timer >= self.rules.post_match_time {
                    self.view.phase = MatchPhase::Warmup;
                    self.phase_timer = self.rules.warmup_time; // straight back into play
                }
            }
        }
        // Respawns (in every phase, so warmup is playable).
        for e in &out.events {
            if let SimEvent::Death { victim, .. } = e {
                if !self.respawn_queue.iter().any(|(p, _)| p == victim) {
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
            let sp = self.pick_spawn();
            spawn_player(&mut self.world, id, sp.position, sp.yaw, cfg);
        }
        self.view.scores.sort_by(|a, b| b.score.cmp(&a.score).then(a.deaths.cmp(&b.deaths)).then(a.id.cmp(&b.id)));
        out
    }

    pub fn respawn_time_left(&self, id: PlayerId) -> Option<f32> {
        self.respawn_queue.iter().find(|(p, _)| *p == id).map(|(_, t)| *t)
    }
}
