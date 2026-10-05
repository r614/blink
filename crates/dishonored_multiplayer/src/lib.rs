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
    /// NPC looks for archetypes from `mission::APPEARANCE_BASE` on: mesh parts (package, object
    /// path) resolved by the host from the pawn tweaks.
    #[serde(default)]
    pub appearances: Vec<Look>,
    /// Animation setups referenced by `Look::anim` (sent with the look table).
    #[serde(default)]
    pub anim_profiles: Vec<dishonored_ir::npc_anim::AnimProfile>,
    /// AI pawns' presentation stance (brain state), every snapshot.
    #[serde(default)]
    pub npc_poses: Vec<(PlayerId, dishonored_ir::npc_anim::NpcStance)>,
    /// Active persistent map (changes on level travel; clients reload the renderer).
    #[serde(default)]
    pub map: String,
    /// Mission mode: objectives, messages, outcome and stats.
    #[serde(default)]
    pub mission: Option<dishonored_mission::runtime::MissionView>,
}

/// An NPC look: skeletal mesh parts (package, object path; body first, then head) and material
/// replacements `(part, material index, material object path)` (`DisSeqAct_NPCSetMaterials`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    pub parts: Vec<(String, String)>,
    #[serde(default)]
    pub materials: Vec<(u8, u32, String)>,
    /// Animation setup of the pawn tweak: `MatchView::anim_profiles[anim - 1]` (0: none, the
    /// renderer falls back to the elite-guard sets).
    #[serde(default)]
    pub anim: u16,
}

/// Presentation stance of every AI pawn, from its brain state (animation only: locomotion
/// modifier, panic, cower, combat stance). See `dishonored_ir::npc_anim::NpcStance`.
pub fn npc_poses(brains: &[dishonored_ai::Brain]) -> Vec<(PlayerId, dishonored_ir::npc_anim::NpcStance)> {
    use dishonored_ai::BrainState as B;
    use dishonored_ir::npc_anim::NpcStance as S;
    brains
        .iter()
        .map(|b| {
            let s = match b.state {
                B::Idle | B::Wander | B::WalkToPoint | B::GroupIdle => S::Relaxed,
                B::Patrol | B::Alert => S::Patrol,
                B::Investigate | B::Search => S::Search,
                B::Combat => S::Combat,
                B::Flee => S::Panic,
                B::Cower => S::Cower,
                B::React | B::Notice => S::Notice,
            };
            (b.id, s)
        })
        .collect()
}
