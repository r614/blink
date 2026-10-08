//! The original Kismet inside a mission match.
//!
//! A mission with `MissionDef::original_kismet` runs the Kismet of its maps and of the global
//! game sequence (`dishonored_kismet`) instead of hand-written rules. This module is the game
//! side of that interpreter ([`dishonored_kismet::Host`]):
//!
//! - story flags are the mission's flags, by original name (`SF_*` sets), shared with the
//!   conversation runner;
//! - objectives and tasks are the original `DishonoredObjective`s (the host builds them into
//!   the mission definition with their localized text); `DisSeqAct_SetTaskState` /
//!   `SetTaskHidden` / `UpdateTaskTarget` / `AddObjective` drive them through the mission
//!   runtime, so they show in `MissionView` as any mission's do;
//! - original spawners spawn their pawns (`DisSeqEvent_Spawned`, `DisSeqVar_SpawnerPawn`), the
//!   AI actions move brains, `NPCSetMaterials` changes looks, `Teleport` / `Destroy` move or
//!   remove pawns and level actors;
//! - travel follows `SetPlayerTravelDestination` -> `PrepareMapChange` -> `CommitMapChange`
//!   with the match's level travel; a change to a map outside the mission ends it, as the
//!   original's level change out of the mission does;
//! - `DisSeqEvent_Used` / `DisSeqEvent_PickupPickedUp` actors become usables, trigger shapes
//!   feed `SeqEvent_Touch`, deaths feed `SeqEvent_Death` / `DisSeqEvent_NPCIncapacitated`;
//! - `DisSeqAct_DialogInputs` start conversations (the one-shot's Kismet hook), and the
//!   conversation runner's Kismet outputs fire `DisSeqEvent_DialogOutputs`; a conversation's
//!   soiree plays its matinee by GUID;
//! - `DisSeqAct_DialogScriptedChoice` opens the shared choice view.
//!
//! Anything not supported is logged once per class by the interpreter and counted.

use super::{Interaction, MissionMatch};
/// [interp] Matinees played on the server (gameplay side of cutscenes).
#[path = "kismet_host/cutscene.rs"]
pub mod cutscene;
use dishonored_ai::BrainState;
use dishonored_kismet::host::Matinee;
use dishonored_kismet::runtime::Ctx;
use dishonored_kismet::{Host, HostResult, Kismet, KismetDb, Obj, OpId, Value};
use dishonored_mission::runtime::MissionEvent;
use dishonored_mission::{Action, TaskState};
use dishonored_sim::SimConfig;
use dishonored_sim::events::SimEvent;
use dishonored_sim::state::{PawnKind, PlayerId};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Everything needed to run a mission's original Kismet (stored in replays).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KismetSetup {
    pub db: KismetDb,
    /// Global remote event that starts the mission (`ChangeLvl_fromHub_toBoyle`).
    pub start_event: String,
    pub seed: u64,
    /// Localized options of each `DisSeqAct_DialogScriptedChoice` (op id -> options).
    #[serde(default)]
    pub choices: BTreeMap<OpId, Vec<String>>,
    /// Localized use prompts of usable actors (actor index -> prompt).
    #[serde(default)]
    pub prompts: BTreeMap<u32, String>,
    /// What each `DisSeqAct_GivePickup` gives (its `m_pPickup` tweak resolved): op id ->
    /// pickup kind.
    #[serde(default)]
    pub gives: BTreeMap<OpId, dishonored_ir::items::PickupKind>,
}

/// Classes that only present something (camera, HUD, tutorials, saves, statistics, ambient
/// audio, materials): run as no-ops, counted apart from the unsupported ones.
pub const PRESENTATION: &[&str] = &[
    "SeqAct_ToggleHUD",
    "SeqAct_Log",
    "SeqAct_StreamInTextures",
    "SeqAct_ConsoleCommand",
    "DisSeqAct_ToggleHUDElement",
    "DisSeqAct_ToggleTutorial",
    "DisSeqAct_SetTutorialMessage",
    "DisSeqAct_SetCurrentChapter",
    "DisSeqAct_EvalAchievement",
    "DisSeqAct_ToggleAchievementEval",
    "DisSeqAct_GetPlayerStat",
    "DisSeqAct_ToggleJournal",
    "DisSeqAct_TogglePowerWheel",
];

/// Classes whose effect is on a system the port does not simulate (decoded; they finish with
/// their automatic outputs and are counted apart): rat spawners and swarm behaviours, the
/// Outsider's render settings, walls of light, scripted time dilation, possession overrides,
/// player power cancel / weapon switching / camera and head tracking, minimum health,
/// damage instigator overrides, the protect-neutrals AI flag, loadouts.
pub const NOT_MODELLED: &[&str] = &[
    "DisSeqAct_ActivateRatSpawner",
    "DisSeqAct_SetRatSwarmCustomBehavior",
    "DisSeqAct_OutsiderConfig",
    "DisSeqAct_WallofLightControl",
    "DisSeqAct_BendTime",
    "DisSeqAct_OverridePossess",
    "DisSeqAct_CancelPlayerActivePower",
    "DisSeqAct_EquipItemType",
    "DisSeqAct_NPCTrackTarget",
    "DisSeqAct_LimitPawnMinHealth",
    "SeqAct_SetDamageInstigator",
    "DisSeqAct_AIProtectNeutralsOverride",
    "DisSeqAct_ApplyPlayerLoadout",
];

/// Event classes the game activates (this host's `feed`, `drain_spawns`, the conversation
/// outputs, `used_stage`); with their subclasses. A level's events of any other class are
/// reported once as unsupported when it loads ([`dishonored_kismet::Host::fires_event`]).
/// Add a class here when its events are fed.
pub const FED_EVENTS: &[&str] = &[
    "SeqEvent_Touch",
    "SeqEvent_Used",
    "SeqEvent_Death",
    "SeqEvent_TakeDamage",
    "SeqEvent_LOS",
    "DisSeqEvent_Used",
    "DisSeqEvent_PickupPickedUp",
    "DisSeqEvent_Spawned",
    "DisSeqEvent_KnockedOut",
    "DisSeqEvent_PutToSleep",
    "DisSeqEvent_NPCIncapacitated",
    "DisSeqEvent_Door",
    "DisSeqEvent_DialogOutputs",
    "DisSeqEvent_AttentionIncreasedTo",
    "DisSeqEvent_AttentionDecreasedTo",
];

/// A usable the Kismet offers (an actor with `DisSeqEvent_Used` or a pickup with
/// `DisSeqEvent_PickupPickedUp`, or an original NPC whose one-shot has a use hook).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum KUse {
    Actor(u32),
    Pickup(u32),
    Talk { pawn: PlayerId, oneshot: String },
}

/// A forced view tracking (`DisSeqAct_PlayerTrackTarget` Start, camera code 0x00aaf4c0 /
/// 0x00aa0cb0, per-tick 0x00ab4e60 / 0x00aa0f10 / 0x00aaf3a0). The camera keeps one channel
/// for yaw and one for pitch (flags 0xC3 / 0xC7: both axes, override, eased); each turns from
/// the view's angle at the start to the look-at angle of the target (its bone + offset, else
/// its location) by `RLerp` (0x00bbb620) with alpha rising at `1 / m_fBlendTime` per second,
/// eased `(1 - cos(alpha pi)) / 2` (flag 0x40). The player keeps `m_fRotationFreedomYaw` /
/// `Pitch` degrees around it. When alpha reaches 1 the channel ends (0x00aa0e60) unless
/// `m_bHoldUntilStopped` (flag 4); Stop (0x00aa0ec0) ends it by ID.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ViewTrack {
    pub id: i32,
    /// Tracked pawn (followed each tick), else a fixed point.
    pub pawn: Option<PlayerId>,
    pub point: Vec3,
    /// View yaw / pitch (degrees) when the tracking started.
    pub start: (f32, f32),
    pub alpha: f32,
    /// Alpha per second (`1 / m_fBlendTime`; 0 = already at 1).
    pub rate: f32,
    pub freedom: (f32, f32),
    pub hold: bool,
    /// The player's wanted view (input) on the first tracked tick: turning from it moves the
    /// view inside the freedom.
    pub want0: Option<(f32, f32)>,
}

impl ViewTrack {
    /// The view the tracking imposes this tick, given the player's eye and wanted view
    /// (degrees): the look-at angle blended from the start, plus the player's offset within the
    /// freedom.
    pub fn apply(&self, eye: Vec3, target: Vec3, want: (f32, f32)) -> (f32, f32) {
        let d = target - eye;
        let look = (d.y.atan2(d.x).to_degrees(), (d.z / d.truncate().length().max(1e-3)).atan().to_degrees());
        let a = (1.0 - (self.alpha.clamp(0.0, 1.0) * std::f32::consts::PI).cos()) * 0.5;
        let lerp = |s: f32, e: f32| s + wrap180(e - s) * a;
        let base = (lerp(self.start.0, look.0), lerp(self.start.1, look.1));
        // The player's own turning since the start moves the view within the freedom.
        let w0 = self.want0.unwrap_or(want);
        let off = (wrap180(want.0 - w0.0).clamp(-self.freedom.0, self.freedom.0), (want.1 - w0.1).clamp(-self.freedom.1, self.freedom.1));
        (base.0 + off.0, (base.1 + off.1).clamp(-89.0, 89.0))
    }
}

fn wrap180(a: f32) -> f32 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// Host-side state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HostState {
    pub started: bool,
    /// Distance trackers of original NPCs' approach hooks: (one-shot, hook node).
    approach: BTreeMap<(u32, u32), dishonored_conversation::hooks::DistanceHook>,
    /// Map whose levels the Kismet has loaded.
    pub map: Option<String>,
    /// `SetPlayerTravelDestination`: tag and the map it was set on.
    pub travel: Option<(String, String)>,
    /// `PrepareMapChange` main level.
    pub pending_map: Option<String>,
    /// `CommitMapChange` ran this step.
    pub commit: bool,
    /// Level actors moved by `SeqAct_Teleport`.
    pub actor_pos: BTreeMap<u32, Vec3>,
    /// Level actors destroyed or picked up.
    pub gone: BTreeSet<u32>,
    /// Actors toggled off (`SeqAct_Toggle` on triggers).
    pub off: BTreeSet<u32>,
    /// Task path -> its HUD target (`UpdateTaskTarget`).
    pub targets: BTreeMap<String, Obj>,
    /// Open scripted choice: (op, player, options, picked).
    pub choice: Option<(OpId, PlayerId, Vec<String>, Option<usize>)>,
    /// `AIGoToActor` in progress: op -> (pawn, goal).
    goto: BTreeMap<OpId, GoTo>,
    /// `StartSpawn` in progress: op -> (spawner originals not yet spawned, spawned so far).
    spawning: BTreeMap<OpId, (Vec<usize>, Vec<PlayerId>)>,
    /// Spawner actor -> index into `MissionSetup::original`.
    pub spawner: BTreeMap<u32, usize>,
    pub spawner_actor: BTreeMap<usize, u32>,
    /// Conversation events already fed to Kismet.
    conv_seen: usize,
    conv_last: Option<dishonored_conversation::runner::ConvEvent>,
    /// Conversation instance -> its package.
    conv_pkg: BTreeMap<u32, String>,
    /// Uses this step: (player, usable).
    pub uses: Vec<(PlayerId, KUse)>,
    /// Mission-level history (harness and reports).
    pub history: Vec<String>,
    /// Presentation-only activations per class.
    pub presentation: BTreeMap<String, u32>,
    /// (op, length, start time, end time) of matinees played.
    pub matinees: Vec<(OpId, f32, f64, Option<f64>)>,
    /// Pawns killed (for NPCIncapacitated / death bookkeeping).
    pub dead: BTreeSet<PlayerId>,
    /// `SeqAct_CameraFade` time left per op.
    fades: BTreeMap<OpId, f32>,
    /// `SeqAct_AkStartAmbientSound` Start (true) / Stop (false) per ambient sound actor.
    pub ambient: BTreeMap<u32, bool>,
    /// Actors hidden by `SeqAct_ToggleHidden`.
    pub hidden: BTreeSet<u32>,
    /// `NPCMarkForVanish` in progress: op -> (pawns, seconds left).
    vanish: BTreeMap<OpId, Vec<(PlayerId, f32)>>,
    /// [interp] Matinees playing with their bound groups.
    #[serde(default)]
    pub cutscenes: Vec<cutscene::Playing>,
    /// [interp] Level actors rotated by matinees.
    #[serde(default)]
    pub actor_rot: BTreeMap<u32, glam::Quat>,
    /// [interp] Matinee op -> conversation instance whose soiree asked for it.
    #[serde(default)]
    pub soiree_conv: BTreeMap<OpId, u32>,
    /// Interpreter time each host op was last activated (a latent op's first update runs in
    /// the same step and still sees the starting impulse).
    act_time: BTreeMap<OpId, f64>,
    /// Forbidden zones changed by `DisSeqAct_ForbiddenZoneOverride`: zone -> level (owning, forbidden).
    zone_backup: BTreeMap<String, (Vec<String>, Vec<String>)>,
    /// `DisDoor` actors driven by `DisSeqAct_Door`: actor -> (`EDisDoorState`: 0 closed,
    /// 1 opened CW, 2 opened CCW; locked). Doors not listed keep their level state.
    pub doors: BTreeMap<u32, (u8, bool)>,
    /// Bumped whenever a door's lock changes (the AI's navigation overlay is rebuilt).
    pub door_version: u32,
    /// `SeqAct_AttachToActor`: attached actor -> (base, offset in the base's frame).
    pub attached: BTreeMap<u32, (Obj, Vec3)>,
    /// `DisSeqAct_ActivateUsable`: usable actor -> its active stage.
    pub usable_stage: BTreeMap<u32, i32>,
    /// `DisSeqAct_Lock` on usables that are not doors: actor -> `m_bLocked` (others keep the
    /// level's value).
    #[serde(default)]
    pub usable_locked: BTreeMap<u32, bool>,
    /// `DisSeqAct_SetBoneCharmEffect`: bone charm actor -> its `m_Effect` (others keep the
    /// level's).
    #[serde(default)]
    pub charm_effect: BTreeMap<u32, i32>,
    /// Door state changes to report as `DisSeqEvent_Door` outputs: (door actor, output).
    door_events: Vec<(u32, usize)>,
    /// `DisSeqAct_AIDoFollow`: follower -> (leader, distance, run).
    followers: BTreeMap<PlayerId, (PlayerId, f32, bool)>,
    /// AI noises made this step (`DisSeqAct_AINoise`), heard by the crowd next step.
    pub noises: Vec<SimEvent>,
    /// Shots each pawn has fired (`SimEvent::Fired`).
    shots: BTreeMap<PlayerId, u32>,
    /// `DisSeqAct_AIShoot` in progress: op -> (pawn, shot count at which it is done).
    shooting: BTreeMap<OpId, Vec<(PlayerId, u32)>>,
    /// `DisSeqAct_PlayerTrackTarget`: forced view tracking per player.
    #[serde(default)]
    pub tracking: BTreeMap<PlayerId, ViewTrack>,
    /// Last tracking ID handed out (0x00aaf4c0 counts from the camera's +0x60c).
    #[serde(default)]
    track_id: i32,
    /// `SeqEvent_LOS` events a player is looking at: (event, player).
    los: BTreeSet<(OpId, PlayerId)>,
    /// Activations of classes whose system the port does not have, per class.
    pub not_modelled: BTreeMap<String, u32>,
    /// Usables moved to a stage by `DisSeqAct_ActivateUsable`: (actor, stage).
    usable_events: Vec<(u32, usize)>,
    /// `SeqAct_ActorFactory` in progress: op -> (spawn points, count, spawned, delay left).
    factories: BTreeMap<OpId, (Vec<Obj>, u32, u32, f32)>,
    /// Attention level of each NPC toward each pawn at the last step (non-zero only), for
    /// `DisSeqEvent_AttentionIncreasedTo` / `DecreasedTo`.
    attn_levels: BTreeMap<(PlayerId, PlayerId), u8>,
    /// `SeqAct_SetPhysics`: the `EPhysics` the Kismet gave a level actor or pawn. Recorded only:
    /// the port has no rigid bodies or per-actor physics modes to apply it to.
    pub physics: BTreeMap<Obj, u8>,
    /// `SeqAct_ToggleCinematicMode`: each player's cinematic mode (`PlayerController.
    /// SetCinematicMode`).
    #[serde(default)]
    pub cinematic: BTreeMap<PlayerId, Cinematic>,
    /// `SeqAct_SetMatInstScalarParam` on a `MaterialInstanceConstant` (object path): parameter ->
    /// value. A material instance is shared by every mesh using it.
    #[serde(default)]
    pub mat_scalars: BTreeMap<String, BTreeMap<String, f32>>,
    /// `SeqAct_SetMatInstScalarParam` with `m_pMaterialOwnerPawn`: (pawn, material index) ->
    /// parameter -> value.
    #[serde(default)]
    pub pawn_mat_scalars: BTreeMap<(PlayerId, i32), BTreeMap<String, f32>>,
    /// `SeqAct_SetMaterial` (`OnSetMaterial`): actor or pawn -> material index -> material path.
    #[serde(default)]
    pub materials: BTreeMap<Obj, BTreeMap<i32, String>>,
}

/// A player's cinematic mode (`PlayerController.SetCinematicMode` 0x00 script, then
/// `DishonoredPlayerController.SetCinematicMode_Native` 0x009eea00 -> 0x00aaf150).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Cinematic {
    /// `bCinematicMode`.
    pub on: bool,
    /// `bCinemaDisableInputMove` / `bCinemaDisableInputLook`: the movement / look input is
    /// ignored (`IgnoreMoveInput` / `IgnoreLookInput`).
    pub no_move: bool,
    pub no_look: bool,
    /// The HUD is hidden (`myHUD.bShowHUD = false`) and the player pawn hidden.
    pub hide_hud: bool,
    pub hide_player: bool,
}

