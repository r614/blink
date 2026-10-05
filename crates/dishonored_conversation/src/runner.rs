//! Deterministic conversation runner (server-authoritative).
//!
//! Plays the original graphs node by node:
//! - a line holds the conversation for its VO length (or a text-length estimate) plus
//!   `m_fBlurbEndDelay`, then follows output 0;
//! - a player choice waits for the chooser's input and follows the chosen output;
//! - branches, conditions and actions run instantly within a step (like the original, which
//!   evaluates them in one tick);
//! - `DisConv_KismetActivateRemoteEvent` produces an [`ConvEvent::Output`] (the mission's
//!   `ConversationOutput(name)` trigger), `DisConv_SetStoryFlag` writes the shared flag
//!   table, `DisConv_Soiree` is recorded (its matinee is not played): output 1 ("started")
//!   runs immediately and output 0 ("finished") once that branch has ended.
//!
//! Condition outputs are ordered "false, true" (output 0 = not set / not seen / not
//! possessed). Evidence: Miss White's `Hits_On_Player` takes `SeenDialogLabel` output 0 into
//! the line carrying the checked label (first visit) and output 1 into the follow-up line;
//! `Idle_With_Jack` only plays through output 0 of the check on the flag set when Jack
//! leaves. See research/LOG.md.
//!
//! All state (branch counters, shuffle bags, time limits, heard labels, fired conversations,
//! RNG) lives here, so a mission replays identically from the same inputs.

use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What the runner asks of the game.
pub trait ConvEnv {
    fn flag(&self, name: &str) -> bool;
    fn set_flag(&mut self, name: &str, value: bool);
    /// The mission NPC (if any) bound to a speaker can talk (alive and present).
    fn speaker_available(&self, _npc: Option<&str>) -> bool {
        true
    }
    /// 0 calm, 1 suspicious, 2 combat / fleeing.
    fn suspicion(&self, _npc: Option<&str>) -> u8 {
        0
    }
    /// Inventory checks (`DisConv_HasInventoryItem*`). The player always carries the sword.
    fn has_item(&self, _player: Option<u32>, item: &str, _equipped: bool) -> bool {
        item.contains("Sword")
    }
    /// Distance between a player and an NPC (for "Player Walked Away"); None = unknown.
    fn distance(&self, _player: Option<u32>, _npc: Option<&str>) -> Option<f32> {
        None
    }
}

/// Plain flag table, for tools and tests.
#[derive(Debug, Clone, Default)]
pub struct SimpleEnv {
    pub flags: BTreeMap<String, bool>,
}

