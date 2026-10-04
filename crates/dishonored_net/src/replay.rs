//! Server-side replays: the exact per-tick inputs, joins and leaves plus the
//! authoritative state hash. Re-simulating a replay must reproduce every hash
//! (determinism check) and can drive a spectator view.

use dishonored_multiplayer::ffa::{FfaMatch, FfaRules, SpawnPoint};
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::PlayerId;
use dishonored_sim::{SimConfig, state_hash};
use dishonored_world::WorldQuery;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayHeader {
    pub map: String,
    pub config_hash: u64,
    pub rules: FfaRules,
    pub spawns: Vec<SpawnPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Roster {
    Join(PlayerId, String),
    Leave(PlayerId),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayFrame {
    pub tick: u64,
    /// Roster changes applied before this tick, in order.
    pub roster: Vec<Roster>,
    pub inputs: Vec<(PlayerId, PlayerInput)>,
    pub hash: u64,
    /// Per-player digest after the tick (id, position, health) for divergence diagnosis.
    #[serde(default)]
    pub digest: Vec<(PlayerId, [f32; 3], f32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Replay {
    pub header: ReplayHeader,
    pub frames: Vec<ReplayFrame>,
}

pub struct ReplayRecorder {
    pub replay: Replay,
}

impl ReplayRecorder {
    pub fn new(header: ReplayHeader) -> Self {
        Self { replay: Replay { header, frames: Vec::new() } }
    }
    pub fn push(&mut self, f: ReplayFrame) {
        self.replay.frames.push(f);
    }
}

impl Replay {
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        std::fs::write(path, bincode::serde::encode_to_vec(self, bincode::config::standard())?)?;
        Ok(())
    }
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let b = std::fs::read(path)?;
        Ok(bincode::serde::decode_from_slice(&b, bincode::config::standard())?.0)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VerifyReport {
    pub frames: usize,
    pub mismatches: usize,
    pub first_mismatch_tick: Option<u64>,
}

/// Re-simulate a replay and compare every per-tick state hash.
/// `on_frame` sees the match after each tick (spectating, video capture, metrics).
pub fn resimulate(replay: &Replay, world: &dyn WorldQuery, sim: &SimConfig, mut on_frame: impl FnMut(&FfaMatch, &ReplayFrame)) -> VerifyReport {
    let mut m = FfaMatch::new(replay.header.rules.clone(), replay.header.spawns.clone());
    let mut rep = VerifyReport::default();
    for f in &replay.frames {
        for r in &f.roster {
            match r {
                Roster::Join(id, name) => m.add_player(*id, name, sim),
                Roster::Leave(id) => m.remove_player(*id),
            }
        }
        m.step(&f.inputs, world, sim);
        rep.frames += 1;
        if state_hash(&m.world) != f.hash && rep.mismatches == 0 {
            for (id, pos, hp) in &f.digest {
                if let Some(p) = m.world.player(*id) {
                    let d = p.position.distance(glam::Vec3::from(*pos));
                    if d > 1e-4 || (p.vitals.health - hp).abs() > 1e-4 {
                        eprintln!("tick {}: player {id} diverged: replay pos {:?} hp {hp} vs resim {:?} hp {} mode {:?}", f.tick, pos, p.position, p.vitals.health, p.mode);
                    }
                } else {
                    eprintln!("tick {}: player {id} missing in resim", f.tick);
                }
            }
            eprintln!("inputs at tick {}: {:?}", f.tick, f.inputs);
        }
        if state_hash(&m.world) != f.hash {
            rep.mismatches += 1;
            rep.first_mismatch_tick.get_or_insert(f.tick);
        }
        on_frame(&m, f);
    }
    rep
}