/// Stage names of a usable: the outputs of its `DisSeqEvent_Used` (the editor names them
/// after the usable's `m_Stages`; the special outputs locked / hold-to-use / missing item
/// are not stages). At least one stage.
fn stage_outputs(k: &Kismet, a: u32) -> Vec<(OpId, usize)> {
    let mut out = Vec::new();
    for e in k.events_for("DisSeqEvent_Used", &[Obj::Actor(a)]) {
        for (i, o) in k.op(e).outputs.iter().enumerate() {
            let n = o.name.to_ascii_lowercase().replace(' ', "");
            if n.contains("locked") || n.contains("holdtouse") || n.contains("missing") {
                continue;
            }
            out.push((e, i));
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

/// A usable reached a stage: each enabled `DisSeqEvent_Used` of the usable is checked with
/// the stage as its only output (0x00a57640: `m_Stage` = the stage, `CheckActivate` with
/// indices `[stage]`; the stage outputs come first, the special outputs after them).
fn used_stage(k: &mut Kismet, h: &mut H, a: u32, stage: usize, instigator: Obj) {
    for e in k.events_for("DisSeqEvent_Used", &[Obj::Actor(a)]) {
        if !k.ops[e as usize].enabled || stage >= k.op(e).outputs.len() {
            continue;
        }
        k.set_prop(e, "m_Stage", Value::Int(stage as i32));
        k.check_activate(h, e, Obj::Actor(a), instigator.clone(), false, Some(vec![stage]), false);
    }
}

/// Stage count of a usable (two, a toggle, when no Kismet event names its stages).
fn usable_stages(k: &Kismet, a: u32) -> i32 {
    match stage_outputs(k, a).len() {
        0 => 2,
        n => n as i32,
    }
}

/// A `DisSeqAct_AIGoToActor` in progress: its latent pawns (each running a go-to).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct GoTo {
    pawns: Vec<PlayerId>,
    /// Destination rotation (`m_bAlignRotation`): the pawn turns to it on arrival.
    yaw: Option<f32>,
    /// The last pawn to report succeeded (Success fires only for the last report).
    last_ok: bool,
}

/// Saved with the mission (`save.rs`); the setup is re-attached on load.
#[derive(Serialize, Deserialize)]
pub struct KismetRun {
    pub k: Kismet,
    pub st: HostState,
    #[serde(skip)]
    pub setup: Arc<KismetSetup>,
    /// The active map's navigation with the Kismet's door locks applied: (map, door version).
    #[serde(skip)]
    nav_cache: Option<(String, u32, Arc<dishonored_world::nav::NavWorld>)>,
}

impl KismetRun {
    pub fn new(setup: &KismetSetup, m: &super::MissionSetup) -> Self {
        let db = Arc::new(setup.db.clone());
        let k = Kismet::new(db.clone(), setup.seed);
        let mut st = HostState::default();
        // Spawner actors of the Kismet -> original spawners of the match (same map, name and
        // position: names repeat across sublevels).
        for (ai, a) in db.actors.iter().enumerate() {
            if !a.class.contains("Spawner") {
                continue;
            }
            let Some(map) = db.packages.iter().position(|p| p.eq_ignore_ascii_case(&a.package)).and_then(|p| db.map_of_package(p as u16)) else { continue };
            let found = m.original.iter().position(|o| o.map.eq_ignore_ascii_case(map) && o.name.eq_ignore_ascii_case(&a.name) && (o.spawn.position.x - a.location[0]).abs() < 2.0 && (o.spawn.position.y - a.location[1]).abs() < 2.0);
            if let Some(i) = found {
                st.spawner.insert(ai as u32, i);
                st.spawner_actor.insert(i, ai as u32);
            }
        }
        KismetRun { k, st, setup: Arc::new(setup.clone()), nav_cache: None }
    }

    /// The state the Kismet set a door (`<package>:<name>`) to (`EDisDoorState`: 0 closed,
    /// 1 opened clockwise, 2 opened counter-clockwise); None when the Kismet has not set it.
    pub fn door_state(&self, actor: &str) -> Option<u8> {
        let a = self.k.db.find_actor(actor)?;
        self.st.doors.get(&a).map(|s| s.0)
    }

    /// Where a mover actor (`<package>:<name>`) is now relative to where the map placed it
    /// (`placed`): its Kismet location and rotation (matinee moves, attachments, teleports) x
    /// the inverse of the placed transform. None when the Kismet has not moved it.
    pub fn mover_delta(&self, actor: &str, placed: glam::Mat4) -> Option<glam::Mat4> {
        let a = self.k.db.find_actor(actor)?;
        let pos = self.st.actor_pos.get(&a).copied();
        let rot = self.st.actor_rot.get(&a).copied();
        if pos.is_none() && rot.is_none() {
            return None;
        }
        let act = &self.k.db.actors[a as usize];
        let pos = pos.unwrap_or_else(|| Vec3::from(act.location));
        let rot = rot.unwrap_or_else(|| glam::Quat::from_mat3(&dishonored_kismet::interp::rot_matrix_units(act.rotation)));
        Some(glam::Mat4::from_rotation_translation(rot, pos) * placed.inverse())
    }

    /// The navigation the AI should path on this step: the map's navmesh with the doors the
    /// Kismet locked or unlocked (`DisSeqAct_Door` Lock / Unlock) applied to their path-object
    /// edges (a locked `DisDoor` blocks its edges, as `m_bLocked` does at level start). None
    /// when the Kismet changed no door of this map.
    pub fn door_nav(&mut self, map: &str, base: Option<&dishonored_world::nav::NavWorld>) -> Option<Arc<dishonored_world::nav::NavWorld>> {
        let base = base?;
        let db = &self.k.db;
        let changes: Vec<(String, bool)> = self
            .st
            .doors
            .iter()
            .filter(|(a, _)| db.packages.iter().position(|p| p.eq_ignore_ascii_case(&db.actors[**a as usize].package)).and_then(|p| db.map_of_package(p as u16)).is_some_and(|m| m.eq_ignore_ascii_case(map)))
            .map(|(a, s)| (db.actors[*a as usize].name.clone(), s.1))
            .collect();
        if changes.is_empty() {
            return None;
        }
        if let Some((m, v, n)) = &self.nav_cache {
            if m.eq_ignore_ascii_case(map) && *v == self.st.door_version {
                return Some(n.clone());
            }
        }
        let mut nav = base.clone();
        for (name, locked) in changes {
            for o in nav.path_objects.iter_mut().filter(|o| o.name.eq_ignore_ascii_case(&name) && o.class.contains("Door")) {
                o.blocking = locked;
            }
        }
        let nav = Arc::new(nav);
        self.nav_cache = Some((map.to_string(), self.st.door_version, nav.clone()));
        Some(nav)
    }

    /// A player is answering a scripted choice (input is frozen like the original's
    /// `StatePlayerMasterInScriptedChoice`).
    pub fn choosing(&self, p: PlayerId) -> bool {
        self.st.choice.as_ref().is_some_and(|c| c.1 == p && c.3.is_none())
    }

    /// [interp] A player is held by a matinee's player group (no movement control).
    pub fn locked(&self, p: PlayerId) -> bool {
        cutscene::locked(self, p)
    }

    pub fn actor_pos(&self, a: u32) -> Vec3 {
        self.st.actor_pos.get(&a).copied().unwrap_or_else(|| Vec3::from(self.k.db.actors[a as usize].location))
    }
}

/// A world whose navigation is replaced (doors locked or unlocked by the Kismet).
pub struct NavOverlay<'a> {
    pub inner: &'a dyn dishonored_world::WorldQuery,
    pub nav: &'a dishonored_world::nav::NavWorld,
}

impl dishonored_world::WorldQuery for NavOverlay<'_> {
    fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32, channels: u8) -> Option<dishonored_world::RayHit> {
        self.inner.raycast(origin, dir, max_dist, channels)
    }
    fn capsule_contacts(&self, cap: &dishonored_world::Capsule, channels: u8, out: &mut Vec<dishonored_world::Contact>) {
        self.inner.capsule_contacts(cap, channels, out)
    }
    fn sweep_capsule(&self, cap: &dishonored_world::Capsule, delta: Vec3, channels: u8) -> Option<dishonored_world::SweepHit> {
        self.inner.sweep_capsule(cap, delta, channels)
    }
    fn nav(&self) -> Option<&dishonored_world::nav::NavWorld> {
        Some(self.nav)
    }
    fn box_contacts(&self, b: &dishonored_world::PawnBox, channels: u8, out: &mut Vec<dishonored_world::Contact>) {
        self.inner.box_contacts(b, channels, out)
    }
    fn sweep_box(&self, b: &dishonored_world::PawnBox, delta: Vec3, channels: u8) -> Option<dishonored_world::SweepHit> {
        self.inner.sweep_box(b, delta, channels)
    }
    fn water_at(&self, p: Vec3) -> Option<dishonored_world::WaterInfo> {
        self.inner.water_at(p)
    }
    fn movers(&self) -> &[dishonored_world::MoverCollision] {
        self.inner.movers()
    }
    fn physics_volumes(&self) -> &[dishonored_world::PhysicsVolume] {
        self.inner.physics_volumes()
    }
    fn climbables(&self) -> &[dishonored_world::Climbable] {
        self.inner.climbables()
    }
    fn hit_package(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<String> {
        self.inner.hit_package(origin, dir, max_dist)
    }
}

/// The Kismet's game side for one step.
struct H<'a> {
    m: &'a mut MissionMatch,
    st: &'a mut HostState,
    setup: Arc<KismetSetup>,
    cfg: &'a SimConfig,
    db: Arc<KismetDb>,
}

fn obj_name(o: &Obj) -> String {
    match o {
        Obj::Path(p) => p.rsplit(['.', ':']).next().unwrap_or(p).to_string(),
        other => format!("{other:?}"),
    }
}

impl H<'_> {
    fn npc_pos(&self) -> BTreeMap<String, Vec3> {
        BTreeMap::new()
    }

    fn act(&mut self, acts: Vec<Action>) {
        let pos = self.npc_pos();
        let cmds = self.m.state.apply(&acts, &pos);
        for c in cmds {
            self.m.apply(c, self.cfg);
        }
    }

    fn note(&mut self, s: String) {
        let t = self.m.state.view.stats.time;
        self.st.history.push(format!("[{t:.1}] {s}"));
    }

    /// The pawn an object stands for (a pawn, or the spawned pawn of a spawner actor).
    fn pawn_of(&self, o: &Obj) -> Option<PlayerId> {
        match o {
            Obj::Pawn(p) => Some(*p),
            Obj::Actor(a) => self.st.spawner.get(a).map(|i| self.m.setup.original[*i].spawn.id).filter(|id| self.m.original_spawned.contains(id)),
            _ => None,
        }
    }

    fn pos_of(&self, o: &Obj) -> Option<Vec3> {
        if let Some(p) = self.pawn_of(o) {
            return self.m.world.player(p).map(|x| x.position).or_else(|| self.m.saved.values().flat_map(|l| l.npcs.iter()).find(|x| x.id == p).map(|x| x.position));
        }
        match o {
            Obj::Actor(a) => Some(self.st.actor_pos.get(a).copied().unwrap_or_else(|| Vec3::from(self.db.actors[*a as usize].location))),
            _ => None,
        }
    }

    /// "Objective.Task" of the `DisSeqVar_Task` variables linked to `link`.
    fn tasks(&self, cx: &Ctx, link: &str) -> Vec<String> {
        let mut out = Vec::new();
        for v in cx.link_vars(link) {
            let obj = cx.k.prop(v, "m_pObjective").and_then(|x| x.as_obj().cloned()).unwrap_or_default();
            let task = cx.k.prop_str(v, "m_TaskName");
            if obj.is_none() || task.is_empty() {
                continue;
            }
            out.push(format!("{}.{task}", obj_name(&obj)));
        }
        out
    }

    fn task_state(e: &str) -> TaskState {
        match e {
            "DTS_Active" => TaskState::Active,
            "DTS_Completed" => TaskState::Completed,
            "DTS_Failed" => TaskState::Failed,
            _ => TaskState::Inactive,
        }
    }

    /// Spawned pawns -> `DisSeqVar_SpawnerPawn` values and `DisSeqEvent_Spawned` events.
    fn drain_spawns(&mut self, k: &mut Kismet) {
        let log = std::mem::take(&mut self.m.spawn_log);
        let mut keep = Vec::new();
        for i in log {
            let id = self.m.setup.original[i].spawn.id;
            let Some(&a) = self.st.spawner_actor.get(&i) else { continue };
            // Pawns spawned before their level's Kismet is loaded report once it is.
            let pkg = self.db.packages.iter().position(|p| p.eq_ignore_ascii_case(&self.db.actors[a as usize].package)).unwrap_or(0);
            if !k.loaded.get(pkg).copied().unwrap_or(false) {
                keep.push(i);
                continue;
            }
            let list = k.spawned.entry(a).or_default();
            if !list.contains(&id) {
                list.push(id);
            }
            let fired = k.trigger(self, "DisSeqEvent_Spawned", &[Obj::Actor(a)], Obj::Pawn(id), Some(vec![0]));
            if !fired.is_empty() {
                let n = &self.db.actors[a as usize];
                self.note(format!("spawned {}:{} as pawn {id} ({} Spawned event(s))", n.package, n.name, fired.len()));
            }
        }
        self.m.spawn_log.extend(keep);
    }

    fn players(&self) -> Vec<PlayerId> {
        self.m.humans.clone()
    }

    fn teleport(&mut self, target: &Obj, to: Vec3, yaw: Option<f32>) {
        if let Some(p) = self.pawn_of(target) {
            if let Some(pl) = self.m.world.player_mut(p) {
                pl.position = to + Vec3::Z * 5.0;
                pl.velocity = Vec3::ZERO;
                if let Some(y) = yaw {
                    pl.yaw = y;
                }
            }
            if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                b.home = to;
                b.goal = None;
                b.follow = Default::default();
            }
            return;
        }
        if let Obj::Actor(a) = target {
            self.st.actor_pos.insert(*a, to);
        }
    }

    fn destroy(&mut self, target: &Obj) {
        if let Some(p) = self.pawn_of(target) {
            self.m.world.players.retain(|x| x.id != p);
            self.m.crowd.brains.retain(|b| b.id != p);
            for ls in self.m.saved.values_mut() {
                ls.npcs.retain(|x| x.id != p);
                ls.brains.retain(|b| b.id != p);
            }
            return;
        }
        if let Obj::Actor(a) = target {
            self.st.gone.insert(*a);
        }
    }

    /// Pawns of an op's "Target" variables (spawned pawns of spawner variables).
    fn target_pawns(&self, cx: &Ctx) -> Vec<PlayerId> {
        cx.objects("Target").iter().filter_map(|o| self.pawn_of(o)).collect()
    }

    /// A running latent op got a new input impulse (not the one that started it this step).
    fn new_input(&self, cx: &Ctx) -> bool {
        cx.input().is_some() && self.st.act_time.get(&cx.op) != Some(&cx.k.time)
    }

    /// Whether a point is in a player's view (the original tests whether the pawn was
    /// rendered; headless, the port uses the player's view cone: 90 degrees wide, within
    /// 6000 uu; occlusion is not tested).
    fn seen_by_player(&self, p: Vec3) -> bool {
        self.m.humans.iter().filter_map(|h| self.m.world.player(*h)).filter(|h| h.alive).any(|h| {
            let eye = h.position + Vec3::Z * 64.0;
            let d = p + Vec3::Z * 40.0 - eye;
            let len = d.length();
            if len > 6000.0 {
                return false;
            }
            if len < 1.0 {
                return true;
            }
            let (yaw, pitch) = (h.yaw.to_radians(), h.pitch.to_radians());
            let fwd = Vec3::new(yaw.cos() * pitch.cos(), yaw.sin() * pitch.cos(), pitch.sin());
            fwd.dot(d / len) > (45f32).to_radians().cos()
        })
    }

    /// A door's (state, locked): the Kismet's, else the level's (`m_InitialDoorState`,
    /// `m_bLocked`).
    fn door(&self, a: u32) -> (u8, bool) {
        if let Some(s) = self.st.doors.get(&a) {
            return *s;
        }
        let act = &self.db.actors[a as usize];
        let state = match act.props.get("m_InitialDoorState").and_then(|v| v.as_str().map(str::to_string)).as_deref() {
            Some("DDS_Opened_CW") => 1,
            Some("DDS_Opened_CCW") => 2,
            _ => 0,
        };
        let locked = act.props.get("m_bLocked").and_then(|v| v.as_bool()).unwrap_or(false);
        (state, locked)
    }

    /// `m_bLocked` of a usable that is not a door: `DisSeqAct_Lock`'s, else the level's.
    fn usable_locked(&self, a: u32) -> bool {
        if let Some(l) = self.st.usable_locked.get(&a) {
            return *l;
        }
        let act = &self.db.actors[a as usize];
        !act.class.contains("Door") && act.props.get("m_bLocked").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    fn players_obj(&self) -> Obj {
        self.m.humans.first().map(|p| Obj::Pawn(*p)).unwrap_or_default()
    }

    /// [world] The world actors of the active map (breakables, batteries, receptacles).
    fn world_defs(&self) -> Option<std::sync::Arc<dishonored_ir::world_actors::WorldActors>> {
        self.m.setup.world_actors.get(&self.m.current_map.to_ascii_lowercase()).map(|w| std::sync::Arc::new(w.clone()))
    }

    /// [world] The Kismet actor of a placed actor (`package`, `name`).
    fn kismet_actor(&self, package: &str, name: &str) -> Option<u32> {
        self.db.find_actor(&format!("{package}:{name}"))
    }

    /// [world] Index of the breakable (battery) a Kismet object names, in the active map.
    fn world_battery(&self, defs: &dishonored_ir::world_actors::WorldActors, o: &Obj) -> Option<usize> {
        let Obj::Actor(a) = o else { return None };
        let act = &self.db.actors[*a as usize];
        dishonored_sim::world_actors::WorldActorsState::find(defs, Some(&act.package), &act.name).filter(|&i| defs.breakables[i].battery.is_some())
    }

    /// [world] Index of the receptacle a Kismet object names, in the active map.
    fn world_receptacle(&self, defs: &dishonored_ir::world_actors::WorldActors, o: &Obj) -> Option<usize> {
        let Obj::Actor(a) = o else { return None };
        let act = &self.db.actors[*a as usize];
        defs.receptacles.iter().position(|r| r.name.eq_ignore_ascii_case(&act.name) && r.package.eq_ignore_ascii_case(&act.package))
    }

    /// [distraction] Index into the crowd's distractors of a Kismet object.
    fn distractor_of(&self, o: &Obj) -> Option<usize> {
        let Obj::Actor(a) = o else { return None };
        let act = &self.db.actors[*a as usize];
        self.m.crowd.distractor(Some(&act.package), &act.name)
    }
}

/// [world] Breakable / battery / receptacle notifications -> their Kismet events, as the
/// original's `0x00bc7b30(actor, class, originator, instigator, 0, [output])` calls fire them:
/// `DisSeqEvent_BreakableBroken` (0x00a3a5f0: originator the breakable, no instigator),
/// `DisSeqEvent_WhaleOilBattery` (0x00c81d70 / 0x00c7d660: originator and instigator the
/// battery), `DisSeqEvent_WhaleOilReceptacle` (0x00c7b490 / 0x00c7b6b0: originator the
/// receptacle, instigator the battery). AI noises are heard next step.
fn world_events(k: &mut Kismet, h: &mut H, evs: Vec<dishonored_sim::world_actors::WorldEvent>) {
    use dishonored_sim::world_actors::WorldEvent;
    if evs.is_empty() {
        return;
    }
    let Some(defs) = h.world_defs() else { return };
    for e in evs {
        let (class, actor, instigator, output) = match e {
            WorldEvent::Noise(s) => {
                h.st.noises.push(s);
                continue;
            }
            WorldEvent::Broken { breakable, output } => {
                let d = &defs.breakables[breakable];
                ("DisSeqEvent_BreakableBroken", h.kismet_actor(&d.package, &d.name), Obj::None, output)
            }
            WorldEvent::Battery { battery, output } => {
                let d = &defs.breakables[battery];
                let a = h.kismet_actor(&d.package, &d.name);
                ("DisSeqEvent_WhaleOilBattery", a, a.map(Obj::Actor).unwrap_or_default(), output)
            }
            WorldEvent::Receptacle { receptacle, battery, output } => {
                let r = &defs.receptacles[receptacle];
                let b = &defs.breakables[battery];
                ("DisSeqEvent_WhaleOilReceptacle", h.kismet_actor(&r.package, &r.name), h.kismet_actor(&b.package, &b.name).map(Obj::Actor).unwrap_or_default(), output)
            }
        };
        let Some(a) = actor else { continue };
        let fired = k.trigger(h, class, &[Obj::Actor(a)], instigator, Some(vec![output]));
        if !fired.is_empty() {
            let n = &h.db.actors[a as usize];
            h.note(format!("{class} output {output} of {}:{}", n.package, n.name));
        }
    }
}

