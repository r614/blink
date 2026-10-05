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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Classes that only present something (camera, HUD, tutorials, saves, statistics, ambient
/// audio, materials): run as no-ops, counted apart from the unsupported ones.
pub const PRESENTATION: &[&str] = &[
    "SeqAct_CameraFade",
    "SeqAct_ToggleCinematicMode",
    "SeqAct_ToggleHUD",
    "SeqAct_Log",
    "SeqAct_AkStartAmbientSound",
    "SeqAct_SetMatInstScalarParam",
    "SeqAct_SetMaterial",
    "SeqAct_StreamInTextures",
    "SeqAct_ConsoleCommand",
    "DisSeqAct_ToggleHUDElement",
    "DisSeqAct_ToggleTutorial",
    "DisSeqAct_SetTutorialMessage",
    "DisSeqAct_SetCurrentChapter",
    "DisSeqAct_MissionStatsTracking",
    "DisSeqAct_ShowMissionStats",
    "DisSeqAct_EvalAchievement",
    "DisSeqAct_ToggleAchievementEval",
    "DisSeqAct_IncrementPlayerStat",
    "DisSeqAct_GetPlayerStat",
    "DisSeqAct_ToggleJournal",
    "DisSeqAct_TogglePowerWheel",
];

/// A usable the Kismet offers (an actor with `DisSeqEvent_Used` or a pickup with
/// `DisSeqEvent_PickupPickedUp`, or an original NPC whose one-shot has a use hook).
#[derive(Debug, Clone, PartialEq)]
pub enum KUse {
    Actor(u32),
    Pickup(u32),
    Talk { pawn: PlayerId, oneshot: String },
}

/// Host-side state.
#[derive(Debug, Clone, Default)]
pub struct HostState {
    pub started: bool,
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
    goto: BTreeMap<OpId, (PlayerId, Vec3)>,
    /// `StartSpawn` in progress: op -> (spawner originals not yet spawned, spawned so far).
    spawning: BTreeMap<OpId, (Vec<usize>, Vec<PlayerId>)>,
    /// Spawner actor -> index into `MissionSetup::original`.
    pub spawner: BTreeMap<u32, usize>,
    pub spawner_actor: BTreeMap<usize, u32>,
    /// Abstract items the player holds (`AddAbstractItem` / `RemoveAbstractItem`).
    pub items: BTreeSet<String>,
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
}

pub struct KismetRun {
    pub k: Kismet,
    pub st: HostState,
    pub setup: Arc<KismetSetup>,
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
        KismetRun { k, st, setup: Arc::new(setup.clone()) }
    }

    /// A player is answering a scripted choice (input is frozen like the original's
    /// `StatePlayerMasterInScriptedChoice`).
    pub fn choosing(&self, p: PlayerId) -> bool {
        self.st.choice.as_ref().is_some_and(|c| c.1 == p && c.3.is_none())
    }

    pub fn actor_pos(&self, a: u32) -> Vec3 {
        self.st.actor_pos.get(&a).copied().unwrap_or_else(|| Vec3::from(self.k.db.actors[a as usize].location))
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
            }
            Matinee::Finished { .. } => {
                let now = cx.k.time;
                if let Some(m) = self.st.matinees.iter_mut().rev().find(|m| m.0 == cx.op && m.3.is_none()) {
                    m.3 = Some(now);
                }
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
        let class = cx.class().to_string();
        if PRESENTATION.contains(&class.as_str()) {
            *self.st.presentation.entry(class).or_insert(0) += 1;
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
            "DisSeqAct_AddAbstractItem" | "DisSeqAct_RemoveAbstractItem" => {
                let item = obj_name(&cx.prop_obj("m_pAbstractItem"));
                if class == "DisSeqAct_AddAbstractItem" {
                    self.st.items.insert(item);
                } else {
                    self.st.items.remove(&item);
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
            "DisSeqAct_AIGoToActor" => {
                let pawn = cx.objects("Target").into_iter().find_map(|o| self.pawn_of(&o));
                let dest = cx.objects("Destination").into_iter().next().or_else(|| Some(cx.prop_obj("m_pDestinationActor")).filter(|o| !o.is_none()));
                let goal = dest.as_ref().and_then(|d| self.pos_of(d));
                cx.fire(0);
                match (pawn, goal) {
                    (Some(p), Some(g)) => {
                        let run = cx.prop_str("m_DesiredMovementSpeed").to_ascii_lowercase().contains("run");
                        if let Some(b) = self.m.crowd.brains.iter_mut().find(|b| b.id == p) {
                            b.goal = Some(g);
                            b.run = run;
                            b.spot = None;
                            b.stay = true;
                            b.state = BrainState::WalkToPoint;
                            b.follow = Default::default();
                        }
                        self.st.goto.insert(cx.op, (p, g));
                        HostResult::Latent
                    }
                    _ => {
                        cx.fire(2);
                        HostResult::Done
                    }
                }
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
            "DisSeqAct_SetDisposition" => {
                let disp = match cx.input() {
                    Some(0) => "Relationship_Hostile".to_string(),
                    Some(1) => "Relationship_Neutral".to_string(),
                    Some(2) => "Relationship_Friendly".to_string(),
                    _ => cx.prop_str("m_Disposition"),
                };
                let _ = disp;
                // Faction-wide dispositions (Target = a faction tweak) are not modelled.
                HostResult::Unhandled
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
            "SeqAct_AkPostEvent" => {
                let ev = obj_name(&cx.prop_obj("Event"));
                if !ev.is_empty() {
                    self.m.state.post_sound(&ev);
                }
                HostResult::Done
            }
            "DisSeqAct_AutoSave" => HostResult::Latent,
            _ => HostResult::Unhandled,
        }
    }

    fn update(&mut self, cx: &mut Ctx, _dt: f32) -> bool {
        match cx.class() {
            "SeqAct_PrepareMapChange" => true,
            "DisSeqAct_AutoSave" => {
                // Saving is not available: the original's "saves disabled" path (output 1).
                cx.fire(1);
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
                let Some((p, g)) = self.st.goto.get(&cx.op).copied() else { return true };
                if cx.has_impulse(1) {
                    self.st.goto.remove(&cx.op);
                    cx.fire(3);
                    return true;
                }
                let Some(pl) = self.m.world.player(p).filter(|x| x.alive) else {
                    self.st.goto.remove(&cx.op);
                    cx.fire(2);
                    return true;
                };
                let b = self.m.crowd.brains.iter().find(|b| b.id == p);
                let reached = pl.position.truncate().distance(g.truncate()) < 90.0;
                if reached {
                    self.st.goto.remove(&cx.op);
                    cx.fire(1);
                    return true;
                }
                match b {
                    Some(b) if b.state == BrainState::WalkToPoint => false,
                    Some(b) if !b.state.is_routine() => {
                        self.st.goto.remove(&cx.op);
                        cx.fire(3);
                        true
                    }
                    Some(b) if b.follow.failed => {
                        self.st.goto.remove(&cx.op);
                        cx.fire(2);
                        true
                    }
                    Some(_) => {
                        // Arrived as far as the navmesh allows.
                        self.st.goto.remove(&cx.op);
                        cx.fire(1);
                        true
                    }
                    None => {
                        self.st.goto.remove(&cx.op);
                        cx.fire(2);
                        true
                    }
                }
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
    for (cls, pick) in [("DisSeqEvent_Used", false), ("DisSeqEvent_PickupPickedUp", true)] {
        for e in k.events_for(cls, &[]) {
            if !k.ops[e as usize].enabled {
                continue;
            }
            let Some(Obj::Actor(a)) = k.event_originator(e) else { continue };
            if run.st.gone.contains(&a) || run.st.off.contains(&a) || !seen.insert(a) {
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
            feed(k, &mut h, sim_events);
            k.tick(&mut h, dt);
            h.drain_spawns(k);
            commit_map_change(k, &mut h);
        }
    }
    m.kismet = Some(run);
}

/// Gameplay -> Kismet events.
fn feed(k: &mut Kismet, h: &mut H, sim_events: &[SimEvent]) {
    h.drain_spawns(k);
    // Uses.
    for (p, u) in std::mem::take(&mut h.st.uses) {
        match u {
            KUse::Actor(a) => {
                h.note(format!("used {}", h.db.actors[a as usize].name));
                k.trigger(h, "DisSeqEvent_Used", &[Obj::Actor(a)], Obj::Pawn(p), None);
            }
            KUse::Pickup(a) => {
                h.note(format!("picked up {}:{}", h.db.actors[a as usize].package, h.db.actors[a as usize].name));
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
        let actor = h.db.actors[a as usize].clone();
        if actor.shape.is_none() {
            continue;
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
            let pos = p.position;
            let pos = if actor.location != [0.0; 3] { pos } else { pos };
            if actor.touches(pos.to_array(), 34.0, 90.0) {
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
                k.touch(h, e, o.clone(), false);
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
            dishonored_conversation::runner::ConvEvent::Soiree { guid, .. } => {
                if let Some(op) = k.play_matinee_guid(&guid) {
                    h.note(format!("soiree plays matinee {}", h.db.describe(op)));
                }
            }
            _ => {}
        }
    }
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
