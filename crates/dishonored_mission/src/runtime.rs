//! Deterministic mission interpreter. Each fixed step the host (a match on a server, offline
//! or in the harness) passes player positions and gameplay events; the interpreter fires
//! matching rules (cascading through flags, tasks, counters and remote events like Kismet
//! does within a frame), and returns commands for the host to apply to NPCs.

use crate::{Action, Behavior, Condition, MissionDef, NpcKind, TargetRef, TaskState, Trigger};
use dishonored_conversation::runner::{ConvEnv, ConvEvent, HookFacts, HookRequest, HookStage, Runner, StartSpec};
use dishonored_conversation::{ConversationDb, Target};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

/// Gameplay facts reported by the host.
#[derive(Debug, Clone, PartialEq)]
pub enum MissionEvent {
    /// An NPC died. `npc` is the mission id for mission NPCs (None for crowd NPCs).
    NpcKilled { npc: Option<String>, by_player: bool, kind: NpcKind },
    NpcAlerted(String),
    PlayerDetected,
    AlarmRaised,
    PlayerKilled(u32),
    /// A player used a usable marker.
    Used(String),
    /// A player talked to an NPC.
    Talked(String),
    /// Players arrived in a persistent map.
    Arrived(String),
    /// A player talked to an NPC (the player becomes the conversation's initiator, who
    /// makes its choices).
    TalkedBy { npc: String, player: u32 },
    /// A player picked option `index` (0-based) of the dialog choice shown to them.
    Choice { player: u32, index: usize },
    /// A player took a pickup: actor name and the abstract item it gave (if any).
    PickedUp { player: u32, pickup: String, item: Option<String> },
    /// An NPC was knocked out (non-lethal). `npc` as in `NpcKilled`.
    NpcKnockedOut { npc: Option<String>, by_player: bool, kind: NpcKind },
}

/// [interp] A matinee playing (replicated: the client's cutscene presentation evaluates the
/// same data at `position`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CutsceneView {
    /// Package and export of the matinee data (`MatineeData` / `InterpData`).
    pub package: String,
    pub export: u32,
    pub position: f32,
    pub length: f32,
    /// Bound objects per group name.
    pub groups: Vec<(String, Vec<CutsceneBound>)>,
    /// Players under matinee control (player group).
    pub players: Vec<u32>,
}

/// [interp] A group binding in a [`CutsceneView`]: a pawn, or a level actor (package, name,
/// class) with its transform when the matinee started and now (location, rotation
/// quaternion xyzw), so clients find and move its meshes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CutsceneBound {
    pub pawn: Option<u32>,
    pub actor: Option<(String, String, String)>,
    pub initial: ([f32; 3], [f32; 4]),
    pub current: Option<([f32; 3], [f32; 4])>,
}

/// A line being spoken (replicated; the HUD shows the subtitle, clients play the VO).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DialogueLineView {
    pub id: u32,
    pub speaker: String,
    /// Mission NPC speaking it (3D VO on its pawn); None with `player` false = unbound.
    pub npc: Option<String>,
    pub player: bool,
    pub text: String,
    /// Wwise event + voice bank (`Bk_<GUID>`) of the original VO.
    pub vo_event: Option<String>,
    pub vo_bank: Option<String>,
    /// Conversation time the line started (seconds) and its length.
    pub start: f32,
    pub duration: f32,
    /// Seconds left at the time of this view.
    pub remaining: f32,
    /// `m_bMainSubtitle` (false for barks).
    pub main: bool,
    pub conversation: String,
    /// The blurb's GUID (`m_iBlurbGUID`, 32 hex digits): names the speaker's FaceFX animation.
    #[serde(default)]
    pub blurb: Option<String>,
    /// `m_fVoiceRadius` (uu): the subtitle shows only within it (`DisSubtitle`, R7).
    #[serde(default)]
    pub voice_radius: f32,
    /// The conversation instance speaking it (0 = a detached line) and the dialog hook that
    /// started that instance (barks): the dominant hook's line is preferred and the others duck.
    #[serde(default)]
    pub instance: u32,
    #[serde(default)]
    pub hook: Option<String>,
}

/// `DisTweaks_ConvSettings` values the client mixes dialogue with (R7): the non-dominant
/// voices' volume scale, its blend speed (per second) and the dominance distance fraction of
/// the voice radius.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct DialogueMix {
    pub non_dominant_scale: f32,
    pub blend_speed: f32,
    pub dominant_pct: f32,
}

impl Default for DialogueMix {
    fn default() -> Self {
        let s = dishonored_conversation::hooks::ConvSettings::default();
        Self { non_dominant_scale: s.non_dominant_volume_scale, blend_speed: s.duck_blend_speed, dominant_pct: s.dominant_max_distance_percent }
    }
}

/// A player choice waiting for input.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DialogueChoiceView {
    pub id: u32,
    pub options: Vec<String>,
    /// Player who chooses (the initiator); None = anyone.
    pub player: Option<u32>,
    pub conversation: String,
}

/// Requests for the host.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Spawn(String),
    Despawn(String),
    Move { npc: String, to: Vec3, run: bool },
    Patrol { npc: String, route: Vec<Vec3> },
    Alert(String),
    Calm(String),
    Sound(String),
    /// Level travel to a tagged start.
    Travel(String),
    SetMaterials { npc: String, body: Vec<(u32, String)>, head: Vec<(u32, String)> },
    ClearMaterials(String),
    /// Give every player these powers and items.
    Equip(crate::Loadout),
}

pub struct StepInput<'a> {
    pub dt: f32,
    /// Active persistent map (zones of markers on other maps are ignored; empty = all).
    pub map: &'a str,
    /// (player id, position, alive)
    pub players: &'a [(u32, Vec3, bool)],
    /// Positions of living mission NPCs (for task targets).
    pub npc_positions: &'a BTreeMap<String, Vec3>,
    pub events: &'a [MissionEvent],
    /// Crowd + mission NPC count (statistics).
    pub total_npcs: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MissionStats {
    pub hostiles_killed: u32,
    pub civilians_killed: u32,
    pub detected: u32,
    pub alarms: u32,
    pub player_deaths: u32,
    pub time: f32,
    pub total_npcs: u32,
    /// NPCs knocked out by players (non-lethal; `clean_hands` stays true).
    #[serde(default)]
    pub knocked_out: u32,
    #[serde(default)]
    pub items_picked_up: u32,
}

impl MissionStats {
    /// Never detected (Dishonored's "Ghost").
    pub fn ghost(&self) -> bool {
        self.detected == 0
    }
    /// Nobody killed by a player (Dishonored's "Clean Hands").
    pub fn clean_hands(&self) -> bool {
        self.hostiles_killed + self.civilians_killed == 0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TaskView {
    pub description: String,
    pub state: TaskState,
    pub optional: bool,
    pub hidden: bool,
}

/// One task marker (`DisSeqAct_UpdateTaskTarget`): where it stands, whether the task is optional
/// (`m_OptionalTaskMarkerSettings`' secondary symbol) and the description it shows
/// (`m_TargetName`, the marker's `_description_mc`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TaskMarkerView {
    pub pos: [f32; 3],
    pub optional: bool,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ObjectiveView {
    pub description: String,
    pub hidden: bool,
    pub tasks: Vec<TaskView>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Outcome {
    pub success: bool,
    pub text: String,
}

/// The mission's statistics record (`DisSeqAct_MissionStatsTracking` input 1, 0x009FE090, stored
/// by 0x009376D0): the tracked values when the mission ended, the chaos entry (1 when the chaos
/// level reached `m_ChaosLevelThreshold`, else 0) and the story flags its summary items test.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MissionStatsRecord {
    /// `m_pMissionStatsTweaks` (object path, e.g. `Twk_MissionStats.Twk_M0_Prison`).
    pub tweak: String,
    /// Tracked stats at the end (`EDisPlayerStat` name, value).
    pub stats: Vec<(String, f32)>,
    /// `ePlayerStat_ChaosLevel` rows read 1 (High) or 0 (Low).
    pub chaos_high: bool,
    /// Story flags set at the end (`m_StoryFlagName`s): the summary items' `m_StoryFlag` tests
    /// (0x00C049F0 / 0x00AB1370 against `m_bStoryFlagValue`).
    pub flags: Vec<String>,
}

impl MissionStatsRecord {
    /// The value a stats row reads (0x009FE090): its stat, plus its additional stat when
    /// `m_bAddOtherStat`; a chaos row reads 1 or 0.
    pub fn row_value(&self, stat: &str, additional: Option<&str>) -> f32 {
        if stat == "ePlayerStat_ChaosLevel" {
            return if self.chaos_high { 1.0 } else { 0.0 };
        }
        let v = |s: &str| self.stats.iter().filter(|(k, _)| k == s).map(|(_, v)| *v).sum::<f32>();
        v(stat) + additional.map(v).unwrap_or(0.0)
    }

    /// Whether a story flag was set when the record was made.
    pub fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f.eq_ignore_ascii_case(name))
    }
}