impl ConvEnv for SimpleEnv {
    fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }
    fn set_flag(&mut self, name: &str, value: bool) {
        self.flags.insert(name.to_string(), value);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Speaker {
    /// Display name (speaker node nick name, else the one-shot's `m_SpeakerNickName`).
    pub name: String,
    /// Mission NPC bound to this speaker.
    pub npc: Option<String>,
    pub player: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConvEvent {
    Started { instance: u32, conversation: u32, name: String, label: String },
    Line { instance: u32, line_id: u32, node: NodeId, speaker: Speaker, text: String, vo: Option<VoRef>, duration: f32 },
    Choice { instance: u32, choice_id: u32, node: NodeId, options: Vec<String>, player: Option<u32> },
    Chosen { instance: u32, index: usize, text: String },
    Output { instance: u32, oneshot: Option<String>, name: String },
    Flag { name: String, value: bool },
    Soiree { instance: u32, guid: String, matinee: Option<String> },
    Ended { instance: u32, conversation: Option<u32>, name: String, label: String, interrupted: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveLine {
    pub id: u32,
    pub node: NodeId,
    pub speaker: Speaker,
    pub text: String,
    pub vo: Option<VoRef>,
    pub started: f64,
    pub duration: f32,
    pub main: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveChoice {
    pub id: u32,
    pub node: NodeId,
    pub options: Vec<String>,
    /// Player allowed to choose (the initiator), None = anyone.
    pub player: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum Wait {
    Running,
    Line { until: f64, next: Option<NodeId> },
    Choice,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Instance {
    pub id: u32,
    pub oneshot: Option<u32>,
    pub conversation: Option<u32>,
    pub initiator: Option<u32>,
    /// One-shot speaker index -> mission NPC id.
    pub bindings: Vec<Option<String>>,
    pub priority: Priority,
    pub line: Option<ActiveLine>,
    pub choice: Option<ActiveChoice>,
    /// Pending "soiree finished" continuations.
    after: Vec<NodeId>,
    wait: Wait,
    walked: bool,
    pub started: f64,
}

/// How to start.
#[derive(Debug, Clone, PartialEq)]
pub struct StartSpec {
    pub target: Target,
    pub initiator: Option<u32>,
    /// One-shot speaker index -> mission NPC id.
    pub bindings: Vec<Option<String>>,
    /// Interrupt whatever the one-shot is playing (Kismet / mission starts do).
    pub force: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RunnerStats {
    pub started: u32,
    pub lines: u32,
    pub choices: u32,
    pub outputs: u32,
}

#[derive(Debug, Clone)]
pub struct Runner {
    pub db: Arc<ConversationDb>,
    pub time: f64,
    rng: u64,
    pub instances: Vec<Instance>,
    next_instance: u32,
    next_id: u32,
    /// (one-shot or tree key, label) of lines heard.
    seen: BTreeSet<(String, String)>,
    fired: BTreeSet<u32>,
    seq: BTreeMap<(NodeId, String), u32>,
    rand_last: BTreeMap<(NodeId, String), usize>,
    bags: BTreeMap<(NodeId, String), Vec<usize>>,
    time_limit: BTreeMap<(NodeId, String), f64>,
    events: Vec<ConvEvent>,
    pub stats: RunnerStats,
}

const MAX_NODES_PER_STEP: u32 = 512;

impl Runner {
    pub fn new(db: Arc<ConversationDb>, seed: u64) -> Self {
        Self {
            db,
            time: 0.0,
            rng: seed ^ 0x9E37_79B9_7F4A_7C15 | 1,
            instances: Vec::new(),
            next_instance: 1,
            next_id: 1,
            seen: BTreeSet::new(),
            fired: BTreeSet::new(),
            seq: BTreeMap::new(),
            rand_last: BTreeMap::new(),
            bags: BTreeMap::new(),
            time_limit: BTreeMap::new(),
            events: Vec::new(),
            stats: RunnerStats::default(),
        }
    }

    fn rand(&mut self, n: usize) -> usize {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        if n == 0 { 0 } else { (self.rng % n as u64) as usize }
    }

    pub fn take_events(&mut self) -> Vec<ConvEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn active(&self) -> impl Iterator<Item = &Instance> {
        self.instances.iter().filter(|i| i.wait != Wait::Done)
    }

    pub fn is_busy(&self, oneshot: u32) -> bool {
        self.active().any(|i| i.oneshot == Some(oneshot))
    }

    pub fn heard(&self, key: &str, label: &str) -> bool {
        self.seen.contains(&(key.to_string(), label.to_string()))
    }

    /// Start a conversation or enter a hook. Returns the instance id, or None when it could
    /// not start (one-shot busy with an equal or higher priority conversation, a
    /// `m_bFireConvOnlyOnce` conversation already fired, a gated speaker unavailable...).
    pub fn start(&mut self, spec: StartSpec, env: &mut dyn ConvEnv) -> Option<u32> {
        let db = self.db.clone();
        let (oneshot, entry, conv, priority) = match spec.target {
            Target::Conversation { oneshot, conversation } => {
                let c = &db.conversations[conversation as usize];
                (oneshot, None, Some(conversation), c.priority)
            }
            Target::Hook { oneshot, hook, output } => {
                let n = db.node(hook);
                let p = match &n.kind {
                    NodeKind::Hook { priority, .. } => priority.unwrap_or(Priority::Low),
                    _ => Priority::Low,
                };
                (Some(oneshot), Some(n.outputs.get(output).copied().flatten()), None, p)
            }
        };
        if let Some(o) = oneshot {
            let busy: Vec<u32> = self.active().filter(|i| i.oneshot == Some(o)).map(|i| i.id).collect();
            if !busy.is_empty() {
                let top = self.active().filter(|i| i.oneshot == Some(o)).map(|i| i.priority).max().unwrap_or(Priority::Low);
                if !spec.force && (priority <= top || top == Priority::NotInterruptable) {
                    return None;
                }
                for b in busy {
                    self.stop(b, env);
                }
            }
        }
        let bindings = spec.bindings.clone();
        let id = self.next_instance;
        self.next_instance += 1;
        let inst = Instance {
            id,
            oneshot,
            conversation: None,
            initiator: spec.initiator,
            bindings,
            priority,
            line: None,
            choice: None,
            after: Vec::new(),
            wait: Wait::Running,
            walked: false,
            started: self.time,
        };
        self.instances.push(inst);
        let idx = self.instances.len() - 1;
        match (conv, entry) {
            (Some(c), _) => {
                if !self.enter_conversation(idx, c, env) {
                    self.instances.pop();
                    self.next_instance -= 1;
                    return None;
                }
            }
            (None, Some(start)) => self.exec(idx, start, env),
            _ => {}
        }
        self.gc();
        Some(id)
    }

    /// `DisConv_ConversationRef` / direct start. False when the conversation cannot fire.
    fn enter_conversation(&mut self, idx: usize, c: u32, env: &mut dyn ConvEnv) -> bool {
        let db = self.db.clone();
        let conv = &db.conversations[c as usize];
        if conv.fire_once && self.fired.contains(&c) {
            return false;
        }
        // Speaker gating (m_bNotWhileInCombat / Fearful / Suspicious on the speaker nodes).
        for (slot, gate) in conv.speakers.iter().zip(&conv.speaker_gates) {
            let npc = self.slot_npc(idx, slot);
            if (slot == &SpeakerSlot::Me || matches!(slot, SpeakerSlot::OneShot(_))) && npc.is_some() {
                if !env.speaker_available(npc.as_deref()) {
                    return false;
                }
                let s = env.suspicion(npc.as_deref());
                if (gate.0 || gate.1) && s >= 2 || gate.2 && s >= 1 {
                    return false;
                }
            }
        }
        self.fired.insert(c);
        self.stats.started += 1;
        let inst = &mut self.instances[idx];
        inst.conversation = Some(c);
        inst.priority = conv.priority;
        self.events.push(ConvEvent::Started { instance: inst.id, conversation: c, name: db.conversation_name(c), label: conv.label.clone() });
        self.exec(idx, conv.start, env);
        true
    }

    fn slot_npc(&self, idx: usize, slot: &SpeakerSlot) -> Option<String> {
        let inst = &self.instances[idx];
        match slot {
            SpeakerSlot::Me => inst.bindings.first().cloned().flatten(),
            SpeakerSlot::OneShot(k) if *k >= 0 => inst.bindings.get(*k as usize + 1).cloned().flatten(),
            _ => None,
        }
    }

    fn speaker_of(&self, idx: usize, node: &Node) -> Speaker {
        let db = &self.db;
        let inst = &self.instances[idx];
        let conv = inst.conversation.or(node.conversation).map(|c| &db.conversations[c as usize]);
        let si = node.speaker.max(0) as usize;
        let slot = conv.and_then(|c| c.speakers.get(si)).cloned().unwrap_or(SpeakerSlot::Me);
        let nick = conv.and_then(|c| c.speaker_nicks.get(si).cloned().flatten());
        let os = inst.oneshot.map(|o| &db.oneshots[o as usize]);
        let os_index = match slot {
            SpeakerSlot::Me => Some(0),
            SpeakerSlot::OneShot(k) if k >= 0 => Some(k as usize + 1),
            _ => None,
        };
        let os_nick = os_index.and_then(|k| os.and_then(|o| o.speakers.get(k))).map(|s| s.nick.clone()).filter(|s| !s.is_empty());
        let name = nick.or(os_nick).unwrap_or_else(|| match slot {
            SpeakerSlot::Player => "Corvo".into(),
            _ => String::new(),
        });
        Speaker { name: name.replace('_', " "), npc: self.slot_npc(idx, &slot), player: slot == SpeakerSlot::Player }
    }

    /// Key for per-speaker branch state (`m_bApplyGlobally` false).
    fn scope_key(&self, idx: usize, global: bool) -> String {
        if global {
            String::new()
        } else {
            let inst = &self.instances[idx];
            inst.bindings.first().cloned().flatten().unwrap_or_else(|| inst.oneshot.map(|o| o.to_string()).unwrap_or_default())
        }
    }

    fn label_key(&self, idx: usize, node: &Node) -> String {
        match self.instances[idx].oneshot {
            Some(o) => self.db.oneshots[o as usize].name.clone(),
            None => self.db.trees[node.tree as usize].path.clone(),
        }
    }

    /// Run from a node until the flow waits (line, choice) or ends.
    fn exec(&mut self, idx: usize, start: Option<NodeId>, env: &mut dyn ConvEnv) {
        self.run_flow(idx, start, env, false);
    }

    fn run_flow(&mut self, idx: usize, start: Option<NodeId>, env: &mut dyn ConvEnv, silent: bool) {
        let db = self.db.clone();
        let mut cur = start;
        let mut guard = 0;
        self.instances[idx].wait = Wait::Running;
        loop {
            guard += 1;
            if guard > MAX_NODES_PER_STEP {
                self.finish(idx, false);
                return;
            }
            let Some(id) = cur else {
                if let Some(a) = self.instances[idx].after.pop() {
                    cur = Some(a);
                    continue;
                }
                self.finish(idx, silent);
                return;
            };
            let node = db.node(id);
            let out = |k: usize| node.outputs.get(k).copied().flatten();
            let inst_id = self.instances[idx].id;
            cur = match &node.kind {
                NodeKind::Line(l) => {
                    if silent {
                        out(0)
                    } else {
                        let speaker = self.speaker_of(idx, node);
                        let dur = l.duration();
                        let line_id = self.next_id;
                        self.next_id += 1;
                        if let Some(label) = &l.label {
                            let key = self.label_key(idx, node);
                            self.seen.insert((key, label.clone()));
                        }
                        self.stats.lines += 1;
                        self.events.push(ConvEvent::Line { instance: inst_id, line_id, node: id, speaker: speaker.clone(), text: l.text.clone(), vo: l.vo.clone(), duration: dur });
                        let inst = &mut self.instances[idx];
                        inst.line = Some(ActiveLine { id: line_id, node: id, speaker, text: l.text.clone(), vo: l.vo.clone(), started: self.time, duration: dur, main: l.main_subtitle });
                        inst.wait = Wait::Line { until: self.time + dur as f64, next: out(0) };
                        return;
                    }
                }
                NodeKind::Choice { options } => {
                    if silent {
                        None
                    } else {
                        let choice_id = self.next_id;
                        self.next_id += 1;
                        self.stats.choices += 1;
                        let player = self.instances[idx].initiator;
                        self.events.push(ConvEvent::Choice { instance: inst_id, choice_id, node: id, options: options.clone(), player });
                        let inst = &mut self.instances[idx];
                        inst.choice = Some(ActiveChoice { id: choice_id, node: id, options: options.clone(), player });
                        inst.wait = Wait::Choice;
                        return;
                    }
                }
                NodeKind::RandomBranch { keep_unique } => {
                    let n = node.outputs.len();
                    let key = (id, String::new());
                    let mut k = self.rand(n);
                    if *keep_unique && n > 1 && self.rand_last.get(&key) == Some(&k) {
                        k = (k + 1 + self.rand(n - 1)) % n;
                    }
                    self.rand_last.insert(key, k);
                    out(k)
                }
                NodeKind::SequentialBranch { looping, global } => {
                    let n = node.outputs.len().max(1);
                    let key = (id, self.scope_key(idx, *global));
                    let c = self.seq.entry(key).or_insert(0);
                    let k = *c as usize;
                    *c += 1;
                    out(if k < n { k } else if *looping { k % n } else { n - 1 })
                }
                NodeKind::RandomSequentialBranch { global, branches } => {
                    let b = (*branches as usize).min(node.outputs.len());
                    let key = (id, self.scope_key(idx, *global));
                    let depleted = node.outputs.len() > b;
                    let bag = self.bags.entry(key.clone()).or_insert_with(|| (0..b).collect());
                    if bag.is_empty() && !depleted {
                        *bag = (0..b).collect();
                    }
                    if bag.is_empty() {
                        out(b)
                    } else {
                        let len = bag.len();
                        let r = self.rand(len);
                        let bag = self.bags.get_mut(&key).unwrap();
                        let k = bag.remove(r);
                        out(k)
                    }
                }
                NodeKind::TimeLimit { seconds, global } => {
                    let key = (id, self.scope_key(idx, *global));
                    let ok = self.time_limit.get(&key).is_none_or(|t| self.time - t >= *seconds as f64);
                    if ok {
                        self.time_limit.insert(key, self.time);
                        out(0)
                    } else {
                        out(1)
                    }
                }
                NodeKind::CheckFlag { flag } => out(env.flag(flag) as usize),
                NodeKind::SetFlag { flag, value } => {
                    if env.flag(flag) != *value {
                        env.set_flag(flag, *value);
                    }
                    self.events.push(ConvEvent::Flag { name: flag.clone(), value: *value });
                    out(0)
                }
                NodeKind::SeenLabel { oneshot, label } => {
                    let key = oneshot.clone().unwrap_or_else(|| self.label_key(idx, node));
                    out(self.seen.contains(&(key, label.clone())) as usize)
                }
                NodeKind::KismetOutput { name } => {
                    self.stats.outputs += 1;
                    let os = self.instances[idx].oneshot.map(|o| db.oneshots[o as usize].name.clone());
                    self.events.push(ConvEvent::Output { instance: inst_id, oneshot: os, name: name.clone() });
                    out(0)
                }
                NodeKind::Soiree { guid, matinee } => {
                    if !silent {
                        self.events.push(ConvEvent::Soiree { instance: inst_id, guid: guid.clone(), matinee: matinee.clone() });
                    }
                    if let Some(fin) = out(0) {
                        self.instances[idx].after.push(fin);
                    }
                    out(1)
                }
                NodeKind::ConversationRef { conversation } => match conversation {
                    Some(c) if self.instances[idx].conversation.is_none() => {
                        if self.enter_conversation(idx, *c, env) {
                            return;
                        }
                        out(1)
                    }
                    _ => out(1),
                },
                NodeKind::CheckSuspicion => {
                    let npc = self.speaker_of(idx, node).npc;
                    out((env.suspicion(npc.as_deref()) >= 1) as usize)
                }
                NodeKind::IsPossessed => out(0),
                NodeKind::HasItem { item, equipped } => {
                    let p = self.instances[idx].initiator;
                    out(env.has_item(p, item, *equipped) as usize)
                }
                NodeKind::SpeakerAvailable => {
                    let npc = self.speaker_of(idx, node).npc;
                    out(env.speaker_available(npc.as_deref()) as usize)
                }
                // Story groups / tweaks / factions of the speaker are not modelled: humans
                // take "Everyone Else", the last output.
                NodeKind::GroupBranch { .. } => out(node.outputs.len().saturating_sub(1)),
                NodeKind::Hook { .. } | NodeKind::TryFallbackTree | NodeKind::Speaker { .. } | NodeKind::LookAt { .. } | NodeKind::Other => out(0),
            };
        }
    }

    fn finish(&mut self, idx: usize, interrupted: bool) {
        let inst = &mut self.instances[idx];
        if inst.wait == Wait::Done {
            return;
        }
        inst.wait = Wait::Done;
        inst.line = None;
        inst.choice = None;
        let (name, label) = match inst.conversation {
            Some(c) => (self.db.conversation_name(c), self.db.conversations[c as usize].label.clone()),
            None => (inst.oneshot.map(|o| self.db.oneshots[o as usize].name.clone()).unwrap_or_default(), String::new()),
        };
        self.events.push(ConvEvent::Ended { instance: inst.id, conversation: inst.conversation, name, label, interrupted });
    }

    fn gc(&mut self) {
        self.instances.retain(|i| i.wait != Wait::Done);
    }

    /// Advance time: lines that finished move on; walk-away checks.
    pub fn update(&mut self, dt: f32, env: &mut dyn ConvEnv) {
        self.time += dt as f64;
        let mut i = 0;
        while i < self.instances.len() {
            // Player walked away (m_bPlayerMustBeNear / m_fPlayerNearRadius).
            if let Some(c) = self.instances[i].conversation {
                let conv = &self.db.conversations[c as usize];
                if conv.player_must_be_near && !self.instances[i].walked && self.instances[i].wait != Wait::Done {
                    let me = self.instances[i].bindings.first().cloned().flatten();
                    let radius = conv.player_near_radius;
                    let walked_away = conv.walked_away;
                    if env.distance(self.instances[i].initiator, me.as_deref()).is_some_and(|d| d > radius) {
                        let inst = &mut self.instances[i];
                        inst.walked = true;
                        inst.line = None;
                        inst.choice = None;
                        inst.after.clear();
                        self.exec(i, walked_away, env);
                    }
                }
            }
            if let Wait::Line { until, next } = self.instances[i].wait.clone() {
                if self.time + 1e-6 >= until {
                    self.instances[i].line = None;
                    self.exec(i, next, env);
                }
            }
            i += 1;
        }
        self.gc();
    }

    /// Player input on a waiting choice. Returns true when accepted.
    pub fn choose(&mut self, player: u32, index: usize, env: &mut dyn ConvEnv) -> bool {
        let Some(i) = self.instances.iter().position(|x| x.wait == Wait::Choice && x.choice.as_ref().is_some_and(|c| c.player.is_none_or(|p| p == player) && index < c.options.len())) else {
            return false;
        };
        let c = self.instances[i].choice.take().unwrap();
        let next = self.db.node(c.node).outputs.get(index).copied().flatten();
        self.events.push(ConvEvent::Chosen { instance: self.instances[i].id, index, text: c.options[index].clone() });
        self.exec(i, next, env);
        self.gc();
        true
    }

    /// Stop an instance: its conversation's `Interrupted` output runs without lines.
    pub fn stop(&mut self, instance: u32, env: &mut dyn ConvEnv) {
        let Some(i) = self.instances.iter().position(|x| x.id == instance && x.wait != Wait::Done) else { return };
        let interrupted = self.instances[i].conversation.and_then(|c| self.db.conversations[c as usize].interrupted);
        let inst = &mut self.instances[i];
        inst.line = None;
        inst.choice = None;
        inst.after.clear();
        self.run_flow(i, interrupted, env, true);
        self.gc();
    }

    /// Stop every instance matching a predicate (conversation reference, one-shot...).
    pub fn stop_where(&mut self, env: &mut dyn ConvEnv, pred: impl Fn(&Instance) -> bool) {
        let ids: Vec<u32> = self.active().filter(|i| pred(i)).map(|i| i.id).collect();
        for id in ids {
            self.stop(id, env);
        }
    }
}