impl Host for H<'_> {
    fn flag(&mut self, name: &str) -> bool {
        self.m.state.flag(name)
    }

    fn set_flag(&mut self, name: &str, value: bool) {
        if self.m.state.flag(name) != value {
            self.note(format!("flag {name} = {value}"));
        }
        self.act(vec![if value { Action::SetFlag(name.to_string()) } else { Action::ClearFlag(name.to_string()) }]);
    }

    fn is_player(&self, o: &Obj) -> bool {
        matches!(o, Obj::Pawn(p) if self.m.humans.contains(p))
    }

    fn log(&mut self, text: &str) {
        let _ = text;
    }

    fn fires_event(&self, class: &str) -> bool {
        FED_EVENTS.iter().any(|c| self.db.is_a(class, c))
    }

    fn matinee(&mut self, cx: &mut Ctx, what: Matinee) {
        match what {
            Matinee::Started { length, .. } => {
                let now = cx.k.time;
                self.st.matinees.push((cx.op, length, now, None));
                cutscene::started(self, cx); // [interp]
            }
            Matinee::Finished { .. } => {
                let now = cx.k.time;
                if let Some(m) = self.st.matinees.iter_mut().rev().find(|m| m.0 == cx.op && m.3.is_none()) {
                    m.3 = Some(now);
                }
                cutscene::finished(self, cx.op); // [interp]
            }
            Matinee::Event(_) => {}
        }
    }

    fn toggle_actors(&mut self, _cx: &mut Ctx, targets: &[Obj], mode: u8) {
        for t in targets {
            let Obj::Actor(a) = t else { continue };
            // [distraction] `DisNPCDistractor.OnToggle` (0x00a4c350): the component's
            // `m_bDistractionEnabled`.
            if let Some(i) = self.distractor_of(t) {
                self.m.crowd.toggle_distractor(i, mode);
                continue;
            }
            let on = match mode {
                0 => true,
                1 => false,
                _ => self.st.off.contains(a),
            };
            if on {
                self.st.off.remove(a);
            } else {
                self.st.off.insert(*a);
            }
        }
    }

    fn activated(&mut self, cx: &mut Ctx) -> HostResult {
        self.st.act_time.insert(cx.op, cx.k.time);
        let class = cx.class().to_string();
        // The no-op classes are named once in the history (the trace's `KismetNote`s), so a
        // run lists what it skipped.
        if PRESENTATION.contains(&class.as_str()) {
            let n = self.st.presentation.entry(class.clone()).or_insert(0);
            *n += 1;
            if *n == 1 {
                self.note(format!("not modelled (presentation) Kismet class {class} (first at {})", cx.describe()));
            }
            return HostResult::Done;
        }
        if NOT_MODELLED.contains(&class.as_str()) {
            let n = self.st.not_modelled.entry(class.clone()).or_insert(0);
            *n += 1;
            if *n == 1 {
                self.note(format!("not modelled (no such system) Kismet class {class} (first at {})", cx.describe()));
            }
            return HostResult::Done;
        }
        match class.as_str() {
            // `DisSeqAct_PlayerTrackTarget::Activated` (0x00b8f9c0): Start with a target asks the
            // player camera to track it (0x00aaf4c0: flags 0xC3, 0xC7 with
            // `m_bHoldUntilStopped`; the target's skeletal mesh and `m_TrackedBone` /
            // `m_TrackedBoneOffset`, freedom pitch / yaw, `m_fBlendTime`) and keeps the returned
            // ID in `m_TrackingID`; Stop ends the tracking with that ID (0x00aa0ec0).
            "DisSeqAct_PlayerTrackTarget" => {
                if cx.input() == Some(1) {
                    let id = cx.prop_int("m_TrackingID");
                    self.st.tracking.retain(|_, t| t.id != id);
                    self.note(format!("view tracking {id} stopped"));
                    return HostResult::Done;
                }
                let target = cx.objects("TrackedTarget").into_iter().next().or_else(|| Some(cx.prop_obj("m_pTrackedTarget")).filter(|o| !o.is_none()));
                let Some(target) = target else { return HostResult::Done };
                let Some(point) = self.pos_of(&target) else { return HostResult::Done };
                let pawn = self.pawn_of(&target);
                let blend = cx.prop_float("m_fBlendTime");
                let freedom = (cx.prop_float("m_fRotationFreedomYaw"), cx.prop_float("m_fRotationFreedomPitch"));
                let hold = cx.prop("m_bHoldUntilStopped").and_then(|v| v.as_bool()).unwrap_or(false);
                self.st.track_id += 1;
                let id = self.st.track_id;
                for p in self.players() {
                    let Some(pl) = self.m.world.player(p) else { continue };
                    let t = ViewTrack { id, pawn, point, start: (pl.yaw, pl.pitch), alpha: if blend > 1e-4 { 0.0 } else { 1.0 }, rate: if blend > 1e-4 { 1.0 / blend } else { 0.0 }, freedom, hold, want0: None };
                    self.st.tracking.insert(p, t);
                }
                cx.set_prop("m_TrackingID", Value::Int(id));
                cx.set_link("TrackingID", Value::Int(id));
                self.note(format!("view tracking {id} of {} (blend {blend} s, freedom {freedom:?})", obj_name(&target)));
                HostResult::Done
            }
            // ----- story flags -----
            "DisSeqAct_SetStoryFlag" => {
                let name = cx.prop_str("m_StoryFlagName");
                let v = match cx.input() {
                    Some(1) => false,
                    Some(2) => !self.m.state.flag(&name),
                    _ => true,
                };
                self.set_flag(&name, v);
                HostResult::Done
            }
            "DisSeqCond_CheckStoryFlag" => {
                let name = cx.prop_str("m_StoryFlagName");
                let set = self.m.state.flag(&name);
                cx.fire(if set { 0 } else { 1 });
                HostResult::Done
            }
            // `DisSeqCond_CompareBoolExtended::Activated` (0x00b9b3e0): over the "Bool" variables,
            // `m_bResultAnd` (starts true) ANDs and `m_bResultOr` (starts false) ORs their
            // values; output 0 when all are true, else output 2 when some are, else output 1.
            "DisSeqCond_CompareBoolExtended" => {
                let vals = cx.bools("Bool");
                let and = vals.iter().all(|b| *b);
                let or = vals.iter().any(|b| *b);
                cx.set_prop("m_bResultAnd", Value::Bool(and));
                cx.set_prop("m_bResultOr", Value::Bool(or));
                cx.fire(if and { 0 } else if or { 2 } else { 1 });
                HostResult::Done
            }
            // `DisSeqCond_IsSentinel::Activated` (0x00b8f070): output 0 when the game has an
            // automated-test manager (`GameInfo.MyAutoTestManager`, +0x304 of
            // `WorldInfo.Game`), else output 1. The port never runs the Sentinel test mode.
            "DisSeqCond_IsSentinel" => {
                cx.fire(1);
                HostResult::Done
            }
            // `PlayerController.OnToggleCinematicMode` -> `DishonoredPlayerController.
            // SetCinematicMode` (script) for each player target: input 0 enables, 1 disables, 2
            // toggles `bCinematicMode`. `PlayerController.SetCinematicMode` ignores the move /
            // look input (`bDisableMovement` / `bDisableTurning`) while on and gives it back
            // when off (only when the state of that input changes); the HUD (`bHideHUD`; the
            // Dishonored override passes it to the native) and the player (`bHidePlayer`) are
            // hidden. The native 0x00aaf150 also swaps the pawn's action permissions when the
            // mode starts (disables mask 0x7faf, enables 0x6010; ends: the reverse): the player
            // cannot act in cinematic mode (applied to the buttons in `cinematic_inputs`).
            "SeqAct_ToggleCinematicMode" => {
                let targets: Vec<PlayerId> = cx.objects("Target").iter().filter_map(|o| self.pawn_of(o)).filter(|p| self.m.humans.contains(p)).collect();
                let input = cx.input();
                let (mv, turn, hud, hide) = (cx.prop_bool("bDisableMovement"), cx.prop_bool("bDisableTurning"), cx.prop_bool("bHideHUD"), cx.prop_bool("bHidePlayer"));
                for p in targets {
                    let c = self.st.cinematic.entry(p).or_default();
                    let on = match input {
                        Some(0) => true,
                        Some(1) => false,
                        Some(2) => !c.on,
                        _ => continue,
                    };
                    c.on = on;
                    if mv && on != c.no_move {
                        c.no_move = on;
                    }
                    if turn && on != c.no_look {
                        c.no_look = on;
                    }
                    if hud {
                        c.hide_hud = on;
                    }
                    c.hide_player = on && hide;
                    let c = *c;
                    self.note(format!("cinematic mode {} for player {p} (move {}, look {}, HUD {})", if on { "on" } else { "off" }, !c.no_move, !c.no_look, !c.hide_hud));
                }
                HostResult::Done
            }
            // `SeqAct_SetMatInstScalarParam::Activated` (0x006d1d90): without
            // `m_pMaterialOwnerPawn`, `MatInst.SetScalarParameterValue(ParamName, ScalarValue)`;
            // with it, the same on the material instance at `m_iMaterialIndex` of that pawn's
            // mesh (a constant instance only).
            "SeqAct_SetMatInstScalarParam" => {
                let param = cx.prop_str("ParamName");
                let v = cx.prop_float("ScalarValue");
                let owner = cx.prop_obj("m_pMaterialOwnerPawn");
                if !owner.is_none() {
                    if let Some(p) = self.pawn_of(&owner) {
                        let idx = cx.prop_int("m_iMaterialIndex");
                        self.st.pawn_mat_scalars.entry((p, idx)).or_default().insert(param, v);
                    }
                } else if let Obj::Path(mi) = cx.prop_obj("MatInst") {
                    self.st.mat_scalars.entry(mi).or_default().insert(param, v);
                }
                HostResult::Done
            }
            // `SeqAct_SetMaterial` -> `OnSetMaterial` of each target (`DynamicSMActor`,
            // `SkeletalMeshActor`: their mesh component; `Pawn`: its `Mesh`):
            // `SetMaterial(MaterialIndex, NewMaterial)`.
            "SeqAct_SetMaterial" => {
                let Obj::Path(mat) = cx.prop_obj("NewMaterial") else { return HostResult::Done };
                let idx = cx.prop_int("MaterialIndex");
                for t in cx.objects("Target") {
                    let t = self.pawn_of(&t).map(Obj::Pawn).unwrap_or(t);
                    self.st.materials.entry(t).or_default().insert(idx, mat.clone());
                }
                HostResult::Done
            }
            // `DisSeqAct_BackupAndClearInventory::Activated` (0x00ba91d0): "Out" fires at once.
            // Each player target whose hands are not both empty (0x00b4e070: hand slots 1 and 2
            // hold nothing) is asked to unequip slots 1 and 2 (0x00b51790) and the op waits
            // (`m_bTryingToUnequip`); Update (0x00baf050) waits until every target's hands are
            // empty. Then the inventory is backed up and cleared (0x00ba8980) and "Finished
            // (After Unequip)" (output 1) fires; the op ends (`SeqAct_Latent`'s DeActivated
            // fires "Out" again). The port has no hand-slot unequip request (the hands follow the
            // player's input, with no holster time), so it takes the original's path for empty
            // hands: both outputs in `Activated`, finished in the first update. This is what
            // lets `fromHub_toFloodedDistrict` reach its travel destination and commit (the
            // L_Flooded_FIntro entry stall). NOT PORTED: the unequip itself and the inventory
            // backup and clear (0x00ba8980: items, upgrades and abstract items minus the skip
            // lists).
            "DisSeqAct_BackupAndClearInventory" => {
                cx.fire(0);
                cx.fire(1);
                HostResult::Done
            }
            // `DisSeqAct_AIGuard` -> `ADishonoredNPCController::OnAIGuard` (0x00b63a80): Start
            // gives the NPC a guard stimulus on `m_pHomeActor` (a controller's pawn) with
            // `m_fGuardAwarenessRadius` (0x00b18820); Stop ends it. The port's brain walks to
            // the home actor and holds there; the awareness radius is not modelled.
            "DisSeqAct_AIGuard" => {
                let start = cx.input() != Some(1);
                let home = cx.objects("Home Actor").into_iter().next().or_else(|| Some(cx.prop_obj("m_pHomeActor")).filter(|o| !o.is_none())).and_then(|o| self.pos_of(&o));
                for p in self.target_pawns(cx) {
                    let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) else { continue };
                    match (start, home) {
                        (true, Some(h)) => {
                            b.home = h;
                            b.goal = Some(h);
                            b.stay = true;
                            if b.state.is_routine() {
                                b.state = BrainState::WalkToPoint;
                            }
                        }
                        (false, _) => b.stay = false,
                        _ => {}
                    }
                }
                HostResult::Done
            }
            // ----- objectives -----
            "DisSeqAct_AddObjective" | "DisSeqAct_RemoveObjective" | "DisSeqAct_SetObjectiveHidden" => {
                let show = match class.as_str() {
                    "DisSeqAct_AddObjective" => true,
                    "DisSeqAct_RemoveObjective" => false,
                    _ => !cx.prop("m_bHidden").and_then(|v| v.as_bool()).unwrap_or(true),
                };
                let objs: Vec<String> = cx.objects("Objective").iter().map(obj_name).collect();
                for o in objs {
                    self.note(format!("objective {o} {}", if show { "added" } else { "hidden" }));
                    self.act(vec![if show { Action::ShowObjective(o) } else { Action::HideObjective(o) }]);
                }
                HostResult::Done
            }
            "DisSeqAct_SetTaskState" => {
                let st = Self::task_state(&cx.prop_str("m_TaskState"));
                for t in self.tasks(cx, "Task") {
                    if self.m.state.task(&t) != Some(st) {
                        self.note(format!("task {t} -> {st:?}"));
                    }
                    self.act(vec![Action::SetTask(t, st)]);
                }
                HostResult::Done
            }
            "DisSeqAct_SetTaskHidden" => {
                let hide = cx.prop("m_bHidden").and_then(|v| v.as_bool()).unwrap_or(true);
                for t in self.tasks(cx, "Task") {
                    self.act(vec![if hide { Action::HideTask(t) } else { Action::ShowTask(t) }]);
                }
                HostResult::Done
            }
            "DisSeqAct_UpdateTaskTarget" => {
                let target = cx.linked_values("Target").into_iter().next().or_else(|| Some(cx.prop_obj("m_pTargetActor")).filter(|o| !o.is_none()));
                for t in self.tasks(cx, "Task") {
                    match &target {
                        Some(o) => {
                            self.st.targets.insert(t, o.clone());
                        }
                        None => {
                            self.st.targets.remove(&t);
                        }
                    }
                }
                HostResult::Done
            }
            "DisSeqCond_TaskState" => {
                if let Some(t) = self.tasks(cx, "Task").into_iter().next() {
                    let i = match self.m.state.task(&t).unwrap_or_default() {
                        TaskState::Inactive => 0,
                        TaskState::Active => 1,
                        TaskState::Completed => 2,
                        TaskState::Failed => 3,
                    };
                    cx.fire(i);
                }
                HostResult::Done
            }
            "DisSeqAct_ShowHUDMessage" => {
                let m = cx.prop_str("m_Message");
                self.act(vec![Action::Message(m)]);
                HostResult::Done
            }
            "DisSeqAct_ShowLocationDiscovery" => {
                let m = cx.prop_str("m_LocationName");
                self.act(vec![Action::Message(m)]);
                HostResult::Done
            }
            "DisSeqAct_ShowTargetNotification" => {
                let state = cx.prop_str("m_TargetNotification");
                let target = obj_name(&cx.prop_obj("m_pChapterTarget"));
                self.note(format!("target notification {state} {target}"));
                self.act(vec![Action::Notify(state.trim_start_matches("DTSN_").to_string(), target)]);
                HostResult::Done
            }
            // ----- inventory -----
            // `m_pItemToAdd` / `m_pItemToRemove`, `m_Quantity` (add: default 1; remove: 0 =
            // all of it). The players' abstract inventory is shared with the conversations
            // (`DisConv_HasAbstractItem`).
            "DisSeqAct_AddAbstractItem" | "DisSeqAct_RemoveAbstractItem" => {
                let add = class == "DisSeqAct_AddAbstractItem";
                let item = obj_name(&cx.prop_obj(if add { "m_pItemToAdd" } else { "m_pItemToRemove" }));
                if item.is_empty() || item == "None" {
                    return HostResult::Done;
                }
                let q = cx.prop("m_Quantity").and_then(|v| v.as_int()).unwrap_or(if add { 1 } else { 0 });
                let have = self.m.state.items.get(&item).copied().unwrap_or(0);
                let now = if add { have + q.max(1) } else if q <= 0 { 0 } else { (have - q).max(0) };
                self.note(format!("abstract item {item}: {have} -> {now}"));
                if now > 0 {
                    self.m.state.items.insert(item, now);
                } else {
                    self.m.state.items.remove(&item);
                }
                HostResult::Done
            }
            // `DisSeqAct_SetBoneCharmEffect` -> `DisWhaleBoneCharm.OnSetBoneCharmEffect` (slot 241,
            // 0x00A22970): the charm targets take the op's `m_Effect` (a regular effect they held
            // goes back to the game's pool, which the port does not model, U-W12).
            "DisSeqAct_SetBoneCharmEffect" => {
                let e = cx.prop("m_Effect").and_then(|v| v.as_int()).unwrap_or(-1);
                for t in cx.objects("Target") {
                    let Obj::Actor(a) = t else { continue };
                    let act = &self.db.actors[a as usize];
                    let (name, pkg) = (act.name.clone(), act.package.clone());
                    self.st.charm_effect.insert(a, e);
                    for pk in self.m.world.pickups.iter_mut().filter(|p| p.name.eq_ignore_ascii_case(&name) && (p.package.is_empty() || p.package.eq_ignore_ascii_case(&pkg))) {
                        if let dishonored_ir::items::PickupKind::BoneCharm(x) = &mut pk.kind {
                            *x = (e >= 0).then_some(e);
                        }
                    }
                    self.note(format!("bone charm {name}: effect {e}"));
                }
                HostResult::Done
            }
            // `DisSeqAct_GiveUpgrade` -> `DishonoredPlayerPawn.OnGiveUpgrade` (0x009EF300): the
            // player owns the `m_pUpgrade` workshop upgrade (its attributes apply).
            "DisSeqAct_GiveUpgrade" => {
                let up = obj_name(&cx.prop_obj("m_pUpgrade"));
                let name = up.trim_start_matches("Twk_Upgrade_").to_string();
                for p in self.players() {
                    if let Some(pl) = self.m.world.player_mut(p) {
                        if !dishonored_sim::powers::give_upgrade(pl, &name, &self.cfg.gameplay) {
                            self.note(format!("upgrade {up}: not in the config"));
                        }
                    }
                }
                self.note(format!("upgrade {up} given"));
                HostResult::Done
            }
            // `DisSeqAct_AddInventoryItem` -> `DishonoredPawn.OnAddInventoryItem` (0x009EC950): the
            // `m_pItemToAdd` inventory item joins the player's inventory.
            "DisSeqAct_AddInventoryItem" => {
                let Obj::Path(item) = cx.prop_obj("m_pItemToAdd") else { return HostResult::Done };
                for p in self.players() {
                    dishonored_sim::inventory::give_item(&mut self.m.world, p, &item, 1);
                }
                self.note(format!("inventory item {item} added"));
                HostResult::Done
            }
            // `DisSeqAct_ModifyElixirCount`: the pawn's elixir count (`ModifyElixirCount`
            // 0x00C04E10, clamped to [0, max]) for `m_ElixirType` by `m_ElixirOp` (add, remove,
            // set) and `m_Value`.
            "DisSeqAct_ModifyElixirCount" => {
                let kind = if cx.prop_str("m_ElixirType").contains("Mana") { dishonored_ir::items::ElixirType::Mana } else { dishonored_ir::items::ElixirType::Health };
                let op = cx.prop_str("m_ElixirOp");
                let v = cx.prop_int("m_Value");
                for p in self.players() {
                    if let Some(pl) = self.m.world.player_mut(p) {
                        let cur = pl.inv.elixirs[kind as usize] as i32;
                        let delta = match op.as_str() {
                            "eDisElixirOp_RemoveElixirs" => -v,
                            "eDisElixirOp_SetElixirCount" => v - cur,
                            _ => v,
                        };
                        dishonored_sim::inventory::add_elixirs(pl, kind, delta, &self.cfg.gameplay);
                    }
                }
                HostResult::Done
            }
            // `DisSeqAct_GetAbstractItemQuantity`: the player's count of `m_pItem` into
            // `m_Quantity` (and its linked variable).
            "DisSeqAct_GetAbstractItemQuantity" => {
                let item = obj_name(&cx.prop_obj("m_pItem"));
                let p = self.players().first().and_then(|p| self.m.world.player(*p)).copied();
                let q = match (item.as_str(), p) {
                    (dishonored_sim::inventory::COINS_ITEM, Some(p)) => p.inv.coins as i32,
                    (dishonored_sim::inventory::RUNE_ITEM, Some(p)) => p.inv.runes as i32,
                    _ => self.m.state.items.iter().filter(|(k, _)| k.eq_ignore_ascii_case(&item)).map(|(_, v)| *v).sum(),
                };
                cx.set_prop("m_Quantity", Value::Int(q));
                cx.set_link("Quantity", Value::Int(q));
                HostResult::Done
            }
            // ----- travel -----
            "DisSeqAct_SetPlayerTravelDestination" => {
                let tag = cx.prop_str("m_Tag");
                self.st.travel = Some((tag.clone(), self.m.current_map.clone()));
                self.note(format!("travel destination {tag}"));
                HostResult::Done
            }
            "DisSeqAct_GotoPlayerTravelDestination" => {
                // Teleports only when arriving from another map (decoded).
                if let Some((tag, from)) = self.st.travel.clone() {
                    if !from.eq_ignore_ascii_case(&self.m.current_map) {
                        if let Some(t) = self.m.setup.tagged_starts.iter().find(|t| t.0.eq_ignore_ascii_case(&self.m.current_map) && t.1.eq_ignore_ascii_case(&tag)).cloned() {
                            for (i, h) in self.players().into_iter().enumerate() {
                                let (p, y) = MissionMatch::fan(i, (t.2, t.3));
                                if let Some(pl) = self.m.world.player_mut(h) {
                                    pl.position = p;
                                    pl.yaw = y;
                                    pl.pitch = 0.0;
                                    pl.velocity = Vec3::ZERO;
                                }
                            }
                        }
                        self.st.travel = None;
                    }
                }
                HostResult::Done
            }
            "SeqAct_PrepareMapChange" => {
                let map = cx.prop_str("MainLevelName");
                self.st.pending_map = Some(map.clone());
                self.note(format!("prepare map change {map}"));
                HostResult::Latent
            }
            "SeqAct_CommitMapChange" => {
                // Applied after the Kismet tick (the match swaps maps between steps).
                self.st.commit = true;
                HostResult::Done
            }
            "DisSeqAct_SaveLevelState" | "DisSeqAct_DiscardLevelState" | "DisSeqAct_DiscardAllLevelStates" => {
                // The match keeps every visited map's state (NPCs, brains) and the Kismet keeps
                // each level's sequences; nothing else to do here.
                HostResult::Done
            }
            // ----- NPCs -----
            "DisSeqAct_StartSpawn" => {
                let spawners: Vec<usize> = cx.linked_values("Target").into_iter().filter_map(|o| if let Obj::Actor(a) = o { self.st.spawner.get(&a).copied() } else { None }).collect();
                cx.fire(0);
                self.st.spawning.insert(cx.op, (spawners, Vec::new()));
                HostResult::Latent
            }
            // `DisSeqAct_AIGoToActor` (decoded): Activated is empty; Update (0x00b90530) on Start
            // runs the target handlers (each pawn starts a `DisBehaviorGoTo` and registers as a
            // latent actor) and fires "Out"; Abort sets `m_bAbortRequested` and finishes at once;
            // otherwise it finishes when no latent actor remains. DeActivated (0x00ba1ba0) only
            // clears the latent list: the results come from the behaviour through the pawn
            // (0x00b4abb0 -> slot 101, 0x00ba1eb0): Failed when the path request fails
            // (0x00ae8600), Success when the behaviour completes (arrived and turned,
            // `m_bSuccess`), Interrupted when it ends any other way (0x00ae4d60). Failed and
            // Interrupted fire per pawn; Success fires once the last pawn has reported.
            "DisSeqAct_AIGoToActor" => {
                if cx.input() != Some(0) {
                    // Abort (or no input): finished, no output.
                    return HostResult::Done;
                }
                let pawns: Vec<PlayerId> = cx.objects("Target").into_iter().filter_map(|o| self.pawn_of(&o)).collect();
                let dest = cx.objects("Destination").into_iter().next().or_else(|| Some(cx.prop_obj("m_pDestinationActor")).filter(|o| !o.is_none()));
                let goal = dest.as_ref().and_then(|d| self.pos_of(d));
                let yaw = match &dest {
                    Some(Obj::Actor(a)) if cx.prop("m_bAlignRotation").and_then(|v| v.as_bool()).unwrap_or(true) => Some(self.db.actors[*a as usize].yaw_deg()),
                    _ => None,
                };
                cx.fire(0);
                // Pawns that cannot take the behaviour (dead, no controller) or no destination:
                // no latent actor, so the action finishes without a result.
                let Some(g) = goal else {
                    return HostResult::Done;
                };
                let run = cx.prop_str("m_DesiredMovementSpeed").to_ascii_lowercase().contains("run");
                let mut started = Vec::new();
                for p in pawns {
                    if !self.m.world.player(p).is_some_and(|x| x.alive) {
                        continue;
                    }
                    let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) else { continue };
                    b.goal = Some(g);
                    b.run = run;
                    b.spot = None;
                    b.stay = true;
                    b.state = BrainState::WalkToPoint;
                    b.follow = Default::default();
                    if cx.prop("m_bSetNewHomeActor").and_then(|v| v.as_bool()).unwrap_or(true) {
                        b.home = g;
                    }
                    started.push(p);
                }
                if started.is_empty() {
                    return HostResult::Done;
                }
                self.st.goto.insert(cx.op, GoTo { pawns: started, yaw, last_ok: false });
                HostResult::Latent
            }
            "DisSeqAct_AISetPatrol" => {
                for o in cx.objects("Target") {
                    if let Some(p) = self.pawn_of(&o) {
                        if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                            b.stay = false;
                            if b.state.is_routine() {
                                b.state = BrainState::Wander;
                            }
                        }
                    }
                }
                HostResult::Done
            }
            // [ai] `DisTweaks_Faction::OnSetDisposition` (0x00c79000) / `ADishonoredPawn::
            // OnSetDisposition`: inputs Set / Clear / ClearAll (`EDisSetDispositionKismetInput`);
            // the targets (factions or pawns) take `m_Disposition` toward `m_Recipients`
            // (factions or pawns), and back with `m_bReciprocate`.
            "DisSeqAct_SetDisposition" => {
                let input = cx.input().unwrap_or(0).min(2) as u8;
                let rel = match cx.prop_str("m_Disposition").as_str() {
                    "Relationship_Ally" => dishonored_ir::ai::Relationship::Ally,
                    "Relationship_Neutral" => dishonored_ir::ai::Relationship::Neutral,
                    _ => dishonored_ir::ai::Relationship::Enemy,
                };
                let reciprocate = cx.prop("m_bReciprocate").and_then(|v| v.as_bool()).unwrap_or(true);
                let recip_objs: Vec<Obj> = cx.prop("m_Recipients").map(|v| v.as_array().iter().filter_map(|x| x.as_obj().cloned()).collect()).unwrap_or_default();
                let mut recipients = Vec::new();
                for o in &recip_objs {
                    match o {
                        Obj::Path(p) => recipients.push(dishonored_ai::Recipient::Faction(self.m.crowd.factions.intern(p))),
                        other => {
                            if let Some(p) = self.pawn_of(other) {
                                recipients.push(dishonored_ai::Recipient::Pawn(p));
                            }
                        }
                    }
                }
                for t in cx.objects("Target") {
                    match &t {
                        Obj::Path(f) => self.m.crowd.set_disposition(f, input, rel, &recipients, reciprocate),
                        other => {
                            let Some(p) = self.pawn_of(other) else { continue };
                            self.m.crowd.set_pawn_disposition(p, input, rel, &recipients);
                        }
                    }
                }
                self.note(format!("disposition {:?} input {input} to {} recipients", rel, recipients.len()));
                HostResult::Done
            }
            // [ai] Deaf / blind / numb NPCs (`m_bDeaf`, `m_bBlind`, `m_bNumb`).
            "DisSeqAct_AISetSenses" => {
                let (deaf, blind, numb) = (cx.prop_bool("m_bDeaf"), cx.prop_bool("m_bBlind"), cx.prop_bool("m_bNumb"));
                for o in cx.objects("Target") {
                    if let Some(p) = self.pawn_of(&o) {
                        if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                            b.deaf = deaf;
                            b.blind = blind;
                            b.numb = numb;
                        }
                    }
                }
                HostResult::Done
            }
            // `ADishonoredPlayerPawn::OnGivePickup` (native 0x009ef600 -> pawn virtual +0x7c8):
            // the player receives what the `m_pPickup` tweak gives, as if picked up.
            "DisSeqAct_GivePickup" => {
                let Some(kind) = self.setup.gives.get(&cx.op).cloned() else {
                    self.note(format!("give pickup {}: tweak not resolved", obj_name(&cx.prop_obj("m_pPickup"))));
                    return HostResult::Done;
                };
                for p in self.players() {
                    match &kind {
                        dishonored_ir::items::PickupKind::Abstract { item, quantity, .. } => {
                            let short = item.rsplit(['.', ':']).next().unwrap_or(item).to_string();
                            *self.m.state.items.entry(short.clone()).or_insert(0) += (*quantity).max(1);
                            dishonored_sim::inventory::give_item(&mut self.m.world, p, item, (*quantity).max(1));
                            self.note(format!("given {short}"));
                        }
                        dishonored_ir::items::PickupKind::Inventory { item } => {
                            dishonored_sim::inventory::give_item(&mut self.m.world, p, item, 1);
                            self.note(format!("given item {item}"));
                        }
                        dishonored_ir::items::PickupKind::Stat { health, mana, .. } => {
                            let v = self.cfg.gameplay.vitals.clone();
                            if let Some(pl) = self.m.world.player_mut(p) {
                                pl.vitals.health = (pl.vitals.health + *health as f32).min(v.health_max);
                                pl.vitals.mana = (pl.vitals.mana + *mana as f32).min(v.mana_max);
                            }
                        }
                        _ => {}
                    }
                }
                HostResult::Done
            }
            // [ai] The NPC controller handlers (`ADishonoredNPCController::OnAI*`, natives
            // 0x009f4d70.. calling controller virtuals +0x45c..+0x490):
            // `OnAIDoSearch` (0x00b63770): Start pushes a search from `m_pSearchStart`, Abort
            // stops it.
            "DisSeqAct_AIDoSearch" => {
                let abort = cx.input() == Some(1);
                let from = cx.objects("Search Start").into_iter().next().or_else(|| Some(cx.prop_obj("m_pSearchStart")).filter(|o| !o.is_none())).and_then(|o| self.pos_of(&o));
                let crumbs = self.m.crowd.ai.search.crumbs;
                for p in self.target_pawns(cx) {
                    let pos = self.m.world.player(p).map(|x| x.position);
                    let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) else { continue };
                    if abort {
                        if b.state == BrainState::Search {
                            b.state = BrainState::Idle;
                            b.threat_pos = None;
                        }
                        continue;
                    }
                    b.threat_pos = from.or(pos);
                    b.suspecting = true;
                    b.target = None;
                    b.state = BrainState::Search;
                    b.crumbs = crumbs;
                    b.goal = None;
                    b.timer = 0.0;
                    b.follow.clear();
                }
                HostResult::Done
            }
            // `OnAISetSuspicionLevel` (0x00b4e770): the brain's suspicion level becomes
            // `m_SuspicionLevel` (`DAISL_Unsuspecting` / `DAISL_Suspecting`).
            "DisSeqAct_AISetSuspicionLevel" => {
                let sus = cx.prop_str("m_SuspicionLevel") == "DAISL_Suspecting";
                for p in self.target_pawns(cx) {
                    if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                        b.suspecting = sus;
                    }
                }
                HostResult::Done
            }
            // `OnAIClearAttention` (0x00b58f70): clears the attention the brain has for the
            // listed objects (all of it when the list is empty) for `m_fForHowLong`.
            "DisSeqAct_AIClearAttention" => {
                let only: Vec<PlayerId> = cx.objects("Attention Target").iter().filter_map(|o| self.pawn_of(o)).collect();
                for p in self.target_pawns(cx) {
                    let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) else { continue };
                    if only.is_empty() {
                        b.calm();
                    } else {
                        b.attn.retain(|a| !only.contains(&a.who));
                        b.hostile.retain(|h| !only.contains(h));
                        if b.target.is_some_and(|t| only.contains(&t)) {
                            b.target = None;
                        }
                    }
                }
                HostResult::Done
            }
            // `OnAIPsychicAttention` (0x00b59080): Start gives the brain full (psychic)
            // attention to the object, Stop removes it.
            "DisSeqAct_AIPsychicAttention" => {
                let start = cx.input() != Some(1);
                let on: Vec<PlayerId> = cx.objects("Attention Target").iter().filter_map(|o| self.pawn_of(o)).collect();
                let on = if on.is_empty() { self.players() } else { on };
                let ai = self.m.crowd.ai.clone();
                for p in self.target_pawns(cx) {
                    for t in &on {
                        let at = self.m.world.player(*t).map(|x| x.position).unwrap_or_default();
                        let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) else { continue };
                        if start {
                            b.aware_of(&ai, *t, at);
                        } else {
                            b.attn.retain(|a| a.who != *t);
                        }
                    }
                }
                HostResult::Done
            }
            // `OnAIRingAlarm` (0x00b63fa0): the NPC goes to ring an alarm against `m_pEnemy`
            // (the player when none).
            "DisSeqAct_AIRingAlarm" => {
                let enemy = cx.objects("Enemy").into_iter().find_map(|o| self.pawn_of(&o)).or_else(|| self.players().first().copied());
                let ai = self.m.crowd.ai.clone();
                for p in self.target_pawns(cx) {
                    let Some(e) = enemy else { continue };
                    let at = self.m.world.player(e).map(|x| x.position).unwrap_or_default();
                    if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                        b.alert_to(&ai, e, at);
                        b.alarm_enemy = Some(e);
                    }
                }
                HostResult::Done
            }
            // `DisSeqAct_AIShoot` (shares `AIGoToActor`'s Update 0x00b90530 and DeActivated):
            // Start runs `OnAIShoot` (0x00b63d00) on each pawn: a shoot already running reports
            // Interrupted, then the pawn shoots at "ShotTarget" (`m_iMaxNumShots`, unlimited
            // when < 1; `m_bOverrideAccuracy`) and reports Success when its shots are done,
            // Interrupted when the shoot behaviour ends otherwise. Abort stops it (no output).
            "DisSeqAct_AIShoot" => {
                if cx.input() != Some(0) {
                    // Abort (`OnAIShoot` with the abort flag): the shoot behaviour stops.
                    for (p, _) in self.st.shooting.remove(&cx.op).unwrap_or_default() {
                        if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                            b.shoot = None;
                        }
                    }
                    return HostResult::Done;
                }
                let target = cx.objects("ShotTarget").into_iter().next().or_else(|| Some(cx.prop_obj("m_pShotTarget")).filter(|o| !o.is_none())).and_then(|o| self.pawn_of(&o));
                let max = cx.prop("m_iMaxNumShots").and_then(|v| v.as_int()).unwrap_or(-1);
                let accuracy = cx.prop("m_bOverrideAccuracy").and_then(|v| v.as_bool()).unwrap_or(false).then(|| cx.prop_float("m_fOverrideAccuracyPercentage"));
                cx.fire(0);
                let Some(t) = target else { return HostResult::Done };
                let mut list = Vec::new();
                for p in self.target_pawns(cx) {
                    let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) else { continue };
                    // The stimulus drives `DisBehaviorShoot`; the target is not made an enemy.
                    b.order_shoot(t, max, accuracy);
                    self.note(format!("pawn {p} shoots at {t}"));
                    let fired = self.st.shots.get(&p).copied().unwrap_or(0);
                    list.push((p, if max < 1 { u32::MAX } else { fired + max as u32 }));
                }
                if list.is_empty() {
                    return HostResult::Done;
                }
                self.st.shooting.insert(cx.op, list);
                HostResult::Latent
            }
            // `DisSeqAct_AINoise::Activated` (0x00b8aee0): with a noise maker, an AI noise at
            // the noise location (else the maker) whose radius is `m_LoudnessDistanceArray`
            // [`m_eNoiseLoudness`] (or `m_fCustomSoundRadius` for the custom value), heard by
            // the NPCs as any noise.
            "DisSeqAct_AINoise" => {
                let maker = cx.objects("NoiseMaker").into_iter().next().or_else(|| Some(cx.prop_obj("m_pNoiseMaker")).filter(|o| !o.is_none()));
                let Some(maker) = maker else { return HostResult::Done };
                let loc = cx.objects("NoiseLocation").into_iter().next().or_else(|| Some(cx.prop_obj("m_pNoiseLocation")).filter(|o| !o.is_none())).unwrap_or(maker.clone());
                let names = ["EAINoiseSilent", "EAINoiseLoudness2m", "EAINoiseWhisper", "EAINoiseLoudness4m", "EAINoiseLoudness1", "EAINoiseLoudness6m", "EAINoiseLoudness8m", "EAINoiseLoudness10m", "EAINoiseLoudness12m", "EAINoiseLoudness2", "EAINoiseLoudness20m", "EAINoiseLoudness25m", "EAINoiseLoudness3", "EAINoiseLoudness40m", "EAINoiseLoudness50m", "EAINoiseLoudness4", "EAINoise_CUSTOM_KISMET_SPECIFIED"];
                let lname = cx.prop_str("m_eNoiseLoudness");
                let li = names.iter().position(|n| *n == lname).unwrap_or(0);
                let radius = if li == 16 { cx.prop_float("m_fCustomSoundRadius") } else { self.m.crowd.ai.loudness_distances.get(li).copied().unwrap_or(0.0) };
                let (Some(pos), true) = (self.pos_of(&loc), radius > 0.0) else { return HostResult::Done };
                let source = self.pawn_of(&maker).unwrap_or(0);
                // `m_eNoiseContext` (`EDisAINoiseLimitedContext`) -> `EAINoiseContext` (0x00B863C0).
                use dishonored_sim::events::NoiseContext;
                let context = match cx.prop_str("m_eNoiseContext").as_str() {
                    "EDisAINoiseLimitedContext_Threatening" => NoiseContext::Threatening,
                    "EDisAINoiseLimitedContext_Danger" => NoiseContext::Danger,
                    "EDisAINoiseLimitedContext_AlarmBell" => NoiseContext::AlarmBell,
                    _ => NoiseContext::Anomaly,
                };
                self.st.noises.push(SimEvent::Sound { position: pos, radius, category: dishonored_sim::events::SoundCategory::Scripted, context, source });
                self.note(format!("AI noise {radius:.0} uu at {:.0},{:.0}", pos.x, pos.y));
                HostResult::Done
            }
            // `DisSeqAct_AlarmBell::Activated` (0x00b91ac0): the first input (Turn Off / Turn
            // On) switches the `DisAlarmBell` targets off or on (0x00a4e230); a bell that is
            // off cannot be rung.
            "DisSeqAct_AlarmBell" => {
                let Some(on) = cx.input().filter(|i| *i < 2).map(|i| i == 1) else { return HostResult::Done };
                for t in cx.objects("Target") {
                    let Obj::Actor(a) = t else { continue };
                    let name = self.db.actors[a as usize].name.clone();
                    for b in self.m.crowd.alarms.iter_mut().filter(|b| b.def.name.eq_ignore_ascii_case(&name)) {
                        if on {
                            if b.cooldown == f32::INFINITY {
                                b.cooldown = 0.0;
                            }
                        } else {
                            b.ringing = false;
                            b.ringer = None;
                            b.cooldown = f32::INFINITY;
                        }
                    }
                    self.note(format!("alarm bell {name} {}", if on { "on" } else { "off" }));
                }
                HostResult::Done
            }
            // `DisSeqAct_ForbiddenZoneOverride` (0x00b909e0 stores the input; the zones'
            // handler applies it): Set gives the target zones `m_OwningFactions` /
            // `m_ForbiddenFactions`, Clear puts their level values back.
            "DisSeqAct_ForbiddenZoneOverride" => {
                let set = cx.input() == Some(0);
                let list = |cx: &Ctx, link: &str, prop: &str| -> Vec<String> {
                    let mut v: Vec<String> = cx.objects(link).iter().filter_map(|o| if let Obj::Path(p) = o { Some(p.clone()) } else { None }).collect();
                    if v.is_empty() {
                        if let Some(Value::Array(a)) = cx.prop(prop) {
                            v = a.iter().filter_map(|x| if let Some(Obj::Path(p)) = x.as_obj() { Some(p.clone()) } else { None }).collect();
                        }
                    }
                    v
                };
                let owning = list(cx, "Owning Factions", "m_OwningFactions");
                let forbidden = list(cx, "Forbidden Factions", "m_ForbiddenFactions");
                for t in cx.objects("Target") {
                    let Obj::Actor(a) = t else { continue };
                    let name = self.db.actors[a as usize].name.clone();
                    let original = self.st.zone_backup.get(&name).cloned();
                    for z in self.m.crowd.zones.iter_mut().filter(|z| z.name.eq_ignore_ascii_case(&name)) {
                        if set {
                            self.st.zone_backup.entry(name.clone()).or_insert((z.owning.clone(), z.forbidden.clone()));
                            z.owning = owning.clone();
                            z.forbidden = forbidden.clone();
                        } else if let Some((o, f)) = &original {
                            z.owning = o.clone();
                            z.forbidden = f.clone();
                        }
                    }
                }
                HostResult::Done
            }
            // `DisSeqAct_GameOver::Activated` (0x00b86010): game over with `m_Reason` after
            // `m_fDelay`.
            "DisSeqAct_GameOver" => {
                let reason = cx.prop_str("m_Reason");
                let delay = cx.prop_float("m_fDelay");
                self.note(format!("game over: {reason}"));
                let fail = Action::Fail(if reason.is_empty() { "Game over".into() } else { reason });
                self.act(vec![if delay > 0.0 { Action::After(delay, vec![fail]) } else { fail }]);
                HostResult::Done
            }
            // `DisSeqAct_AIDoBehavior_Base::Activated` (0x00bb1790): each NPC target's
            // controller pushes the behaviour (slot 102 of the subclass); Abort pops it.
            // `DisSeqAct_AIDoFollow` (slot 102, 0x00ba7c80): the target follows the
            // "BehaviorTarget" pawn (`m_FollowParameters`: distance, angle, speed).
            // `DisSeqAct_AIDoSimpleBehaviors`: Panic or Beg.
            "DisSeqAct_AIDoFollow" | "DisSeqAct_AIDoSimpleBehaviors" => {
                let abort = cx.input() == Some(1);
                let pawns = self.target_pawns(cx);
                if class == "DisSeqAct_AIDoFollow" {
                    let leader = cx.objects("BehaviorTarget").into_iter().find_map(|o| self.pawn_of(&o));
                    let fp = cx.prop("m_FollowParameters");
                    let dist = fp.as_ref().and_then(|v| v.field("m_fDistance")).and_then(|v| v.as_float()).unwrap_or(100.0);
                    let run = !fp.as_ref().and_then(|v| v.field("m_eDesiredSpeed")).and_then(|v| v.as_str().map(str::to_string)).is_some_and(|s| s.contains("Walk"));
                    for p in pawns {
                        if abort {
                            self.st.followers.remove(&p);
                            continue;
                        }
                        if let Some(l) = leader {
                            self.st.followers.insert(p, (l, dist, run));
                        }
                    }
                } else {
                    let beg = cx.prop_str("m_eSimpleBehaviorToRun") == "AIDoSimpleBehavior_Beg";
                    for p in pawns {
                        if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                            if abort {
                                b.state = BrainState::Idle;
                            } else {
                                b.state = if beg { BrainState::Cower } else { BrainState::Flee };
                                b.timer = 0.0;
                            }
                        }
                    }
                }
                if abort {
                    cx.fire(1);
                } else {
                    cx.fire(0);
                }
                HostResult::Done
            }
            // `OnAIStartDistraction` (0x00b63ed0): for each NPC target, when the distractor is
            // ready (its cooldown ignored) and the NPC has a positive animate chance, the
            // animate message: `DisBehaviorInteract` walks it to the distractor, which then plays
            // (`dishonored_ai::distraction`).
            "DisSeqAct_AIStartDistraction" => {
                let d = cx.objects("Distractor").into_iter().next().or_else(|| Some(cx.prop_obj("m_pDistractor")).filter(|o| !o.is_none())).and_then(|o| self.distractor_of(&o));
                if let Some(i) = d {
                    for p in self.target_pawns(cx) {
                        if self.m.crowd.start_distraction(p, i) {
                            self.note(format!("pawn {p} starts distraction {}", self.m.crowd.distractors[i].def.name));
                        }
                    }
                }
                HostResult::Done
            }
            // [world] `DisSeqAct_RefillWhaleOilBattery::Activated` (0x00c82190): every battery
            // target gets `SetCharges(int(m_PercentageCharge * 0.01 * m_NumberOfCharges), 0)`
            // (truncated; not notified, an emptied battery leaves its receptacle).
            "DisSeqAct_RefillWhaleOilBattery" => {
                let Some(defs) = self.world_defs() else { return HostResult::Done };
                let pct = cx.prop_float("m_PercentageCharge");
                let mut evs = Vec::new();
                for t in cx.objects("Battery") {
                    let Some(b) = self.world_battery(&defs, &t) else { continue };
                    let max = defs.breakables[b].battery.as_ref().map_or(0, |x| x.max_charges);
                    let n = (pct * 0.01 * max as f32) as i32;
                    let key = self.m.current_map.to_ascii_lowercase();
                    let st = self.m.world_actors.entry(key).or_insert_with(|| dishonored_sim::world_actors::WorldActorsState::new(&defs));
                    evs.extend(st.set_charges(&defs, b, n, false));
                    self.note(format!("battery {} refilled to {n}", defs.breakables[b].name));
                }
                world_events(cx.k, self, evs);
                HostResult::Done
            }
            // [world] `DisSeqAct_PlugWhaleOilBattery::Activated` (0x00c7d7c0): with both inputs
            // nothing; the batteries ("Battery", `Targets`) and receptacles ("Receptacle") pair up
            // when their counts match: Plug (input 0) plugs each battery, notified (0x00c7b490),
            // unless the receptacle holds a charged battery; Unplug (input 1) ejects it
            // (0x00c7d190).
            "DisSeqAct_PlugWhaleOilBattery" => {
                let Some(defs) = self.world_defs() else { return HostResult::Done };
                let plug = cx.has_impulse(0);
                if plug && cx.has_impulse(1) {
                    return HostResult::Done;
                }
                let bats = cx.objects("Battery");
                let recs = cx.objects("Receptacle");
                if bats.len() != recs.len() {
                    return HostResult::Done;
                }
                let key = self.m.current_map.to_ascii_lowercase();
                let mut evs = Vec::new();
                for (bo, ro) in bats.iter().zip(recs.iter()) {
                    let (Some(b), Some(r)) = (self.world_battery(&defs, bo), self.world_receptacle(&defs, ro)) else { continue };
                    let st = self.m.world_actors.entry(key.clone()).or_insert_with(|| dishonored_sim::world_actors::WorldActorsState::new(&defs));
                    if plug {
                        let held = st.receptacles[r].attached.and_then(|a| st.batteries.get(&a)).is_some_and(|x| x.charges >= 1);
                        if held {
                            continue;
                        }
                        evs.extend(st.plug(&defs, r, b, true).1);
                    } else {
                        evs.extend(st.eject(b));
                    }
                }
                world_events(cx.k, self, evs);
                HostResult::Done
            }
            // `DisSeqAct_SetIgnoreDeath`: the target NPCs ignore the deaths of `m_Ignorers`
            // (no corpse reaction).
            "DisSeqAct_SetIgnoreDeath" => {
                let ign: Vec<PlayerId> = cx.objects("Ignorers").iter().chain(cx.prop("m_Ignorers").map(|v| v.as_array().iter().filter_map(|x| x.as_obj().cloned()).collect::<Vec<_>>()).unwrap_or_default().iter()).filter_map(|o| self.pawn_of(o)).collect();
                for p in self.target_pawns(cx) {
                    if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                        for i in &ign {
                            if !b.corpses_seen.contains(i) {
                                b.corpses_seen.push(*i);
                            }
                        }
                    }
                }
                HostResult::Done
            }
            // `DisSeqAct_AttachPickup`: Attach puts the pickup on the target NPC (carried,
            // pickpocketable), Detach drops it where the NPC is.
            "DisSeqAct_AttachPickup" => {
                let attach = cx.input() != Some(1);
                let pk = cx.objects("Pickup").into_iter().next().or_else(|| Some(cx.prop_obj("m_pPickup")).filter(|o| !o.is_none()));
                let Some(Obj::Actor(a)) = pk else { return HostResult::Done };
                let name = self.db.actors[a as usize].name.clone();
                let owner = self.target_pawns(cx).first().copied();
                let pos = owner.and_then(|o| self.m.world.player(o).map(|p| p.position));
                if let Some(p) = self.m.world.pickups.iter_mut().find(|p| p.name.eq_ignore_ascii_case(&name)) {
                    if attach {
                        p.carried_by = owner;
                    } else {
                        p.carried_by = None;
                        if let Some(x) = pos {
                            p.position = x;
                        }
                    }
                }
                HostResult::Done
            }
            // `SeqAct_ModifyHealth` (engine): `Amount` of damage (or healing with `bHeal`) to
            // the targets, from the "Instigator".
            "SeqAct_ModifyHealth" => {
                let amount = cx.prop_float("Amount");
                let heal = cx.prop_bool("bHeal");
                for p in self.target_pawns(cx) {
                    if let Some(pl) = self.m.world.player_mut(p) {
                        if heal {
                            pl.vitals.health += amount;
                        } else {
                            pl.vitals.health -= amount;
                        }
                    }
                }
                HostResult::Done
            }
            // Chaos (`DisSeqAct_AddDarkness`, 0x00bb3360): the darkness manager's `Add(m_Value)`
            // (0x00C41B50, clamped at 0), then `m_Text` on the HUD when it is not empty.
            "DisSeqAct_AddDarkness" => {
                let v = cx.prop_int("m_Value");
                self.m.darkness.add(v);
                self.m.sync_darkness(self.cfg);
                self.note(format!("chaos {v:+} (score {}, level {})", self.m.darkness.score, self.m.state.darkness_level));
                let text = cx.prop_str("m_Text");
                if !text.is_empty() {
                    self.act(vec![Action::Message(text)]);
                }
                HostResult::Done
            }
            // `DisSeqAct_IncrementPlayerStat::Activated` (0x00AB9B90) ends with the stat increment
            // (0x00AB9560: `m_StatToIncrement`, `m_fNumToIncrementBy`, the victim `m_pTweaks`),
            // which ends with the chaos deeds (0x00C4D2C0). The statistics themselves are not kept.
            "DisSeqAct_IncrementPlayerStat" => {
                *self.st.presentation.entry(class).or_insert(0) += 1;
                let stat = cx.prop_str("m_StatToIncrement");
                let amount = cx.prop("m_fNumToIncrementBy").and_then(|v| v.as_float()).unwrap_or(1.0) as i32;
                let tweak = match cx.prop_obj("m_pTweaks") {
                    Obj::Path(p) => Some(p),
                    _ => None,
                };
                let lineage = tweak.map(|t| self.m.setup.original.iter().find(|o| o.tweak.eq_ignore_ascii_case(&t)).map(|o| o.lineage.clone()).unwrap_or_else(|| vec![t]));
                self.m.player_stat(&stat, lineage.as_deref(), amount, self.cfg);
                HostResult::Done
            }
            // `DisSeqAct_NotifyMissionEnd` (0x00B8B110): the darkness manager's deed counters are
            // cleared (0x00C4D500); the score stays.
            "DisSeqAct_NotifyMissionEnd" => {
                self.m.darkness.mission_end();
                self.note("mission end: chaos deed counters cleared".into());
                HostResult::Done
            }
            // `DisSeqAct_MissionStatsTracking` (0x009FE090): the stats screen's chaos entry is
            // High when the chaos level reaches `m_ChaosLevelThreshold`.
            "DisSeqAct_MissionStatsTracking" => {
                *self.st.presentation.entry(class).or_insert(0) += 1;
                // An op that leaves the threshold unset has the class default 0.
                self.m.state.chaos_threshold = Some(cx.prop("m_ChaosLevelThreshold").and_then(|v| v.as_int()).unwrap_or(0));
                HostResult::Done
            }
            "DisSeqAct_NPCSetMaterials" => {
                let clear = cx.input() == Some(1);
                let mut mats: Vec<(u8, u32, String)> = Vec::new();
                for (part, prop) in [(0u8, "m_NewBodyMaterials"), (1u8, "m_NewHeadMaterials")] {
                    if let Some(Value::Array(a)) = cx.prop(prop) {
                        for e in a {
                            let idx = e.field("m_MaterialIndex").and_then(|v| v.as_int()).unwrap_or(0).max(0) as u32;
                            if let Some(Obj::Path(p)) = e.field("m_pMaterial").and_then(|v| v.as_obj()) {
                                mats.push((part, idx, p.clone()));
                            }
                        }
                    }
                }
                let targets: Vec<Obj> = cx.objects("NPC(s)");
                for o in targets {
                    let Some(p) = self.pawn_of(&o) else { continue };
                    self.note(format!("materials of pawn {p}: {}", if clear { "cleared".to_string() } else { mats.iter().map(|m| m.2.rsplit('.').next().unwrap_or("").to_string()).collect::<Vec<_>>().join(", ") }));
                    self.m.set_materials_id(p, if clear { None } else { Some(mats.clone()) });
                }
                HostResult::Done
            }
            // `SeqAct_SetPhysics` (kismet.md 3.6): `SequenceAction::Activated` calls
            // `OnSetPhysics(action)` on every target: `Actor.OnSetPhysics` is
            // `SetPhysics(newPhysics)`, `Controller.OnSetPhysics` forwards to its pawn (the port's
            // targets are already pawns). `newPhysics` is stored only when not `PHYS_None`. The
            // port has no rigid bodies or per-actor physics modes: the mode is recorded for the
            // actors, nothing moves (kismet.md F-11, left open).
            "SeqAct_SetPhysics" => {
                const PHYS: [&str; 15] = ["PHYS_None", "PHYS_Walking", "PHYS_Falling", "PHYS_Swimming", "PHYS_Flying", "PHYS_Rotating", "PHYS_Projectile", "PHYS_Interpolating", "PHYS_Spider", "PHYS_Ladder", "PHYS_RigidBody", "PHYS_SoftBody", "PHYS_NavMeshWalking", "PHYS_Unused", "PHYS_Custom"];
                let mode = match cx.prop("newPhysics") {
                    Some(Value::Enum(s)) | Some(Value::Name(s)) => PHYS.iter().position(|n| n.eq_ignore_ascii_case(&s)).unwrap_or(0) as u8,
                    Some(Value::Byte(b)) => b,
                    _ => 0,
                };
                for t in cx.objects("Target") {
                    if matches!(t, Obj::Actor(_) | Obj::Pawn(_)) {
                        self.note(format!("set physics of {t:?}: {}", PHYS.get(mode as usize).unwrap_or(&"?")));
                        self.st.physics.insert(t, mode);
                    }
                }
                HostResult::Done
            }
            "SeqAct_Teleport" => {
                let dest = cx.objects("Destination").into_iter().next();
                let Some(d) = dest else { return HostResult::Done };
                let to = self.pos_of(&d);
                let yaw = match &d {
                    Obj::Actor(a) => Some(self.db.actors[*a as usize].yaw_deg()),
                    _ => None,
                };
                if let Some(to) = to {
                    for t in cx.objects("Target") {
                        self.teleport(&t, to, yaw);
                    }
                }
                HostResult::Done
            }
            // `DisSeqAct_Door::Activated` (0x00b91d70): the first input with an impulse (Open
            // CW, Open CCW, Close, Lock, Unlock) is applied to every `DisDoor` target (open
            // 0x00a1a450 with the direction, close 0x00a1a4b0, lock / unlock through the door's
            // virtuals +0x3b8 / +0x3bc), then the targets' handlers; automatic outputs.
            "DisSeqAct_Door" => {
                let Some(input) = cx.input().filter(|i| *i < 5) else { return HostResult::Done };
                for t in cx.objects("Target") {
                    let Obj::Actor(a) = t else { continue };
                    if !self.db.actors[a as usize].class.contains("Door") {
                        continue;
                    }
                    let (state, locked) = self.door(a);
                    // Open (0x00A1A450) only from closed, Close (0x00A1A4B0) only from open; both
                    // also need CanUse (0x00A4E2E0), which the port's doors (no transition
                    // time, never disabled) always pass.
                    let (ns, nl) = match input {
                        0 if state == 0 => (1, locked),
                        1 if state == 0 => (2, locked),
                        0 | 1 => (state, locked),
                        2 => (0, locked),
                        3 => (state, true),
                        _ => (state, false),
                    };
                    self.st.doors.insert(a, (ns, nl));
                    if nl != locked {
                        self.st.door_version += 1;
                    }
                    let name = self.db.actors[a as usize].name.clone();
                    self.note(format!("door {name}: {} -> {}{}", ["closed", "open CW", "open CCW"][state as usize], ["closed", "open CW", "open CCW"][ns as usize], if nl { " (locked)" } else { "" }));
                    // `DisSeqEvent_Door` (Opened / Closed) on the door when its state changes.
                    if (state == 0) != (ns == 0) {
                        self.st.door_events.push((a, if ns == 0 { 1 } else { 0 }));
                    }
                }
                HostResult::Done
            }
            // `DisSeqAct_Lock::Activated` (0x00B8F730): the first input with an impulse (Lock /
            // Unlock) on every `DishonoredUsableObject` target: `Lock` (slot 238, 0x00A4E870) sets
            // `m_bLocked`, `Unlock` (slot 239, 0x00A4E910) clears it; a door's override also
            // switches its path blocker (0x00A32490 / 0x00A324B0 -> 0x00A2D430).
            "DisSeqAct_Lock" => {
                let Some(input) = cx.input().filter(|i| *i < 2) else { return HostResult::Done };
                let lock = input == 0;
                for t in cx.objects("Target") {
                    let Obj::Actor(a) = t else { continue };
                    let act = &self.db.actors[a as usize];
                    let name = act.name.clone();
                    if act.class.contains("Door") {
                        let (state, locked) = self.door(a);
                        self.st.doors.insert(a, (state, lock));
                        if locked != lock {
                            self.st.door_version += 1;
                        }
                    } else {
                        self.st.usable_locked.insert(a, lock);
                    }
                    self.note(format!("{} {name}", if lock { "locked" } else { "unlocked" }));
                }
                HostResult::Done
            }
            // `SeqAct_AttachToActor` (engine class, no native override: the handler attaches):
            // each "Attachment" actor is attached to the "Target" actor (at `BoneName` when
            // set), keeping its offset unless `bUseRelativeOffset`; `bDetach` detaches. The
            // port moves attached level actors with their base each step (a trigger attached
            // to Lord Shaw's head follows him).
            "SeqAct_AttachToActor" => {
                let detach = cx.prop_bool("bDetach");
                let base = cx.objects("Target").into_iter().next();
                let rel = if cx.prop_bool("bUseRelativeOffset") { cx.prop("RelativeOffset").and_then(|v| if let Value::Vector(v) = v { Some(Vec3::from(v)) } else { None }) } else { None };
                for o in cx.objects("Attachment") {
                    let Obj::Actor(a) = o else { continue };
                    if detach {
                        self.st.attached.remove(&a);
                        continue;
                    }
                    let Some(b) = base.clone() else { continue };
                    let Some(bp) = self.pos_of(&b) else { continue };
                    let ap = self.pos_of(&Obj::Actor(a)).unwrap_or(bp);
                    let offset = rel.unwrap_or(ap - bp);
                    self.note(format!("attached {} to {b:?}", self.db.actors[a as usize].name));
                    self.st.attached.insert(a, (b, offset));
                }
                HostResult::Done
            }
            // `SeqAct_ActorFactory` (Activated 0x006dae60, Update 0x006daed0, Spawned slot 101
            // 0x006f90f0): spawns `SpawnCount` actors from `Factory` at the spawn points (in
            // order, random with `PointSelection` 1: `rand() % (n - i) + i` swaps, reversed
            // with 2), one per `SpawnDelay`; each spawn is written to "Spawned" and fires
            // output 0; it finishes when `SpawnCount` actors exist (DeActivated fires nothing).
            // Boyle's factories make movable ambient sound emitters (presentation).
            "SeqAct_ActorFactory" => {
                *self.st.presentation.entry(class).or_insert(0) += 1;
                let points = cx.objects("Spawn Point");
                let n = cx.prop("SpawnCount").and_then(|v| v.as_int()).unwrap_or(1).max(0) as u32;
                if n == 0 || points.is_empty() {
                    return HostResult::Done;
                }
                self.st.factories.insert(cx.op, (points, n, 0, 0.0));
                HostResult::Latent
            }
            // `DisSeqAct_ActivateUsable` (Activated 0x00b9aae0, Update 0x00b9ac90): the goal
            // stage is `m_TargetStageIndex`, or the next stage (wrapping) when it is -1; an
            // index out of range aborts; a usable already at the goal is left alone unless
            // `m_bForceAdvance`. The usable is used toward it (as the player's use would) and
            // the action finishes once it has reached the goal ("Finished"; "Aborted").
            "DisSeqAct_ActivateUsable" => {
                let target = cx.prop("m_TargetStageIndex").and_then(|v| v.as_int()).unwrap_or(-1);
                let force = cx.prop_bool("m_bForceAdvance");
                for o in cx.objects("Target") {
                    let Obj::Actor(a) = o else { continue };
                    let stages = usable_stages(cx.k, a);
                    let cur = self.st.usable_stage.get(&a).copied().unwrap_or(-1);
                    let goal = if target == -1 { (cur + 1) % stages.max(1) } else { target };
                    if goal < -1 || goal >= stages {
                        cx.set_aborted();
                        continue;
                    }
                    if goal == cur && !force {
                        continue;
                    }
                    self.st.usable_stage.insert(a, goal);
                    self.st.usable_events.push((a, goal as usize));
                    self.note(format!("usable {} -> stage {goal}", self.db.actors[a as usize].name));
                }
                HostResult::Done
            }
            "SeqAct_Destroy" => {
                for t in cx.objects("Target") {
                    self.destroy(&t);
                }
                HostResult::Done
            }
            // ----- conversations -----
            "DisSeqAct_DialogInputs" => {
                let os = cx.prop_obj("m_pDialogOneShot");
                let os = if os.is_none() { cx.objects("DialogOneShot").into_iter().next().unwrap_or_default() } else { os };
                let Obj::Actor(a) = os else {
                    return HostResult::Unhandled;
                };
                let actor = &self.db.actors[a as usize];
                let reference = match cx.input().and_then(|i| cx.data().inputs.get(i)).map(|l| l.name.clone()) {
                    Some(hook) => format!("{}:{}/{hook}", actor.package, actor.name),
                    None => return HostResult::Done,
                };
                self.note(format!("dialog input {reference}"));
                self.act(vec![Action::Conversation(reference, Vec::new())]);
                HostResult::Done
            }
            "DisSeqAct_DialogScriptedChoice" => {
                if self.st.choice.is_some() {
                    cx.set_aborted();
                    return HostResult::Done;
                }
                let options = self.setup.choices.get(&cx.op).cloned().unwrap_or_default();
                let Some(p) = self.players().first().copied() else {
                    cx.set_aborted();
                    return HostResult::Done;
                };
                self.note(format!("scripted choice {}", options.join(" / ")));
                self.st.choice = Some((cx.op, p, options, None));
                HostResult::Latent
            }
            "SeqAct_AkStartAmbientSound" => {
                // Inputs Start (0) / Stop (1) on the `Targets` ambient sound actors; the
                // client plays or stops their emitters (presentation).
                *self.st.presentation.entry(class).or_insert(0) += 1;
                let start = cx.input() != Some(1);
                for t in cx.objects("Targets") {
                    if let Obj::Actor(a) = t {
                        self.st.ambient.insert(a, start);
                    }
                }
                HostResult::Done
            }
            "SeqAct_AkPostEvent" => {
                let ev = obj_name(&cx.prop_obj("Event"));
                if !ev.is_empty() {
                    self.m.state.post_sound(&ev);
                }
                HostResult::Done
            }
            "DisSeqAct_AutoSave" => {
                // [save] The engine's pending autosave / mission-start save (save.rs).
                super::save::activated(self.m, cx.op, cx.prop_bool("m_bMissionStart"), cx.prop_int("m_MissionNumber"), cx.prop_bool("m_bEvenInCombat"), cx.prop_bool("m_bSkipAutosaveDelay"));
                HostResult::Latent
            }
            "SeqAct_CameraFade" => {
                // Not drawn; its timing is kept: "Out" now, "Finished" after FadeTime.
                *self.st.presentation.entry(class).or_insert(0) += 1;
                cx.fire(0);
                let t = cx.prop_float("FadeTime");
                self.st.fades.insert(cx.op, t);
                HostResult::Latent
            }
            "DisSeqAct_ShowMissionStats" => {
                // The end-of-mission statistics screen: shown and closed at once (the player
                // closes it in the original), so "Out" then "Closed".
                *self.st.presentation.entry(class).or_insert(0) += 1;
                self.note("mission statistics screen".into());
                cx.fire(0);
                cx.fire_name("Closed");
                HostResult::Done
            }
            "DisSeqCond_PawnIsPossessed" => {
                // Possession is not available in the port: nobody is possessed.
                cx.fire(1);
                HostResult::Done
            }
            "DisSeqAct_SetPlayerHealth" | "DisSeqAct_SetPlayerMana" => {
                let health = class == "DisSeqAct_SetPlayerHealth";
                let pct = (if health { cx.prop_int("m_Health") } else { cx.prop_int("m_Mana") }) as f32 / 100.0;
                let v = &self.cfg.gameplay.vitals;
                let (hmax, mmax) = (v.health_max, v.mana_max);
                for p in self.players() {
                    if let Some(pl) = self.m.world.player_mut(p) {
                        if health {
                            pl.vitals.health = hmax * pct;
                        } else {
                            pl.vitals.mana = mmax * pct;
                            pl.vitals.mana_regen_cap = pl.vitals.mana_regen_cap.max(pl.vitals.mana);
                        }
                    }
                }
                HostResult::Done
            }
            "SeqAct_ToggleHidden" | "SeqAct_ChangeCollision" => {
                // Hidden or collision-less level actors cannot be used or touched.
                let off = match class.as_str() {
                    "SeqAct_ChangeCollision" => Some(cx.prop_str("CollisionType") == "COLLIDE_NoCollision"),
                    _ => match cx.input() {
                        Some(0) => Some(true),
                        Some(1) => Some(false),
                        _ => None,
                    },
                };
                for t in cx.objects("Target") {
                    let Obj::Actor(a) = t else { continue };
                    let set = if class == "SeqAct_ToggleHidden" { &mut self.st.hidden } else { &mut self.st.off };
                    let now = off.unwrap_or(!set.contains(&a));
                    if now {
                        set.insert(a);
                    } else {
                        set.remove(&a);
                    }
                }
                HostResult::Done
            }
            // `DisSeqAct_NPCMarkForVanish` (Update 0x00b90f90): Mark / Cancel go to the
            // targets' `OnNPCMarkForVanish` (0x00b4acb0) and Mark fires "Out". A marked pawn
            // (tick 0x00b64480) counts `m_fOffscreenTimeRequired` down while it is not seen,
            // resets it while it is; once it has run out and the vanish test passes (not in
            // search / combat with `m_bRespectSearchAndCombat`, at least `m_fRequiredDistance`
            // from the players, 0x00b5f7d0) it is destroyed and reports "Vanished" (0x00ba2190).
            // Cancel unmarks without an output. The action finishes when no pawn is left.
            "DisSeqAct_NPCMarkForVanish" => {
                let mut v = self.st.vanish.remove(&cx.op).unwrap_or_default();
                let pawns: Vec<PlayerId> = cx.objects("Target").iter().filter_map(|o| self.pawn_of(o)).collect();
                match cx.input() {
                    Some(0) => {
                        let t = cx.prop_float("m_fOffscreenTimeRequired");
                        for p in pawns {
                            v.retain(|x| x.0 != p);
                            v.push((p, t));
                        }
                        cx.fire(0);
                    }
                    Some(1) => v.retain(|x| !pawns.contains(&x.0)),
                    _ => {}
                }
                if v.is_empty() {
                    return HostResult::Done;
                }
                self.st.vanish.insert(cx.op, v);
                HostResult::Latent
            }
            "SeqAct_WaitForLevelsVisible" => {
                // Every level of a map is loaded and visible with it.
                HostResult::Done
            }
            _ => HostResult::Unhandled,
        }
    }

    fn update(&mut self, cx: &mut Ctx, dt: f32) -> bool {
        match cx.class() {
            "SeqAct_PrepareMapChange" => true,
            "DisSeqAct_NPCMarkForVanish" => {
                if self.new_input(cx) {
                    // A new Mark / Cancel while running.
                    return self.activated(cx) != HostResult::Latent;
                }
                let Some(v) = self.st.vanish.remove(&cx.op) else { return true };
                let respect = cx.prop("m_bRespectSearchAndCombat").and_then(|x| x.as_bool()).unwrap_or(true);
                let dist = cx.prop_float("m_fRequiredDistance");
                let mut left = Vec::new();
                for (p, t) in v {
                    let Some(pl) = self.m.world.player(p).cloned() else { continue };
                    if self.seen_by_player(pl.position) {
                        left.push((p, cx.prop_float("m_fOffscreenTimeRequired")));
                        continue;
                    }
                    let t = t - dt;
                    let busy = respect && self.m.crowd.brains.iter().find(|b| b.id == p).is_some_and(|b| !b.state.is_routine());
                    let near = self.m.humans.iter().filter_map(|h| self.m.world.player(*h)).any(|h| h.position.distance(pl.position) < dist);
                    if t < 0.0 && !busy && !near {
                        self.note(format!("pawn {p} vanished"));
                        self.destroy(&Obj::Pawn(p));
                        cx.fire(1);
                        continue;
                    }
                    left.push((p, t));
                }
                if left.is_empty() {
                    return true;
                }
                self.st.vanish.insert(cx.op, left);
                false
            }
            "DisSeqAct_AIShoot" => {
                if self.new_input(cx) {
                    return self.activated(cx) != HostResult::Latent;
                }
                let Some(list) = self.st.shooting.remove(&cx.op) else { return true };
                let mut left = Vec::new();
                let mut last_ok = false;
                for (p, done_at) in list {
                    let alive = self.m.world.player(p).is_some_and(|x| x.alive);
                    let b = self.m.crowd.brains.iter().find(|b| b.id == p);
                    if self.st.shots.get(&p).copied().unwrap_or(0) >= done_at {
                        last_ok = true;
                        continue;
                    }
                    // The shoot behaviour ends with its pawn (death, knock-out) or without its
                    // target.
                    if !alive || b.is_none_or(|b| b.shoot.is_none()) {
                        last_ok = false;
                        cx.fire(3);
                        continue;
                    }
                    left.push((p, done_at));
                }
                if left.is_empty() {
                    if last_ok {
                        cx.fire(1);
                    }
                    return true;
                }
                self.st.shooting.insert(cx.op, left);
                false
            }
            "SeqAct_ActorFactory" => {
                let Some((points, n, mut spawned, mut delay)) = self.st.factories.remove(&cx.op) else { return true };
                if delay > 0.0 {
                    delay -= dt;
                } else {
                    // One spawn per update, at the next point in order.
                    let at = points[spawned as usize % points.len()].clone();
                    let name = format!("{}_Spawned{spawned}", cx.describe());
                    let _ = at;
                    spawned += 1;
                    cx.set_link("Spawned", Value::Obj(Obj::Path(name)));
                    cx.fire(0);
                    delay = cx.prop_float("SpawnDelay");
                }
                if spawned >= n {
                    return true;
                }
                self.st.factories.insert(cx.op, (points, n, spawned, delay));
                false
            }
            "SeqAct_CameraFade" => {
                let left = self.st.fades.get(&cx.op).copied().unwrap_or(0.0) - dt;
                if left <= 0.0 {
                    self.st.fades.remove(&cx.op);
                    cx.fire(1);
                    true
                } else {
                    self.st.fades.insert(cx.op, left);
                    false
                }
            }
            "DisSeqAct_AutoSave" => {
                // [save] "Accepted" (1) once the save is written, "Denied" (2) when dropped.
                match super::save::update(self.m, cx.op) {
                    Some(true) => cx.fire(1),
                    Some(false) => cx.fire(2),
                    None => return false,
                };
                true
            }
            "DisSeqAct_StartSpawn" => {
                let Some((todo, done)) = self.st.spawning.get(&cx.op).cloned() else { return true };
                let mut done = done;
                let mut failed = 0;
                for i in todo {
                    // Spawning needs the bookkeeping of the whole Kismet (Spawned events), which
                    // is not reachable from inside an op; the spawn is queued for after the tick.
                    match self.m.setup.original.get(i).cloned() {
                        Some(o) if o.map.eq_ignore_ascii_case(&self.m.current_map) => {
                            if !self.m.original_spawned.contains(&o.spawn.id) {
                                self.m.spawn_original(i, false, self.cfg);
                            }
                            done.push(o.spawn.id);
                            cx.fire(2);
                        }
                        _ => {
                            failed += 1;
                            cx.fire(3);
                        }
                    }
                }
                let _ = failed;
                self.st.spawning.remove(&cx.op);
                cx.fire(1);
                true
            }
            "DisSeqAct_AIGoToActor" => {
                if cx.has_impulse(0) && self.new_input(cx) {
                    // Start again while running: the handlers restart the go-to.
                    self.st.goto.remove(&cx.op);
                    return self.activated(cx) != HostResult::Latent;
                }
                let Some(mut gt) = self.st.goto.remove(&cx.op) else { return true };
                if cx.has_impulse(1) {
                    // Abort: finished, no output (decoded Update).
                    return true;
                }
                // Each pawn reports once (the engine's per-actor result callback).
                let mut left = Vec::new();
                for p in std::mem::take(&mut gt.pawns) {
                    let alive = self.m.world.player(p).is_some_and(|x| x.alive);
                    let b = self.m.crowd.brains.iter().find(|b| b.id == p);
                    let result = match b {
                        // The behaviour is popped (death, knock-out, removal): Interrupted.
                        None => Some(3),
                        _ if !alive => Some(3),
                        // The path request failed.
                        Some(b) if b.state == BrainState::WalkToPoint && b.follow.failed && b.follow.path.is_empty() => Some(2),
                        Some(b) if b.state == BrainState::WalkToPoint => None,
                        // Something else took over (alert, combat, flee, conversation...).
                        Some(b) if !b.state.is_routine() => Some(3),
                        // Arrived: the go-to completes (turned to the destination's rotation).
                        Some(_) => Some(1),
                    };
                    match result {
                        None => left.push(p),
                        Some(1) => {
                            gt.last_ok = true;
                            self.note(format!("pawn {p} arrived ({})", cx.describe()));
                            if let (Some(y), Some(pl)) = (gt.yaw, self.m.world.player_mut(p)) {
                                pl.yaw = y;
                            }
                        }
                        Some(r) => {
                            gt.last_ok = false;
                            self.note(format!("pawn {p} go-to {} ({})", if r == 2 { "failed" } else { "interrupted" }, cx.describe()));
                            cx.fire(r);
                        }
                    }
                }
                if left.is_empty() {
                    if gt.last_ok {
                        cx.fire(1);
                    }
                    return true;
                }
                gt.pawns = left;
                self.st.goto.insert(cx.op, gt);
                false
            }
            "DisSeqAct_DialogScriptedChoice" => {
                let Some((op, _, _, picked)) = self.st.choice.clone() else { return true };
                if op != cx.op {
                    return true;
                }
                match picked {
                    Some(i) => {
                        self.st.choice = None;
                        self.note(format!("scripted choice picked {}", i + 1));
                        cx.fire(i + 1);
                        true
                    }
                    None => false,
                }
            }
            _ => true,
        }
    }
}