impl MissionView {
    /// Add to a tracked stat (0x00AA97E0).
    pub fn add_player_stat(&mut self, stat: &str, amount: f32) {
        match self.player_stats.iter_mut().find(|(k, _)| k == stat) {
            Some((_, v)) => *v += amount,
            None => self.player_stats.push((stat.to_string(), amount)),
        }
    }

    /// A tracked stat's value (0 when never raised).
    pub fn player_stat(&self, stat: &str) -> f32 {
        self.player_stats.iter().find(|(k, _)| k == stat).map(|(_, v)| *v).unwrap_or(0.0)
    }
}

/// A Kismet screen grade: the ArkPp settings the action overrides (by `m_*` name, as the
/// level's post settings carry them) blended in by `weight` (0..1).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PostFxView {
    pub values: Vec<(String, f32)>,
    pub vectors: Vec<(String, [f32; 3])>,
    pub weight: f32,
}

/// Replicated to clients (inside the match view) and drawn by the HUD.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MissionView {
    pub title: String,
    pub briefing: String,
    pub objectives: Vec<ObjectiveView>,
    /// Recent HUD messages (id increases; clients show new ids for a few seconds).
    pub messages: Vec<(u32, String)>,
    /// Latest target notification (id, title, text).
    pub notification: Option<(u32, String, String)>,
    /// Latest location discovery (id, name): `DisSeqAct_ShowLocationDiscovery`.
    #[serde(default)]
    pub location: Option<(u32, String)>,
    /// Latest tutorial line (id, text, seconds): `DisSeqAct_SetTutorialMessage`.
    #[serde(default)]
    pub tutorial: Option<(u32, String, f32)>,
    /// Latest tutorial window (id, picture, text): a mission clue.
    #[serde(default)]
    pub tutorial_window: Option<(u32, u8, String)>,
    /// Every visible active task with a target (`m_TaskMarkers`): the HUD's task markers.
    #[serde(default)]
    pub task_markers: Vec<TaskMarkerView>,
    pub outcome: Option<Outcome>,
    pub stats: MissionStats,
    /// Chaos "High" on the mission-stats screen: the chaos level reached the stats op's
    /// `m_ChaosLevelThreshold` (0x009FE090).
    pub high_chaos: bool,
    /// The game's chaos score and level (`DisDarknessManager`).
    #[serde(default)]
    pub darkness_score: i32,
    #[serde(default)]
    pub darkness_level: i32,
    /// The mission's tracked player statistics (`EDisPlayerStat` name, value; the pawn's list at
    /// +0xB7C, `dishonored_sim::player_stats`): what the player raised since the last
    /// `DisSeqAct_MissionStatsTracking` start, or since the mission started.
    #[serde(default)]
    pub player_stats: Vec<(String, f32)>,
    /// What `DisSeqAct_MissionStatsTracking` input 1 recorded for the stats screen.
    #[serde(default)]
    pub stats_record: Option<MissionStatsRecord>,
    /// `DisSeqAct_ShowMissionStats` activations so far: the clients open the end-of-mission
    /// statistics screen when this grows.
    #[serde(default)]
    pub stats_shown: u32,
    /// `DisSeqAct_OpenJournal` (0x00BD9E10): (activations so far, `m_Tab`); the clients open the
    /// journal at that tab when the count grows.
    #[serde(default)]
    pub journal_open: Option<(u32, i32)>,
    /// World position the HUD marker points at (active task target).
    pub target: Option<[f32; 3]>,
    pub sounds: Vec<(u32, String)>,
    /// Kismet-controlled ambient sound actors ("package:actor name", playing).
    #[serde(default)]
    pub ambient: Vec<(String, bool)>,
    /// Kismet `SeqAct_Toggle`s of scene capture actors (`SceneCaptureActor.OnToggle`): (`package:actor`,
    /// the last explicit On / Off, whether an odd number of Toggle impulses followed it). The renderer
    /// combines them with the capture component's `bEnabled` from the level (`reflect.rs`).
    #[serde(default)]
    pub captures: Vec<(String, Option<bool>, bool)>,
    /// Usable markers: (position, prompt).
    #[serde(default)]
    pub usables: Vec<([f32; 3], String)>,
    /// Talkable mission NPCs: (npc id, prompt).
    #[serde(default)]
    pub talkers: Vec<(String, String)>,
    /// Display names of the mission NPCs that have one (npc id, name): the interaction window's
    /// title over a talker.
    #[serde(default)]
    pub npc_names: Vec<(String, String)>,
    /// Mission NPC id -> pawn id (set by the host), so clients can find talkers.
    #[serde(default)]
    pub npc_ids: Vec<(String, u32)>,
    /// Open dialog choices: (player id, options). Picked with the CHOICE_n buttons.
    /// Scripted choices (travel doors) and conversation choices share this list.
    #[serde(default)]
    pub choices: Vec<(u32, Vec<String>)>,
    /// Lines being spoken in original conversations (oldest first).
    #[serde(default)]
    pub dialogue: Vec<DialogueLineView>,
    #[serde(default)]
    pub dialogue_mix: DialogueMix,
    /// Player choice waiting for input.
    #[serde(default)]
    pub choice: Option<DialogueChoiceView>,
    /// [interp] Matinees playing.
    #[serde(default)]
    pub cutscenes: Vec<CutsceneView>,
    /// The Kismet screen grade (`DisSeqAct_UberPostProcess`) while it shows.
    #[serde(default)]
    pub post_fx: Option<PostFxView>,
    /// The Kismet's screen effects running (`DisSeqAct_PostProcess`: `Epp_Knocked`,
    /// `Epp_Weepers`, `Epp_MaskOn`), drawn by the post-process graph (`ppgraph`).
    #[serde(default)]
    pub post_effects: Vec<String>,
    /// `DisDoor`s the Kismet opened or closed (`DisSeqAct_Door`): ("package:actor",
    /// `EDisDoorState` 0 closed, 1 opened CW, 2 opened CCW). Other doors keep their level state.
    #[serde(default)]
    pub doors: Vec<(String, u8)>,
    /// Level actors the Kismet moved (matinees, attachments, teleports), where they are now.
    #[serde(default)]
    pub movers: Vec<MoverView>,
}

/// A level actor (mover, door, usable) away from where the map placed it: the actor's current
/// transform times the inverse of its placed one (Dishonored world space).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MoverView {
    /// "package:actor".
    pub actor: String,
    /// The actor's placed location (tells same-named actors of different sublevels apart).
    pub placed: [f32; 3],
    /// Delta translation and rotation (quaternion xyzw).
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Debug, Clone, PartialEq)]
enum Ev {
    Start,
    Timer,
    Enter(String),
    Leave(String),
    NpcKilled(String),
    AnyKill,
    NpcAlerted(String),
    PlayerDetected,
    AlarmRaised,
    PlayerKilled,
    Flag(String, bool),
    Task(String, TaskState),
    Counter(String),
    Remote(String),
    Use(String),
    Talk(String),
    Arrived(String),
    ConvOutput { name: String, oneshot: Option<String> },
    ConvEnded { name: String, label: String },
    PickedUp { pickup: String, item: Option<String> },
    NpcKnockedOut(String),
}

