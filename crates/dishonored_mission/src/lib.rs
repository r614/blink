//! Custom mission framework.
//!
//! Modelled on how Dishonored builds its missions (see `research/dishonored/MISSIONS.md`):
//! Arkane authors **objectives** made of **tasks** (`DishonoredObjective` /
//! `DishonoredTask_Custom`, each task Inactive/Active/Completed/Failed and possibly hidden),
//! global **story flags** (`SF_*` sets), **spawners** and named level actors, and wires them
//! together in Kismet graphs of **events -> conditions -> actions** (`SeqEvent_Touch`,
//! `DisSeqEvent_Spawned`, `DisSeqCond_CheckStoryFlag`, `DisSeqAct_SetTaskState`,
//! `DisSeqAct_ShowHUDMessage`, `DisSeqAct_AIGoToActor`, `SeqAct_Delay`, remote events...).
//!
//! A mission here is the same model as plain data (RON): objectives/tasks, flags, counters,
//! markers (positions or original actor names such as `TargetPoint_5`), mission NPCs, and
//! rules `on: <event>, when: [<conditions>], then: [<actions>]`. The runtime
//! ([`runtime::MissionState`]) is deterministic and independent of rendering, so missions run
//! on dedicated servers, in replays and in the harness exactly as offline.

pub mod runtime;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A whole mission file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MissionDef {
    pub title: String,
    /// Map the mission plays on (persistent level name, e.g. `L_Boyle_Ext_P`).
    pub map: String,
    /// Further persistent maps reachable through the original travel doors
    /// (e.g. `L_Boyle_Int_P`). Level state is saved per map, as in the original.
    #[serde(default)]
    pub maps: Vec<String>,
    #[serde(default)]
    pub briefing: String,
    pub objectives: Vec<ObjectiveDef>,
    /// Story flags (all start false unless listed in `flags_set`).
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub flags_set: Vec<String>,
    /// Integer counters (start at 0).
    #[serde(default)]
    pub counters: Vec<String>,
    #[serde(default)]
    pub markers: Vec<MarkerDef>,
    #[serde(default)]
    pub npcs: Vec<NpcDef>,
    #[serde(default)]
    pub rules: Vec<RuleDef>,
    /// Marker where players start (default: the map's player starts).
    #[serde(default)]
    pub player_start: Option<String>,
    /// Ambient crowd (civilians/guards from the original placement data).
    #[serde(default)]
    pub crowd: Option<CrowdDef>,
    /// Maps populated from their original spawners (`DishonoredSpawner`s that spawn on begin
    /// play) on first visit; their alarm squads and `DisTravelSpawner` pursuers follow the
    /// original rules (see dishonored_multiplayer::mission).
    #[serde(default)]
    pub original_npcs: Vec<String>,
    #[serde(default)]
    pub settings: MissionSettings,
    /// Extra cooked packages whose dialog graphs to load (the packages named by
    /// conversation references, and the map's packages when a reference is unqualified,
    /// are loaded anyway).
    #[serde(default)]
    pub dialog_packages: Vec<String>,
    /// Run the original Kismet of the mission maps and the global game sequence
    /// (`dishonored_kismet`) instead of hand-written rules: objectives and tasks, travel,
    /// spawns and the scripted logic come from the original data.
    #[serde(default)]
    pub original_kismet: Option<OriginalKismet>,
}

/// How a mission runs the original Kismet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OriginalKismet {
    /// Remote event of the global game sequence that starts the mission, the way the
    /// original's level change into the first map does (`ChangeLvl_fromHub_toBoyle`).
    pub start_event: String,
    /// Seed of the original random switches (`SeqAct_RandomSwitch`); runs with the same seed
    /// play out the same.
    #[serde(default)]
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissionSettings {
    /// Players respawn at the start after dying (otherwise death fails the mission when
    /// everyone is down).
    pub respawn: bool,
    pub respawn_delay: f32,
    /// Seconds; 0 = none. Running out fails the mission.
    pub time_limit: f32,
    /// Fraction of mission NPCs + crowd killed above which chaos is reported High
    /// (APPROXIMATION of Dishonored's kill-ratio chaos).
    pub high_chaos_kill_ratio: f32,
}