/// Kismet usables of the active map: (usable, position, prompt).
pub fn usables(m: &MissionMatch) -> Vec<(KUse, Vec3, String)> {
    let Some(run) = &m.kismet else { return Vec::new() };
    let k = &run.k;
    let db = &k.db;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for (cls, pick) in [("DisSeqEvent_Used", false), ("SeqEvent_Used", false), ("DisSeqEvent_PickupPickedUp", true)] {
        for e in k.events_for(cls, &[]) {
            if !k.ops[e as usize].enabled {
                continue;
            }
            let Some(Obj::Actor(a)) = k.event_originator(e) else { continue };
            if run.st.gone.contains(&a) || run.st.off.contains(&a) || run.st.hidden.contains(&a) || !seen.insert(a) {
                continue;
            }
            let max = k.prop_int(e, "MaxTriggerCount");
            if max != 0 && k.ops[e as usize].trigger_count >= max as u32 {
                continue;
            }
            let actor = &db.actors[a as usize];
            let prompt = run.setup.prompts.get(&a).cloned().unwrap_or_else(|| if pick { format!("Pick up ({})", actor.name) } else { format!("Use ({})", actor.name) });
            out.push((if pick { KUse::Pickup(a) } else { KUse::Actor(a) }, run.actor_pos(a), prompt));
        }
    }
    // Original NPCs whose one-shot has a use hook (`DDH_PLAYER_USED`).
    if let Some(cdb) = &m.setup.conversations {
        for (oi, o) in cdb.oneshots.iter().enumerate() {
            let Some(me) = o.speakers.first().and_then(|s| s.actor.clone()) else { continue };
            if cdb.used_hook(oi as u32).is_none() {
                continue;
            }
            let Some(idx) = run.st.spawner.iter().find(|(a, _)| db.actors[**a as usize].name.eq_ignore_ascii_case(&me) && db.actors[**a as usize].package.eq_ignore_ascii_case(&o.package)).map(|x| *x.1) else { continue };
            let id = m.setup.original[idx].spawn.id;
            let Some(p) = m.world.player(id).filter(|p| p.alive) else { continue };
            out.push((KUse::Talk { pawn: id, oneshot: format!("{}:{}", o.package, o.name) }, p.position + Vec3::Z * 40.0, "Talk".to_string()));
        }
    }
    out
}