/// The mission as seen by the conversation runner.
struct Env<'a> {
    flags: &'a mut BTreeMap<String, bool>,
    changed: Vec<(String, bool)>,
    alive: &'a BTreeSet<String>,
    suspicion: &'a BTreeMap<String, u8>,
    npc_pos: &'a BTreeMap<String, Vec3>,
    players: &'a [(u32, Vec3, bool)],
    items: &'a BTreeMap<String, i32>,
    darkness_level: i32,
    /// [ai] Positions of every NPC that can bark (mission and original), set by the host.
    speaker_pos: &'a BTreeMap<String, Vec3>,
    dead: &'a BTreeSet<String>,
    facts: &'a BTreeMap<String, HookFacts>,
}

impl Env<'_> {
    /// Count of an abstract item by object path or name (`Package.Name` -> `Name`).
    fn abstract_count(&self, item: &str) -> i32 {
        let short = item.rsplit(['.', ':']).next().unwrap_or(item);
        self.items.iter().filter(|(k, _)| k.eq_ignore_ascii_case(short)).map(|(_, v)| *v).sum()
    }
}

impl ConvEnv for Env<'_> {
    fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }
    fn set_flag(&mut self, name: &str, value: bool) {
        if self.flags.insert(name.to_string(), value) != Some(value) {
            self.changed.push((name.to_string(), value));
        }
    }
    fn speaker_available(&self, npc: Option<&str>) -> bool {
        npc.is_none_or(|n| self.alive.contains(n))
    }
    fn suspicion(&self, npc: Option<&str>) -> u8 {
        npc.and_then(|n| self.suspicion.get(n).copied()).unwrap_or(0)
    }
    fn has_item(&self, _player: Option<u32>, item: &str, _equipped: bool) -> bool {
        item.contains("Sword") || self.abstract_count(item) > 0
    }
    fn item_count(&self, _player: Option<u32>, item: &str) -> i32 {
        self.abstract_count(item) + item.contains("Sword") as i32
    }
    fn darkness_level(&self) -> i32 {
        self.darkness_level
    }
    fn distance(&self, player: Option<u32>, npc: Option<&str>) -> Option<f32> {
        let np = self.pos_of(npc?)?;
        let p = self.players.iter().find(|p| Some(p.0) == player || player.is_none())?;
        Some(p.1.distance(np))
    }
    fn npc_dead(&self, npc: &str) -> bool {
        self.dead.contains(npc)
    }
    /// The nearest living player (the original has one listener, the local player pawn).
    fn listener_distance(&self, npc: &str) -> Option<f32> {
        let np = self.pos_of(npc)?;
        self.players.iter().filter(|p| p.2).map(|p| p.1.distance(np)).min_by(f32::total_cmp)
    }
    fn player_distance_2d(&self, npc: &str) -> Option<f32> {
        let np = self.pos_of(npc)?;
        self.players.iter().filter(|p| p.2).map(|p| p.1.truncate().distance(np.truncate())).min_by(f32::total_cmp)
    }
    fn hook_facts(&self, npc: &str, _initiator: Option<u32>) -> HookFacts {
        self.facts.get(npc).copied().unwrap_or_default()
    }
}

impl Env<'_> {
    fn pos_of(&self, npc: &str) -> Option<Vec3> {
        self.speaker_pos.get(npc).or_else(|| self.npc_pos.get(npc)).copied()
    }
}

fn actor_matches(reference: &str, package: &str, name: &str) -> bool {
    match reference.split_once(':') {
        Some((p, n)) => p.eq_ignore_ascii_case(package) && n.eq_ignore_ascii_case(name),
        None => reference.eq_ignore_ascii_case(name),
    }
}

