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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MissionDef {
    pub title: String,
    /// Map the mission plays on (persistent level name, e.g. `L_Boyle_Ext_P`).
    pub map: String,
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
    #[serde(default)]
    pub settings: MissionSettings,
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
                    Action::MoveNpc(n, m) | Action::RunNpc(n, m) => {
                        npc(n, &mut e);
                        marker(m, &mut e);
                    }
                    Action::Patrol(n, route) => {
                        npc(n, &mut e);
                        route.iter().for_each(|m| marker(m, &mut e));
                    }
                    Action::EnableRule(r) | Action::DisableRule(r) if !rules.contains(r.as_str()) => e.push(format!("unknown rule '{r}'")),
                    _ => {}
                });
            }
            let label = r.id.clone().unwrap_or_else(|| format!("#{ri}"));
            errs.extend(e.into_iter().map(|m| format!("rule {label}: {m}")));
        }
        if self.objectives.is_empty() {
            errs.push("mission has no objectives".into());
        }
        if !self.rules.iter().any(|r| contains_end(&r.then)) {
            errs.push("no rule ever completes the mission (Complete action)".into());
        }
        errs
    }
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