/// [inventory] Pickup actors the Kismet scripts (`DisSeqEvent_PickupPickedUp`): (package, name).
/// They stay Kismet usables (scripts move and destroy them), so the simulation's inventory
/// does not hold a second copy.
pub fn scripted_pickups(m: &MissionMatch) -> Vec<(String, String)> {
    let Some(run) = &m.kismet else { return Vec::new() };
    let k = &run.k;
    k.events_for("DisSeqEvent_PickupPickedUp", &[])
        .into_iter()
        .filter_map(|e| match k.event_originator(e) {
            Some(Obj::Actor(a)) => Some((k.db.actors[a as usize].package.clone(), k.db.actors[a as usize].name.clone())),
            _ => None,
        })
        .collect()
}

/// Interaction candidates for `MissionMatch::interaction`.
pub fn interactions(m: &MissionMatch) -> Vec<(Vec3, f32, Interaction)> {
    usables(m).into_iter().enumerate().map(|(i, (_, p, _))| (p, 220.0, Interaction::Kismet(i))).collect()
}

/// A player used Kismet usable `i` (index into [`usables`] of this step).
pub fn use_usable(m: &mut MissionMatch, player: PlayerId, i: usize) {
    let list = usables(m);
    let Some((u, _, _)) = list.into_iter().nth(i) else { return };
    if let Some(run) = &mut m.kismet {
        run.st.uses.push((player, u));
    }
}