/// `ConversationEnded(x)`: label, `OneShot/label` or the full `Package:OneShot/label`.
fn conv_name_matches(want: &str, name: &str, label: &str) -> bool {
    want.eq_ignore_ascii_case(name) || (!label.is_empty() && want.eq_ignore_ascii_case(label)) || name.to_ascii_lowercase().ends_with(&format!(":{}", want.to_ascii_lowercase()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionState {
    pub def: MissionDef,
    /// Resolved marker positions and radii.
    pub markers: BTreeMap<String, (Vec3, f32)>,
    /// Map of each marker (empty = the mission's start map). Set by the host.
    pub marker_map: BTreeMap<String, String>,
    current_map: String,
    task_state: Vec<Vec<TaskState>>,
    task_hidden: Vec<Vec<bool>>,
    obj_hidden: Vec<bool>,
    /// `m_ObjectiveState` Completed (an objective completed with its last task, 0x00ACBC70).
    #[serde(default)]
    obj_completed: Vec<bool>,
    flags: BTreeMap<String, bool>,
    counters: BTreeMap<String, i32>,
    rule_fired: Vec<bool>,
    rule_enabled: Vec<bool>,
    every_next: Vec<f32>,
    delayed: Vec<(f32, Vec<Action>)>,
    time: f32,
    started: bool,
    inside: BTreeSet<(String, u32)>,
    pub npc_alive: BTreeSet<String>,
    detected_any: bool,
    alarm: bool,
    next_id: u32,
    /// Enabled interactions: marker -> prompt, npc -> prompt.
    pub usable: BTreeMap<String, String>,
    pub talk: BTreeMap<String, String>,
    pub view: MissionView,
    /// Original conversations (when the mission references any and the host loaded them).
    pub conv: Option<Runner>,
    /// Every conversation event so far (tools and the harness read it; capped).
    pub conv_log: Vec<ConvEvent>,
    /// Conversation references that failed to start, with the reason.
    pub conv_errors: Vec<String>,
    /// Mission NPC -> 0 calm / 1 suspicious / 2 combat or fleeing (set by the host).
    pub npc_suspicion: BTreeMap<String, u8>,
    last_players: Vec<(u32, Vec3, bool)>,
    last_npc_pos: BTreeMap<String, Vec3>,
    /// `DDH_PLAYER_APPROACHES` hooks of one-shots whose me-speaker a mission NPC stands in
    /// for: (npc, one-shot, hook node, close/medium/far distances).
    approach: Vec<(String, u32, u32, [f32; 3])>,
    /// Distance tracker per (NPC, approach hook node), as `DisConv_Hook_Distance_InGameData`.
    approach_track: BTreeMap<(String, u32), dishonored_conversation::hooks::DistanceHook>,
    /// [ai] Dialog hooks the AI fired this step (barks): (NPC binding name, speaker actor,
    /// `DDH_*` hook, initiator, the pawn's generic voice data path if it has one, the pawn).
    /// Queued by the host, requested in `step` (`Runner::request_hook`) and fired by the bark
    /// manager's tick (`Runner::tick_hooks`).
    pub ai_hooks: Vec<(String, String, String, Option<u32>, Option<String>, Option<u32>)>,
    /// [ai] Where every NPC that can bark stands (mission NPCs and original spawns, by bark
    /// name), the dead ones, and what their hook nodes read from their AI. Set by the host
    /// before each step.
    #[serde(skip)]
    pub speaker_pos: BTreeMap<String, Vec3>,
    #[serde(default)]
    pub dead_speakers: BTreeSet<String>,
    #[serde(skip)]
    pub hook_facts: BTreeMap<String, HookFacts>,
    /// Polled player hooks (`DDH_PLAYER_LOITERS(_RADIAL)`, `DDH_PLAYER_LOOKAT(_RADIAL)`) of
    /// one-shots whose me-speaker a mission NPC stands in for: (npc, tree, hook).
    #[serde(default)]
    player_hooks: Vec<(String, u32, String)>,
    /// [ai] Dialog hook requests and their outcome (`ConvEvent::Hook`; tools drain it; capped).
    #[serde(skip)]
    pub hook_log: Vec<ConvEvent>,
    /// [ai] Barks that started a conversation so far.
    pub ai_hooks_fired: u32,
    /// [ai] Barks asked for so far (started or not).
    #[serde(default)]
    pub ai_hooks_requested: u32,
    /// [ai] Why bark requests did not start (diagnostics).
    #[serde(skip)]
    pub ai_hook_rejections: BTreeMap<String, u32>,
    /// [kismet] Abstract items the players hold (`DisSeqAct_AddAbstractItem` /
    /// `RemoveAbstractItem`, by object name): the conversations' `DisConv_HasAbstractItem` checks.
    pub items: BTreeMap<String, i32>,
    /// The game's chaos score and level (`DisDarknessManager`, kept by the host; set before
    /// each step): `DisConv_CompareDarknessLevel` reads the level.
    #[serde(default)]
    pub darkness_score: i32,
    #[serde(default)]
    pub darkness_level: i32,
    /// `DisSeqAct_MissionStatsTracking.m_ChaosLevelThreshold` of the last stats op the Kismet
    /// ran (else the mission's `chaos_level_threshold`): the HUD's High / Low.
    #[serde(default)]
    pub chaos_threshold: Option<i32>,
    /// The objective manager's task notifications since the host last drained them (the original
    /// dispatcher 0x00AD4F60: kind 3 `TaskActivated` 0x00AD1080, kind 5 `TaskStateChanged`
    /// 0x00AD1320), in the order the task code raised them.
    #[serde(skip)]
    pub task_log: Vec<TaskNote>,
}

/// A task notification (`DishonoredTask` -> the objective manager): what the original raises
/// through 0x00BBE6A0 when a task changes.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskNote {
    /// Kind 3: the task became active and visible (`SetState(Active)` 0x00AC17B0, or its hidden
    /// flag cleared while active, 0x00AC63C0). Path `Objective.Task`.
    Activated(String),
    /// Kind 5: the task's state changed (0x00AC17B0 raises it after Activated); the state is
    /// the new `EDisTaskState`.
    Changed(String, TaskState),
    /// Kind 1: the objective (its id) completed (0x00ACBC70).
    ObjectiveCompleted(String),
}

impl MissionState {
    /// `markers`: positions resolved by the host (actor names looked up in the level).
    pub fn new(def: MissionDef, markers: BTreeMap<String, (Vec3, f32)>) -> Self {
        let task_state = def.objectives.iter().map(|o| o.tasks.iter().map(|t| t.initial).collect()).collect();
        let task_hidden = def.objectives.iter().map(|o| o.tasks.iter().map(|t| t.hidden).collect()).collect();
        let obj_hidden: Vec<bool> = def.objectives.iter().map(|o| o.hidden).collect();
        let mut flags: BTreeMap<String, bool> = def.flags.iter().map(|f| (f.clone(), false)).collect();
        for f in &def.flags_set {
            flags.insert(f.clone(), true);
        }
        let counters = def.counters.iter().map(|c| (c.clone(), 0)).collect();
        let n = def.rules.len();
        let rule_enabled = def.rules.iter().map(|r| !r.disabled).collect();
        let every_next = def.rules.iter().map(|r| if let Trigger::Every(s) = r.on { s } else { 0.0 }).collect();
        let npc_alive = def.npcs.iter().filter(|n| n.spawn_at_start).map(|n| n.id.clone()).collect();
        let view = MissionView { title: def.title.clone(), briefing: def.briefing.clone(), ..Default::default() };
        let usable = def.markers.iter().filter_map(|m| m.usable.clone().map(|p| (m.id.clone(), p))).collect();
        let talk = def.npcs.iter().filter_map(|n| n.talk.clone().or_else(|| n.talk_conversation.as_ref().map(|_| "Talk".to_string())).map(|p| (n.id.clone(), p))).collect();
        let mut s = MissionState {
            def,
            markers,
            marker_map: BTreeMap::new(),
            current_map: String::new(),
            task_state,
            task_hidden,
            obj_completed: vec![false; obj_hidden.len()],
            obj_hidden,
            flags,
            counters,
            rule_fired: vec![false; n],
            rule_enabled,
            every_next,
            delayed: Vec::new(),
            time: 0.0,
            started: false,
            inside: BTreeSet::new(),
            npc_alive,
            detected_any: false,
            alarm: false,
            next_id: 1,
            usable,
            talk,
            view,
            conv: None,
            conv_log: Vec::new(),
            conv_errors: Vec::new(),
            npc_suspicion: BTreeMap::new(),
            last_players: Vec::new(),
            last_npc_pos: BTreeMap::new(),
            approach: Vec::new(),
            approach_track: BTreeMap::new(),
            ai_hooks: Vec::new(),
            speaker_pos: BTreeMap::new(),
            dead_speakers: BTreeSet::new(),
            hook_facts: BTreeMap::new(),
            player_hooks: Vec::new(),
            hook_log: Vec::new(),
            ai_hooks_fired: 0,
            ai_hooks_requested: 0,
            ai_hook_rejections: BTreeMap::new(),
            items: BTreeMap::new(),
            darkness_score: 0,
            darkness_level: 0,
            chaos_threshold: None,
            task_log: Vec::new(),
        };
        s.refresh_view(&BTreeMap::new());
        s
    }

    /// Attach the original dialog data the mission references (loaded by the host from the
    /// install). The runner seed derives from the mission title, so replays match.
    pub fn set_conversations(&mut self, db: Arc<ConversationDb>) {
        let seed = self.def.title.bytes().fold(0xC0FFEEu64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
        self.approach.clear();
        self.player_hooks.clear();
        for n in &self.def.npcs {
            let Some(actor) = n.speaker_actor() else { continue };
            for (oi, o) in db.oneshots.iter().enumerate() {
                let me = o.speakers.first().and_then(|s| s.actor.as_deref());
                if !me.is_some_and(|m| actor_matches(actor, &o.package, m)) {
                    continue;
                }
                let Some(t) = o.tree else { continue };
                for h in &db.trees[t as usize].hooks {
                    let node = db.node(*h);
                    if let dishonored_conversation::NodeKind::Hook { hook, distances: Some(d), .. } = &node.kind {
                        if (hook == "DDH_PLAYER_APPROACHES" || hook == "DDH_PLAYER_APPROACHES_RADIAL") && node.outputs.iter().any(|o| o.is_some()) {
                            self.approach.push((n.id.clone(), oi as u32, *h, *d));
                        }
                    }
                    if let dishonored_conversation::NodeKind::Hook { hook, class: dishonored_conversation::model::HookClass::PlayerLoiter { .. } | dishonored_conversation::model::HookClass::PlayerLookAt { .. }, .. } = &node.kind {
                        if !self.player_hooks.iter().any(|p| p.0 == n.id && p.1 == t && &p.2 == hook) {
                            self.player_hooks.push((n.id.clone(), t, hook.clone()));
                        }
                    }
                }
            }
        }
        self.conv = Some(Runner::new(db, seed));
    }

    /// Players coming closer to an NPC enter its one-shot's approach hook at the band's
    /// output (close / medium / far), with the original's `DisConv_Hook_Distance` edge rules
    /// ([`dishonored_conversation::hooks::DistanceHook`]; the "aware" check of
    /// `DDH_PLAYER_APPROACHES` is not modelled: the NPC counts as aware of the player).
    fn approach_hooks(&mut self, queue: &mut VecDeque<Ev>) {
        for k in 0..self.approach.len() {
            let (npc, oneshot, hook, d) = self.approach[k].clone();
            let Some(np) = self.last_npc_pos.get(&npc).copied() else { continue };
            let nearest = self.last_players.iter().filter(|p| p.2).map(|p| (p.0, p.1.distance(np))).min_by(|a, b| a.1.total_cmp(&b.1));
            let Some(r) = &self.conv else { return };
            let Some((_, dist)) = nearest else { continue };
            let outs = &r.db.node(hook).outputs;
            let linked = [0, 1, 2].map(|i| outs.get(i).copied().flatten().is_some());
            let Some(band) = self.approach_track.entry((npc.clone(), hook)).or_default().update(dist, d, linked) else { continue };
            let target = Target::Hook { oneshot, hook, output: band };
            let bindings = self.conv_bindings(&target, &[], Some(&npc));
            let spec = StartSpec { target, initiator: nearest.map(|n| n.0), bindings, force: false };
            self.with_conv(queue, move |runner, env| {
                runner.start(spec, env);
            });
        }
    }

    /// [ai] AI barks: each request goes through `FireHook` (`Runner::request_hook`: the NPC's
    /// trees are the trees of the one-shots whose first speaker is its actor, then the generic
    /// trees of its voice, `m_pGenericDialogData_Cached` 0x00C8D2C0), the polled player hooks
    /// ask too, then the bark manager's tick (`Runner::tick_hooks`) fires the winners.
    fn fire_ai_hooks(&mut self, dt: f32, queue: &mut VecDeque<Ev>) {
        let hooks = std::mem::take(&mut self.ai_hooks);
        let Some(r) = &self.conv else { return };
        let db = r.db.clone();
        let mut reqs = Vec::new();
        for (npc, actor, hook, initiator, voice, owner) in hooks {
            self.ai_hooks_requested += 1;
            let mut trees: Vec<u32> = db
                .oneshots
                .iter()
                .filter(|o| o.speakers.first().and_then(|s| s.actor.as_deref()).is_some_and(|m| actor_matches(&actor, &o.package, m)))
                .filter_map(|o| o.tree)
                .collect();
            let oneshot_trees = trees.len();
            if let Some(v) = voice.as_ref().and_then(|v| db.voice_trees.get(v)) {
                trees.extend(v.iter().copied().filter(|t| !trees.contains(t)).collect::<Vec<_>>());
            }
            if trees.is_empty() {
                continue;
            }
            reqs.push(HookRequest { npc, trees, oneshot_trees, hook, output: None, initiator, owner });
        }
        // Player hooks: asked every step; the node's own state machine decides (R5, F12).
        let nearest = |np: Vec3| self.last_players.iter().filter(|p| p.2).min_by(|a, b| a.1.distance(np).total_cmp(&b.1.distance(np))).map(|p| p.0);
        for (npc, tree, hook) in &self.player_hooks {
            let Some(np) = self.speaker_pos.get(npc).or_else(|| self.last_npc_pos.get(npc)).copied() else { continue };
            reqs.push(HookRequest { npc: npc.clone(), trees: vec![*tree], oneshot_trees: 1, hook: hook.clone(), output: None, initiator: nearest(np), owner: None });
        }
        self.with_conv(queue, |runner, env| {
            for req in reqs {
                let _ = runner.request_hook(req, env);
            }
            runner.tick_hooks(dt, env);
        });
    }
    fn conv_bindings(&self, target: &Target, explicit: &[String], talker: Option<&str>) -> Vec<Option<String>> {
        let Some(r) = &self.conv else { return Vec::new() };
        let oneshot = match target {
            Target::Conversation { oneshot, .. } => *oneshot,
            Target::Hook { oneshot, .. } => Some(*oneshot),
            Target::TreeHook { tree, .. } => r.db.trees[*tree as usize].oneshot,
        };
        let pick = |k: usize| explicit.get(k).filter(|s| !s.is_empty()).cloned();
        let Some(o) = oneshot else { return (0..explicit.len().max(1)).map(|k| pick(k).or_else(|| (k == 0).then(|| talker.map(str::to_string)).flatten())).collect() };
        let os = &r.db.oneshots[o as usize];
        (0..os.speakers.len().max(1))
            .map(|k| {
                if let Some(e) = pick(k) {
                    return Some(e);
                }
                let auto = os.speakers.get(k).and_then(|sp| sp.actor.as_ref()).and_then(|actor| self.def.npcs.iter().find(|n| n.speaker_actor().is_some_and(|a| actor_matches(a, &os.package, actor)))).map(|n| n.id.clone());
                auto.or_else(|| (k == 0).then(|| talker.map(str::to_string)).flatten())
            })
            .collect()
    }

    /// Start an original conversation by reference (mission action, NPC talk).
    fn start_conversation(&mut self, reference: &str, speakers: &[String], initiator: Option<u32>, talker: Option<&str>, force: bool, queue: &mut VecDeque<Ev>) {
        let Some(r) = &self.conv else {
            self.conv_errors.push(format!("{reference}: no dialog data loaded"));
            return;
        };
        let target = match r.db.resolve(reference) {
            Ok(t) => t,
            Err(e) => {
                self.conv_errors.push(e);
                return;
            }
        };
        let bindings = self.conv_bindings(&target, speakers, talker);
        let initiator = initiator.or_else(|| self.last_players.iter().find(|p| p.2).map(|p| p.0));
        let spec = StartSpec { target, initiator, bindings, force };
        self.with_conv(queue, move |runner, env| {
            runner.start(spec, env);
        });
    }

    /// Run something against the conversation runner, then feed its events into the rule
    /// queue (Kismet-style cascade within the step).
    fn with_conv(&mut self, queue: &mut VecDeque<Ev>, f: impl FnOnce(&mut Runner, &mut dyn ConvEnv)) {
        let Some(runner) = self.conv.as_mut() else { return };
        let mut env = Env { flags: &mut self.flags, changed: Vec::new(), alive: &self.npc_alive, suspicion: &self.npc_suspicion, npc_pos: &self.last_npc_pos, players: &self.last_players, items: &self.items, darkness_level: self.darkness_level, speaker_pos: &self.speaker_pos, dead: &self.dead_speakers, facts: &self.hook_facts };
        f(runner, &mut env);
        let changed = env.changed;
        let events = runner.take_events();
        for (name, v) in changed {
            queue.push_back(Ev::Flag(name, v));
        }
        for e in events {
            match &e {
                ConvEvent::Output { name, oneshot, .. } => queue.push_back(Ev::ConvOutput { name: name.clone(), oneshot: oneshot.clone() }),
                ConvEvent::Ended { name, label, .. } => queue.push_back(Ev::ConvEnded { name: name.clone(), label: label.clone() }),
                ConvEvent::Hook { result, .. } => {
                    match result {
                        Ok(HookStage::Fired) => self.ai_hooks_fired += 1,
                        Ok(HookStage::Queued) => {}
                        Err(e) => *self.ai_hook_rejections.entry(format!("{e:?}")).or_default() += 1,
                    }
                    // Hook requests (one per ambient bark tick) have their own log.
                    self.hook_log.push(e);
                    continue;
                }
                _ => {}
            }
            self.conv_log.push(e);
        }
        let n = self.conv_log.len();
        if n > 4096 {
            self.conv_log.drain(..n - 4096);
        }
        let n = self.hook_log.len();
        if n > 4096 {
            self.hook_log.drain(..n - 4096);
        }
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.view.outcome.as_ref()
    }

    pub fn task(&self, path: &str) -> Option<TaskState> {
        let (o, t) = self.def.task_index(path)?;
        Some(self.task_state[o][t])
    }

    pub fn flag(&self, f: &str) -> bool {
        self.flags.get(f).copied().unwrap_or(false)
    }

    /// The names of the flags set now.
    pub fn flags_set(&self) -> Vec<String> {
        self.flags.iter().filter(|(_, v)| **v).map(|(k, _)| k.clone()).collect()
    }

    /// Initial NPC route/behaviour commands (call once after spawning mission NPCs).
    pub fn initial_commands(&self) -> Vec<Command> {
        let mut out = Vec::new();
        for n in &self.def.npcs {
            if let Behavior::Patrol(route) = &n.behavior {
                let r: Vec<Vec3> = route.iter().filter_map(|m| self.markers.get(m).map(|x| x.0)).collect();
                if !r.is_empty() {
                    out.push(Command::Patrol { npc: n.id.clone(), route: r });
                }
            }
        }
        out
    }

    pub fn step(&mut self, input: &StepInput) -> Vec<Command> {
        let mut out = Vec::new();
        if self.view.outcome.is_some() {
            return out;
        }
        self.time += input.dt;
        self.current_map = input.map.to_string();
        self.view.stats.time = self.time;
        self.view.stats.total_npcs = input.total_npcs;
        self.last_players = input.players.to_vec();
        self.last_npc_pos = input.npc_positions.clone();
        let mut queue: VecDeque<Ev> = VecDeque::new();
        if !self.started {
            self.started = true;
            queue.push_back(Ev::Start);
        }
        queue.push_back(Ev::Timer);
        // Zones (per player, edge-triggered).
        let mut now: BTreeSet<(String, u32)> = BTreeSet::new();
        for (id, pos, alive) in input.players {
            if !alive {
                continue;
            }
            for (m, (c, r)) in &self.markers {
                if !input.map.is_empty() && self.marker_map.get(m).is_some_and(|mm| !mm.eq_ignore_ascii_case(input.map)) {
                    continue;
                }
                if (pos.truncate() - c.truncate()).length() <= *r && (pos.z - c.z).abs() <= r.max(200.0) {
                    now.insert((m.clone(), *id));
                }
            }
        }
        for k in now.difference(&self.inside) {
            queue.push_back(Ev::Enter(k.0.clone()));
        }
        for k in self.inside.difference(&now) {
            queue.push_back(Ev::Leave(k.0.clone()));
        }
        self.inside = now;
        for e in input.events {
            match e {
                MissionEvent::NpcKilled { npc, by_player, kind } => {
                    // Specific before generic, so rules on this NPC see the kill first.
                    if let Some(n) = npc {
                        self.npc_alive.remove(n);
                        queue.push_back(Ev::NpcKilled(n.clone()));
                    }
                    if *by_player {
                        match kind {
                            NpcKind::Guard => self.view.stats.hostiles_killed += 1,
                            NpcKind::Civilian => self.view.stats.civilians_killed += 1,
                        }
                        queue.push_back(Ev::AnyKill);
                    }
                }
                MissionEvent::NpcAlerted(n) => queue.push_back(Ev::NpcAlerted(n.clone())),
                MissionEvent::PlayerDetected => {
                    self.view.stats.detected += 1;
                    self.detected_any = true;
                    queue.push_back(Ev::PlayerDetected);
                }
                MissionEvent::AlarmRaised => {
                    self.view.stats.alarms += 1;
                    if !self.alarm {
                        self.alarm = true;
                        queue.push_back(Ev::AlarmRaised);
                    }
                }
                MissionEvent::PlayerKilled(_) => {
                    self.view.stats.player_deaths += 1;
                    queue.push_back(Ev::PlayerKilled);
                }
                MissionEvent::Used(m) => queue.push_back(Ev::Use(m.clone())),
                MissionEvent::Arrived(m) => queue.push_back(Ev::Arrived(m.clone())),
                MissionEvent::Talked(n) => {
                    queue.push_back(Ev::Talk(n.clone()));
                    self.talk_conversation(n, None, &mut queue);
                }
                MissionEvent::TalkedBy { npc, player } => {
                    queue.push_back(Ev::Talk(npc.clone()));
                    self.talk_conversation(npc, Some(*player), &mut queue);
                }
                MissionEvent::PickedUp { pickup, item, .. } => {
                    self.view.stats.items_picked_up += 1;
                    queue.push_back(Ev::PickedUp { pickup: pickup.clone(), item: item.clone() });
                }
                MissionEvent::NpcKnockedOut { npc, by_player, .. } => {
                    // Non-lethal: only `HostilePutToSleep` / `CivilianPutToSleep` (0x00B738F0),
                    // which no chaos deed counts.
                    if *by_player {
                        self.view.stats.knocked_out += 1;
                    }
                    if let Some(n) = npc {
                        queue.push_back(Ev::NpcKnockedOut(n.clone()));
                    }
                }
                MissionEvent::Choice { player, index } => {
                    let (p, i) = (*player, *index);
                    self.with_conv(&mut queue, |r, env| {
                        r.choose(p, i, env);
                    });
                }
            }
        }
        // Conversations advance (lines end, the next nodes run).
        let dt = input.dt;
        self.with_conv(&mut queue, |r, env| r.update(dt, env));
        self.approach_hooks(&mut queue);
        self.fire_ai_hooks(dt, &mut queue);
        // Delayed actions that are due run before rules (they were scheduled earlier).
        let due: Vec<Vec<Action>> = {
            let t = self.time;
            let (d, keep): (Vec<_>, Vec<_>) = self.delayed.drain(..).partition(|(at, _)| *at <= t);
            self.delayed = keep;
            d.into_iter().map(|x| x.1).collect()
        };
        for acts in due {
            self.run(&acts, &mut queue, &mut out, input.npc_positions);
        }
        // Cascade like Kismet within one frame (bounded).
        let mut guard = 0;
        while let Some(ev) = queue.pop_front() {
            guard += 1;
            if guard > 2000 || self.view.outcome.is_some() {
                break;
            }
            for ri in 0..self.def.rules.len() {
                if !self.rule_enabled[ri] || (self.def.rules[ri].once && self.rule_fired[ri]) {
                    continue;
                }
                if !self.matches(ri, &ev) {
                    continue;
                }
                if !self.def.rules[ri].when.iter().all(|c| self.cond(c)) {
                    continue;
                }
                self.rule_fired[ri] = true;
                let acts = self.def.rules[ri].then.clone();
                self.run(&acts, &mut queue, &mut out, input.npc_positions);
            }
        }
        // Built-in ends.
        if self.view.outcome.is_none() {
            let s = &self.def.settings;
            if s.time_limit > 0.0 && self.time >= s.time_limit {
                self.end(false, "Out of time".into());
            } else if !s.respawn && !input.players.is_empty() && input.players.iter().all(|p| !p.2) {
                self.end(false, "You died".into());
            }
        }
        self.refresh_view(input.npc_positions);
        out
    }

    /// Run actions for a host that drives the mission itself (the original Kismet): the same
    /// effects as a rule's `then`, with the rules they trigger cascading as in [`Self::step`].
    /// Returns the commands for the host.
    pub fn apply(&mut self, acts: &[Action], npc_positions: &BTreeMap<String, Vec3>) -> Vec<Command> {
        let mut out = Vec::new();
        if self.view.outcome.is_some() {
            return out;
        }
        let mut queue: VecDeque<Ev> = VecDeque::new();
        self.run(acts, &mut queue, &mut out, npc_positions);
        let mut guard = 0;
        while let Some(ev) = queue.pop_front() {
            guard += 1;
            if guard > 2000 || self.view.outcome.is_some() {
                break;
            }
            for ri in 0..self.def.rules.len() {
                if !self.rule_enabled[ri] || (self.def.rules[ri].once && self.rule_fired[ri]) {
                    continue;
                }
                if !self.matches(ri, &ev) || !self.def.rules[ri].when.iter().all(|c| self.cond(c)) {
                    continue;
                }
                self.rule_fired[ri] = true;
                let acts = self.def.rules[ri].then.clone();
                self.run(&acts, &mut queue, &mut out, npc_positions);
            }
        }
        self.refresh_view(npc_positions);
        out
    }

    /// Talking to an NPC with `talk_conversation` enters it (low priority: an NPC already
    /// in a conversation ignores the request, like the original's use hook).
    fn talk_conversation(&mut self, npc: &str, player: Option<u32>, queue: &mut VecDeque<Ev>) {
        let Some(reference) = self.def.npcs.iter().find(|n| n.id == npc).and_then(|n| n.talk_conversation.clone()) else { return };
        self.start_conversation(&reference, &[], player, Some(npc), false, queue);
    }

    fn matches(&mut self, ri: usize, ev: &Ev) -> bool {
        let r = &self.def.rules[ri];
        match (&r.on, ev) {
            (Trigger::Start, Ev::Start) => true,
            (Trigger::Timer(s), Ev::Timer) => self.time >= *s,
            (Trigger::Every(s), Ev::Timer) => {
                if self.time >= self.every_next[ri] {
                    self.every_next[ri] += s.max(0.01);
                    true
                } else {
                    false
                }
            }
            (Trigger::Enter(m), Ev::Enter(x)) | (Trigger::Leave(m), Ev::Leave(x)) => m == x,
            (Trigger::NpcKilled(n), Ev::NpcKilled(x)) | (Trigger::NpcAlerted(n), Ev::NpcAlerted(x)) => n == x,
            (Trigger::AnyKill, Ev::AnyKill) => true,
            (Trigger::PlayerDetected, Ev::PlayerDetected) => true,
            (Trigger::AlarmRaised, Ev::AlarmRaised) => true,
            (Trigger::PlayerKilled, Ev::PlayerKilled) => true,
            (Trigger::FlagSet(f), Ev::Flag(x, true)) | (Trigger::FlagCleared(f), Ev::Flag(x, false)) => f == x,
            (Trigger::TaskState(t, s), Ev::Task(x, y)) => t == x && s == y,
            (Trigger::CounterAtLeast(c, v), Ev::Counter(x)) => c == x && self.counters.get(c).copied().unwrap_or(0) >= *v,
            (Trigger::Remote(n), Ev::Remote(x)) => n == x,
            (Trigger::Use(m), Ev::Use(x)) => m == x,
            (Trigger::Talk(n), Ev::Talk(x)) => n == x,
            (Trigger::PickedUp(w), Ev::PickedUp { pickup, item }) => w.eq_ignore_ascii_case(pickup) || item.as_deref().is_some_and(|i| w.eq_ignore_ascii_case(i) || w.eq_ignore_ascii_case(i.rsplit('.').next().unwrap_or(i))),
            (Trigger::NpcKnockedOut(n), Ev::NpcKnockedOut(x)) => n == x,
            (Trigger::Arrived(m), Ev::Arrived(x)) => m.eq_ignore_ascii_case(x),
            (Trigger::ConversationOutput(want), Ev::ConvOutput { name, oneshot }) => match want.split_once('/') {
                Some((os, n)) => n.eq_ignore_ascii_case(name) && oneshot.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(os.rsplit(':').next().unwrap_or(os))),
                None => want.eq_ignore_ascii_case(name),
            },
            (Trigger::ConversationEnded(want), Ev::ConvEnded { name, label }) => conv_name_matches(want, name, label),
            _ => false,
        }
    }

    fn cond(&self, c: &Condition) -> bool {
        match c {
            Condition::Flag(f) => self.flag(f),
            Condition::NotFlag(f) => !self.flag(f),
            Condition::Task(t, s) => self.task(t) == Some(*s),
            Condition::CounterAtLeast(n, v) => self.counters.get(n).copied().unwrap_or(0) >= *v,
            Condition::CounterBelow(n, v) => self.counters.get(n).copied().unwrap_or(0) < *v,
            Condition::Detected => self.detected_any,
            Condition::Inside(m) => self.inside.iter().any(|(x, _)| x == m),
            Condition::NpcAlive(n) => self.npc_alive.contains(n),
            Condition::Any(v) => v.iter().any(|x| self.cond(x)),
            Condition::All(v) => v.iter().all(|x| self.cond(x)),
            Condition::Not(x) => !self.cond(x),
        }
    }

    /// Post a sound for clients (original Wwise event name), as `Action::Sound` does.
    pub fn post_sound(&mut self, s: &str) {
        let id = self.id();
        self.view.sounds.push((id, s.to_string()));
        let n = self.view.sounds.len();
        if n > 8 {
            self.view.sounds.drain(..n - 8);
        }
    }

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn end(&mut self, success: bool, text: String) {
        if self.view.outcome.is_none() {
            self.view.outcome = Some(Outcome { success, text });
        }
    }

    fn set_task(&mut self, path: &str, state: TaskState, queue: &mut VecDeque<Ev>) {
        if let Some((o, t)) = self.def.task_index(path) {
            if self.task_state[o][t] != state {
                self.task_state[o][t] = state;
                if state == TaskState::Active {
                    // Activating a task reveals it (Arkane pairs SetTaskState with
                    // SetTaskHidden false).
                    self.task_hidden[o][t] = false;
                    self.obj_hidden[o] = false;
                }
                queue.push_back(Ev::Task(path.to_string(), state));
                // 0x00AC17B0: Active raises TaskActivated, then every change TaskStateChanged.
                if state == TaskState::Active {
                    self.task_log.push(TaskNote::Activated(path.to_string()));
                }
                self.task_log.push(TaskNote::Changed(path.to_string(), state));
                // 0x00ACBC70 (after the TaskStateChanged dispatch, 0x00AD1320): an objective that
                // completes with its tasks, all of them Completed, becomes Completed (kind 1).
                let od = &self.def.objectives[o];
                if self.obj_completed.len() != self.def.objectives.len() {
                    self.obj_completed.resize(self.def.objectives.len(), false);
                }
                if od.complete_when_all_tasks && !self.obj_completed[o] && self.task_state[o].iter().all(|s| *s == TaskState::Completed) {
                    self.obj_completed[o] = true;
                    self.task_log.push(TaskNote::ObjectiveCompleted(od.id.clone()));
                }
            }
        }
    }

    fn run(&mut self, acts: &[Action], queue: &mut VecDeque<Ev>, out: &mut Vec<Command>, npc_pos: &BTreeMap<String, Vec3>) {
        let _ = npc_pos;
        for a in acts {
            match a {
                Action::SetTask(t, s) => self.set_task(t, *s, queue),
                Action::ShowTask(t) | Action::HideTask(t) => {
                    if let Some((o, i)) = self.def.task_index(t) {
                        let hide = matches!(a, Action::HideTask(_));
                        // 0x00AC63C0: clearing the hidden flag of an active task raises TaskActivated.
                        if !hide && self.task_hidden[o][i] && self.task_state[o][i] == TaskState::Active {
                            self.task_log.push(TaskNote::Activated(t.clone()));
                        }
                        self.task_hidden[o][i] = hide;
                        if !hide {
                            self.obj_hidden[o] = false;
                        }
                    }
                }
                Action::ShowObjective(o) | Action::HideObjective(o) => {
                    if let Some(i) = self.def.objectives.iter().position(|x| &x.id == o) {
                        self.obj_hidden[i] = matches!(a, Action::HideObjective(_));
                    }
                }
                Action::SetFlag(f) | Action::ClearFlag(f) => {
                    let v = matches!(a, Action::SetFlag(_));
                    if self.flags.insert(f.clone(), v) != Some(v) {
                        queue.push_back(Ev::Flag(f.clone(), v));
                    }
                }
                Action::Add(c, d) => {
                    *self.counters.entry(c.clone()).or_default() += d;
                    queue.push_back(Ev::Counter(c.clone()));
                }
                Action::Set(c, v) => {
                    self.counters.insert(c.clone(), *v);
                    queue.push_back(Ev::Counter(c.clone()));
                }
                Action::Message(m) => {
                    let id = self.id();
                    self.view.messages.push((id, m.clone()));
                    let n = self.view.messages.len();
                    if n > 6 {
                        self.view.messages.drain(..n - 6);
                    }
                }
                Action::Notify(t, x) => {
                    let id = self.id();
                    self.view.notification = Some((id, t.clone(), x.clone()));
                }
                Action::Location(l) => {
                    let id = self.id();
                    self.view.location = Some((id, l.clone()));
                }
                Action::Tutorial(t, secs) => {
                    let id = self.id();
                    self.view.tutorial = Some((id, t.clone(), *secs));
                }
                Action::TutorialWindow(img, t) => {
                    let id = self.id();
                    self.view.tutorial_window = Some((id, *img, t.clone()));
                }
                Action::Spawn(n) => {
                    if self.npc_alive.insert(n.clone()) {
                        out.push(Command::Spawn(n.clone()));
                    }
                }
                Action::Despawn(n) => {
                    if self.npc_alive.remove(n) {
                        out.push(Command::Despawn(n.clone()));
                    }
                }
                Action::MoveNpc(n, m) | Action::RunNpc(n, m) => {
                    if let Some((p, _)) = self.markers.get(m) {
                        out.push(Command::Move { npc: n.clone(), to: *p, run: matches!(a, Action::RunNpc(..)) });
                    }
                }
                Action::Patrol(n, route) => {
                    let r: Vec<Vec3> = route.iter().filter_map(|m| self.markers.get(m).map(|x| x.0)).collect();
                    out.push(Command::Patrol { npc: n.clone(), route: r });
                }
                Action::Alert(n) => out.push(Command::Alert(n.clone())),
                Action::Calm(n) => out.push(Command::Calm(n.clone())),
                Action::Fire(e) => queue.push_back(Ev::Remote(e.clone())),
                Action::After(s, v) => self.delayed.push((self.time + s.max(0.0), v.clone())),
                Action::EnableRule(r) | Action::DisableRule(r) => {
                    if let Some(i) = self.def.rules.iter().position(|x| x.id.as_deref() == Some(r.as_str())) {
                        self.rule_enabled[i] = matches!(a, Action::EnableRule(_));
                    }
                }
                Action::Sound(s) => {
                    self.post_sound(s);
                    out.push(Command::Sound(s.clone()));
                }
                Action::Usable(m, p) => {
                    self.usable.insert(m.clone(), p.clone());
                }
                Action::NotUsable(m) => {
                    self.usable.remove(m);
                }
                Action::Travel(tag) => out.push(Command::Travel(tag.clone())),
                Action::SetMaterials { npc, body, head } => out.push(Command::SetMaterials { npc: npc.clone(), body: body.clone(), head: head.clone() }),
                Action::ClearMaterials(npc) => out.push(Command::ClearMaterials(npc.clone())),
                Action::Equip(l) => out.push(Command::Equip(l.clone())),
                // Kismet starts go through `DDH_KISMET_ACTIVATED` (flags 0x500, not "interrupts"):
                // busy speakers refuse unless the conversation outranks theirs.
                Action::Conversation(c, speakers) => self.start_conversation(c, speakers, None, None, false, queue),
                Action::StopConversation(c) => {
                    let c = c.clone();
                    let db = self.conv.as_ref().map(|r| r.db.clone());
                    self.with_conv(queue, |r, env| {
                        let Some(db) = db else { return };
                        r.stop_where(env, |i| {
                            if c.is_empty() {
                                return true;
                            }
                            let os = i.oneshot.map(|o| db.oneshots[o as usize].name.clone()).unwrap_or_default();
                            let (name, label) = i.conversation.map(|cv| (db.conversation_name(cv), db.conversations[cv as usize].label.clone())).unwrap_or_default();
                            conv_name_matches(&c, &name, &label) || os.eq_ignore_ascii_case(c.rsplit(':').next().unwrap_or(&c))
                        });
                    });
                }
                Action::Complete(t) => self.end(true, t.clone()),
                Action::Fail(t) => self.end(false, t.clone()),
            }
        }
    }

    fn refresh_view(&mut self, npc_pos: &BTreeMap<String, Vec3>) {
        self.view.objectives = self
            .def
            .objectives
            .iter()
            .enumerate()
            .map(|(oi, o)| ObjectiveView {
                description: o.description.clone(),
                hidden: self.obj_hidden[oi],
                tasks: o
                    .tasks
                    .iter()
                    .enumerate()
                    .map(|(ti, t)| TaskView { description: t.description.clone(), state: self.task_state[oi][ti], optional: t.optional, hidden: self.task_hidden[oi][ti] })
                    .collect(),
            })
            .collect();
        // The mission-stats chaos line (0x009FE090): High when the chaos level reaches the
        // threshold of `DisSeqAct_MissionStatsTracking` (or the mission's own).
        let threshold = self.chaos_threshold.or(self.def.settings.chaos_level_threshold);
        self.view.high_chaos = threshold.is_some_and(|t| self.darkness_level >= t);
        self.view.darkness_score = self.darkness_score;
        self.view.darkness_level = self.darkness_level;
        let on_map = |m: &String| self.current_map.is_empty() || self.marker_map.get(m).is_none_or(|mm| mm.eq_ignore_ascii_case(&self.current_map));
        self.view.usables = self.usable.iter().filter(|(m, _)| on_map(m)).filter_map(|(m, p)| self.markers.get(m).map(|x| (x.0.to_array(), p.clone()))).collect();
        self.view.talkers = self.talk.iter().filter(|(n, _)| self.npc_alive.contains(*n)).map(|(n, p)| (n.clone(), p.clone())).collect();
        self.view.npc_names = self.def.npcs.iter().filter_map(|n| n.name.clone().map(|name| (n.id.clone(), name))).collect();
        self.view.dialogue.clear();
        self.view.choice = None;
        if let Some(r) = &self.conv {
            // A line shows (and plays) only when the original would show a subtitle: lines
            // without a VO event are silent skips there. Lines started by an `Interrupted`
            // branch play out detached from their ended conversation.
            let line_view = |l: &dishonored_conversation::runner::ActiveLine, conversation: String, instance: u32, hook: Option<String>| {
                let elapsed = (r.time - l.started) as f32;
                DialogueLineView {
                    id: l.id,
                    speaker: l.speaker.name.clone(),
                    npc: l.speaker.npc.clone(),
                    player: l.speaker.player,
                    text: l.text.clone(),
                    vo_event: l.vo.as_ref().map(|v| v.event.clone()),
                    vo_bank: l.vo.as_ref().map(|v| v.bank.clone()),
                    start: l.started as f32,
                    duration: l.duration,
                    remaining: (l.duration - elapsed).max(0.0),
                    main: l.main,
                    conversation,
                    blurb: match &r.db.node(l.node).kind {
                        dishonored_conversation::NodeKind::Line(x) => Some(x.guid.clone()),
                        _ => None,
                    },
                    voice_radius: match &r.db.node(l.node).kind {
                        dishonored_conversation::NodeKind::Line(x) => x.voice_radius,
                        _ => 3000.0,
                    },
                    instance,
                    hook,
                }
            };
            self.view.dialogue_mix = DialogueMix { non_dominant_scale: r.db.settings.non_dominant_volume_scale, blend_speed: r.db.settings.duck_blend_speed, dominant_pct: r.db.settings.dominant_max_distance_percent };
            // A subtitle goes with its owner: dropped once the speaker is dead (`DisSubtitle`
            // owner health <= 0, R7).
            let alive = |l: &&dishonored_conversation::runner::ActiveLine| l.subtitle && l.speaker.npc.as_ref().is_none_or(|n| !self.dead_speakers.contains(n));
            for l in r.detached.iter().filter(alive) {
                self.view.dialogue.push(line_view(l, String::new(), 0, None));
            }
            for inst in r.active() {
                let conversation = inst.conversation.map(|c| r.db.conversation_name(c)).unwrap_or_default();
                if let Some(l) = inst.line.as_ref().filter(alive) {
                    self.view.dialogue.push(line_view(l, conversation.clone(), inst.id, inst.hook.clone()));
                }
                if let (Some(c), None) = (&inst.choice, &self.view.choice) {
                    self.view.choice = Some(DialogueChoiceView { id: c.id, options: c.options.clone(), player: c.player, conversation });
                }
            }
            self.view.dialogue.sort_by_key(|l| l.id);
        }
        // Marker: first visible active task with a target (required tasks first).
        let mut best: Option<(bool, [f32; 3])> = None;
        self.view.task_markers.clear();
        for (oi, o) in self.def.objectives.iter().enumerate() {
            if self.obj_hidden[oi] {
                continue;
            }
            for (ti, t) in o.tasks.iter().enumerate() {
                if self.task_state[oi][ti] != TaskState::Active || self.task_hidden[oi][ti] {
                    continue;
                }
                let pos = match &t.target {
                    Some(TargetRef::Marker(m)) => self.markers.get(m).map(|x| x.0),
                    Some(TargetRef::Npc(n)) => npc_pos.get(n).copied(),
                    None => None,
                };
                if let Some(p) = pos {
                    let required = !t.optional;
                    if best.is_none_or(|b| required && !b.0) {
                        best = Some((required, p.to_array()));
                    }
                    // The marker's description is the target's name (`m_TargetName`).
                    let name = match &t.target {
                        Some(TargetRef::Npc(n)) => self.def.npcs.iter().find(|x| &x.id == n).and_then(|x| x.name.clone()).unwrap_or_default(),
                        _ => String::new(),
                    };
                    self.view.task_markers.push(TaskMarkerView { pos: p.to_array(), optional: t.optional, name });
                }
            }
        }
        self.view.target = best.map(|b| b.1);
    }
}

