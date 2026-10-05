//! Multiplayer rules that the original single-player game never had to define.
//! Faithful mechanics stay in `dishonored_sim`; this crate decides spawning, scoring,
//! respawn and match flow, and (later) Contracts.

pub mod contracts;
pub mod ffa;
pub mod game;
pub mod mission;

use dishonored_sim::state::PlayerId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MatchPhase {
    #[default]
    Warmup,
    Playing,
    /// Match over; holds for `post_match_time` then resets.
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ScoreEntry {
    pub id: PlayerId,
    pub name: String,
    pub kills: i32,
    pub deaths: i32,
    pub score: i32,
}

/// What clients see of the match (replicated in snapshots).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct MatchView {
    pub mode: String,
    pub phase: MatchPhase,
    pub time_left: f32,
    pub score_limit: i32,
    pub scores: Vec<ScoreEntry>,
    pub winner: Option<PlayerId>,
    pub match_number: u32,
    /// Mission mode: objectives, messages, outcome and stats.
    #[serde(default)]
    pub mission: Option<dishonored_mission::runtime::MissionView>,
}