/// A choice button press: consumed when the player answers a scripted choice.
pub fn choice(m: &mut MissionMatch, player: PlayerId, index: usize) -> bool {
    let Some(run) = &mut m.kismet else { return false };
    match &mut run.st.choice {
        Some(c) if c.1 == player && c.3.is_none() && index < c.2.len().max(1) => {
            c.3 = Some(index);
            true
        }
        _ => false,
    }
}

/// Cinematic mode (`SeqAct_ToggleCinematicMode`) applied to the players' inputs before the
/// world steps: movement input ignored (`IgnoreMoveInput`), the view kept where it is
/// (`IgnoreLookInput`), and no actions (the pawn's action permissions are off while in
/// cinematic mode, 0x00aaf150). The bit-by-bit meaning of the permission mask 0x7faf is not
/// decoded: every button is refused.
pub fn cinematic_inputs(m: &MissionMatch, inputs: &mut [(PlayerId, dishonored_sim::input::PlayerInput)]) {
    let Some(run) = m.kismet.as_ref() else { return };
    if run.st.cinematic.is_empty() {
        return;
    }
    for (id, i) in inputs.iter_mut() {
        let Some(c) = run.st.cinematic.get(id).filter(|c| c.on || c.no_move || c.no_look) else { continue };
        let Some(me) = m.world.player(*id) else { continue };
        if c.no_move {
            i.forward = 0;
            i.right = 0;
        }
        if c.no_look {
            i.yaw = me.yaw;
            i.pitch = me.pitch;
        }
        if c.on {
            i.buttons = 0;
            i.action = 0;
            i.left_hand = me.left.item;
        }
    }
}

