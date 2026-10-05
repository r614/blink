//! Deterministic mission interpreter. Each fixed step the host (a match on a server, offline
//! or in the harness) passes player positions and gameplay events; the interpreter fires
//! matching rules (cascading through flags, tasks, counters and remote events like Kismet
//! does within a frame), and returns commands for the host to apply to NPCs.

use crate::{Action, Behavior, Condition, MissionDef, NpcKind, TargetRef, TaskState, Trigger};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
    /// Crowd + mission NPC count, for the chaos ratio.
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
    pub fn high_chaos(&self, ratio: f32) -> bool {
        self.total_npcs > 0 && (self.hostiles_killed + self.civilians_killed) as f32 / self.total_npcs as f32 > ratio
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TaskView {
    pub description: String,
    pub state: TaskState,
    pub optional: bool,
    pub hidden: bool,
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
    pub outcome: Option<Outcome>,
    pub stats: MissionStats,
    pub high_chaos: bool,
    /// World position the HUD marker points at (active task target).
    pub target: Option<[f32; 3]>,
    pub sounds: Vec<(u32, String)>,
    /// Usable markers: (position, prompt).
    #[serde(default)]
    pub usables: Vec<([f32; 3], String)>,
    /// Talkable mission NPCs: (npc id, prompt).
    #[serde(default)]
    pub talkers: Vec<(String, String)>,
    /// Mission NPC id -> pawn id (set by the host), so clients can find talkers.
    #[serde(default)]
    pub npc_ids: Vec<(String, u32)>,
    /// Open dialog choices: (player id, options). Picked with the CHOICE_n buttons.
    #[serde(default)]
    pub choices: Vec<(u32, Vec<String>)>,
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
}

#[derive(Debug, Clone)]
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
}

impl MissionState {
    /// `markers`: positions resolved by the host (actor names looked up in the level).
    pub fn new(def: MissionDef, markers: BTreeMap<String, (Vec3, f32)>) -> Self {
        let task_state = def.objectives.iter().map(|o| o.tasks.iter().map(|t| t.initial).collect()).collect();
        let task_hidden = def.objectives.iter().map(|o| o.tasks.iter().map(|t| t.hidden).collect()).collect();
        let obj_hidden = def.objectives.iter().map(|o| o.hidden).collect();
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
        let talk = def.npcs.iter().filter_map(|n| n.talk.clone().map(|p| (n.id.clone(), p))).collect();
        let mut s = MissionState {
            def,
            markers,
            marker_map: BTreeMap::new(),
            current_map: String::new(),
            task_state,
            task_hidden,
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
        };
        s.refresh_view(&BTreeMap::new());
        s
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
                MissionEvent::Talked(n) => queue.push_back(Ev::Talk(n.clone())),
                MissionEvent::Arrived(m) => queue.push_back(Ev::Arrived(m.clone())),
            }
        }
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
            (Trigger::Arrived(m), Ev::Arrived(x)) => m.eq_ignore_ascii_case(x),
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
        self.view.high_chaos = self.view.stats.high_chaos(self.def.settings.high_chaos_kill_ratio);
        let on_map = |m: &String| self.current_map.is_empty() || self.marker_map.get(m).is_none_or(|mm| mm.eq_ignore_ascii_case(&self.current_map));
        self.view.usables = self.usable.iter().filter(|(m, _)| on_map(m)).filter_map(|(m, p)| self.markers.get(m).map(|x| (x.0.to_array(), p.clone()))).collect();
        self.view.talkers = self.talk.iter().filter(|(n, _)| self.npc_alive.contains(*n)).map(|(n, p)| (n.clone(), p.clone())).collect();
        // Marker: first visible active task with a target (required tasks first).
        let mut best: Option<(bool, [f32; 3])> = None;
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
                }
            }
        }
        self.view.target = best.map(|b| b.1);
    }
}
