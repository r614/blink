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
    /// The GenericAction animation each AI pawn's behaviour last asked for: (pawn,
    /// `eDisNPCAnimations` index, request serial).
    #[serde(default)]
    pub npc_actions: Vec<(PlayerId, u8, u32)>,
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
    /// Level actors the Kismet highlights (`DisSeqAct_Highlight`): the clients draw each with
    /// its interactable tweak's highlight material over its mesh.
    #[serde(default)]
    pub highlights: Vec<HighlightView>,
    /// The player camera's rain emitter (`DisSeqAct_SetRainEmitter`), if the Kismet set one.
    #[serde(default)]
    pub rain: Option<RainView>,
    /// The camera fade (`SeqAct_CameraFade` -> `PlayerController.ClientSetCameraFade`), if one
    /// is enabled: the screen fade the Kismet plays around its teleports.
    #[serde(default)]
    pub camera_fade: Option<CameraFadeView>,
    /// Each human's crosshair target as the host's step chose it (`dishonored_sim::crosshair`)
    /// with what the HUD's interactions window shows for it. Host-filled in missions.
    #[serde(default)]
    pub focus: Vec<FocusView>,
}

/// What a player's crosshair is on (`m_pCrosshairActor`, chosen by the sim's camera update) and
/// what its interactions window shows: the target's `m_InteractText` (with its key tag) and name,
/// the point the crosshair moves to, and whether the Block button would choke (the choke's
/// `DUI_Special_Choke` line).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FocusView {
    pub player: PlayerId,
    /// The stored crosshair (None: nothing interactable in view).
    pub crosshair: Option<dishonored_sim::crosshair::Crosshair>,
    /// The target's interaction text when the host knows one (a host target's prompt, a prop's
    /// `m_InteractText`, a climbable's); empty: the HUD's text for the target's kind.
    pub text: String,
    /// `m_InteractableName` (the window's title): a talker's or launcher's name.
    pub name: String,
    /// Where the crosshair moves to (world, Dishonored units).
    pub point: Option<[f32; 3]>,
    /// `crosshair::choke_target` holds: a Block press now starts a choke.
    pub choke: bool,
    /// The interaction probe's `m_bCanAssassinate` / `m_bCanDropAssassinate` (0x00BA50F0 /
    /// 0x00BA5420): the Special and SpecialQTE interaction groups.
    #[serde(default)]
    pub assassinate: bool,
    #[serde(default)]
    pub drop_assassinate: bool,
}

/// A level actor highlighted by the Kismet: `<package>:<actor>` (the map's
/// `HighlightDef::actor`) and where the level placed it (same-named actors of different
/// sublevels are told apart by it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HighlightView {
    pub actor: String,
    pub placed: [f32; 3],
}

/// The player camera's rain set by `DisSeqAct_SetRainEmitter` (0x00B9ADF0 -> the camera's
/// 0x00AC0570): the `Emitter` actor whose rain box follows the camera (`<package>:<actor>`, a
/// placed emitter of the map; its template's first module gives the box's `m_Extent`), the
/// action's drop count and impact distances, and the rain direction (`m_pRainRotationActor`'s
/// rotation, else down).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RainView {
    /// Bumped on every `SetRainEmitter` (the camera re-activates the emitter).
    pub id: u32,
    pub emitter: String,
    pub drops: i32,
    pub impacts_min: f32,
    pub impacts_max: f32,
    pub direction: [f32; 3],
    /// The rotation actor's `Rotation` (pitch, yaw, roll; UE3 units) the emitter takes, None
    /// without one (the emitter keeps the default rotator).
    #[serde(default)]
    pub rotation: Option<[i32; 3]>,
}

/// An enabled camera fade (UE3 `Camera` `FadeColor` / `FadeAlpha` / `FadeTime` /
/// `FadeTimeRemaining`, `FadeAmount` = lerp from X to Y as the time runs out): the clients draw
/// `color` over the view at `amount`, advancing it locally until the next snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraFadeView {
    /// Bumped on every new fade.
    pub id: u32,
    /// sRGB colour (`FadeColor`, bytes / 255).
    pub color: [f32; 3],
    /// `FadeAlpha` (from, to).
    pub alpha: [f32; 2],
    pub time: f32,
    pub remaining: f32,
    /// `FadeAmount` now.
    pub amount: f32,
}

impl CameraFadeView {
    /// UE3 `Camera::UpdateCamera` (0x005E5EB0): with time remaining, the remaining time runs down
    /// (not below 0) and, for a timed fade, `FadeAmount = X + (1 - remaining / time) (Y - X)`.
    pub fn advance(&mut self, dt: f32) {
        if self.remaining > 0.0 {
            self.remaining = (self.remaining - dt).max(0.0);
            if self.time > 0.0 {
                self.amount = self.alpha[0] + (1.0 - self.remaining / self.time) * (self.alpha[1] - self.alpha[0]);
            }
        }
    }
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

/// The behaviours' GenericAction chains the pawns' states took (`dishonored_sim::pawn_states`:
/// PlayAnim from the brains' requests), with their request serials, for the presentation.
pub fn npc_actions(world: &dishonored_sim::state::WorldState) -> Vec<(PlayerId, u8, u32)> {
    world.pawn_states.iter().filter(|(_, s)| s.play_serial != 0).map(|(id, s)| (*id, s.play_anim, s.play_serial)).collect()
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