/// Forced view tracking (`ViewTrack`) applied to the players' inputs before the world steps:
/// the tracked view replaces the input's yaw / pitch, alpha advances, and a channel that
/// reached 1 without `m_bHoldUntilStopped` ends (0x00aaf3a0 -> 0x00aa0e60).
/// APPROXIMATION: a tracked pawn is aimed at 60 uu above its centre (no server-side bones for
/// `m_TrackedBone`); the eye is 64 uu above the player's centre; the player's offset inside the
/// freedom follows its input at once (the original interpolates it at 18, 0x00bbbb80).
pub fn track_views(m: &mut MissionMatch, inputs: &mut [(PlayerId, dishonored_sim::input::PlayerInput)], dt: f32) {
    let Some(run) = m.kismet.as_mut() else { return };
    if run.st.tracking.is_empty() {
        return;
    }
    for (id, i) in inputs.iter_mut() {
        let Some(t) = run.st.tracking.get_mut(id) else { continue };
        let Some(me) = m.world.player(*id) else { continue };
        let target = match t.pawn {
            Some(p) => m.world.player(p).map(|x| x.position + Vec3::Z * 60.0).unwrap_or(t.point),
            None => t.point,
        };
        t.alpha = (t.alpha + t.rate * dt).min(1.0);
        t.want0.get_or_insert((i.yaw, i.pitch));
        let (yaw, pitch) = t.apply(me.position + Vec3::Z * 64.0, target, (i.yaw, i.pitch));
        i.yaw = yaw;
        i.pitch = pitch;
    }
    run.st.tracking.retain(|_, t| t.hold || t.alpha < 1.0);
}

/// One step of the Kismet: feed gameplay events, tick, apply map changes.
pub fn step(m: &mut MissionMatch, sim_events: &[SimEvent], dt: f32, cfg: &SimConfig) {
    let Some(mut run) = m.kismet.take() else { return };
    let setup = run.setup.clone();
    let db = run.k.db.clone();
    run.k.players = m.humans.clone();
    {
        let KismetRun { k, st, .. } = &mut run;
        let mut h = H { m, st, setup: setup.clone(), cfg, db: db.clone() };
        if !h.st.started && !h.m.humans.is_empty() {
            h.st.started = true;
            let ev = setup.start_event.clone();
            h.note(format!("mission start: global remote event {ev}"));
            let inst = h.players_obj();
            let n = k.remote_event(&mut h, &ev, inst);
            if n == 0 {
                h.note(format!("start event {ev} has no listener"));
            }
        }
        if h.st.started {
            // [interp] Soirees wait for the matinees the host plays.
            if let Some(r) = h.m.state.conv.as_mut() {
                r.external_matinees = true;
            }
            feed(k, &mut h, sim_events);
            approach_hooks(&mut h);
            // The world tick's dt is scaled by `WorldInfo.TimeDilation` (the player dilation);
            // `ExecuteActiveOps` scales it again by world / player for the ops not flagged
            // `m_bAlwaysOutOfBendTime` (kismet.md K-2, F-3).
            let player = dishonored_sim::powers::player_time_dilation(&h.m.world, &h.cfg.gameplay);
            let world = dishonored_sim::powers::world_time_scale(&h.m.world, &h.cfg.gameplay, false);
            let ratio = if player != 0.0 { world / player } else { 1.0 };
            k.tick_dilated(&mut h, dt * player, ratio);
            cutscene::apply(k, &mut h); // [interp]
            h.drain_spawns(k);
            commit_map_change(k, &mut h);
        }
        // Unsupported op / event classes the interpreter met (once per class).
        for n in std::mem::take(&mut k.notices) {
            h.note(n);
        }
    }
    m.kismet = Some(run);
}

/// `EDisAttentionLevel` by name (`DAL_Unaware` 0 .. `DAL_Busted` 4).
fn attention_level(v: &Value) -> Option<u8> {
    match v {
        Value::Enum(s) | Value::Name(s) => ["DAL_Unaware", "DAL_HeadTrack", "DAL_TurnToFace", "DAL_Investigate", "DAL_Busted"].iter().position(|n| n.eq_ignore_ascii_case(s)).map(|i| i as u8),
        Value::Byte(b) => Some(*b),
        Value::Int(i) => u8::try_from(*i).ok(),
        _ => None,
    }
}

/// `DisSeqEvent_AttentionIncreasedTo` / `AttentionDecreasedTo` (kismet.md 3.3). The attention
/// tick hands each target's batch of level changes to 0x00b3b350, which calls 0x00b363d0 once
/// with the level before the first change and after the last (a jump, not each level passed).
/// When they differ, every enabled event of that direction in the observer's
/// `GeneratedEvents` gets `m_eActivatedAttentionLevel` = the new level and `CheckActivate`
/// (observer, target); the override `DisSeqEvent_Attention::CheckActivate` (0x00b8a2a0) passes
/// only when that level equals the event's `m_eAttentionLevel` and, with `bPlayerOnly`, the
/// target is the player pawn. The port's brains update attention once per tick, so the batch is
/// the change since the last step. The noise pseudo-target is not a pawn and is skipped; the
/// DLC06 variants (fired with `ActivateIndices = [level]`) are not in the port's maps.
fn attention_events(k: &mut Kismet, h: &mut H) {
    let mut now: BTreeMap<(PlayerId, PlayerId), u8> = BTreeMap::new();
    for b in &h.m.crowd.brains {
        if !h.m.world.player(b.id).is_some_and(|p| p.alive) {
            continue;
        }
        for a in &b.attn {
            if a.who != dishonored_ai::NOISE && a.level > 0 {
                now.insert((b.id, a.who), a.level);
            }
        }
    }
    let old = std::mem::replace(&mut h.st.attn_levels, now.clone());
    let keys: BTreeSet<(PlayerId, PlayerId)> = old.keys().chain(now.keys()).copied().collect();
    for (observer, target) in keys {
        let (from, to) = (old.get(&(observer, target)).copied().unwrap_or(0), now.get(&(observer, target)).copied().unwrap_or(0));
        if from == to || !h.m.world.player(observer).is_some_and(|p| p.alive) {
            continue;
        }
        let class = if from < to { "DisSeqEvent_AttentionIncreasedTo" } else { "DisSeqEvent_AttentionDecreasedTo" };
        let mut origs = vec![Obj::Pawn(observer)];
        if let Some((&a, _)) = h.st.spawner.iter().find(|(_, i)| h.m.setup.original[**i].spawn.id == observer) {
            origs.push(Obj::Actor(a));
        }
        for ev in k.events_for(class, &origs) {
            if !k.ops[ev as usize].enabled {
                continue;
            }
            k.set_prop(ev, "m_eActivatedAttentionLevel", Value::Byte(to));
            let want = k.prop(ev, "m_eAttentionLevel").as_ref().and_then(attention_level).unwrap_or(0);
            if want != to || (k.prop_bool(ev, "bPlayerOnly") && !h.m.humans.contains(&target)) {
                continue;
            }
            // Originator: the observer pawn (0x00b363d0 passes the component's owner).
            if k.check_activate(h, ev, Obj::Pawn(observer), Obj::Pawn(target), false, None, false) {
                h.note(format!("attention of pawn {observer} on {target}: {from} -> {to}: {}", k.describe(ev)));
            }
        }
    }
}

/// Gameplay -> Kismet events.
fn feed(k: &mut Kismet, h: &mut H, sim_events: &[SimEvent]) {
    h.drain_spawns(k);
    // [world] Breakables, batteries and receptacles of this step.
    let evs = std::mem::take(&mut h.m.world_events);
    world_events(k, h, evs);
    // [distraction] `FireDistracted` (0x00b79b80): every `DisSeqEvent_Distracted` of the
    // distractor (originator) checks with the pawn as instigator and the output as its index.
    let distracted: Vec<(usize, PlayerId, u8)> = h.m.crowd.events.iter().filter_map(|e| match e {
        dishonored_ai::AiEvent::Distracted { distractor, pawn, output } => Some((*distractor, *pawn, *output)),
        _ => None,
    }).collect();
    for (i, pawn, out) in distracted {
        let Some((pkg, name)) = h.m.crowd.distractors.get(i).map(|d| (d.def.package.clone(), d.def.name.clone())) else { continue };
        let Some(a) = h.kismet_actor(&pkg, &name) else { continue };
        let fired = k.trigger(h, "DisSeqEvent_Distracted", &[Obj::Actor(a)], Obj::Pawn(pawn), Some(vec![out as usize]));
        if !fired.is_empty() {
            h.note(format!("pawn {pawn} distracted by {pkg}:{name}: output {out}"));
        }
    }
    // Usables the Kismet moved to a stage (`DisSeqAct_ActivateUsable`): the usable's
    // `DisSeqEvent_Used` output for that stage, instigated by the player.
    for (a, stage) in std::mem::take(&mut h.st.usable_events) {
        let inst = h.players_obj();
        used_stage(k, h, a, stage, inst);
    }
    // Attached actors follow their base (`SeqAct_AttachToActor`).
    for (a, (base, offset)) in h.st.attached.clone() {
        if let Some(bp) = h.pos_of(&base) {
            h.st.actor_pos.insert(a, bp + offset);
        }
    }
    // `DisSeqAct_AIDoFollow`: followers walk (or run) after their leader, stopping within the
    // follow distance; the follow ends when either is gone or the follower leaves its
    // routine.
    for (p, (leader, dist, run)) in h.st.followers.clone() {
        let lp = h.m.world.player(leader).filter(|x| x.alive).map(|x| x.position);
        let me = h.m.world.player(p).filter(|x| x.alive).map(|x| x.position);
        let (Some(lp), Some(me)) = (lp, me) else {
            h.st.followers.remove(&p);
            continue;
        };
        let Some(b) = h.m.crowd.brains.iter_mut().find(|b| b.id == p) else {
            h.st.followers.remove(&p);
            continue;
        };
        if !b.state.is_routine() {
            h.st.followers.remove(&p);
            continue;
        }
        if me.truncate().distance(lp.truncate()) > dist + 60.0 {
            b.goal = Some(lp);
            b.run = run;
            b.stay = true;
            b.state = BrainState::WalkToPoint;
        }
    }
    // Doors the Kismet opened or closed last step: `DisSeqEvent_Door` Opened (0) / Closed (1).
    for (a, out) in std::mem::take(&mut h.st.door_events) {
        let n = k.trigger(h, "DisSeqEvent_Door", &[Obj::Actor(a)], Obj::None, Some(vec![out]));
        if !n.is_empty() {
            h.note(format!("door {} {}: {} event(s)", h.db.actors[a as usize].name, if out == 0 { "opened" } else { "closed" }, n.len()));
        }
    }
    // Uses.
    for (p, u) in std::mem::take(&mut h.st.uses) {
        match u {
            KUse::Actor(a) if h.usable_locked(a) => {
                // A locked usable is not used: its `DisSeqEvent_Used` fires the special
                // "used but locked" output (`UOSO_UsedButLocked`) instead of a stage.
                h.note(format!("used {} while locked", h.db.actors[a as usize].name));
                for e in k.events_for("DisSeqEvent_Used", &[Obj::Actor(a)]) {
                    let out = k.op(e).outputs.iter().position(|o| o.name.to_ascii_lowercase().replace(' ', "").contains("locked"));
                    if let (Some(i), true) = (out, k.ops[e as usize].enabled) {
                        k.check_activate(h, e, Obj::Actor(a), Obj::Pawn(p), false, Some(vec![i]), false);
                    }
                }
            }
            KUse::Actor(a) => {
                // A use moves the usable to its next stage; when the transition completes the
                // usable's `DisSeqEvent_Used` fires that stage's output (0x00a57640).
                let n = usable_stages(k, a);
                let stage = (h.st.usable_stage.get(&a).copied().unwrap_or(-1) + 1).rem_euclid(n);
                h.st.usable_stage.insert(a, stage);
                h.note(format!("used {} (stage {stage})", h.db.actors[a as usize].name));
                used_stage(k, h, a, stage as usize, Obj::Pawn(p));
                // The engine's `SeqEvent_Used` (a trigger's use event: output "Used").
                k.trigger(h, "SeqEvent_Used", &[Obj::Actor(a)], Obj::Pawn(p), Some(vec![0]));
            }
            KUse::Pickup(a) => {
                h.note(format!("picked up {}:{}", h.db.actors[a as usize].package, h.db.actors[a as usize].name));
                // An abstract item pickup (`DisAbstractItemPickup.m_pAbstractItem`) adds its
                // item to the abstract inventory.
                if let Some(Obj::Path(p)) = h.db.actors[a as usize].props.get("m_pAbstractItem").and_then(|v| v.as_obj().cloned()) {
                    let item = obj_name(&Obj::Path(p));
                    *h.m.state.items.entry(item).or_insert(0) += 1;
                }
                // A bone charm gives its effect (0x00A19D50 -> 0x00ABBA90).
                if h.db.actors[a as usize].class == "DisWhaleBoneCharm" {
                    let e = h.st.charm_effect.get(&a).copied().or_else(|| h.db.actors[a as usize].props.get("m_Effect").and_then(|v| v.as_int())).filter(|e| *e >= 0);
                    if let Some(pl) = h.m.world.player_mut(p) {
                        dishonored_sim::inventory::give_charm_effect(pl, &h.cfg.gameplay, e, a);
                    }
                }
                let fired = k.trigger(h, "DisSeqEvent_PickupPickedUp", &[Obj::Actor(a)], Obj::Pawn(p), None);
                if fired.is_empty() || fired.iter().any(|e| k.prop(*e, "m_bDestroyPickup").and_then(|v| v.as_bool()).unwrap_or(true)) {
                    h.st.gone.insert(a);
                }
            }
            KUse::Talk { oneshot, .. } => {
                h.note(format!("talk {oneshot}"));
                h.act(vec![Action::Conversation(oneshot, Vec::new())]);
            }
        }
    }
    // [inventory] Pickups taken through the simulation's inventory fire their actor's
    // `DisSeqEvent_PickupPickedUp`; knock-outs fire `DisSeqEvent_KnockedOut` /
    // `DisSeqEvent_PutToSleep` and `DisSeqEvent_NPCIncapacitated` output 1 (Asleep).
    for e in sim_events {
        match e {
            SimEvent::PickedUp { id, pickup } => {
                // Abstract item pickups (the party drink, invitations, notes...) also count for
                // the conversations' `DisConv_HasAbstractItem`.
                if let Some(dishonored_ir::items::PickupKind::Abstract { item, quantity, .. }) = h.m.world.pickups.iter().find(|p| p.id == *pickup).map(|p| p.kind.clone()) {
                    let short = item.rsplit(['.', ':']).next().unwrap_or(&item).to_string();
                    *h.m.state.items.entry(short).or_insert(0) += quantity.max(1);
                }
                let Some((name, pkg)) = h.m.world.pickups.iter().find(|p| p.id == *pickup).map(|p| (p.name.clone(), p.package.clone())) else { continue };
                let Some(a) = h.db.actors.iter().position(|x| x.name.eq_ignore_ascii_case(&name) && (pkg.is_empty() || x.package.eq_ignore_ascii_case(&pkg))) else { continue };
                let a = a as u32;
                if h.st.gone.contains(&a) {
                    continue;
                }
                h.note(format!("picked up {name} (inventory)"));
                k.trigger(h, "DisSeqEvent_PickupPickedUp", &[Obj::Actor(a)], Obj::Pawn(*id), None);
                h.st.gone.insert(a);
            }
            SimEvent::Fired { id, .. } => {
                *h.st.shots.entry(*id).or_insert(0) += 1;
            }
            SimEvent::KnockedOut { victim, by, cause } => {
                if h.m.humans.contains(victim) {
                    continue;
                }
                let mut origs = vec![Obj::Pawn(*victim)];
                if let Some((&a, _)) = h.st.spawner.iter().find(|(_, i)| h.m.setup.original[**i].spawn.id == *victim) {
                    origs.push(Obj::Actor(a));
                }
                let inst = by.map(Obj::Pawn).unwrap_or(Obj::Pawn(*victim));
                let class = if *cause == dishonored_sim::events::KnockoutCause::Choke { "DisSeqEvent_KnockedOut" } else { "DisSeqEvent_PutToSleep" };
                let a = k.trigger(h, class, &origs, inst.clone(), None);
                let n = k.trigger(h, "DisSeqEvent_NPCIncapacitated", &origs, inst, Some(vec![1]));
                h.note(format!("knock-out of pawn {victim}: {} {class} / {} incapacitated events", a.len(), n.len()));
            }
            _ => {}
        }
    }
    // Damage -> `SeqEvent_TakeDamage` on the victim (or its spawner): a hit of at least
    // `MinDamageAmount` adds to the event's `CurrentDamage`; reaching `DamageThreshold`
    // activates it with the attacker as instigator and starts the count again (engine
    // `HandleDamage`).
    for e in sim_events {
        let SimEvent::Damage { victim, attacker, amount, .. } = e else { continue };
        if h.m.humans.contains(victim) {
            continue;
        }
        let mut origs = vec![Obj::Pawn(*victim)];
        if let Some((&a, _)) = h.st.spawner.iter().find(|(_, i)| h.m.setup.original[**i].spawn.id == *victim) {
            origs.push(Obj::Actor(a));
        }
        let inst = attacker.map(Obj::Pawn).unwrap_or_default();
        for ev in k.events_for("SeqEvent_TakeDamage", &origs) {
            if *amount < k.prop_float(ev, "MinDamageAmount") {
                continue;
            }
            let cur = k.prop_float(ev, "CurrentDamage") + *amount;
            let threshold = k.prop_float(ev, "DamageThreshold");
            if cur >= threshold {
                k.set_prop(ev, "CurrentDamage", Value::Float(0.0));
                let orig = k.event_originator(ev).unwrap_or(Obj::Pawn(*victim));
                if k.check_activate(h, ev, orig, inst.clone(), false, None, false) {
                    h.note(format!("pawn {victim} took {amount:.0} damage: {}", k.describe(ev)));
                }
            } else {
                k.set_prop(ev, "CurrentDamage", Value::Float(cur));
            }
        }
    }
    // Deaths.
    for e in sim_events {
        if let SimEvent::Death { victim, killer, .. } = e {
            if h.m.humans.contains(victim) || !h.st.dead.insert(*victim) {
                continue;
            }
            let mut origs = vec![Obj::Pawn(*victim)];
            if let Some((&a, _)) = h.st.spawner.iter().find(|(_, i)| h.m.setup.original[**i].spawn.id == *victim) {
                origs.push(Obj::Actor(a));
            }
            let inst = killer.map(Obj::Pawn).unwrap_or(Obj::Pawn(*victim));
            let d = k.trigger(h, "SeqEvent_Death", &origs, inst.clone(), None);
            let n = k.trigger(h, "DisSeqEvent_NPCIncapacitated", &origs, inst, Some(vec![0]));
            if !d.is_empty() || !n.is_empty() {
                h.note(format!("death of pawn {victim} (killer {killer:?}): {} death / {} incapacitated events", d.len(), n.len()));
            }
        }
    }
    attention_events(k, h);
    // Touches (players; NPC pawns for events that are not player-only).
    let touch_events = k.events_for("SeqEvent_Touch", &[]);
    for e in touch_events {
        let Some(Obj::Actor(a)) = k.event_originator(e) else { continue };
        if h.st.gone.contains(&a) || h.st.off.contains(&a) {
            continue;
        }
        let mut actor = h.db.actors[a as usize].clone();
        if actor.shape.is_none() {
            continue;
        }
        // Moved by Teleport / attachment.
        if let Some(p) = h.st.actor_pos.get(&a) {
            actor.location = p.to_array();
        }
        let player_only = k.prop_bool(e, "bPlayerOnly");
        let dead_ok = k.prop_bool(e, "bAllowDeadPawns");
        let mut inside: Vec<Obj> = Vec::new();
        for p in &h.m.world.players {
            let is_human = h.m.humans.contains(&p.id);
            if player_only && !is_human {
                continue;
            }
            if !p.alive && !dead_ok {
                continue;
            }
            // An actor attached to a pawn does not touch its base.
            if h.st.attached.get(&a).is_some_and(|(b, _)| h.pawn_of(b) == Some(p.id)) {
                continue;
            }
            if !touch_filter(k, h, e, p.id, is_human) {
                continue;
            }
            if actor.touches(p.position.to_array(), 34.0, 90.0) {
                inside.push(Obj::Pawn(p.id));
            }
        }
        let before: Vec<Obj> = k.touching(e).to_vec();
        for o in &inside {
            if !before.contains(o) && k.touch(h, e, o.clone(), true) {
                h.note(format!("touched {}:{} by {o:?}", actor.package, actor.name));
            }
        }
        for o in &before {
            if !inside.contains(o) {
                if k.touch(h, e, o.clone(), false) {
                    h.note(format!("untouched {}:{} by {o:?}", actor.package, actor.name));
                }
            }
        }
    }
    // `SeqEvent_LOS` (engine): a player looking at the originator, within `TriggerDistance`
    // and with it within `ScreenCenterDistance` pixels of the screen centre, activates "Look";
    // looking away activates "Stop Look". The port measures the screen distance on a
    // 1280-pixel-wide view with the default 90 degree field of view; obstructions are not
    // tested.
    for e in k.events_for("SeqEvent_LOS", &[]) {
        let Some(orig @ Obj::Actor(a)) = k.event_originator(e) else { continue };
        let Some(target) = h.pos_of(&orig) else { continue };
        let range = k.prop_float(e, "TriggerDistance");
        let px = k.prop_float(e, "ScreenCenterDistance");
        let max_angle = (px / 640.0).atan();
        for p in h.m.humans.clone() {
            let Some(pl) = h.m.world.player(p).filter(|x| x.alive) else { continue };
            let eye = pl.position + Vec3::Z * 64.0;
            let d = target - eye;
            let (yaw, pitch) = (pl.yaw.to_radians(), pl.pitch.to_radians());
            let fwd = Vec3::new(yaw.cos() * pitch.cos(), yaw.sin() * pitch.cos(), pitch.sin());
            let looking = (range <= 0.0 || d.length() <= range) && fwd.angle_between(d.normalize_or_zero()) <= max_angle;
            let was = h.st.los.contains(&(e, p));
            if looking && !was {
                h.st.los.insert((e, p));
                if k.check_activate(h, e, Obj::Actor(a), Obj::Pawn(p), false, Some(vec![0]), false) {
                    h.note(format!("looked at {}", h.db.actors[a as usize].name));
                }
            } else if !looking && was {
                h.st.los.remove(&(e, p));
                if k.op(e).outputs.len() > 1 {
                    k.check_activate(h, e, Obj::Actor(a), Obj::Pawn(p), false, Some(vec![1]), false);
                }
            }
        }
    }
    // Conversation outputs and soirees since the last step.
    let log = &h.m.state.conv_log;
    let start = match &h.st.conv_last {
        Some(last) => log.iter().rposition(|e| e == last).map(|i| i + 1).unwrap_or(h.st.conv_seen.min(log.len())),
        None => 0,
    };
    let new: Vec<_> = log[start.min(log.len())..].to_vec();
    h.st.conv_last = log.last().cloned();
    h.st.conv_seen = log.len();
    for ev in new {
        match ev {
            dishonored_conversation::runner::ConvEvent::Started { instance, conversation, .. } => {
                // Remember the instance's package (instances are gone once they end).
                if let Some(c) = &h.m.setup.conversations {
                    if let Some(cv) = c.conversations.get(conversation as usize) {
                        if let Some(t) = c.trees.get(cv.tree as usize) {
                            h.st.conv_pkg.insert(instance, t.package.clone());
                        }
                    }
                }
            }
            dishonored_conversation::runner::ConvEvent::Output { instance, oneshot, name } => {
                let pkg = h.st.conv_pkg.get(&instance).cloned().or_else(|| h.m.state.conv.as_ref().and_then(|r| r.instances.iter().find(|i| i.id == instance)).and_then(|i| i.oneshot).and_then(|o| h.m.setup.conversations.as_ref().map(|c| c.oneshots[o as usize].package.clone())));
                let Some(os) = oneshot else { continue };
                // A hook output outside any conversation (e.g. a use hook's Kismet output) has
                // no instance package: the one-shot of that name in the loaded levels.
                let pkg = pkg.or_else(|| {
                    let cdb = h.m.setup.conversations.as_ref()?;
                    cdb.oneshots.iter().filter(|o| o.name.eq_ignore_ascii_case(&os)).map(|o| o.package.clone()).find(|p| h.db.packages.iter().position(|x| x.eq_ignore_ascii_case(p)).is_some_and(|i| k.loaded.get(i).copied().unwrap_or(false)))
                });
                let reference = match &pkg {
                    Some(p) => format!("{p}:{os}"),
                    None => os.clone(),
                };
                let Some(a) = h.db.find_actor(&reference) else { continue };
                let evs = k.events_for("DisSeqEvent_DialogOutputs", &[Obj::Actor(a)]);
                let mut fired = 0;
                for e in evs {
                    if let Some(i) = k.op(e).output(&name) {
                        let inst = h.players_obj();
                        if k.check_activate(h, e, Obj::Actor(a), inst, false, Some(vec![i]), false) {
                            fired += 1;
                        }
                    }
                }
                h.note(format!("dialog output {reference}/{name} -> {fired} Kismet event(s)"));
            }
            dishonored_conversation::runner::ConvEvent::Soiree { instance, guid, .. } => {
                let op = k.play_matinee_guid(&guid);
                if let Some(op) = op {
                    h.note(format!("soiree plays matinee {}", h.db.describe(op)));
                }
                cutscene::soiree(h, instance, op); // [interp]
            }
            _ => {}
        }
    }
}

