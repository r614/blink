//! Mode-independent match wrapper used by servers, replays, the runtime and the harness.

use crate::MatchView;
use crate::contracts::{ContractsMatch, ContractsRules, TargetInfo};
use crate::ffa::{FfaMatch, FfaRules, SpawnPoint};
use dishonored_ai::{Crowd, CrowdConfig};
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{PlayerId, WorldState};
use dishonored_sim::{SimConfig, StepOutput};
use dishonored_world::WorldQuery;
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Everything needed to (re)create a match deterministically (stored in replays).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GameSetup {
    Ffa { rules: FfaRules, spawns: Vec<SpawnPoint> },
    Contracts { rules: ContractsRules, spawns: Vec<(Vec3, f32)>, crowd: Option<Crowd> },
    Mission(Box<crate::mission::MissionSetup>),
}

pub enum Game {
    Ffa(FfaMatch),
    Contracts(Box<ContractsMatch>),
    Mission(Box<crate::mission::MissionMatch>),
}

/// Original AI points + guard spawns used to build a crowd.
pub struct CrowdSource<'a> {
    pub points: &'a [(Vec3, f32)],
    pub guard_spawns: &'a [(Vec3, f32)],
    /// `DisFleePointActor` positions (civilian panic destinations).
    pub flee_points: &'a [Vec3],
}

impl GameSetup {
    pub fn contracts(rules: ContractsRules, spawns: Vec<(Vec3, f32)>, source: Option<CrowdSource>, world: &dyn WorldQuery) -> Self {
        let crowd = match (&rules.crowd, source) {
            (Some(cfg), Some(src)) => {
                let mut c = Crowd::populate(cfg, src.points, src.guard_spawns, world);
                c.set_flee_points(src.flee_points, world);
                Some(c)
            }
            _ => None,
        };
        GameSetup::Contracts { rules, spawns, crowd }
    }

    pub fn build(&self, cfg: &SimConfig) -> Game {
        match self {
            GameSetup::Ffa { rules, spawns } => Game::Ffa(FfaMatch::new(rules.clone(), spawns.clone())),
            GameSetup::Contracts { rules, spawns, crowd } => Game::Contracts(Box::new(ContractsMatch::new(rules.clone(), spawns.clone(), crowd.clone(), cfg))),
            GameSetup::Mission(s) => Game::Mission(Box::new(crate::mission::MissionMatch::new((**s).clone(), cfg))),
        }
    }

    pub fn max_players(&self) -> usize {
        match self {
            GameSetup::Ffa { rules, .. } => rules.max_players,
            GameSetup::Contracts { rules, .. } => rules.max_players,
            GameSetup::Mission(_) => 8,
        }
    }
}

impl Game {
    pub fn add_player(&mut self, id: PlayerId, name: &str, archetype: u8, world: &dyn WorldQuery, cfg: &SimConfig) {
        match self {
            Game::Ffa(m) => {
                m.add_player(id, name, cfg);
                if let Some(p) = m.world.player_mut(id) {
                    p.archetype = archetype;
                }
            }
            Game::Contracts(m) => m.add_player(id, name, archetype, world, cfg),
            Game::Mission(m) => m.add_player(id, name, archetype, cfg),
        }
    }
    pub fn remove_player(&mut self, id: PlayerId) {
        match self {
            Game::Ffa(m) => m.remove_player(id),
            Game::Contracts(m) => m.remove_player(id),
            Game::Mission(m) => m.remove_player(id),
        }
    }
    pub fn step(&mut self, inputs: &[(PlayerId, PlayerInput)], world: &dyn WorldQuery, cfg: &SimConfig) -> StepOutput {
        match self {
            Game::Ffa(m) => m.step(inputs, world, cfg),
            Game::Contracts(m) => m.step(inputs, world, cfg),
            Game::Mission(m) => m.step(inputs, world, cfg),
        }
    }
    pub fn world(&self) -> &WorldState {
        match self {
            Game::Ffa(m) => &m.world,
            Game::Contracts(m) => &m.world,
            Game::Mission(m) => &m.world,
        }
    }
    pub fn world_mut(&mut self) -> &mut WorldState {
        match self {
            Game::Ffa(m) => &mut m.world,
            Game::Contracts(m) => &mut m.world,
            Game::Mission(m) => &mut m.world,
        }
    }
    pub fn view(&self) -> &MatchView {
        match self {
            Game::Ffa(m) => &m.view,
            Game::Contracts(m) => &m.view,
            Game::Mission(m) => &m.view,
        }
    }
    pub fn target_info(&self, id: PlayerId) -> Option<TargetInfo> {
        match self {
            Game::Ffa(_) => None,
            Game::Contracts(m) => Some(m.target_info(id)),
            Game::Mission(_) => None,
        }
    }
    pub fn respawn_time_left(&self, id: PlayerId) -> Option<f32> {
        match self {
            Game::Ffa(m) => m.respawn_time_left(id),
            Game::Contracts(_) => None,
            Game::Mission(m) => m.respawn_time_left(id),
        }
    }
    pub fn last_events(&self) -> &[SimEvent] {
        match self {
            Game::Ffa(_) => &[],
            Game::Contracts(m) => &m.last_events,
            Game::Mission(m) => &m.last_events,
        }
    }
}

/// Default crowd for Boyle Ext contracts (archetype indices into the runtime's pool:
/// 0/2 aristocrats for civilians and players, 1 elite guard for guards).
pub fn default_crowd(center: Vec3) -> CrowdConfig {
    CrowdConfig { civilians: 28, guards: 6, center, radius: 4500.0, seed: 0xB0E1, civilian_archetypes: vec![0, 2, 3], guard_archetypes: vec![1] }
}

/// Crowd centre: the medoid of the player spawns (robust to outlying PlayerStarts).
pub fn crowd_center(spawns: &[(Vec3, f32)]) -> Vec3 {
    spawns
        .iter()
        .min_by(|a, b| {
            let sa: f32 = spawns.iter().map(|s| s.0.distance(a.0)).sum();
            let sb: f32 = spawns.iter().map(|s| s.0.distance(b.0)).sum();
            sa.total_cmp(&sb)
        })
        .map(|s| s.0)
        .unwrap_or_default()
}
