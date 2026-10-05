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

/// One NPC as the music manager sees it (presentation only).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct AiPresence {
    pub id: PlayerId,
    /// Hostile to the human players (guards, or a brain holding a human as enemy).
    pub hostile: bool,
    /// In combat with a human player.
    pub combat: bool,
    /// `Brain::suspicion_level`.
    pub suspicion: u8,
    /// `EDisCombatMusicIntensityObjectType` of the pawn (4 = guard).
    pub music_type: u8,
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
    /// NPC looks for archetypes from `mission::APPEARANCE_BASE` on: mesh parts (package, object
    /// path) resolved by the host from the pawn tweaks.
    #[serde(default)]
    pub appearances: Vec<Look>,
    /// Active persistent map (changes on level travel; clients reload the renderer).
    #[serde(default)]
    pub map: String,
    /// Mission mode: objectives, messages, outcome and stats.
    #[serde(default)]
    pub mission: Option<dishonored_mission::runtime::MissionView>,
    /// NPC awareness near the human players, for client presentation (the original music
    /// manager, `DisGlobalMusicManager`). Host-filled; empty outside missions.
    #[serde(default)]
    pub ai: Vec<AiPresence>,
}

/// An NPC look: skeletal mesh parts (package, object path; body first, then head) and material
/// replacements `(part, material index, material object path)` (`DisSeqAct_NPCSetMaterials`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    pub parts: Vec<(String, String)>,
    #[serde(default)]
    pub materials: Vec<(u8, u32, String)>,
}