/// `SeqEvent_Touch`'s toucher test (slot 99, 0x006da210, before `CheckActivate`): a toucher
/// of an `IgnoredClassProximityTypes` class or using an ignored tweak is refused; when
/// `ClassProximityTypes` or `ProximityTweaks` are set the toucher must be of one of those
/// classes or use one of those pawn tweaks. (Pawn classes in the port: the players are
/// `DishonoredPlayerPawn`, NPCs `DishonoredNPCPawn`, both `DishonoredPawn` / `Pawn`.)
fn touch_filter(k: &Kismet, h: &H, e: OpId, pawn: PlayerId, human: bool) -> bool {
    let classes: &[&str] = if human { &["DishonoredPlayerPawn", "DishonoredPawn", "GamePawn", "Pawn", "Actor"] } else { &["DishonoredNPCPawn", "DishonoredPawn", "GamePawn", "Pawn", "Actor"] };
    let names = |prop: &str| -> Vec<String> { k.prop(e, prop).map(|v| v.as_array().iter().filter_map(|x| x.as_str().map(str::to_string).or_else(|| match x.as_obj() { Some(Obj::Path(p)) => Some(p.clone()), _ => None })).collect()).unwrap_or_default() };
    let tweak = h.st.spawner.values().map(|i| &h.m.setup.original[*i]).find(|o| o.spawn.id == pawn).map(|o| o.tweak.to_ascii_lowercase());
    let tweak_is = |t: &String| tweak.as_ref().is_some_and(|mine| { let tl = t.to_ascii_lowercase(); mine == &tl || mine.ends_with(&format!(".{}", tl.rsplit('.').next().unwrap_or(&tl))) });
    let is_class = |c: &String| classes.iter().any(|x| x.eq_ignore_ascii_case(c.rsplit('.').next().unwrap_or(c)));
    if names("IgnoredClassProximityTypes").iter().any(is_class) || names("IgnoredProximityTweaks").iter().any(tweak_is) {
        return false;
    }
    let want_classes = names("ClassProximityTypes");
    let want_tweaks = names("ProximityTweaks");
    if want_classes.is_empty() && want_tweaks.is_empty() {
        return true;
    }
    want_classes.iter().any(is_class) || want_tweaks.iter().any(tweak_is)
}

/// `SeqAct_CommitMapChange` ran this step: swap maps (or end the mission when the map is not
/// one of its maps).
fn commit_map_change(k: &mut Kismet, h: &mut H) {
    if !std::mem::take(&mut h.st.commit) {
        return;
    }
    let Some(map) = h.st.pending_map.take() else { return };
    let in_mission = h.m.setup.maps.iter().any(|m| m.eq_ignore_ascii_case(&map));
    if !in_mission {
        h.note(format!("level change to {map}: leaves the mission"));
        h.act(vec![Action::Complete(format!("Mission complete: travel to {map}"))]);
        return;
    }
    let tag = h.st.travel.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    let from = h.st.map.clone();
    h.note(format!("commit map change {map} at {tag}"));
    if let Some(f) = &from {
        if !f.eq_ignore_ascii_case(&map) {
            k.unload_map(f);
        }
    }
    if !h.m.current_map.eq_ignore_ascii_case(&map) || from.is_none() {
        let cfg = h.cfg;
        h.m.travel(&map, &tag, cfg);
    }
    if from.as_deref().is_none_or(|f| !f.eq_ignore_ascii_case(&map)) {
        h.st.map = Some(map.clone());
        k.load_map(h, &map);
    }
    h.drain_spawns(k);
}

/// Kismet parts of the mission view: usables, the scripted choice, the HUD target.
pub fn view(m: &MissionMatch, mv: &mut dishonored_mission::runtime::MissionView) {
    let Some(run) = &m.kismet else { return };
    // Ambient sound actors started/stopped (AkStartAmbientSound) or toggled off (Toggle).
    for (i, a) in run.k.db.actors.iter().enumerate() {
        if !a.class.starts_with("AkAmbientSound") {
            continue;
        }
        let i = i as u32;
        let state = if run.st.off.contains(&i) { Some(false) } else { run.st.ambient.get(&i).copied() };
        if let Some(on) = state {
            mv.ambient.push((format!("{}:{}", a.package, a.name), on));
        }
    }
    for (_, p, prompt) in usables(m) {
        mv.usables.push(((p + Vec3::Z * 20.0).to_array(), prompt));
    }
    if let Some((_, p, opts, None)) = &run.st.choice {
        if !mv.choices.iter().any(|c| c.0 == *p) {
            mv.choices.push((*p, opts.clone()));
        }
    }
    // HUD marker: first visible active task (in objective order) with a Kismet target.
    let mut target = None;
    for (oi, o) in m.state.def.objectives.iter().enumerate() {
        let ov = mv.objectives.get(oi);
        if ov.is_none_or(|v| v.hidden) {
            continue;
        }
        for (ti, t) in o.tasks.iter().enumerate() {
            let tv = &ov.unwrap().tasks[ti];
            if tv.state != TaskState::Active || tv.hidden {
                continue;
            }
            let path = format!("{}.{}", o.id, t.id);
            let Some(obj) = run.st.targets.get(&path) else { continue };
            let pos = match obj {
                Obj::Actor(a) => match run.st.spawner.get(a) {
                    Some(i) => m.world.player(m.setup.original[*i].spawn.id).map(|p| p.position),
                    None => Some(run.actor_pos(*a)),
                },
                Obj::Pawn(p) => m.world.player(*p).map(|p| p.position),
                _ => None,
            };
            if let Some(p) = pos {
                target = Some(p.to_array());
                break;
            }
        }
        if target.is_some() {
            break;
        }
    }
    if target.is_some() {
        mv.target = target;
    }
    mv.cutscenes = cutscene::view(run); // [interp]
    let db = &run.k.db;
    let on_map = |a: u32| {
        let Some(map) = run.st.map.as_deref() else { return true };
        let act = &db.actors[a as usize];
        db.packages.iter().position(|p| p.eq_ignore_ascii_case(&act.package)).and_then(|p| db.map_of_package(p as u16)).is_none_or(|m| m.eq_ignore_ascii_case(map))
    };
    // Doors the Kismet opened or closed, for the renderer (level_props.rs).
    for (&a, &(state, _)) in run.st.doors.iter().filter(|(a, _)| on_map(**a)) {
        let act = &db.actors[a as usize];
        mv.doors.push((format!("{}:{}", act.package, act.name), state));
    }
    // Level actors the Kismet moved, drawn where they are now.
    let moved: BTreeSet<u32> = run.st.actor_pos.keys().chain(run.st.actor_rot.keys()).copied().collect();
    for a in moved.into_iter().filter(|a| renders_moved(&db.actors[*a as usize].class) && on_map(*a)) {
        let act = &db.actors[a as usize];
        let placed = Vec3::from(act.location);
        let placed_rot = glam::Quat::from_mat3(&dishonored_kismet::interp::rot_matrix_units(act.rotation));
        let pos = run.st.actor_pos.get(&a).copied().unwrap_or(placed);
        let rot = run.st.actor_rot.get(&a).copied().unwrap_or(placed_rot);
        let (t, q) = mover_delta_parts(placed, placed_rot, pos, rot);
        if t.length_squared() < 1e-6 && q.abs_diff_eq(glam::Quat::IDENTITY, 1e-6) {
            continue;
        }
        mv.movers.push(dishonored_mission::runtime::MoverView { actor: format!("{}:{}", act.package, act.name), placed: placed.to_array(), translation: t.to_array(), rotation: q.to_array() });
    }
}

/// Classes whose moved actors the renderer draws where the Kismet put them: movers (their static
/// meshes), physics props, doors and usables (their skeletal meshes).
fn renders_moved(class: &str) -> bool {
    // The usables are `dishonored_assets::usable_props::is_usable_class`.
    class.starts_with("InterpActor") || class.starts_with("KActor") || matches!(class, "DisDoor" | "DishonoredUsableObject" | "DishonoredDynamicUsableObject" | "DisWaterSource" | "DisAlarmBell" | "DisAudioLogPlayer")
}

/// The rigid delta taking an actor from its placed transform (`p0`, `q0`) to its current one
/// (`p1`, `q1`): `current x placed^-1` as (translation, rotation).
pub fn mover_delta_parts(p0: Vec3, q0: glam::Quat, p1: Vec3, q1: glam::Quat) -> (Vec3, glam::Quat) {
    let q = (q1 * q0.inverse()).normalize();
    (p1 - q * p0, q)
}

/// Events the mission runtime should not see in Kismet missions (consumed choices).
pub fn filter_events(m: &mut MissionMatch, events: &mut Vec<MissionEvent>) {
    events.retain(|e| match e {
        MissionEvent::Choice { player, index } => !choice(m, *player, *index),
        _ => true,
    });
}

/// NPC pawns whose kind matters for touches (unused).
#[allow(dead_code)]
fn is_npc(k: PawnKind) -> bool {
    k != PawnKind::Human
}

/// Original NPCs' approach hooks (`DDH_PLAYER_APPROACHES` / `_RADIAL` of the one-shot whose me
/// speaker is the NPC's spawner): the nearest player entering a distance band starts the hook's
/// output for that band, with the original's band-edge rules
/// (`dishonored_conversation::hooks::DistanceHook`). Mirrors the use-hook binding in [`usables`].
/// E.g. Brisby's `Lord_Brisby_Quest_Start` (close band, 250 uu) on Boyle Int.
fn approach_hooks(h: &mut H) {
    let Some(cdb) = h.m.setup.conversations.clone() else { return };
    let players: Vec<Vec3> = h.m.humans.iter().filter_map(|p| h.m.world.player(*p)).filter(|p| p.alive).map(|p| p.position).collect();
    if players.is_empty() {
        return;
    }
    let mut start = Vec::new();
    for (oi, o) in cdb.oneshots.iter().enumerate() {
        let Some(me) = o.speakers.first().and_then(|s| s.actor.clone()) else { continue };
        let Some(t) = o.tree else { continue };
        let Some(idx) = h.st.spawner.iter().find(|(a, _)| h.db.actors[**a as usize].name.eq_ignore_ascii_case(&me) && h.db.actors[**a as usize].package.eq_ignore_ascii_case(&o.package)).map(|x| *x.1) else { continue };
        let id = h.m.setup.original[idx].spawn.id;
        let Some(np) = h.m.world.player(id).filter(|p| p.alive).map(|p| p.position) else { continue };
        let dist = players.iter().map(|p| p.distance(np)).fold(f32::MAX, f32::min);
        for hk in &cdb.trees[t as usize].hooks {
            let node = cdb.node(*hk);
            let dishonored_conversation::NodeKind::Hook { hook, distances: Some(d), .. } = &node.kind else { continue };
            if hook != "DDH_PLAYER_APPROACHES" && hook != "DDH_PLAYER_APPROACHES_RADIAL" {
                continue;
            }
            let linked = [0, 1, 2].map(|i| node.outputs.get(i).copied().flatten().is_some());
            if let Some(band) = h.st.approach.entry((oi as u32, *hk)).or_default().update(dist, *d, linked) {
                start.push(format!("{}:{}/{hook}#{band}", o.package, o.name));
            }
        }
    }
    for reference in start {
        h.note(format!("approach {reference}"));
        h.act(vec![Action::Conversation(reference, Vec::new())]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The replicated mover delta takes the placed transform onto the current one.
    #[test]
    fn mover_delta_maps_placed_onto_current() {
        let (p0, q0) = (Vec3::new(100.0, -50.0, 20.0), glam::Quat::from_rotation_z(0.3));
        let (p1, q1) = (Vec3::new(140.0, 10.0, 25.0), glam::Quat::from_rotation_z(1.9));
        let (t, q) = mover_delta_parts(p0, q0, p1, q1);
        let delta = glam::Mat4::from_rotation_translation(q, t);
        let now = delta * glam::Mat4::from_rotation_translation(q0, p0);
        assert!(now.abs_diff_eq(glam::Mat4::from_rotation_translation(q1, p1), 1e-3));
        // A point of the mesh moves with the actor.
        let local = Vec3::new(10.0, 0.0, 0.0);
        assert!((delta.transform_point3(p0 + q0 * local) - (p1 + q1 * local)).length() < 1e-3);
        assert!(!renders_moved("TargetPoint") && renders_moved("InterpActor") && renders_moved("DisDoor"));
    }

    #[test]
    fn view_tracking_blends_eased_and_keeps_the_freedom() {
        let t = |alpha: f32| ViewTrack { id: 1, pawn: None, point: Vec3::new(0.0, 1000.0, 0.0), start: (0.0, 0.0), alpha, rate: 1.0 / 3.0, freedom: (20.0, 20.0), hold: false, want0: Some((0.0, 0.0)) };
        let eye = Vec3::ZERO;
        // Target straight left (yaw 90): at the start the view is unchanged, halfway (eased) at
        // alpha 0.5, on target at 1.
        assert!((t(0.0).apply(eye, Vec3::new(0.0, 1000.0, 0.0), (0.0, 0.0)).0).abs() < 1e-4);
        assert!((t(0.5).apply(eye, Vec3::new(0.0, 1000.0, 0.0), (0.0, 0.0)).0 - 45.0).abs() < 1e-3);
        let eased = t(0.25).apply(eye, Vec3::new(0.0, 1000.0, 0.0), (0.0, 0.0)).0;
        assert!((eased - 90.0 * (1.0 - (0.25f32 * std::f32::consts::PI).cos()) * 0.5).abs() < 1e-3);
        // The player turned 50 degrees right since the start: kept to 20 around the target.
        let (y, _) = t(1.0).apply(eye, Vec3::new(0.0, 1000.0, 0.0), (-50.0, 0.0));
        assert!((y - 70.0).abs() < 1e-3);
    }
}
