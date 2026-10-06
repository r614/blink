//! Deterministic conversation runner (server-authoritative), following the original's
//! native dialog code (research/dishonored/CONVERSATIONS.md has the addresses).
//!
//! - Walking (`FUN_00ca3c10`): each node's `Evaluate` returns an output index (or waits); the
//!   walker follows that link in the same tick until a line, a choice or a matinee waits. A
//!   null link ends the conversation normally; an index past the links ends it as an error.
//! - Every node with an `m_Label` records the label in the memory of each speaker of the
//!   running conversation (`FUN_00ca3a80`). Starting a conversation records "had this
//!   conversation" per speaker and "fired" globally (`FUN_00ca3700`).
//! - Branch state written before a conversation has started (hooks -> branches ->
//!   `DisConv_ConversationRef`) is pending and only commits when a conversation starts
//!   (`FUN_00c97100`); inside a conversation it is written at once.
//! - Starting (`FUN_00ca3700`): `m_bPlayerMustBeNear` needs the player within
//!   `m_fPlayerNearRadius` of a speaker; each speaker must be available: an NPC already in a
//!   conversation of equal or higher priority refuses (`FUN_00c9d1f0`) unless the request
//!   interrupts (hook flag 0x2) and that conversation is not uninterruptible. A started
//!   conversation takes its speakers away from whatever they were in (ended as
//!   "new conversation").
//! - Ending for any reason but "finished" runs the conversation's `Interrupted` output
//!   (`FUN_00ca4410`): its instant nodes run and its first line starts but nothing waits on it.
//! - "Player Walked Away" (`FUN_00ca5610`): checked every tick for conversations that do not
//!   block the player; once the player is no longer near (same rule as the start), the
//!   `Player Walked Away` output runs, or the conversation ends (which runs `Interrupted`).
//!
//! All state (branch counters, shuffles, time limits, memories, RNG) lives here, so a mission
//! replays identically from the same inputs.

