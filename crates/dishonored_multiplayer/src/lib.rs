//! Multiplayer rules that the original single-player game never had to define.
//! Faithful mechanics stay in `dishonored_sim`; this crate decides spawning, scoring,
//! respawn and match flow, and (later) Contracts.

pub mod contracts;
pub mod ffa;
pub mod game;
pub mod interact;
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

/// One NPC's attention toward a human player, for the HUD's awareness markers
/// (`DisGFxMoviePlayerHUD::m_AwarenessMarkers`, research/dishonored/HUD.md): the level of the
/// brain's attention entry for that player (`EDisAttentionLevel`, 1 HeadTrack .. 4 Busted).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct AwarenessMark {
    pub id: PlayerId,
    /// The human the attention is toward.
    pub who: PlayerId,
    pub level: u8,
    /// `EAIAwareness` of the pawn (`PlayerState::ai_awareness`), tested against
    /// `m_AllowedAIAwareness`.
    pub awareness: u8,
    /// Where the NPC believes the player is (the attention proxy), Dishonored units.
    pub proxy: [f32; 3],
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
    /// [streaming] The active map's Kismet-streamed sublevels (`LevelStreamingKismet`) that are
    /// not visible now: the renderer hides their instances (and the rest of their actors).
    #[serde(default)]
    pub hidden_levels: Vec<String>,
    /// Mission mode: objectives, messages, outcome and stats.
    #[serde(default)]
    pub mission: Option<dishonored_mission::runtime::MissionView>,
    /// NPC awareness near the human players, for client presentation (the original music
    /// manager, `DisGlobalMusicManager`). Host-filled; empty outside missions.
    #[serde(default)]
    pub ai: Vec<AiPresence>,
    /// NPC attention toward the human players (awareness markers). Host-filled; empty outside
    /// missions.
    #[serde(default)]
    pub marks: Vec<AwarenessMark>,
    /// AI pawns' look-at aims (the vision head's cursor split into the head and torso grids),
    /// every snapshot: the animation turns the visible head the way the AI looks.
    #[serde(default)]
    pub npc_heads: Vec<dishonored_ir::npc_anim::NpcHead>,
    /// Players with their eye at a door keyhole (`StatePlayerMasterHolePeeking`): the view the
    /// client renders (the eye, the field of view, the black fade). Host-filled.
    #[serde(default)]
    pub peeks: Vec<(PlayerId, dishonored_sim::interactables::PeekView)>,
    /// The active map's tripwires (`DisTripwire`): their ends and whether they have tripped.
    #[serde(default)]
    pub tripwires: Vec<TripwireView>,
    /// The active map's projectile launchers (`DisProjectileLauncher`): where they are, whether
    /// they are armed, and the disarm prompt (research/dishonored/systems/interactables.md).
    #[serde(default)]
    pub launchers: Vec<LauncherView>,
    /// The active map's walls of light (`DisWallOfLight`): whether each is shown (powered on;
    /// research/dishonored/systems/walls_of_light.md).
    #[serde(default)]
    pub walls: Vec<WallView>,
    /// Camera lens effects the Kismet spawned (`DisSeqAct_SpawnCameraLensEffect`), the latest
    /// last: the clients draw each once, before their camera.
    #[serde(default)]
    pub lens_effects: Vec<LensEffectView>,
}

/// A spawned camera lens effect: the spawning action (`<package>:<object path>`, the map's
/// `LensEffectDef::action`) and whether "Stop Looping" ended it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LensEffectView {
    pub id: u32,
    pub action: String,
    pub stopped: bool,
}

/// A wall of light as clients see it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WallView {
    /// The actor's object name (`DisWallOfLight_0`) and placed location (same-named walls of
    /// different sublevels are told apart by it).
    pub name: String,
    pub position: [f32; 3],
    pub powered: bool,
    /// While the kill effect (or a shock) runs: where the wall's beams end.
    #[serde(default)]
    pub effect: Option<[f32; 3]>,
}

/// A launcher as clients see it: its place and bounds (the interaction focus), its state and
/// the interactable tweak's prompt and label.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LauncherView {
    /// `<package>:<actor>`.
    pub actor: String,
    pub position: [f32; 3],
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// `m_fMaxDistance` of its interactable tweak.
    pub max_distance: f32,
    /// `m_bArmed` / `m_bHarvested`.
    pub armed: bool,
    pub harvested: bool,
    /// The tripped / deactivated animations have been started (`m_TrippedAnimName`,
    /// `m_DeactivatedAnimName`): a launcher that fired, a launcher that was defused.
    pub fired: bool,
    pub defused: bool,
    /// `m_InteractText` (with its key tag) and `m_Name`.
    pub prompt: String,
    pub label: String,
}

/// A tripwire as clients draw it: the wire between its ends, snapped once tripped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TripwireView {
    /// `<package>:<actor>`.
    pub actor: String,
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub tripped: bool,
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
    /// Hats and masks rolled for the pawn (`m_RandomAccessoryInfo`): (static mesh, socket).
    #[serde(default)]
    pub accessories: Vec<((String, String), String)>,
    /// The pawn tweak the look comes from (`Package.Object`): clients show its interactable
    /// subobject's localized `m_Name` as the pawn's name (kill feed).
    #[serde(default)]
    pub tweak: String,
}

/// The look-at aims of the AI pawns whose head turns (`dishonored_ai::Brain::head_parts`);
/// pawns looking straight ahead are left out.
pub fn npc_heads(brains: &[dishonored_ai::Brain], ai: &dishonored_ir::ai::AiConfig, players: &[dishonored_sim::state::PlayerState]) -> Vec<dishonored_ir::npc_anim::NpcHead> {
    brains
        .iter()
        .filter_map(|b| {
            let me = players.iter().find(|p| p.id == b.id)?;
            let (head, torso) = b.head_parts(me, ai);
            let still = head.iter().chain(torso.iter()).all(|v| v.abs() < 1e-4);
            (!still).then_some(dishonored_ir::npc_anim::NpcHead { id: b.id, head, torso })
        })
        .collect()
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
                // Running to the alarm bell: the alert search stance (weapon drawn, running).
                B::Investigate | B::Search | B::RingAlarm => S::Search,
                B::Combat => S::Combat,
                B::Flee => S::Panic,
                B::Cower => S::Cower,
                B::React | B::Notice | B::Awe => S::Notice,
            };
            (b.id, s)
        })
        .collect()
}
