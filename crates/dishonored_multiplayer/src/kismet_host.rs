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
    "SeqAct_ToggleCinematicMode",
    "SeqAct_ToggleHUD",
    "SeqAct_Log",

    "SeqAct_SetMatInstScalarParam",
    "SeqAct_SetMaterial",
    "SeqAct_StreamInTextures",
    "SeqAct_ConsoleCommand",
    "DisSeqAct_ToggleHUDElement",
    "DisSeqAct_ToggleTutorial",
    "DisSeqAct_SetTutorialMessage",
    "DisSeqAct_SetCurrentChapter",
    "DisSeqAct_MissionStatsTracking",
    "DisSeqAct_EvalAchievement",
    "DisSeqAct_ToggleAchievementEval",
    "DisSeqAct_IncrementPlayerStat",
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
    "DisSeqAct_PlayerTrackTarget",
    "DisSeqAct_NPCTrackTarget",
    "DisSeqAct_LimitPawnMinHealth",
    "SeqAct_SetDamageInstigator",
    "DisSeqAct_AIProtectNeutralsOverride",
    "DisSeqAct_ApplyPlayerLoadout",
];

/// A usable the Kismet offers (an actor with `DisSeqEvent_Used` or a pickup with
/// `DisSeqEvent_PickupPickedUp`, or an original NPC whose one-shot has a use hook).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum KUse {
    Actor(u32),
    Pickup(u32),
    Talk { pawn: PlayerId, oneshot: String },
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
    /// `SeqEvent_LOS` events a player is looking at: (event, player).
    los: BTreeSet<(OpId, PlayerId)>,
    /// Chaos added by `DisSeqAct_AddDarkness`.
    pub chaos: i32,
    /// Activations of classes whose system the port does not have, per class.
    pub not_modelled: BTreeMap<String, u32>,
    /// Usables moved to a stage by `DisSeqAct_ActivateUsable`: (actor, stage).
    usable_events: Vec<(u32, usize)>,
    /// `SeqAct_ActorFactory` in progress: op -> (spawn points, count, spawned, delay left).
    factories: BTreeMap<OpId, (Vec<Obj>, u32, u32, f32)>,
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

    fn players_obj(&self) -> Obj {
        self.m.humans.first().map(|p| Obj::Pawn(*p)).unwrap_or_default()
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
        if PRESENTATION.contains(&class.as_str()) {
            *self.st.presentation.entry(class).or_insert(0) += 1;
            return HostResult::Done;
        }
        if NOT_MODELLED.contains(&class.as_str()) {
            *self.st.not_modelled.entry(class).or_insert(0) += 1;
            return HostResult::Done;
        }
        match class.as_str() {
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
                    for (p, _) in self.st.shooting.remove(&cx.op).unwrap_or_default() {
                        if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                            b.calm();
                        }
                    }
                    return HostResult::Done;
                }
                let target = cx.objects("ShotTarget").into_iter().next().or_else(|| Some(cx.prop_obj("m_pShotTarget")).filter(|o| !o.is_none())).and_then(|o| self.pawn_of(&o));
                let max = cx.prop("m_iMaxNumShots").and_then(|v| v.as_int()).unwrap_or(-1);
                cx.fire(0);
                let Some(t) = target else { return HostResult::Done };
                let at = self.m.world.player(t).map(|x| x.position).unwrap_or_default();
                let ai = self.m.crowd.ai.clone();
                let mut list = Vec::new();
                for p in self.target_pawns(cx) {
                    let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) else { continue };
                    b.alert_to(&ai, t, at);
                    b.target = Some(t);
                    b.state = BrainState::Combat;
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
                self.st.noises.push(SimEvent::Sound { position: pos, radius, category: dishonored_sim::events::SoundCategory::Pistol, source });
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
            // `OnAIStartDistraction` (0x00b63ed0): an NPC with a distraction behaviour goes to
            // the distractor (a soiree object) and plays it.
            "DisSeqAct_AIStartDistraction" => {
                let at = cx.objects("Distractor").into_iter().next().or_else(|| Some(cx.prop_obj("m_pDistractor")).filter(|o| !o.is_none())).and_then(|o| self.pos_of(&o));
                if let Some(g) = at {
                    for p in self.target_pawns(cx) {
                        if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p).filter(|b| b.state.is_routine()) {
                            b.goal = Some(g);
                            b.run = false;
                            b.stay = true;
                            b.state = BrainState::WalkToPoint;
                            b.follow = Default::default();
                        }
                    }
                }
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
            // Chaos (`DisSeqAct_AddDarkness`, 0x00bb3360: adds `m_Value` to the player's chaos
            // and shows `m_Text`): kept with the mission statistics.
            "DisSeqAct_AddDarkness" => {
                let v = cx.prop_int("m_Value");
                self.st.chaos += v;
                self.note(format!("chaos {v:+} (total {})", self.st.chaos));
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
                    let (ns, nl) = match input {
                        0 => (1, locked),
                        1 => (2, locked),
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
                    // The shoot behaviour ends with its pawn (death, knock-out) or when the pawn
                    // calms down without its target.
                    if !alive || b.is_none_or(|b| b.target.is_none() && b.state.is_routine()) {
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
            k.tick(&mut h, dt);
            cutscene::apply(k, &mut h); // [interp]
            h.drain_spawns(k);
            commit_map_change(k, &mut h);
        }
    }
    m.kismet = Some(run);
}

/// Gameplay -> Kismet events.
fn feed(k: &mut Kismet, h: &mut H, sim_events: &[SimEvent]) {
    h.drain_spawns(k);
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
                    h.note(format!("pawn {victim} took {amount:.0} damage: {}", h.db.describe(ev)));
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
                h.note(format!("death of pawn {victim}: {} death / {} incapacitated events", d.len(), n.len()));
            }
        }
    }
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