use crate::hooks::{self, HookClock, HookRejection};
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What the runner asks of the game. Every method has a neutral default.
pub trait ConvEnv {
    fn flag(&self, name: &str) -> bool;
    fn set_flag(&mut self, name: &str, value: bool);
    /// The mission NPC (if any) bound to a speaker can talk (alive and present).
    fn speaker_available(&self, _npc: Option<&str>) -> bool {
        true
    }
    /// Suspicion level of the speaker's pawn (`DisConv_CheckSpeakerSuspicionLevel` takes the
    /// output with this index): 0 calm, 1 suspicious, 2 combat / fleeing.
    fn suspicion(&self, _npc: Option<&str>) -> u8 {
        0
    }
    /// `DisConv_IsPossessed`: 0 not possessed, 1 possessed, 2 possessed by the player.
    fn possessed(&self, _npc: Option<&str>) -> u8 {
        0
    }
    /// `DisConv_SpeakerCombatStatus` (`FUN_00caba70`): 0 = dead/incapacitated or the AI's
    /// state 5, 1 = state 3, 2 = state 4, 3 = anything else (no pawn, other states).
    fn combat_status(&self, _npc: Option<&str>) -> u8 {
        3
    }
    /// A player is in combat (`m_bNotWhenPlayerIsInCombat`).
    fn player_in_combat(&self, _player: Option<u32>) -> bool {
        false
    }
    /// Inventory checks (`DisConv_HasInventoryItem*`). The player always carries the sword.
    fn has_item(&self, _player: Option<u32>, item: &str, _equipped: bool) -> bool {
        item.contains("Sword")
    }
    /// `DisConv_HasAbstractItem`: how many of an abstract item (money, runes...) a player has.
    fn item_count(&self, player: Option<u32>, item: &str) -> i32 {
        self.has_item(player, item, false) as i32
    }
    /// Story group / tweaks / faction / dialog-tree membership of an NPC's pawn (the group
    /// branches). Unknown = false, so the branch takes "Everyone Else".
    fn npc_matches(&self, _npc: &str, _kind: GroupKind, _name: &str, _exact: bool) -> bool {
        false
    }
    /// The `DishonoredSpawner` an NPC came from (`DisConv_SpawnerBranch`).
    fn npc_spawner(&self, _npc: &str) -> Option<String> {
        None
    }
    /// Distance between a player and an NPC; None = unknown.
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

/// `DisConvoEndReasonEnum` (the ones the runner produces).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EndReason {
    Error,
    Finished,
    BehaviorChange,
    Death,
    PlayerWalkedAway,
    NewConvo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConvEvent {
    Started { instance: u32, conversation: u32, name: String, label: String },
    /// A line starts. `subtitle` is false when the original shows nothing (no VO event).
    Line { instance: u32, line_id: u32, node: NodeId, speaker: Speaker, text: String, vo: Option<VoRef>, duration: f32, subtitle: bool },
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
    #[serde(default)]
    pub subtitle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveChoice {
    pub id: u32,
    pub node: NodeId,
    pub options: Vec<String>,
    /// Output link per visible option (static choices, then the optional ones shown).
    #[serde(default)]
    pub outputs: Vec<usize>,
    /// Player allowed to choose (the initiator), None = anyone.
    pub player: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum Wait {
    Running,
    Line { until: f64, next: Option<NodeId> },
    Choice,
    /// [interp] A soiree waiting for its matinee (`done` set by the host).
    Matinee { next: Option<NodeId>, done: bool },
    Done,
}

/// Branch state written while no conversation has started yet (committed when one does).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum Pending {
    Seq(BranchKey, u32),
    Rsb(BranchKey, u32, bool),
    RandLast(NodeId, usize),
    Time(BranchKey, f64),
}

type BranchKey = (NodeId, String);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Instance {
    pub id: u32,
    pub oneshot: Option<u32>,
    pub conversation: Option<u32>,
    pub initiator: Option<u32>,
    /// One-shot speaker index -> mission NPC id.
    pub bindings: Vec<Option<String>>,
    pub priority: Priority,
    /// Hook enum that started this instance (its flags decide interruptibility).
    #[serde(default)]
    pub hook: Option<String>,
    /// The start request interrupts busy speakers ([`StartSpec::force`]).
    #[serde(default)]
    pub force: bool,
    pub line: Option<ActiveLine>,
    pub choice: Option<ActiveChoice>,
    /// Pending "matinee finished" continuations (soiree output 0).
    after: Vec<NodeId>,
    wait: Wait,
    /// `m_bDidPlayerWalkAwayCheck`.
    walked: bool,
    pending: Vec<Pending>,
    pub started: f64,
}

/// How to start.
#[derive(Debug, Clone, PartialEq)]
pub struct StartSpec {
    pub target: Target,
    pub initiator: Option<u32>,
    /// One-shot speaker index -> mission NPC id.
    pub bindings: Vec<Option<String>>,
    /// The request interrupts busy speakers (hook flag 0x2): they take it unless their
    /// conversation came from an uninterruptible hook.
    pub force: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RunnerStats {
    pub started: u32,
    pub lines: u32,
    pub choices: u32,
    pub outputs: u32,
}

/// A speaker's dialog memory (`DisConvSaveData`): conversations had and labels heard.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct Memory {
    had: u32,
    labels: BTreeMap<String, u32>,
}

/// A request to fire a dialog hook on an NPC (barks; see [`Runner::fire_hook`]).
#[derive(Debug, Clone, PartialEq)]
pub struct HookRequest {
    /// The NPC the hook fires on (its trees are searched in `trees` order: one-shot trees,
    /// additional generic trees, then its generic tree, as `FUN_00ca5670` does).
    pub npc: String,
    pub trees: Vec<u32>,
    /// `DDH_*` name.
    pub hook: String,
    /// Output of the hook node (Kismet hook name index; 0 otherwise).
    pub output: usize,
    pub initiator: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Runner {
    pub db: Arc<ConversationDb>,
    pub time: f64,
    rng: u64,
    pub instances: Vec<Instance>,
    next_instance: u32,
    next_id: u32,
    /// (speaker key, conversation) -> memory.
    memory: BTreeMap<(String, u32), Memory>,
    /// Conversation -> times started (global memory: fire-once, `ConversationFired`).
    fired: BTreeMap<u32, u32>,
    seq: BTreeMap<BranchKey, u32>,
    rand_last: BTreeMap<NodeId, usize>,
    rsb: BTreeMap<BranchKey, (u32, Vec<usize>)>,
    time_limit: BTreeMap<BranchKey, f64>,
    /// Lines started by an `Interrupted` branch: they play out on their own.
    pub detached: Vec<ActiveLine>,
    pub hooks: HookClock,
    events: Vec<ConvEvent>,
    pub stats: RunnerStats,
    /// [interp] The host plays soiree matinees itself (their dialog keys speak the blurbs at
    /// the keyed times): a soiree waits for [`Runner::matinee_done`] instead of playing the
    /// preview chain.
    pub external_matinees: bool,
}

const MAX_NODES_PER_STEP: u32 = 512;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    /// Running an `Interrupted` branch: instant nodes only, the first line is detached.
    Interrupt,
}

impl Runner {
    pub fn new(db: Arc<ConversationDb>, seed: u64) -> Self {
        Self {
            db,
            time: 0.0,
            rng: seed ^ 0x9E37_79B9_7F4A_7C15 | 1,
            instances: Vec::new(),
            next_instance: 1,
            next_id: 1,
            memory: BTreeMap::new(),
            fired: BTreeMap::new(),
            seq: BTreeMap::new(),
            rand_last: BTreeMap::new(),
            rsb: BTreeMap::new(),
            time_limit: BTreeMap::new(),
            detached: Vec::new(),
            hooks: HookClock::default(),
            events: Vec::new(),
            stats: RunnerStats::default(),
            external_matinees: false,
        }
    }

    fn next_rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn rand(&mut self, n: usize) -> usize {
        let r = self.next_rand();
        if n == 0 { 0 } else { (r % n as u64) as usize }
    }

    /// Like the C runtime's `rand()`: 0..=32767.
    fn crand(&mut self) -> u32 {
        (self.next_rand() >> 17) as u32 & 0x7fff
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

    /// Whether a speaker heard `label` in `conversation` (speaker key: NPC id, `player`, or
    /// `os:<one-shot>:<slot>` for unbound one-shot speakers).
    pub fn heard(&self, speaker: &str, conversation: u32, label: &str) -> bool {
        self.memory.get(&(speaker.to_string(), conversation)).is_some_and(|m| m.labels.get(label).copied().unwrap_or(0) > 0)
    }

    /// Times a conversation started.
    pub fn fired_count(&self, conversation: u32) -> u32 {
        self.fired.get(&conversation).copied().unwrap_or(0)
    }

    /// Start a conversation or enter a hook. Returns the instance id, or None when a
    /// conversation was asked for and could not start (speakers busy with an equal or higher
    /// priority conversation, the player not near enough...). A hook always returns its
    /// instance, even when its graph led to no conversation.
    pub fn start(&mut self, spec: StartSpec, env: &mut dyn ConvEnv) -> Option<u32> {
        let db = self.db.clone();
        let (oneshot, entry, conv, priority, hook) = match spec.target {
            Target::Conversation { oneshot, conversation } => (oneshot, None, Some(conversation), db.conversations[conversation as usize].priority, None),
            Target::Hook { oneshot, hook, output } => {
                let (p, h) = hook_meta(db.node(hook));
                (Some(oneshot), Some(db.node(hook).outputs.get(output).copied().flatten()), None, p, h)
            }
            Target::TreeHook { tree, hook, output } => {
                let (p, h) = hook_meta(db.node(hook));
                (db.trees[tree as usize].oneshot, Some(db.node(hook).outputs.get(output).copied().flatten()), None, p, h)
            }
        };
        let id = self.next_instance;
        self.next_instance += 1;
        self.instances.push(Instance {
            id,
            oneshot,
            conversation: None,
            initiator: spec.initiator,
            bindings: spec.bindings.clone(),
            priority,
            hook,
            force: spec.force,
            line: None,
            choice: None,
            after: Vec::new(),
            wait: Wait::Running,
            walked: false,
            pending: Vec::new(),
            started: self.time,
        });
        let idx = self.instances.len() - 1;
        match (conv, entry) {
            (Some(c), _) => {
                if !self.enter_conversation(idx, c, env) {
                    self.instances.pop();
                    self.next_instance -= 1;
                    return None;
                }
                let start = db.conversations[c as usize].start;
                self.run_flow(idx, start, env, Mode::Normal);
            }
            (None, Some(start)) => self.run_flow(idx, start, env, Mode::Normal),
            _ => self.end(idx, EndReason::Finished, env),
        }
        self.gc();
        Some(id)
    }

    /// Fire a dialog hook on an NPC: the bark entry point for the AI.
    ///
    /// The refire delay of the hook on that NPC, the hook's global refire delay and its fire
    /// group's minimum spacing gate the request (`HookClock`); the group's cap on concurrent
    /// conversations and `m_MaxDialogHooksAtOnce` (unless the group ignores the global cap)
    /// limit it; then the NPC's trees are searched in order for a hook node of that type. A
    /// hook flagged "interrupts" (dying, pain, falling...) takes the NPC even when it is busy.
    /// Success (a conversation started) re-arms the refire delay.
    pub fn fire_hook(&mut self, req: HookRequest, env: &mut dyn ConvEnv) -> Result<u32, HookRejection> {
        let now = self.time;
        self.hooks.allowed(&req.npc, &req.hook, now)?;
        let info = hooks::hook_info(&req.hook);
        let cfg = hooks::fire_group(info.group);
        let in_group = self.active().filter(|i| i.hook.as_deref().is_some_and(|h| hooks::hook_info(h).group == info.group)).count();
        if cfg.max_per_group >= 0 && in_group >= cfg.max_per_group as usize {
            return Err(HookRejection::GroupCap);
        }
        let ai_hooks = self.active().filter(|i| i.hook.as_deref().is_some_and(|h| hooks::hook_info(h).flags & 0x80 != 0)).count();
        if !cfg.ignore_global_cap && info.flags & 0x80 != 0 && ai_hooks >= hooks::MAX_HOOKS_AT_ONCE {
            return Err(HookRejection::GroupCap);
        }
        if cfg.max_per_initiator >= 0 {
            let by_initiator = self.active().filter(|i| i.initiator == req.initiator && i.initiator.is_some() && i.hook.as_deref().is_some_and(|h| hooks::hook_info(h).group == info.group)).count();
            if by_initiator >= cfg.max_per_initiator as usize {
                return Err(HookRejection::GroupCap);
            }
        }
        let db = self.db.clone();
        for &tree in &req.trees {
            let t = &db.trees[tree as usize];
            let Some(&h) = t.hooks.iter().find(|h| matches!(&db.node(**h).kind, NodeKind::Hook { hook, .. } if hook.eq_ignore_ascii_case(&req.hook))) else { continue };
            let target = match t.oneshot {
                Some(o) => Target::Hook { oneshot: o, hook: h, output: req.output },
                None => Target::TreeHook { tree, hook: h, output: req.output },
            };
            let before = self.stats.started;
            let spec = StartSpec { target, initiator: req.initiator, bindings: vec![Some(req.npc.clone())], force: info.flags & hooks::FLAG_INTERRUPTS != 0 };
            if let Some(id) = self.start(spec, env) {
                if self.stats.started > before {
                    let unit = self.crand() as f32 / 32767.0;
                    self.hooks.fired(&req.npc, &req.hook, now, unit);
                    return Ok(id);
                }
            }
        }
        Err(HookRejection::NoContent)
    }

    /// Speaker identity for memories and engagement: the bound NPC, the player, or the
    /// one-shot speaker slot when no NPC stands in for it.
    fn speaker_key(&self, idx: usize, slot: &SpeakerSlot) -> Option<String> {
        let inst = &self.instances[idx];
        let os = || inst.oneshot.map(|o| self.db.oneshots[o as usize].name.clone()).unwrap_or_default();
        match slot {
            SpeakerSlot::Me => Some(inst.bindings.first().cloned().flatten().unwrap_or_else(|| format!("os:{}:0", os()))),
            SpeakerSlot::OneShot(k) if *k >= 0 => Some(inst.bindings.get(*k as usize + 1).cloned().flatten().unwrap_or_else(|| format!("os:{}:{}", os(), k + 1))),
            SpeakerSlot::OneShot(_) => None,
            SpeakerSlot::Player | SpeakerSlot::Initiator => Some("player".into()),
            SpeakerSlot::Other(s) => Some(format!("slot:{s}")),
        }
    }

    /// Keys of the NPC speakers (not the player) of a conversation, as bound in an instance.
    fn npc_keys(&self, idx: usize, c: u32) -> Vec<String> {
        self.db.conversations[c as usize].speakers.iter().filter(|s| !matches!(s, SpeakerSlot::Player | SpeakerSlot::Initiator)).filter_map(|s| self.speaker_key(idx, s)).collect()
    }

    /// `FUN_00c96410`: with `m_bPlayerMustBeNear`, the player must be within the radius of one
    /// of the speakers (distances the game does not know count as near).
    fn player_near(&self, idx: usize, c: u32, env: &dyn ConvEnv) -> bool {
        let conv = &self.db.conversations[c as usize];
        if !conv.player_must_be_near {
            return true;
        }
        let mut known = false;
        for s in &conv.speakers {
            let npc = self.slot_npc(idx, s);
            if npc.is_none() {
                continue;
            }
            if let Some(d) = env.distance(self.instances[idx].initiator, npc.as_deref()) {
                known = true;
                if d < conv.player_near_radius {
                    return true;
                }
            }
        }
        !known
    }

    /// `DisConv_ConversationRef` / direct start (`FUN_00ca3700`). False when the
    /// conversation cannot start.
    fn enter_conversation(&mut self, idx: usize, c: u32, env: &mut dyn ConvEnv) -> bool {
        let db = self.db.clone();
        let conv = &db.conversations[c as usize];
        if !self.player_near(idx, c, env) {
            return false;
        }
        // Speaker availability. The priority rule applies when the request comes from a hook
        // (no conversation running in this instance yet); a busy speaker whose conversation
        // came from an uninterruptible hook always refuses.
        let fresh = self.instances[idx].conversation.is_none();
        let force = self.instances[idx].force;
        let me = self.instances[idx].id;
        for (k, slot) in conv.speakers.iter().enumerate() {
            if matches!(slot, SpeakerSlot::Player | SpeakerSlot::Initiator) {
                continue;
            }
            let gate = conv.speaker_gates.get(k).copied().unwrap_or_default();
            let npc = self.slot_npc(idx, slot);
            let mut ok = true;
            if npc.is_some() {
                if !env.speaker_available(npc.as_deref()) {
                    ok = false;
                }
                let s = env.suspicion(npc.as_deref());
                if (gate.not_in_combat || gate.not_fearful) && s >= 2 || gate.not_suspicious && s >= 1 {
                    ok = false;
                }
            }
            if ok {
                if let Some(key) = self.speaker_key(idx, slot) {
                    for other in self.active().filter(|o| o.id != me && o.conversation.is_some()) {
                        let oc = other.conversation.unwrap();
                        let oi = self.instances.iter().position(|x| x.id == other.id).unwrap();
                        if !self.npc_keys(oi, oc).contains(&key) {
                            continue;
                        }
                        let locked = other.hook.as_deref().is_some_and(|h| hooks::hook_info(h).flags & hooks::FLAG_NOT_INTERRUPTIBLE != 0);
                        if locked || fresh && !force && other.priority >= conv.priority {
                            ok = false;
                        }
                    }
                }
            }
            if !ok && !gate.optional {
                return false;
            }
        }
        // The speakers leave whatever they were in ("new conversation").
        let keys = self.npc_keys(idx, c);
        let others: Vec<u32> = self
            .instances
            .iter()
            .enumerate()
            .filter(|(i, o)| o.id != me && o.wait != Wait::Done && o.conversation.is_some_and(|oc| self.npc_keys(*i, oc).iter().any(|k| keys.contains(k))))
            .map(|(_, o)| o.id)
            .collect();
        for o in others {
            if let Some(i) = self.instances.iter().position(|x| x.id == o) {
                self.end(i, EndReason::NewConvo, env);
            }
        }
        // Commit the branch state written on the way here.
        let pending = std::mem::take(&mut self.instances[idx].pending);
        for p in pending {
            self.commit(p);
        }
        *self.fired.entry(c).or_default() += 1;
        for slot in &conv.speakers {
            if let Some(key) = self.speaker_key(idx, slot) {
                self.memory.entry((key, c)).or_default().had += 1;
            }
        }
        self.stats.started += 1;
        let inst = &mut self.instances[idx];
        inst.conversation = Some(c);
        inst.priority = conv.priority;
        inst.walked = false;
        self.events.push(ConvEvent::Started { instance: inst.id, conversation: c, name: db.conversation_name(c), label: conv.label.clone() });
        true
    }

    fn commit(&mut self, p: Pending) {
        match p {
            Pending::Seq(k, v) => {
                self.seq.insert(k, v);
            }
            Pending::Rsb(k, v, reshuffle) => {
                let n = self.rsb.get(&k).map(|s| s.1.len()).unwrap_or(0);
                if let Some(s) = self.rsb.get_mut(&k) {
                    s.0 = v;
                }
                if reshuffle && v as usize == n {
                    let order = self.shuffle(n);
                    if let Some(s) = self.rsb.get_mut(&k) {
                        s.1 = order;
                    }
                }
            }
            Pending::RandLast(n, v) => {
                self.rand_last.insert(n, v);
            }
            Pending::Time(k, t) => {
                self.time_limit.insert(k, t);
            }
        }
    }

    /// Write branch state: at once inside a conversation, else pending (`FUN_00c8b2b0`).
    fn write(&mut self, idx: usize, p: Pending) {
        if self.instances[idx].conversation.is_some() {
            // Immediate writes skip the looping reshuffle (only the commit path does it).
            let p = match p {
                Pending::Rsb(k, v, _) => Pending::Rsb(k, v, false),
                p => p,
            };
            self.commit(p);
        } else {
            self.instances[idx].pending.push(p);
        }
    }

    fn shuffle(&mut self, n: usize) -> Vec<usize> {
        let mut v: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            let j = self.rand(i + 1);
            v.swap(i, j);
        }
        v
    }

    fn slot_npc(&self, idx: usize, slot: &SpeakerSlot) -> Option<String> {
        let inst = &self.instances[idx];
        match slot {
            SpeakerSlot::Me => inst.bindings.first().cloned().flatten(),
            SpeakerSlot::OneShot(k) if *k >= 0 => inst.bindings.get(*k as usize + 1).cloned().flatten(),
            _ => None,
        }
    }

    fn node_slot(&self, idx: usize, node: &Node) -> SpeakerSlot {
        let inst = &self.instances[idx];
        let conv = inst.conversation.or(node.conversation).map(|c| &self.db.conversations[c as usize]);
        conv.and_then(|c| c.speakers.get(node.speaker.max(0) as usize)).cloned().unwrap_or(SpeakerSlot::Me)
    }

    fn speaker_of(&self, idx: usize, node: &Node) -> Speaker {
        let db = &self.db;
        let inst = &self.instances[idx];
        let conv = inst.conversation.or(node.conversation).map(|c| &db.conversations[c as usize]);
        let si = node.speaker.max(0) as usize;
        let slot = self.node_slot(idx, node);
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

    /// The original actor a node's speaker slot is attached to (`m_pAttachedActor`).
    fn slot_actor(&self, idx: usize, slot: &SpeakerSlot) -> Option<String> {
        let o = self.instances[idx].oneshot?;
        let k = match slot {
            SpeakerSlot::Me => 0,
            SpeakerSlot::OneShot(k) if *k >= 0 => *k as usize + 1,
            _ => return None,
        };
        self.db.oneshots[o as usize].speakers.get(k).and_then(|s| s.actor.clone())
    }

    /// Branch state scope: global, else per one-shot (or tree) and me-speaker.
    fn scope_key(&self, idx: usize, node: &Node, global: bool) -> String {
        if global {
            return String::new();
        }
        let inst = &self.instances[idx];
        let owner = inst.oneshot.map(|o| self.db.oneshots[o as usize].name.clone()).unwrap_or_else(|| self.db.trees[node.tree as usize].path.clone());
        format!("{owner}|{}", inst.bindings.first().cloned().flatten().unwrap_or_default())
    }

    /// `FUN_00c8c820`: "apply globally" only counts in trees no one-shot owns.
    fn tree_global(&self, node: &Node, global: bool) -> bool {
        global && self.db.trees[node.tree as usize].oneshot.is_none()
    }

    fn record_label(&mut self, idx: usize, label: &str) {
        let Some(c) = self.instances[idx].conversation else { return };
        let db = self.db.clone();
        for slot in &db.conversations[c as usize].speakers {
            if let Some(key) = self.speaker_key(idx, slot) {
                *self.memory.entry((key, c)).or_default().labels.entry(label.to_string()).or_default() += 1;
            }
        }
    }

    /// Run from a node until the flow waits (line, choice) or ends.
    fn run_flow(&mut self, idx: usize, start: Option<NodeId>, env: &mut dyn ConvEnv, mode: Mode) {
        let db = self.db.clone();
        let mut cur = start;
        let mut guard = 0;
        if mode == Mode::Normal {
            self.instances[idx].wait = Wait::Running;
        }
        loop {
            guard += 1;
            if guard > MAX_NODES_PER_STEP {
                if mode == Mode::Normal {
                    self.end(idx, EndReason::Error, env);
                }
                return;
            }
            let Some(id) = cur else {
                if mode == Mode::Normal {
                    if let Some(a) = self.instances[idx].after.pop() {
                        cur = Some(a);
                        continue;
                    }
                    self.end(idx, EndReason::Finished, env);
                }
                return;
            };
            let node = db.node(id);
            let step = self.eval(idx, id, env, mode);
            if let Some(l) = &node.label {
                if !matches!(node.kind, NodeKind::ConversationRef { .. }) {
                    self.record_label(idx, l);
                }
            }
            match step {
                Step::Out(k) => {
                    cur = match node.outputs.get(k) {
                        Some(link) => *link,
                        None if node.outputs.is_empty() => None,
                        None => {
                            if mode == Mode::Normal {
                                self.end(idx, EndReason::Error, env);
                            }
                            return;
                        }
                    }
                }
                Step::Jump(n) => cur = n,
                Step::Wait => return,
                Step::End(r) => {
                    if mode == Mode::Normal {
                        self.end(idx, r, env);
                    }
                    return;
                }
            }
        }
    }

    /// One node's `Evaluate`.
    fn eval(&mut self, idx: usize, id: NodeId, env: &mut dyn ConvEnv, mode: Mode) -> Step {
        let db = self.db.clone();
        let node = db.node(id);
        let inst_id = self.instances[idx].id;
        let out0 = node.outputs.first().copied().flatten();
        match &node.kind {
            NodeKind::Line(l) => {
                let speaker = self.speaker_of(idx, node);
                let dur = l.duration();
                let line_id = self.next_id;
                self.next_id += 1;
                self.stats.lines += 1;
                let subtitle = l.has_subtitle();
                self.events.push(ConvEvent::Line { instance: inst_id, line_id, node: id, speaker: speaker.clone(), text: l.text.clone(), vo: l.vo.clone(), duration: dur, subtitle });
                let active = ActiveLine { id: line_id, node: id, speaker, text: l.text.clone(), vo: l.vo.clone(), started: self.time, duration: dur, main: l.main_subtitle, subtitle };
                if mode == Mode::Interrupt {
                    self.detached.push(active);
                    return Step::End(EndReason::Finished);
                }
                let inst = &mut self.instances[idx];
                inst.line = Some(active);
                inst.wait = Wait::Line { until: self.time + dur as f64, next: out0 };
                Step::Wait
            }
            NodeKind::Choice { options, optional } => {
                if mode == Mode::Interrupt {
                    return Step::End(EndReason::Finished);
                }
                if options.is_empty() {
                    return Step::Out(0);
                }
                let mut shown = options.clone();
                let mut outputs: Vec<usize> = (0..options.len()).collect();
                for (j, o) in optional.iter().enumerate() {
                    if env.flag(&o.flag) != o.must_be_false {
                        shown.push(o.text.clone());
                        outputs.push(options.len() + j);
                    }
                }
                let choice_id = self.next_id;
                self.next_id += 1;
                self.stats.choices += 1;
                let player = self.instances[idx].initiator;
                self.events.push(ConvEvent::Choice { instance: inst_id, choice_id, node: id, options: shown.clone(), player });
                let inst = &mut self.instances[idx];
                inst.choice = Some(ActiveChoice { id: choice_id, node: id, options: shown, outputs, player });
                inst.wait = Wait::Choice;
                Step::Wait
            }
            NodeKind::RandomBranch { keep_unique, weights } => {
                let w: Vec<f32> = if weights.is_empty() { vec![100.0 / node.outputs.len().max(1) as f32; node.outputs.len()] } else { weights.clone() };
                let n = w.len();
                if n == 0 {
                    return Step::Out(0);
                }
                let roll = self.crand() as f32 * (100.0 / 32767.0);
                let mut acc = 0.0;
                let mut pick = n;
                for (i, wi) in w.iter().enumerate() {
                    acc += wi;
                    if roll < acc {
                        pick = i;
                        break;
                    }
                }
                pick = pick.min(n - 1);
                if *keep_unique {
                    // The entry starts at 0, so output 0 also counts as "just played" once.
                    let last = self.rand_last.get(&id).copied().unwrap_or(0);
                    if pick == last && n > 1 {
                        let target = w[pick];
                        let mut best = f32::MAX;
                        for (i, wi) in w.iter().enumerate() {
                            if i != last && (target - wi).abs() < best {
                                best = (target - wi).abs();
                                pick = i;
                            }
                        }
                    }
                    self.write(idx, Pending::RandLast(id, pick));
                }
                Step::Out(pick)
            }
            NodeKind::SequentialBranch { looping, global } => {
                let n = node.outputs.len() as u32;
                if n == 0 {
                    return Step::End(EndReason::Finished);
                }
                let key = (id, self.scope_key(idx, node, *global));
                let c = self.seq.get(&key).copied().unwrap_or(0);
                let mut next = c + 1;
                if next > n {
                    next = if *looping { 1 } else { n };
                }
                self.write(idx, Pending::Seq(key, next));
                Step::Out(next as usize - 1)
            }
            NodeKind::RandomSequentialBranch { global, branches, looping } => {
                let b = (*branches as usize).min(node.outputs.len());
                let g = self.tree_global(node, *global);
                let key = (id, self.scope_key(idx, node, g));
                if !self.rsb.contains_key(&key) {
                    let order = self.shuffle(b);
                    self.rsb.insert(key.clone(), (0, order));
                }
                let (c, order) = self.rsb[&key].clone();
                let mut next = c + 1;
                if next as usize > b {
                    next = if *looping { 1 } else { b as u32 + 1 };
                }
                self.write(idx, Pending::Rsb(key, next, *looping));
                let k = next as usize - 1;
                Step::Out(order.get(k).copied().unwrap_or(k))
            }
            NodeKind::TimeLimit { seconds, global } => {
                let g = self.tree_global(node, *global);
                let key = (id, self.scope_key(idx, node, g));
                if self.time_limit.get(&key).is_some_and(|t| self.time - t <= *seconds as f64) {
                    return Step::Out(1);
                }
                let now = self.time;
                self.write(idx, Pending::Time(key, now));
                Step::Out(0)
            }
            NodeKind::CheckFlag { flag } => Step::Out(env.flag(flag) as usize),
            NodeKind::SetFlag { flag, value } => {
                if env.flag(flag) != *value {
                    env.set_flag(flag, *value);
                }
                self.events.push(ConvEvent::Flag { name: flag.clone(), value: *value });
                Step::Out(0)
            }
            NodeKind::SeenLabel { conversation, label, .. } => {
                let Some(c) = conversation else { return Step::Out(0) };
                let slot = self.node_slot(idx, node);
                let heard = self.speaker_key(idx, &slot).is_some_and(|k| self.heard(&k, *c, label));
                Step::Out(heard as usize)
            }
            NodeKind::HadConversation { conversation } => {
                let Some(c) = conversation else { return Step::Out(0) };
                let slot = self.node_slot(idx, node);
                let had = self.speaker_key(idx, &slot).is_some_and(|k| self.memory.get(&(k, *c)).is_some_and(|m| m.had > 0));
                Step::Out(had as usize)
            }
            NodeKind::ConversationFired { conversation } => Step::Out(conversation.is_some_and(|c| self.fired_count(c) > 0) as usize),
            NodeKind::KismetOutput { name } => {
                self.stats.outputs += 1;
                let os = self.instances[idx].oneshot.map(|o| db.oneshots[o as usize].name.clone());
                self.events.push(ConvEvent::Output { instance: inst_id, oneshot: os, name: name.clone() });
                Step::Out(0)
            }
            NodeKind::Soiree { guid, matinee, preview_only } => {
                if mode == Mode::Interrupt {
                    return Step::End(EndReason::Finished);
                }
                // Skipped in a conversation that does not block its speakers.
                let blocking = self.instances[idx].conversation.is_some_and(|c| db.conversations[c as usize].blocking);
                if !blocking {
                    return Step::Out(0);
                }
                self.events.push(ConvEvent::Soiree { instance: inst_id, guid: guid.clone(), matinee: matinee.clone() });
                // [interp] The original plays the matinee and waits for it (output 0 after).
                if self.external_matinees && !preview_only {
                    self.instances[idx].wait = Wait::Matinee { next: out0, done: false };
                    return Step::Wait;
                }
                // The preview chain (output 1) plays, then output 0. With
                // `m_UsePreviewBlurbsOnly` this is the original; otherwise the original plays
                // the matinee, whose dialog keys speak these blurbs (APPROXIMATION).
                match node.outputs.get(1).copied().flatten() {
                    Some(preview) => {
                        if let Some(fin) = out0 {
                            self.instances[idx].after.push(fin);
                        }
                        Step::Jump(Some(preview))
                    }
                    None => Step::Out(0),
                }
            }
            NodeKind::ConversationRef { conversation } => {
                if mode == Mode::Interrupt {
                    return Step::End(EndReason::Finished);
                }
                let Some(c) = *conversation else { return Step::Out(1) };
                let conv = &db.conversations[c as usize];
                if conv.fire_once && self.fired_count(c) > 0 {
                    return Step::Out(1);
                }
                if conv.not_when_player_in_combat && env.player_in_combat(self.instances[idx].initiator) {
                    return Step::Out(1);
                }
                if conv.start.is_none() {
                    return Step::Out(1);
                }
                if self.enter_conversation(idx, c, env) { Step::Jump(conv.start) } else { Step::Out(1) }
            }
            NodeKind::CheckSuspicion => {
                let npc = self.speaker_of(idx, node).npc;
                Step::Out(env.suspicion(npc.as_deref()) as usize)
            }
            NodeKind::IsPossessed => {
                let npc = self.speaker_of(idx, node).npc;
                Step::Out(env.possessed(npc.as_deref()) as usize)
            }
            NodeKind::CombatStatus => {
                let npc = self.speaker_of(idx, node).npc;
                Step::Out(env.combat_status(npc.as_deref()) as usize)
            }
            NodeKind::HasItem { item, equipped, quantity } => {
                let p = self.instances[idx].initiator;
                let yes = match quantity {
                    Some(q) => env.item_count(p, item) >= *q,
                    None => env.has_item(p, item, *equipped),
                };
                Step::Out(yes as usize)
            }
            NodeKind::SpeakerAvailable => {
                let npc = self.speaker_of(idx, node).npc;
                Step::Out(env.speaker_available(npc.as_deref()) as usize)
            }
            NodeKind::GroupBranch { kind, entries, groups } => {
                let n = if entries.is_empty() { node.outputs.len().saturating_sub(1).max(groups.len()) } else { entries.len() };
                let slot = self.node_slot(idx, node);
                let npc = self.slot_npc(idx, &slot);
                let actor = self.slot_actor(idx, &slot);
                for (i, e) in entries.iter().enumerate() {
                    if e.name.is_empty() {
                        continue;
                    }
                    let hit = match kind {
                        GroupKind::Spawner => actor.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&e.name)) || npc.as_deref().and_then(|n| env.npc_spawner(n)).is_some_and(|s| s.eq_ignore_ascii_case(&e.name)),
                        k => npc.as_deref().is_some_and(|n| env.npc_matches(n, *k, &e.name, e.exact)),
                    };
                    if hit {
                        return Step::Out(i);
                    }
                }
                Step::Out(n)
            }
            NodeKind::TryFallbackTree => Step::End(EndReason::Finished),
            NodeKind::Hook { .. } | NodeKind::Speaker { .. } | NodeKind::LookAt { .. } | NodeKind::Other => Step::Out(0),
        }
    }

    /// End an instance (`FUN_00ca4410`): any reason but "finished" runs the conversation's
    /// `Interrupted` output first.
    fn end(&mut self, idx: usize, reason: EndReason, env: &mut dyn ConvEnv) {
        if self.instances[idx].wait == Wait::Done {
            return;
        }
        let interrupted = reason != EndReason::Finished;
        {
            let inst = &mut self.instances[idx];
            inst.wait = Wait::Done;
            inst.line = None;
            inst.choice = None;
            inst.after.clear();
        }
        let conv = self.instances[idx].conversation;
        if interrupted {
            if let Some(link) = conv.and_then(|c| self.db.conversations[c as usize].interrupted) {
                self.run_flow(idx, Some(link), env, Mode::Interrupt);
            }
        }
        let inst = &self.instances[idx];
        let (name, label) = match inst.conversation {
            Some(c) => (self.db.conversation_name(c), self.db.conversations[c as usize].label.clone()),
            None => (inst.oneshot.map(|o| self.db.oneshots[o as usize].name.clone()).unwrap_or_default(), String::new()),
        };
        self.events.push(ConvEvent::Ended { instance: inst.id, conversation: inst.conversation, name, label, interrupted });
    }

    fn gc(&mut self) {
        self.instances.retain(|i| i.wait != Wait::Done);
    }

    /// Advance time: lines that finished move on; "Player Walked Away" checks.
    pub fn update(&mut self, dt: f32, env: &mut dyn ConvEnv) {
        self.time += dt as f64;
        let now = self.time;
        self.detached.retain(|l| now < l.started + l.duration as f64);
        let db = self.db.clone();
        let mut i = 0;
        while i < self.instances.len() {
            if self.instances[i].wait == Wait::Done {
                i += 1;
                continue;
            }
            if let Some(c) = self.instances[i].conversation {
                let conv = &db.conversations[c as usize];
                if !conv.blocking_player && !self.instances[i].walked && !self.player_near(i, c, env) {
                    self.instances[i].walked = true;
                    match conv.walked_away {
                        Some(w) => {
                            let inst = &mut self.instances[i];
                            inst.line = None;
                            inst.choice = None;
                            inst.after.clear();
                            self.run_flow(i, Some(w), env, Mode::Normal);
                        }
                        None => self.end(i, EndReason::PlayerWalkedAway, env),
                    }
                    i += 1;
                    continue;
                }
            }
            if let Wait::Line { until, next } = self.instances[i].wait.clone() {
                if self.time + 1e-6 >= until {
                    self.instances[i].line = None;
                    self.run_flow(i, next, env, Mode::Normal);
                }
            }
            // [interp] The soiree's matinee finished.
            if let Wait::Matinee { next, done: true } = self.instances[i].wait.clone() {
                self.instances[i].wait = Wait::Running;
                self.run_flow(i, next, env, Mode::Normal);
            }
            i += 1;
        }
        self.gc();
    }

    /// [interp] The host finished playing (or could not find) the matinee of a soiree waiting
    /// in `instance`: output 0 follows on the next update.
    pub fn matinee_done(&mut self, instance: u32) {
        for inst in self.instances.iter_mut().filter(|x| x.id == instance) {
            if let Wait::Matinee { done, .. } = &mut inst.wait {
                *done = true;
            }
        }
    }

    /// [interp] A matinee dialog key (`InterpTrackDialog`, `InterpTrackInstDialog` 0x00c73750)
    /// speaks blurb `guid`: the blurb node of that GUID (in the instance's conversation first)
    /// plays as a line on its own, like the original's `DisConv_Blurb` started by the track,
    /// `offset` seconds in. Returns its length.
    pub fn speak_blurb(&mut self, instance: Option<u32>, guid: &str, offset: f32) -> Option<f32> {
        let db = self.db.clone();
        let idx = instance.and_then(|id| self.instances.iter().position(|x| x.id == id));
        let conv = idx.and_then(|i| self.instances[i].conversation);
        let pick = |want_conv: Option<u32>| db.nodes.iter().position(|n| matches!(&n.kind, NodeKind::Line(l) if l.guid.eq_ignore_ascii_case(guid)) && (want_conv.is_none() || n.conversation == want_conv));
        let ni = pick(conv).or_else(|| pick(None))?;
        let node = &db.nodes[ni];
        let NodeKind::Line(l) = &node.kind else { return None };
        let speaker = match idx {
            Some(i) => self.speaker_of(i, node),
            None => Speaker { name: node.conversation.and_then(|c| db.conversations[c as usize].speaker_nicks.get(node.speaker.max(0) as usize).cloned().flatten()).unwrap_or_default().replace('_', " "), npc: None, player: false },
        };
        let dur = l.duration();
        let line_id = self.next_id;
        self.next_id += 1;
        self.stats.lines += 1;
        let subtitle = l.has_subtitle();
        let inst_id = instance.unwrap_or(0);
        self.events.push(ConvEvent::Line { instance: inst_id, line_id, node: ni as NodeId, speaker: speaker.clone(), text: l.text.clone(), vo: l.vo.clone(), duration: dur, subtitle });
        self.detached.push(ActiveLine { id: line_id, node: ni as NodeId, speaker, text: l.text.clone(), vo: l.vo.clone(), started: self.time - offset.max(0.0) as f64, duration: dur, main: l.main_subtitle, subtitle });
        Some(dur)
    }

    /// Player input on a waiting choice. Returns true when accepted.
    pub fn choose(&mut self, player: u32, index: usize, env: &mut dyn ConvEnv) -> bool {
        let Some(i) = self.instances.iter().position(|x| x.wait == Wait::Choice && x.choice.as_ref().is_some_and(|c| c.player.is_none_or(|p| p == player) && index < c.options.len())) else {
            return false;
        };
        let c = self.instances[i].choice.take().unwrap();
        let out = c.outputs.get(index).copied().unwrap_or(index);
        let next = self.db.node(c.node).outputs.get(out).copied().flatten();
        self.events.push(ConvEvent::Chosen { instance: self.instances[i].id, index, text: c.options[index].clone() });
        self.run_flow(i, next, env, Mode::Normal);
        self.gc();
        true
    }

    /// Stop an instance (mission script / behaviour change): its `Interrupted` output runs.
    pub fn stop(&mut self, instance: u32, env: &mut dyn ConvEnv) {
        let Some(i) = self.instances.iter().position(|x| x.id == instance && x.wait != Wait::Done) else { return };
        self.end(i, EndReason::BehaviorChange, env);
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

/// What a node's evaluation asks the walker to do.
enum Step {
    /// Follow output link k.
    Out(usize),
    /// Continue at a node directly (conversation start, soiree preview chain).
    Jump(Option<NodeId>),
    Wait,
    End(EndReason),
}

/// Priority and hook name of a hook node (`m_Priority`, default `LOW_PRIORITY_CONV`).
fn hook_meta(n: &Node) -> (Priority, Option<String>) {
    match &n.kind {
        NodeKind::Hook { priority, hook, .. } => (priority.unwrap_or(Priority::Low), Some(hook.clone())),
        _ => (Priority::Low, None),
    }
}