impl Default for MissionSettings {
    fn default() -> Self {
        Self { respawn: false, respawn_delay: 5.0, time_limit: 0.0, high_chaos_kill_ratio: 0.2 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectiveDef {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub hidden: bool,
    pub tasks: Vec<TaskDef>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TaskState {
    #[default]
    Inactive,
    Active,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskDef {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub initial: TaskState,
    #[serde(default)]
    pub hidden: bool,
    /// Optional tasks are shown as such and never block completion.
    #[serde(default)]
    pub optional: bool,
    /// What the HUD marker points at while the task is active.
    #[serde(default)]
    pub target: Option<TargetRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TargetRef {
    Marker(String),
    Npc(String),
}

/// Where something is in the world.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Place {
    /// Dishonored world units.
    At(f32, f32, f32),
    /// An original level actor by name (`TargetPoint_5`, `DishonoredSpawner_46`,
    /// `PlayerStart_1`, `DisTrigger_32`...), optionally `Package:Name`.
    Actor(String),
    /// A `PlayerStart` by its travel tag (`PlayerStart_Garden`), in any mission map.
    Start(String),
    /// Another marker of this mission.
    Marker(String),
    /// Offset from a place.
    Offset(Box<Place>, (f32, f32, f32)),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarkerDef {
    pub id: String,
    pub at: Place,
    /// Zone radius for Enter/Leave events (uu).
    #[serde(default = "default_radius")]
    pub radius: f32,
    /// Interaction prompt ("Open", "Pick up", "Read"...): the marker can be used with F.
    #[serde(default)]
    pub usable: Option<String>,
}

fn default_radius() -> f32 {
    250.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NpcKind {
    Civilian,
    Guard,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum Behavior {
    /// Stand at the spawn point.
    #[default]
    Idle,
    /// Stroll between nearby original AI points.
    Wander,
    /// Walk a route of markers in a loop.
    Patrol(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NpcDef {
    pub id: String,
    pub kind: NpcKind,
    /// Body archetype (0 masked gentleman, 1 guard, 2 gentleman B, 3 masked lady).
    #[serde(default)]
    pub archetype: Option<u8>,
    pub at: Place,
    #[serde(default = "yes")]
    pub spawn_at_start: bool,
    #[serde(default)]
    pub behavior: Behavior,
    /// Display name for messages and the target notification.
    #[serde(default)]
    pub name: Option<String>,
    /// Talk prompt: the NPC can be talked to with F (`Talk(npc)` trigger).
    #[serde(default)]
    pub talk: Option<String>,
    /// Original conversation played when a player talks to this NPC: a one-shot
    /// (`"L_Boyle_Int_Script:DisDialogOneShot_9"`, entered through its `DDH_PLAYER_USED`
    /// hook like pressing use in the original) or a conversation
    /// (`"L_Boyle_Int_Script:DisDialogOneShot_9/Hits_On_Player"`). Implies a "Talk" prompt.
    #[serde(default)]
    pub talk_conversation: Option<String>,
    /// Original actor this NPC stands in for (normally the `DishonoredSpawner_N` a dialog
    /// one-shot attaches its speaker to). Defaults to `at` when that is an `Actor(..)`.
    #[serde(default)]
    pub speaker: Option<String>,
}

impl NpcDef {
    /// The original actor whose dialog lines this NPC speaks.
    pub fn speaker_actor(&self) -> Option<&str> {
        self.speaker.as_deref().or(match &self.at {
            Place::Actor(a) => Some(a.as_str()),
            _ => None,
        })
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrowdDef {
    pub civilians: usize,
    pub guards: usize,
    /// Centre of the crowd (default: player start).
    #[serde(default)]
    pub center: Option<Place>,
    #[serde(default = "crowd_radius")]
    pub radius: f32,
}

fn crowd_radius() -> f32 {
    4000.0
}

/// `on: <trigger>, when: [conditions], then: [actions]` (Kismet event -> condition -> action).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleDef {
    #[serde(default)]
    pub id: Option<String>,
    pub on: Trigger,
    #[serde(default)]
    pub when: Vec<Condition>,
    pub then: Vec<Action>,
    /// Fire at most once (Kismet `MaxTriggerCount = 1`), default true.
    #[serde(default = "yes")]
    pub once: bool,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Trigger {
    /// Mission begins (`SeqEvent_LevelLoaded`).
    Start,
    /// Seconds after start.
    Timer(f32),
    /// Every N seconds.
    Every(f32),
    /// A player enters / leaves a marker zone (`SeqEvent_Touch` Touched/UnTouched).
    Enter(String),
    Leave(String),
    /// A mission NPC dies; `NpcKilledBy` requires a player killer.
    NpcKilled(String),
    /// Any NPC (crowd or mission) killed by a player.
    AnyKill,
    /// A mission NPC becomes alerted (alert, combat or fleeing).
    NpcAlerted(String),
    /// A guard notices a player (enters combat with them).
    PlayerDetected,
    /// First guard goes to combat.
    AlarmRaised,
    /// A player dies.
    PlayerKilled,
    FlagSet(String),
    FlagCleared(String),
    /// A task reaches a state (`"objective.task"`).
    TaskState(String, TaskState),
    /// A counter reaches at least a value.
    CounterAtLeast(String, i32),
    /// Fired by the `Fire` action (`SeqEvent_RemoteEvent`).
    Remote(String),
    /// Players arrived in a persistent map after level travel (`SeqEvent_LevelLoaded`).
    Arrived(String),
    /// A player uses a usable marker (F), `DisSeqEvent_Used`.
    Use(String),
    /// A player talks to an NPC (F).
    Talk(String),
    /// An original conversation reaches a `DisConv_KismetActivateRemoteEvent` (the
    /// `DisSeqEvent_DialogOutputs` output of that name). `"Name"` or
    /// `"DisDialogOneShot_N/Name"` to require the one-shot.
    ConversationOutput(String),
    /// An original conversation ends (label, `OneShot/label` or `Package:OneShot/label`).
    ConversationEnded(String),
    /// A player picked up an item (`DisSeqEvent_PickupPickedUp`): the pickup actor name
    /// (`DisAbstractItemPickup_11`) or the abstract item it gives (`Rune_AbsItm`, full or short).
    PickedUp(String),
    /// A mission NPC was knocked out (choke, sleep dart): `DisSeqEvent_KnockedOut`.
    NpcKnockedOut(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Condition {
    Flag(String),
    NotFlag(String),
    Task(String, TaskState),
    CounterAtLeast(String, i32),
    CounterBelow(String, i32),
    /// Any player has been detected so far.
    Detected,
    /// Some player is inside a marker zone.
    Inside(String),
    NpcAlive(String),
    Any(Vec<Condition>),
    All(Vec<Condition>),
    Not(Box<Condition>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Action {
    /// `DisSeqAct_SetTaskState`: `"objective.task"`.
    SetTask(String, TaskState),
    /// `DisSeqAct_SetTaskHidden(false/true)`.
    ShowTask(String),
    HideTask(String),
    ShowObjective(String),
    HideObjective(String),
    /// `DisSeqAct_SetStoryFlag`.
    SetFlag(String),
    ClearFlag(String),
    Add(String, i32),
    Set(String, i32),
    /// `DisSeqAct_ShowHUDMessage`.
    Message(String),
    /// `DisSeqAct_ShowTargetNotification`: title and text on the target paper.
    Notify(String, String),
    /// `DisSeqAct_StartSpawn` / despawn.
    Spawn(String),
    Despawn(String),
    /// `DisSeqAct_AIGoToActor` (walk) / `DisSeqAct_AISetPatrol`.
    MoveNpc(String, String),
    RunNpc(String, String),
    Patrol(String, Vec<String>),
    /// Turn a mission NPC hostile to the nearest player / calm it down
    /// (`DisSeqAct_SetDisposition`).
    Alert(String),
    Calm(String),
    /// `SeqAct_ActivateRemoteEvent`.
    Fire(String),
    /// `SeqAct_Delay`: run actions later.
    After(f32, Vec<Action>),
    EnableRule(String),
    DisableRule(String),
    /// `SeqAct_AkPostEvent`: a Wwise event name (presentation only).
    Sound(String),
    /// Make a marker usable with a prompt, or not usable.
    Usable(String, String),
    NotUsable(String),
    /// Level travel to a tagged `PlayerStart` (`SetPlayerTravelDestination` + change level).
    Travel(String),
    /// `DisSeqAct_NPCSetMaterials` "Set": replace materials of an NPC's body and head meshes
    /// (`m_NewBodyMaterials` / `m_NewHeadMaterials`: material index, material object path), e.g.
    /// Boyle's dresses: `SetMaterials(npc: "DishonoredSpawner_11", body: [(0, "Npc_Boyle.Materials.Boyle_red_inst")],
    /// head: [(0, "Npc_Masks.BOYLE.BoyleMask_red_inst")])`. The NPC is a mission NPC id or an
    /// original spawner name (maps listed in `original_npcs`).
    SetMaterials { npc: String, body: Vec<(u32, String)>, head: Vec<(u32, String)> },
    /// `DisSeqAct_NPCSetMaterials` "Clear": back to the NPC's own materials.
    ClearMaterials(String),
    /// Start an original conversation (`DisSeqAct_DialogInputs`): a conversation
    /// (`"Package:DisDialogOneShot_N/<label>"`), a Kismet hook name of the one-shot
    /// (`".../Come In"`) or a one-shot (its use hook). The NPC ids speak the one-shot's
    /// speakers in order (me, then others); unlisted speakers bind to NPCs standing in for
    /// the speaker's original actor (`NpcDef.speaker`/`at`). Interrupts the one-shot.
    Conversation(String, Vec<String>),
    /// Stop conversations matching a reference (`""` = all); their `Interrupted` output runs.
    StopConversation(String),
    /// End the mission.
    Complete(String),
    Fail(String),
}

impl MissionDef {
    pub fn from_ron(text: &str) -> Result<MissionDef> {
        let def: MissionDef = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME | ron::extensions::Extensions::UNWRAP_NEWTYPES)
            .from_str(text)
            .map_err(|e| anyhow::anyhow!("mission parse error: {e}"))?;
        Ok(def)
    }

    pub fn load(path: &std::path::Path) -> Result<MissionDef> {
        Self::from_ron(&std::fs::read_to_string(path)?)
    }

    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default().struct_names(true)).unwrap_or_default()
    }

    /// Look up `"objective.task"`.
    pub fn task_index(&self, path: &str) -> Option<(usize, usize)> {
        let (o, t) = path.split_once('.')?;
        let oi = self.objectives.iter().position(|x| x.id == o)?;
        let ti = self.objectives[oi].tasks.iter().position(|x| x.id == t)?;
        Some((oi, ti))
    }

    /// Static checks: every reference resolves (actor names are checked by
    /// [`validate_actors`] when the level's markers are known).
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let markers: BTreeSet<&str> = self.markers.iter().map(|m| m.id.as_str()).collect();
        let npcs: BTreeSet<&str> = self.npcs.iter().map(|n| n.id.as_str()).collect();
        let flags: BTreeSet<&str> = self.flags.iter().chain(&self.flags_set).map(|s| s.as_str()).collect();
        let counters: BTreeSet<&str> = self.counters.iter().map(|s| s.as_str()).collect();
        let rules: BTreeSet<&str> = self.rules.iter().filter_map(|r| r.id.as_deref()).collect();
        let mut dup = BTreeSet::new();
        for id in self.markers.iter().map(|m| &m.id).chain(self.npcs.iter().map(|n| &n.id)) {
            if !dup.insert(id.clone()) {
                errs.push(format!("duplicate id '{id}'"));
            }
        }
        let obj_ids: BTreeSet<&str> = self.objectives.iter().map(|o| o.id.as_str()).collect();
        let task = |p: &str, errs: &mut Vec<String>| {
            if self.task_index(p).is_none() {
                errs.push(format!("unknown task '{p}' (use \"objective.task\")"));
            }
        };
        let marker = |m: &str, errs: &mut Vec<String>| {
            if !markers.contains(m) {
                errs.push(format!("unknown marker '{m}'"));
            }
        };
        let npc = |n: &str, errs: &mut Vec<String>| {
            if !npcs.contains(n) {
                errs.push(format!("unknown npc '{n}'"));
            }
        };
        let flag = |f: &str, errs: &mut Vec<String>| {
            if !flags.contains(f) {
                errs.push(format!("undeclared flag '{f}' (add it to `flags`)"));
            }
        };
        let counter = |c: &str, errs: &mut Vec<String>| {
            if !counters.contains(c) {
                errs.push(format!("undeclared counter '{c}' (add it to `counters`)"));
            }
        };
        fn place(p: &Place, markers: &BTreeSet<&str>, errs: &mut Vec<String>) {
            match p {
                Place::Marker(m) if !markers.contains(m.as_str()) => errs.push(format!("unknown marker '{m}'")),
                Place::Offset(inner, _) => place(inner, markers, errs),
                _ => {}
            }
        }
        for o in &self.objectives {
            for t in &o.tasks {
                match &t.target {
                    Some(TargetRef::Marker(m)) => marker(m, &mut errs),
                    Some(TargetRef::Npc(n)) => npc(n, &mut errs),
                    None => {}
                }
            }
        }
        for m in &self.markers {
            place(&m.at, &markers, &mut errs);
        }
        for n in &self.npcs {
            place(&n.at, &markers, &mut errs);
            if let Behavior::Patrol(route) = &n.behavior {
                for m in route {
                    marker(m, &mut errs);
                }
            }
        }
        if let Some(s) = &self.player_start {
            marker(s, &mut errs);
        }
        fn cond(c: &Condition, f: &mut dyn FnMut(&Condition)) {
            f(c);
            match c {
                Condition::Any(v) | Condition::All(v) => v.iter().for_each(|x| cond(x, f)),
                Condition::Not(x) => cond(x, f),
                _ => {}
            }
        }
        fn act(a: &Action, f: &mut dyn FnMut(&Action)) {
            f(a);
            if let Action::After(_, v) = a {
                v.iter().for_each(|x| act(x, f));
            }
        }
        for (ri, r) in self.rules.iter().enumerate() {
            let mut e = Vec::new();
            match &r.on {
                Trigger::Enter(m) | Trigger::Leave(m) => marker(m, &mut e),
                Trigger::NpcKilled(n) | Trigger::NpcAlerted(n) => npc(n, &mut e),
                Trigger::FlagSet(x) | Trigger::FlagCleared(x) => flag(x, &mut e),
                Trigger::Use(m) => marker(m, &mut e),
                Trigger::Talk(n) | Trigger::NpcKnockedOut(n) => npc(n, &mut e),
                Trigger::TaskState(t, _) => task(t, &mut e),
                Trigger::CounterAtLeast(c, _) => counter(c, &mut e),
                Trigger::Timer(s) | Trigger::Every(s) if *s <= 0.0 => e.push("timer must be > 0".into()),
                _ => {}
            }
            for c in &r.when {
                cond(c, &mut |c| match c {
                    Condition::Flag(x) | Condition::NotFlag(x) => flag(x, &mut e),
                    Condition::Task(t, _) => task(t, &mut e),
                    Condition::CounterAtLeast(x, _) | Condition::CounterBelow(x, _) => counter(x, &mut e),
                    Condition::Inside(m) => marker(m, &mut e),
                    Condition::NpcAlive(n) => npc(n, &mut e),
                    _ => {}
                });
            }
            for a in &r.then {
                act(a, &mut |a| match a {
                    Action::SetTask(t, _) | Action::ShowTask(t) | Action::HideTask(t) => task(t, &mut e),
                    Action::ShowObjective(o) | Action::HideObjective(o) if !obj_ids.contains(o.as_str()) => e.push(format!("unknown objective '{o}'")),
                    Action::SetFlag(x) | Action::ClearFlag(x) => flag(x, &mut e),
                    Action::Add(x, _) | Action::Set(x, _) => counter(x, &mut e),
                    Action::Spawn(n) | Action::Despawn(n) | Action::Alert(n) | Action::Calm(n) => npc(n, &mut e),
                    Action::Usable(m, _) | Action::NotUsable(m) => marker(m, &mut e),
                    Action::MoveNpc(n, m) | Action::RunNpc(n, m) => {
                        npc(n, &mut e);
                        marker(m, &mut e);
                    }
                    Action::Patrol(n, route) => {
                        npc(n, &mut e);
                        route.iter().for_each(|m| marker(m, &mut e));
                    }
                    Action::EnableRule(r) | Action::DisableRule(r) if !rules.contains(r.as_str()) => e.push(format!("unknown rule '{r}'")),
                    Action::Conversation(c, speakers) => {
                        if c.trim().is_empty() {
                            e.push("Conversation needs a reference".into());
                        }
                        speakers.iter().filter(|s| !s.is_empty()).for_each(|s| npc(s, &mut e));
                    }
                    _ => {}
                });
            }
            let label = r.id.clone().unwrap_or_else(|| format!("#{ri}"));
            errs.extend(e.into_iter().map(|m| format!("rule {label}: {m}")));
        }
        // Original-Kismet missions take their objectives and their end from the original data.
        if self.objectives.is_empty() && self.original_kismet.is_none() {
            errs.push("mission has no objectives".into());
        }
        if self.original_kismet.is_none() && !self.rules.iter().any(|r| contains_end(&r.then)) {
            errs.push("no rule ever completes the mission (Complete action)".into());
        }
        errs
    }
}

impl MissionDef {
    /// Every original conversation reference in the mission (actions and NPC talk).
    pub fn conversation_refs(&self) -> Vec<String> {
        fn walk(a: &[Action], out: &mut Vec<String>) {
            for x in a {
                match x {
                    Action::Conversation(c, _) => out.push(c.clone()),
                    Action::After(_, v) => walk(v, out),
                    _ => {}
                }
            }
        }
        let mut out: Vec<String> = self.npcs.iter().filter_map(|n| n.talk_conversation.clone()).collect();
        for r in &self.rules {
            walk(&r.then, &mut out);
        }
        out.sort();
        out.dedup();
        out
    }

    /// Cooked packages whose dialog graphs the mission needs: the package prefix of each
    /// qualified reference, `dialog_packages`, and `map_packages` when a reference has no
    /// package.
    pub fn dialog_packages_needed(&self, map_packages: &[String]) -> Vec<String> {
        let mut out = self.dialog_packages.clone();
        for r in self.conversation_refs() {
            match r.split_once(':') {
                Some((p, _)) => out.push(p.to_string()),
                None => out.extend(map_packages.iter().cloned()),
            }
        }
        let mut seen = BTreeSet::new();
        out.retain(|p| seen.insert(p.to_ascii_lowercase()));
        out
    }
}

/// Check every conversation reference against the loaded dialog data.
pub fn validate_conversations(def: &MissionDef, db: &dishonored_conversation::ConversationDb) -> Vec<String> {
    def.conversation_refs().iter().filter_map(|r| db.resolve(r).err()).collect()
}

fn contains_end(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::Complete(_) => true,
        Action::After(_, v) => contains_end(v),
        _ => false,
    })
}

/// Check `Place::Actor` names against the level's actor names (`Name` or `Package:Name`).
pub fn validate_actors(def: &MissionDef, actor_names: &[(String, String)]) -> Vec<String> {
    let mut errs = Vec::new();
    let mut check = |p: &Place| {
        let mut p = p;
        while let Place::Offset(inner, _) = p {
            p = inner;
        }
        if let Place::Actor(a) = p {
            if let Err(e) = resolve_actor(a, actor_names) {
                errs.push(format!("{e} (map {})", def.map));
            }
        }
    };
    def.markers.iter().for_each(|m| check(&m.at));
    def.npcs.iter().for_each(|n| check(&n.at));
    if let Some(c) = def.crowd.as_ref().and_then(|c| c.center.as_ref()) {
        check(c);
    }
    errs
}

/// Resolve an actor reference to an index into `actor_names` ((package, name) pairs).
pub fn resolve_actor(reference: &str, actor_names: &[(String, String)]) -> Result<usize> {
    let (pkg, name) = match reference.split_once(':') {
        Some((p, n)) => (Some(p), n),
        None => (None, reference),
    };
    let hits: Vec<usize> = actor_names.iter().enumerate().filter(|(_, (p, n))| n.eq_ignore_ascii_case(name) && pkg.is_none_or(|q| p.eq_ignore_ascii_case(q))).map(|(i, _)| i).collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => bail!("actor '{reference}' not found"),
        _ => bail!("actor '{reference}' is ambiguous; prefix its package (e.g. `L_Boyle_Ext_Script:{name}`)"),
    }
}
