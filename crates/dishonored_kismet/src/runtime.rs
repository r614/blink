//! Deterministic Kismet interpreter.
//!
//! The execution model follows the engine code of `Dishonored.exe` (decoded with Ghidra;
//! research/dishonored/KISMET.md has the details and addresses):
//!
//! - Every `Sequence` keeps its own stack of active ops, a list of delayed activations and a
//!   queue of event activations that arrived while the event was still active. Each tick a
//!   sequence first runs its own ops (`ExecuteActiveOps`), then its nested sequences, in
//!   order. Levels' root sequences are nested under the always-loaded global sequence, so a
//!   map's Kismet runs after the global Kismet each tick.
//! - `ExecuteActiveOps` pops the top op (at most 1000 per tick), copies linked variable
//!   values into its properties, calls `Activated` the first time (incrementing its
//!   activation count), then `Update`. A finished op calls `DeActivated` (auto outputs) and
//!   publishes its properties back to its variables. A latent op still running is kept for
//!   the next tick and its outputs propagate at once. Outputs with an impulse send an
//!   impulse to each linked input (a second impulse on an input that has one counts a
//!   queued activation) and push the target on top of the stack, in link order; links with
//!   an activation delay go to the delayed list. Inputs are cleared unless queued
//!   activations remain (then the op is queued again), except for latent ops.
//! - Events: `CheckActivate` requires an originator, an enabled parent sequence, a player
//!   instigator for `bPlayerOnly` events, `TriggerCount < MaxTriggerCount` (0 = no limit) and
//!   `ReTriggerDelay` elapsed; disabled events pass the check but do not activate.
//!   `ActivateEvent` counts the trigger, writes the `Instigator` variables, sets its output
//!   impulses (all, or the given indices) and queues itself in its sequence; an event already
//!   active is queued for later.
//! - Remote events reach every enabled `SeqEvent_RemoteEvent` of that name in every loaded
//!   level. `SeqAct_AttachToEvent` duplicates events onto actors as the engine does.

use crate::host::{Host, HostResult, Matinee};
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

/// Engine limit on ops processed per sequence per tick.
pub const MAX_STEPS: u32 = 1000;

/// Runtime state of one op.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OpRt {
    pub active: bool,
    pub activate_count: u32,
    pub in_imp: Vec<bool>,
    pub in_queued: Vec<u32>,
    pub out_imp: Vec<bool>,
    pub in_disabled: Vec<bool>,
    pub out_disabled: Vec<bool>,
    /// Properties changed at runtime (variable values, gate state, switch indices...).
    pub props: Props,
    // Events.
    pub enabled: bool,
    pub trigger_count: u32,
    pub activation_time: f64,
    pub originator: Obj,
    pub instigator: Obj,
    /// The host runs this op as a latent action.
    pub host_latent: bool,
    /// Scratch for latent engine ops (delay remaining / matinee position, start time).
    pub t: f64,
    pub t2: f64,
    pub flags: u32,
}

/// An event duplicated onto an actor by `SeqAct_AttachToEvent` (`<Event>_AttachDup`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Dup {
    pub source: OpId,
    /// Sequence the duplicate was added to (the attaching op's sequence).
    pub parent: OpId,
    pub originator: Obj,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct QueuedEvent {
    event: OpId,
    originator: Obj,
    instigator: Obj,
    indices: Option<Vec<usize>>,
    push_top: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct SeqRt {
    active: Vec<OpId>,
    /// (op, input, remaining seconds)
    delayed: Vec<(OpId, u16, f32)>,
    queued_events: VecDeque<QueuedEvent>,
}

/// What ran and what did not.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Coverage {
    /// Activations per op id.
    pub activations: BTreeMap<OpId, u32>,
    /// Activations per class run by the interpreter or the host.
    pub handled: BTreeMap<String, u32>,
    /// Activations per class nobody supports.
    pub unhandled: BTreeMap<String, u32>,
    /// Ops per tick that hit the engine's step limit.
    pub step_limit_hits: u32,
}

impl Coverage {
    pub fn total_handled(&self) -> u32 {
        self.handled.values().sum()
    }
    pub fn total_unhandled(&self) -> u32 {
        self.unhandled.values().sum()
    }
}

/// The runtime state serializes (saved games); the database and the event-class cache are not
/// saved: the loader re-attaches the mission's database.
#[derive(Serialize, Deserialize)]
pub struct Kismet {
    #[serde(skip)]
    pub db: Arc<KismetDb>,
    pub ops: Vec<OpRt>,
    pub dups: Vec<Dup>,
    seqs: BTreeMap<OpId, SeqRt>,
    /// Packages whose levels are loaded.
    pub loaded: Vec<bool>,
    /// Packages whose levels were loaded before (their state is restored on return).
    pub visited: Vec<bool>,
    pub time: f64,
    rng: CrtRand,
    /// Remote event name (lowercase) -> `SeqEvent_RemoteEvent` ops.
    remote: BTreeMap<String, Vec<OpId>>,
    /// Pawns of the players (`SeqVar_Player`).
    pub players: Vec<u32>,
    /// Spawner actor -> its spawned pawns (`DisSeqVar_SpawnerPawn`).
    pub spawned: BTreeMap<u32, Vec<u32>>,
    pub coverage: Coverage,
    /// Classes already reported as unsupported.
    pub reported: BTreeSet<String>,
    /// Recent interpreter messages (capped).
    pub log: Vec<String>,
    /// Sequence whose ops are executing (new impulses queue into it).
    exec_seq: Option<OpId>,
    level_roots: Vec<OpId>,
    /// Touch events: pawns touching (engine `TouchedList`).
    pub touched: BTreeMap<OpId, Vec<Obj>>,
    /// Requested event class (lowercase) -> static events of that class or a subclass.
    #[serde(skip)]
    event_index: std::sync::Mutex<BTreeMap<String, Vec<OpId>>>,
    /// [interp] Soiree control state of playing matinees (loops, pauses, broken pins, blurbs
    /// the host reports as speaking).
    #[serde(default)]
    pub interp_play: BTreeMap<OpId, crate::interp::PlayState>,
    /// The game's chaos (score, level) as the host last reported it: what
    /// `DisSeqVar_Darkness` / `DisSeqVar_DarknessLevel` read.
    #[serde(default)]
    pub darkness: (i32, i32),
}

/// The view an op has of the interpreter while it runs (host ops get one).
pub struct Ctx<'a> {
    pub k: &'a mut Kismet,
    pub op: OpId,
}

/// What an op's `DeActivated` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeAct {
    /// `SequenceOp::DeActivated`: every output when `bAutoActivateOutputLinks`.
    Auto,
    /// No automatic outputs (the class fires its own, or a host does).
    None,
    /// `SeqAct_Latent::DeActivated`: output 0 ("Finished"), or 1 ("Aborted") when aborted.
    Latent,
    /// Output 0 only when aborted (`DisSeqAct_DialogScriptedChoice`: "Player Busy").
    AbortedFirst,
}

/// `DeActivated` per class, from the vtables of `Dishonored.exe` (slot 94): the engine
/// default (`SequenceOp`, automatic outputs), an empty override, `SeqAct_Latent`'s, or a
/// class's own. Classes not listed keep the engine default. (research/dishonored/KISMET.md)
pub fn deactivation(class: &str) -> DeAct {
    match class {
        // Empty overrides: the class fires its outputs itself.
        "SeqAct_Switch" | "SeqAct_RandomSwitch" | "SeqAct_Delay" | "SeqAct_StreamInTextures" | "DisSeqAct_StartSpawn" | "DisSeqAct_AIDoFollow" | "DisSeqAct_AIDoSimpleBehaviors" | "DisSeqAct_NPCMarkForVanish" => DeAct::None,
        // SeqAct_Latent's (PrepareMapChange and AutoSave call it from their own).
        "SeqAct_Latent" | "DisSeqAct_Latent" | "SeqAct_PrepareMapChange" | "DisSeqAct_AutoSave" | "DisSeqAct_ActivateUsable" | "DisSeqAct_BackupAndClearInventory" | "SeqAct_WaitForLevelsVisible" => DeAct::Latent,
        "DisSeqAct_DialogScriptedChoice" => DeAct::AbortedFirst,
        // Own outputs decided by the op (matinee position; the AI behaviour's result).
        "SeqAct_Interp" | "DisSeqAct_AIGoToActor" | "DisSeqAct_AIShoot" | "SeqAct_ActorFactory" => DeAct::None,
        _ => DeAct::Auto,
    }
}

/// The C runtime `rand()` the engine imports (MSVC: a per-thread linear congruential
/// generator, `holdrand = holdrand * 214013 + 2531011`, returning bits 16..30, 0..=32767).
/// `appRand()` is this call; `appFrand()` is `rand() * (1 / 32767)` (constant at 0x00fb9318).
/// The engine seeds it once at start-up (`FEngineLoop::PreInit` 0x005fe910: `srand` with the
/// low half of `QueryPerformanceCounter`, or `srand(0)` with `-FIXEDSEED`) and every system on
/// the game thread draws from the same stream (258 call sites in 181 functions: AI, particles,
/// accessories, barks, ...), so the original's outcome for a given seed depends on everything
/// that ran before the draw. The port keeps the generator and gives the Kismet its own stream,
/// seeded per match (research/dishonored/KISMET.md "Random numbers", U-082).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrtRand {
    pub holdrand: u32,
}

impl CrtRand {
    /// `srand(seed)`.
    pub fn new(seed: u32) -> Self {
        CrtRand { holdrand: seed }
    }
    /// `rand()`: 0..=32767.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> i32 {
        self.holdrand = self.holdrand.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.holdrand >> 16) & 0x7fff) as i32
    }
    /// `appFrand()`: `rand()` times the float constant 1/32767 (0x38000100).
    pub fn frand(&mut self) -> f32 {
        self.next() as f32 * f32::from_bits(0x3800_0100)
    }
}

impl Kismet {
    /// A fresh interpreter. `seed` drives `SeqAct_RandomSwitch` / `SeqVar_RandomFloat`
    /// (the engine uses the C runtime `rand()`); one seed per match keeps runs reproducible.
    pub fn new(db: Arc<KismetDb>, seed: u64) -> Self {
        let n = db.ops.len();
        let mut ops = Vec::with_capacity(n);
        for o in &db.ops {
            ops.push(OpRt {
                in_imp: vec![false; o.inputs.len()],
                in_queued: vec![0; o.inputs.len()],
                out_imp: vec![false; o.outputs.len()],
                in_disabled: o.inputs.iter().map(|l| l.disabled).collect(),
                out_disabled: o.outputs.iter().map(|l| l.disabled).collect(),
                enabled: o.props.get("bEnabled").and_then(Value::as_bool).unwrap_or(true),
                ..Default::default()
            });
        }
        let mut remote: BTreeMap<String, Vec<OpId>> = BTreeMap::new();
        for o in &db.ops {
            if db.is_a(&o.class, "SeqEvent_RemoteEvent") {
                remote.entry(o.props.str("EventName").to_ascii_lowercase()).or_default().push(o.id);
            }
        }
        let mut seqs = BTreeMap::new();
        for o in &db.ops {
            if o.kind == Kind::Sequence {
                seqs.insert(o.id, SeqRt::default());
            }
        }
        let np = db.packages.len();
        let mut k = Kismet {
            db,
            ops,
            dups: Vec::new(),
            seqs,
            loaded: vec![false; np],
            visited: vec![false; np],
            time: 0.0,
            rng: CrtRand::new(seed as u32),
            remote,
            players: Vec::new(),
            spawned: BTreeMap::new(),
            coverage: Coverage::default(),
            reported: BTreeSet::new(),
            log: Vec::new(),
            exec_seq: None,
            level_roots: Vec::new(),
            event_index: Default::default(),
            touched: BTreeMap::new(),
            interp_play: BTreeMap::new(),
            darkness: (0, 0),
        };
        for g in k.db.global.clone() {
            k.loaded[g as usize] = true;
            k.visited[g as usize] = true;
        }
        k
    }

    // ----- static data -----

    fn n_static(&self) -> usize {
        self.db.ops.len()
    }

    /// Static data of an op (a duplicate shares its source's).
    pub fn op(&self, id: OpId) -> &Op {
        let i = id as usize;
        if i < self.db.ops.len() { &self.db.ops[i] } else { &self.db.ops[self.dups[i - self.db.ops.len()].source as usize] }
    }

    pub fn class(&self, id: OpId) -> &str {
        &self.op(id).class
    }

    pub fn parent(&self, id: OpId) -> Option<OpId> {
        let i = id as usize;
        if i < self.db.ops.len() { self.db.ops[i].parent } else { Some(self.dups[i - self.db.ops.len()].parent) }
    }

    fn package(&self, id: OpId) -> u16 {
        self.op(id).package
    }

    pub fn is_loaded(&self, id: OpId) -> bool {
        self.loaded.get(self.package(id) as usize).copied().unwrap_or(false)
    }

    /// Property: runtime value, else the loaded value.
    pub fn prop(&self, id: OpId, name: &str) -> Option<Value> {
        self.ops[id as usize].props.get(name).cloned().or_else(|| self.op(id).props.get(name).cloned())
    }
    pub fn prop_bool(&self, id: OpId, name: &str) -> bool {
        self.prop(id, name).and_then(|v| v.as_bool()).unwrap_or(false)
    }
    pub fn prop_int(&self, id: OpId, name: &str) -> i32 {
        self.prop(id, name).and_then(|v| v.as_int()).unwrap_or(0)
    }
    pub fn prop_float(&self, id: OpId, name: &str) -> f32 {
        self.prop(id, name).and_then(|v| v.as_float()).unwrap_or(0.0)
    }
    pub fn prop_str(&self, id: OpId, name: &str) -> String {
        self.prop(id, name).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }
    pub fn set_prop(&mut self, id: OpId, name: &str, v: Value) {
        self.ops[id as usize].props.set(name, v);
    }

    fn is_a(&self, id: OpId, class: &str) -> bool {
        self.db.is_a(&self.op(id).class, class)
    }

    fn latent(&self, id: OpId) -> bool {
        self.is_a(id, "SeqAct_Latent") || self.ops[id as usize].host_latent
    }

    pub fn note(&mut self, s: String) {
        self.log.push(format!("[{:.2}] {s}", self.time));
        let n = self.log.len();
        if n > 2000 {
            self.log.drain(..n - 2000);
        }
    }

    pub fn rand(&mut self, n: usize) -> usize {
        // `rand() % n`, as `SeqAct_RandomSwitch` (0x006dc630) and `SeqAct_ActorFactory` do.
        if n == 0 { 0 } else { (self.rng.next() % n as i32) as usize }
    }

    /// `appFrand()` (`SeqVar_RandomFloat` slot 96, 0x006ee230: `Min + (Max - Min) * frand`).
    pub fn rand_unit(&mut self) -> f32 {
        self.rng.frand()
    }

    // ----- levels -----

    /// Load a map's levels. First visit: fresh state, `SeqEvent_LevelLoaded` fires
    /// "Loaded and Visible" and "Beginning of Level"; return visit (state kept, as the
    /// original's `DisSeqAct_SaveLevelState` keeps Kismet with the level): "State Restored".
    pub fn load_map(&mut self, host: &mut dyn Host, map: &str) {
        let Some(pk) = self.db.maps.iter().find(|m| m.0.eq_ignore_ascii_case(map)).map(|m| m.1.clone()) else { return };
        let restored = pk.iter().any(|p| self.visited[*p as usize]);
        for p in &pk {
            self.loaded[*p as usize] = true;
            self.visited[*p as usize] = true;
            for r in self.db.roots[*p as usize].clone() {
                if !self.level_roots.contains(&r) {
                    self.level_roots.push(r);
                }
            }
        }
        self.note(format!("level loaded: {map}{}", if restored { " (state restored)" } else { "" }));
        let events: Vec<OpId> = (0..self.n_static() as OpId).filter(|&e| pk.contains(&self.db.ops[e as usize].package) && self.is_a(e, "SeqEvent_LevelLoaded")).collect();
        // Each output is activated by its own engine call, and only for events whose output
        // has links: "Loaded and Visible" by `Sequence` slot 106 (0x00700120), "Beginning of
        // Level" by `NotifyMatchStarted` (slot 107, 0x007004b0); "State Restored" by the
        // level state restore (0x00a02760, from 0x00a13070), which first resets the event's
        // trigger count so it fires even when `MaxTriggerCount` was reached.
        let linked = |k: &Self, e: OpId, i: usize| k.op(e).outputs.get(i).is_some_and(|o| !o.links.is_empty());
        for e in events {
            if restored {
                if linked(self, e, 3) {
                    self.ops[e as usize].trigger_count = 0;
                    self.check_activate(host, e, Obj::Path("WorldInfo".into()), Obj::None, false, Some(vec![3]), false);
                }
                continue;
            }
            for i in [0, 1] {
                if linked(self, e, i) {
                    self.check_activate(host, e, Obj::Path("WorldInfo".into()), Obj::None, false, Some(vec![i]), false);
                }
            }
        }
    }

    /// Unload a map's levels (their Kismet state is kept for a return visit).
    pub fn unload_map(&mut self, map: &str) {
        let Some(pk) = self.db.maps.iter().find(|m| m.0.eq_ignore_ascii_case(map)).map(|m| m.1.clone()) else { return };
        for p in &pk {
            self.loaded[*p as usize] = false;
        }
        let roots: Vec<OpId> = pk.iter().flat_map(|p| self.db.roots[*p as usize].clone()).collect();
        self.level_roots.retain(|r| !roots.contains(r));
        self.note(format!("level unloaded: {map}"));
    }

    // ----- ticking -----

    /// One fixed step: global sequence, then the loaded levels' sequences.
    pub fn tick(&mut self, host: &mut dyn Host, dt: f32) {
        self.time += dt as f64;
        let globals: Vec<OpId> = self.db.global.iter().flat_map(|p| self.db.roots[*p as usize].clone()).collect();
        for r in &globals {
            self.update_seq(host, *r, dt, true);
        }
        for r in self.level_roots.clone() {
            self.update_seq(host, r, dt, true);
        }
    }

    fn update_seq(&mut self, host: &mut dyn Host, seq: OpId, dt: f32, _root: bool) {
        if !self.is_loaded(seq) || !self.ops[seq as usize].enabled {
            return;
        }
        self.execute_active_ops(host, seq, dt);
        let nested: Vec<OpId> = self.op(seq).children.iter().copied().filter(|c| self.db.ops[*c as usize].kind == Kind::Sequence).collect();
        for n in nested {
            self.update_seq(host, n, dt, false);
        }
    }

    /// Push an op on a sequence's stack (top) or bottom, unless already there.
    fn queue_op(&mut self, seq: OpId, op: OpId, push_top: bool) {
        let s = self.seqs.entry(seq).or_default();
        if !s.active.contains(&op) {
            if push_top {
                s.active.push(op);
            } else {
                s.active.insert(0, op);
            }
        }
    }

    /// Give an input an impulse (a second one counts a queued activation).
    fn impulse(&mut self, op: OpId, input: usize) -> bool {
        let rt = &mut self.ops[op as usize];
        if input >= rt.in_imp.len() {
            rt.in_imp.resize(input + 1, false);
            rt.in_queued.resize(input + 1, 0);
            rt.in_disabled.resize(input + 1, false);
        }
        if rt.in_disabled[input] {
            return false;
        }
        if rt.in_imp[input] {
            rt.in_queued[input] += 1;
        }
        rt.in_imp[input] = true;
        true
    }

    /// Activate an input of an op from outside the graph (host-started ops, tests), as an
    /// output link would.
    pub fn activate_input(&mut self, op: OpId, input: usize) {
        if self.impulse(op, input) {
            let seq = self.parent(op).unwrap_or(op);
            self.queue_op(seq, op, true);
        }
    }

    fn execute_active_ops(&mut self, host: &mut dyn Host, seq: OpId, dt: f32) {
        // Delayed activations.
        let mut due = Vec::new();
        if let Some(s) = self.seqs.get_mut(&seq) {
            for d in s.delayed.iter_mut() {
                d.2 -= dt;
            }
            let (now, later): (Vec<_>, Vec<_>) = s.delayed.drain(..).partition(|d| d.2 <= 0.0);
            s.delayed = later;
            due = now;
        }
        for (op, input, _) in due {
            if self.impulse(op, input as usize) {
                self.queue_op(seq, op, true);
            }
        }
        let prev = self.exec_seq.replace(seq);
        let mut latent_keep: Vec<OpId> = Vec::new();
        let mut steps = 0u32;
        loop {
            let next = self.seqs.get_mut(&seq).and_then(|s| s.active.pop());
            let Some(op) = next else {
                // Stack empty: one queued event activation, then continue.
                let q = self.seqs.get_mut(&seq).and_then(|s| s.queued_events.pop_front());
                match q {
                    Some(q) => {
                        self.activate_event(host, q.event, q.originator, q.instigator, q.indices, q.push_top);
                        if self.seqs.get(&seq).is_some_and(|s| s.active.is_empty()) {
                            continue;
                        }
                        continue;
                    }
                    None => break,
                }
            };
            steps += 1;
            if steps >= MAX_STEPS {
                self.coverage.step_limit_hits += 1;
                self.note(format!("sequence {} hit the {MAX_STEPS}-op limit", self.db.describe(seq)));
                self.queue_op(seq, op, true);
                break;
            }
            self.run_op(host, seq, op, dt, &mut latent_keep);
        }
        if let Some(s) = self.seqs.get_mut(&seq) {
            for op in latent_keep {
                if !s.active.contains(&op) {
                    s.active.push(op);
                }
            }
        }
        self.exec_seq = prev;
    }

    fn run_op(&mut self, host: &mut dyn Host, seq: OpId, op: OpId, dt: f32, latent_keep: &mut Vec<OpId>) {
        self.populate(op);
        if !self.ops[op as usize].active {
            self.ops[op as usize].active = true;
            self.ops[op as usize].activate_count += 1;
            *self.coverage.activations.entry(op).or_insert(0) += 1;
            self.activated(host, op);
        }
        let mut finished = false;
        let mut propagate = false;
        if self.ops[op as usize].active {
            let done = self.update(host, op, dt);
            self.ops[op as usize].active = !done;
            if done {
                self.deactivated(op);
                self.publish(op);
                finished = true;
                propagate = true;
            } else if self.latent(op) {
                if !latent_keep.contains(&op) {
                    latent_keep.push(op);
                }
                propagate = true;
            }
        }
        let _ = finished;
        // Outputs -> linked inputs.
        let mut targets: Vec<(OpId, u16)> = Vec::new();
        if propagate {
            let n_out = self.ops[op as usize].out_imp.len();
            for i in 0..n_out {
                if !self.ops[op as usize].out_imp[i] {
                    continue;
                }
                let link = self.op(op).outputs.get(i).cloned().unwrap_or_default();
                for (t, idx) in link.links {
                    let in_delay = self.op(t).inputs.get(idx as usize).map(|l| l.delay).unwrap_or(0.0);
                    let delay = link.delay + in_delay;
                    if delay > 0.0 {
                        let s = self.seqs.entry(seq).or_default();
                        match s.delayed.iter_mut().find(|d| d.0 == t && d.1 == idx) {
                            Some(d) => d.2 = delay,
                            None => s.delayed.push((t, idx, delay)),
                        }
                    } else {
                        targets.push((t, idx));
                    }
                }
            }
        }
        // Inputs: cleared, unless queued activations remain (non-latent ops run again).
        let latent = self.latent(op);
        let n_in = self.ops[op as usize].in_imp.len();
        let mut again = false;
        for i in 0..n_in {
            let rt = &mut self.ops[op as usize];
            if rt.in_queued[i] < 1 || latent {
                rt.in_imp[i] = false;
                rt.in_queued[i] = 0;
            } else {
                rt.in_queued[i] -= 1;
                again = true;
            }
        }
        if again {
            self.queue_op(seq, op, true);
        }
        for o in self.ops[op as usize].out_imp.iter_mut() {
            *o = false;
        }
        for (t, idx) in targets.into_iter().rev() {
            if self.impulse(t, idx as usize) {
                self.queue_op(seq, t, true);
            }
        }
    }

    // ----- variables -----

    /// Linked variables of a link (named variables resolved).
    pub fn link_vars(&self, op: OpId, link: &str) -> Vec<OpId> {
        let Some(l) = self.op(op).var_link(link) else { return Vec::new() };
        l.vars.iter().map(|v| self.resolve_var(*v)).collect()
    }

    fn resolve_var(&self, v: OpId) -> OpId {
        let o = self.op(v);
        if o.class == "SeqVar_Named" { o.named.unwrap_or(v) } else { v }
    }

    /// Value of a variable.
    pub fn var_value(&mut self, v: OpId) -> Value {
        let v = self.resolve_var(v);
        let class = self.op(v).class.clone();
        let db = self.db.clone();
        if db.is_a(&class, "SeqVar_Bool") {
            Value::Bool(self.prop_int(v, "bValue") != 0)
        } else if db.is_a(&class, "SeqVar_RandomFloat") {
            let (lo, hi) = (self.prop_float(v, "Min"), self.prop_float(v, "Max"));
            let u = self.rand_unit();
            Value::Float(lo + (hi - lo) * u)
        } else if db.is_a(&class, "SeqVar_Float") {
            Value::Float(self.prop_float(v, "FloatValue"))
        } else if class == "DisSeqVar_DarknessLevel" {
            // 0x00B8BAA0: the darkness manager's level (0x00C487D0), not `IntValue`.
            Value::Int(self.darkness.1)
        } else if class == "DisSeqVar_Darkness" {
            // 0x00B8B990: the darkness manager's score (`m_DarknessScore`).
            Value::Int(self.darkness.0)
        } else if db.is_a(&class, "SeqVar_Int") {
            Value::Int(self.prop_int(v, "IntValue"))
        } else if db.is_a(&class, "SeqVar_String") {
            Value::Str(self.prop_str(v, "StrValue"))
        } else if db.is_a(&class, "SeqVar_Object") || db.is_a(&class, "SequenceVariable") {
            Value::Obj(self.objects_of(v).into_iter().next().unwrap_or_default())
        } else {
            Value::Obj(Obj::Op(v))
        }
    }

    /// Objects a variable holds (a player variable or a spawner's pawns can hold several).
    pub fn objects_of(&self, v: OpId) -> Vec<Obj> {
        let v = self.resolve_var(v);
        let class = self.op(v).class.as_str();
        let db = &self.db;
        if db.is_a(class, "SeqVar_Player") {
            if self.prop_bool(v, "bAllPlayers") || self.players.len() <= 1 {
                return self.players.iter().map(|p| Obj::Pawn(*p)).collect();
            }
            let i = self.prop_int(v, "PlayerIdx").max(0) as usize;
            return self.players.get(i).map(|p| vec![Obj::Pawn(*p)]).unwrap_or_default();
        }
        if class == "DisSeqVar_PlayerPawn" {
            return self.players.first().map(|p| vec![Obj::Pawn(*p)]).unwrap_or_default();
        }
        let val = self.prop(v, "ObjValue").and_then(|x| x.as_obj().cloned()).unwrap_or_default();
        if class == "DisSeqVar_SpawnerPawn" {
            // The spawner's spawned pawns (none until it spawns).
            if let Obj::Actor(a) = val {
                return self.spawned.get(&a).map(|ps| ps.iter().map(|p| Obj::Pawn(*p)).collect()).unwrap_or_default();
            }
            return if matches!(val, Obj::Pawn(_)) { vec![val] } else { Vec::new() };
        }
        if class == "InterpData" {
            return vec![Obj::Op(v)];
        }
        if val.is_none() { Vec::new() } else { vec![val] }
    }

    pub fn bools(&mut self, op: OpId, link: &str) -> Vec<bool> {
        self.link_vars(op, link).into_iter().map(|v| self.var_value(v).as_bool().unwrap_or(false)).collect()
    }
    pub fn ints(&mut self, op: OpId, link: &str) -> Vec<i32> {
        self.link_vars(op, link).into_iter().map(|v| self.var_value(v).as_int().unwrap_or(0)).collect()
    }
    pub fn floats(&mut self, op: OpId, link: &str) -> Vec<f32> {
        self.link_vars(op, link).into_iter().map(|v| self.var_value(v).as_float().unwrap_or(0.0)).collect()
    }
    pub fn strings(&mut self, op: OpId, link: &str) -> Vec<String> {
        self.link_vars(op, link).into_iter().map(|v| self.var_value(v).as_str().unwrap_or("").to_string()).collect()
    }
    pub fn objects(&self, op: OpId, link: &str) -> Vec<Obj> {
        self.link_vars(op, link).into_iter().flat_map(|v| self.objects_of(v)).collect()
    }

    /// Write a value into a variable (typed by the variable's class).
    pub fn write_var(&mut self, v: OpId, val: &Value) {
        let v = self.resolve_var(v);
        let class = self.op(v).class.clone();
        let db = self.db.clone();
        if db.is_a(&class, "SeqVar_Bool") {
            let b = val.as_bool().unwrap_or(false);
            self.set_prop(v, "bValue", Value::Int(b as i32));
        } else if db.is_a(&class, "SeqVar_Float") {
            self.set_prop(v, "FloatValue", Value::Float(val.as_float().unwrap_or(0.0)));
        } else if db.is_a(&class, "SeqVar_Int") {
            let i = match val {
                Value::Array(a) => a.first().and_then(Value::as_int).unwrap_or(0),
                _ => val.as_int().unwrap_or(0),
            };
            self.set_prop(v, "IntValue", Value::Int(i));
        } else if db.is_a(&class, "SeqVar_String") {
            self.set_prop(v, "StrValue", Value::Str(val.as_str().unwrap_or("").to_string()));
        } else if db.is_a(&class, "SeqVar_Object") {
            let o = match val {
                Value::Obj(o) => o.clone(),
                Value::Array(a) => a.first().and_then(|x| x.as_obj().cloned()).unwrap_or_default(),
                _ => Obj::None,
            };
            if !db.is_a(&class, "SeqVar_Player") {
                self.set_prop(v, "ObjValue", Value::Obj(o));
            }
        }
    }

    pub fn set_link(&mut self, op: OpId, link: &str, val: Value) {
        for v in self.link_vars(op, link) {
            self.write_var(v, &val);
        }
    }

    /// Variable links with a `PropertyName`: copy the first linked variable (all of them for
    /// array properties) into the op's property (engine `PopulateLinkedVariableValues`).
    fn populate(&mut self, op: OpId) {
        let links = self.op(op).vars.clone();
        for l in links {
            if l.property.is_empty() || l.property == "None" || l.vars.is_empty() {
                continue;
            }
            // Array properties take every linked variable (empty arrays are not cooked, so
            // the known array properties are listed).
            let is_array = matches!(self.op(op).props.get(&l.property), Some(Value::Array(_)))
                || matches!(l.property.as_str(), "Indices" | "Targets")
                || (l.property == "Value" && matches!(self.class(op), "SeqAct_SetInt" | "SeqAct_SetFloat"));
            if is_array {
                let mut vals = Vec::new();
                for v in &l.vars {
                    if self.is_a(self.resolve_var(*v), "SeqVar_Object") {
                        vals.extend(self.objects_of(*v).into_iter().map(Value::Obj));
                    } else {
                        vals.push(self.var_value(*v));
                    }
                }
                self.set_prop(op, &l.property, Value::Array(vals));
            } else {
                let v = self.var_value(l.vars[0]);
                self.set_prop(op, &l.property, v);
            }
        }
    }

    /// Copy properties back to their linked variables (engine `PublishLinkedVariableValues`;
    /// what each variable gets: [`publish_values`]).
    fn publish(&mut self, op: OpId) {
        let links = self.op(op).vars.clone();
        for l in links {
            if l.property.is_empty() || l.property == "None" || l.vars.is_empty() {
                continue;
            }
            let Some(v) = self.prop(op, &l.property) else { continue };
            // An array property goes back element by element: entry i into the i-th linked
            // variable (UE3 `USeqVar_Object::PublishValue` copies `min(array, vars)` entries in
            // order). Writing the whole array into every variable gave each one entry 0: an op
            // with `Targets` linked to "Lord Shaw", "Shaw_Elite" and "Shaw_Elite_Pistols" made
            // all three Lord Shaw (U-221). `populate` skips empty object variables, so when the
            // counts differ the entries cannot be matched to their variables and nothing is
            // written back.
            for (var, val) in l.vars.iter().zip(publish_values(&v, l.vars.len())) {
                if let Some(val) = val {
                    self.write_var(*var, &val);
                }
            }
        }
    }

    // ----- outputs -----

    /// Set an output impulse unless the link is disabled.
    pub fn fire(&mut self, op: OpId, out: usize) -> bool {
        let rt = &mut self.ops[op as usize];
        if out >= rt.out_imp.len() {
            return false;
        }
        if rt.out_disabled[out] {
            return false;
        }
        rt.out_imp[out] = true;
        true
    }

    pub fn fire_name(&mut self, op: OpId, name: &str) -> bool {
        match self.op(op).output(name) {
            Some(i) => self.fire(op, i),
            None => false,
        }
    }

    /// First input with an impulse.
    pub fn input(&self, op: OpId) -> Option<usize> {
        self.ops[op as usize].in_imp.iter().position(|b| *b)
    }

    pub fn has_impulse(&self, op: OpId, input: usize) -> bool {
        self.ops[op as usize].in_imp.get(input).copied().unwrap_or(false)
    }

    // ----- events -----

    /// The engine's `SequenceEvent::CheckActivate`.
    #[allow(clippy::too_many_arguments)]
    pub fn check_activate(&mut self, host: &mut dyn Host, ev: OpId, originator: Obj, instigator: Obj, test: bool, indices: Option<Vec<usize>>, push_top: bool) -> bool {
        if !self.is_loaded(ev) || originator.is_none() {
            return false;
        }
        if let Some(p) = self.parent(ev) {
            if !self.ops[p as usize].enabled {
                return false;
            }
        }
        if self.prop_bool(ev, "bPlayerOnly") && !host.is_player(&instigator) {
            return false;
        }
        let max = self.prop_int(ev, "MaxTriggerCount");
        let rt = &self.ops[ev as usize];
        if max != 0 && rt.trigger_count >= max as u32 {
            return false;
        }
        let delay = self.prop_float(ev, "ReTriggerDelay") as f64;
        if delay != 0.0 && rt.trigger_count != 0 && !(delay < self.time - rt.activation_time) {
            return false;
        }
        if !test && rt.enabled {
            self.activate_event(host, ev, originator, instigator, indices, push_top);
        }
        true
    }

    /// The engine's `SequenceEvent::ActivateEvent`.
    fn activate_event(&mut self, host: &mut dyn Host, ev: OpId, originator: Obj, instigator: Obj, indices: Option<Vec<usize>>, push_top: bool) {
        let parent = self.parent(ev);
        {
            let now = self.time;
            let rt = &mut self.ops[ev as usize];
            rt.originator = originator.clone();
            rt.instigator = instigator.clone();
            rt.activation_time = now;
            rt.trigger_count += 1;
        }
        if !self.ops[ev as usize].active || parent.is_none() {
            self.ops[ev as usize].active = true;
            self.ops[ev as usize].activate_count += 1;
            *self.coverage.activations.entry(ev).or_insert(0) += 1;
            *self.coverage.handled.entry(self.class(ev).to_string()).or_insert(0) += 1;
            // InitializeLinkedVariableValues: the Instigator variables.
            if !instigator.is_none() {
                self.set_link(ev, "Instigator", Value::Obj(instigator));
            }
            self.publish(ev);
            let n = self.ops[ev as usize].out_imp.len();
            match indices {
                None => (0..n).for_each(|i| {
                    self.fire(ev, i);
                }),
                Some(ix) => ix.into_iter().for_each(|i| {
                    self.fire(ev, i);
                }),
            }
            let seq = parent.unwrap_or(ev);
            self.queue_op(seq, ev, push_top);
            let _ = host;
        } else if let Some(p) = parent {
            self.seqs.entry(p).or_default().queued_events.push_back(QueuedEvent { event: ev, originator, instigator, indices, push_top });
        }
    }

    /// Events of a class (or subclass) whose originator is one of `originators` (static
    /// `Originator`, or the actor a duplicate is attached to), in loaded levels.
    pub fn events_for(&self, class: &str, originators: &[Obj]) -> Vec<OpId> {
        let mut out = Vec::new();
        let key = class.to_ascii_lowercase();
        let statics = {
            let mut cache = self.event_index.lock().unwrap();
            cache
                .entry(key)
                .or_insert_with(|| self.db.ops.iter().filter(|o| o.kind == Kind::Event && self.db.is_a(&o.class, class)).map(|o| o.id).collect())
                .clone()
        };
        let n = self.n_static();
        let dyns = self.dups.iter().enumerate().filter(|(_, d)| self.db.is_a(&self.db.ops[d.source as usize].class, class)).map(|(i, _)| (n + i) as OpId);
        for e in statics.into_iter().chain(dyns) {
            if !self.is_loaded(e) {
                continue;
            }
            let o = if (e as usize) < self.n_static() { self.db.ops[e as usize].props.obj("Originator") } else { self.dups[e as usize - self.n_static()].originator.clone() };
            if originators.is_empty() || originators.contains(&o) {
                out.push(e);
            }
        }
        out
    }

    /// Fire matching events (host gameplay: touch, use, death, spawned...). `indices`:
    /// outputs to activate (None = all). Returns the events that activated.
    pub fn trigger(&mut self, host: &mut dyn Host, class: &str, originators: &[Obj], instigator: Obj, indices: Option<Vec<usize>>) -> Vec<OpId> {
        let mut fired = Vec::new();
        for e in self.events_for(class, originators) {
            let before = self.ops[e as usize].trigger_count;
            let orig = if originators.is_empty() { Obj::Path("WorldInfo".into()) } else { self.event_originator(e).unwrap_or_else(|| originators[0].clone()) };
            self.check_activate(host, e, orig, instigator.clone(), false, indices.clone(), false);
            if self.ops[e as usize].trigger_count > before {
                fired.push(e);
            }
        }
        fired
    }

    /// `SeqEvent_Touch` (decoded): a pawn starting to touch passes `CheckActivate` as a test,
    /// then the event activates its "Touched" output and remembers the pawn; a remembered
    /// pawn leaving activates "UnTouched" (with `bPlayerOnly` and the retrigger delay
    /// ignored), plus "Empty" when nobody is left. `MaxTriggerCount` still applies.
    pub fn touch(&mut self, host: &mut dyn Host, ev: OpId, instigator: Obj, entering: bool) -> bool {
        if !self.ops[ev as usize].enabled || !self.is_loaded(ev) {
            return false;
        }
        let orig = self.event_originator(ev).unwrap_or(Obj::Path("WorldInfo".into()));
        let list = self.touched.entry(ev).or_default().clone();
        if entering {
            if list.contains(&instigator) {
                return false;
            }
            if !self.check_activate(host, ev, orig.clone(), instigator.clone(), true, None, false) {
                return false;
            }
            self.activate_event(host, ev, orig, instigator.clone(), Some(vec![0]), false);
            self.touched.entry(ev).or_default().push(instigator);
            true
        } else {
            if !list.contains(&instigator) {
                return false;
            }
            let po = self.prop(ev, "bPlayerOnly");
            let at = self.ops[ev as usize].activation_time;
            self.set_prop(ev, "bPlayerOnly", Value::Bool(false));
            self.ops[ev as usize].activation_time = 0.0;
            let delay = self.prop_float(ev, "ReTriggerDelay");
            self.set_prop(ev, "ReTriggerDelay", Value::Float(0.0));
            let ok = self.check_activate(host, ev, orig.clone(), instigator.clone(), true, None, false);
            self.set_prop(ev, "ReTriggerDelay", Value::Float(delay));
            self.ops[ev as usize].activation_time = at;
            match po {
                Some(v) => self.set_prop(ev, "bPlayerOnly", v),
                None => {
                    self.ops[ev as usize].props.0.retain(|(k, _)| k != "bPlayerOnly");
                }
            }
            if !ok {
                return false;
            }
            let l = self.touched.entry(ev).or_default();
            l.retain(|x| *x != instigator);
            let idx = if l.is_empty() { vec![1, 2] } else { vec![1] };
            self.activate_event(host, ev, orig, instigator, Some(idx), false);
            true
        }
    }

    /// Pawns an event considers touching it.
    pub fn touching(&self, ev: OpId) -> &[Obj] {
        self.touched.get(&ev).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn event_originator(&self, e: OpId) -> Option<Obj> {
        let o = if (e as usize) < self.n_static() { self.db.ops[e as usize].props.obj("Originator") } else { self.dups[e as usize - self.n_static()].originator.clone() };
        if o.is_none() { None } else { Some(o) }
    }

    /// `SeqEvent_Console` by its console name.
    pub fn console(&mut self, host: &mut dyn Host, name: &str) -> usize {
        let evs: Vec<OpId> = (0..self.n_static() as OpId).filter(|&e| self.is_a(e, "SeqEvent_Console") && self.db.ops[e as usize].props.str("ConsoleEventName").eq_ignore_ascii_case(name)).collect();
        let mut n = 0;
        for e in evs {
            if self.check_activate(host, e, Obj::Path("WorldInfo".into()), self.players.first().map(|p| Obj::Pawn(*p)).unwrap_or_default(), false, None, false) {
                n += 1;
            }
        }
        n
    }

    /// `SeqAct_ActivateRemoteEvent` equivalent from outside the graph.
    pub fn remote_event(&mut self, host: &mut dyn Host, name: &str, instigator: Obj) -> usize {
        let list = self.remote.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default();
        // Static events in search order (global first, then loaded levels in load order).
        let mut order: Vec<OpId> = list.into_iter().filter(|e| self.is_loaded(*e)).collect();
        order.sort_by_key(|e| (self.package_rank(self.package(*e)), *e));
        let mut n = 0;
        for e in order {
            if !self.ops[e as usize].enabled {
                continue;
            }
            self.populate(e);
            if self.check_activate(host, e, Obj::Path("WorldInfo".into()), instigator.clone(), false, None, false) {
                n += 1;
            }
        }
        n
    }

    fn package_rank(&self, p: u16) -> usize {
        if let Some(i) = self.db.global.iter().position(|g| *g == p) {
            return i;
        }
        let roots_order: Vec<u16> = self.level_roots.iter().map(|r| self.package(*r)).collect();
        roots_order.iter().position(|x| *x == p).map(|i| 100 + i).unwrap_or(10_000 + p as usize)
    }

    // ----- op behaviour -----

    fn activated(&mut self, host: &mut dyn Host, op: OpId) {
        let class = self.class(op).to_string();
        let kind = self.op(op).kind;
        if kind == Kind::Event {
            // Events do their work in ActivateEvent.
            return;
        }
        let engine = self.engine_activated(host, op, &class);
        match engine {
            Some(()) => {
                *self.coverage.handled.entry(class).or_insert(0) += 1;
            }
            None => {
                let r = {
                    let mut cx = Ctx { k: self, op };
                    host.activated(&mut cx)
                };
                match r {
                    HostResult::Done => {
                        *self.coverage.handled.entry(class).or_insert(0) += 1;
                    }
                    HostResult::Latent => {
                        *self.coverage.handled.entry(class).or_insert(0) += 1;
                        self.ops[op as usize].host_latent = true;
                    }
                    HostResult::Unhandled => {
                        *self.coverage.unhandled.entry(class.clone()).or_insert(0) += 1;
                        if self.reported.insert(class.clone()) {
                            self.note(format!("unsupported Kismet class {class} (first at {})", self.db.describe(op)));
                        }
                    }
                }
            }
        }
    }

    /// Engine classes run by the interpreter itself. None: not an engine op here.
    fn engine_activated(&mut self, host: &mut dyn Host, op: OpId, class: &str) -> Option<()> {
        match class {
            _ if self.op(op).kind == Kind::Sequence => {
                // Start the SequenceActivated event of the first input with an impulse.
                if let Some(i) = self.input(op) {
                    let link = self.op(op).inputs[i].clone();
                    let target = link.linked_op.or_else(|| {
                        self.op(op).children.iter().copied().find(|c| self.is_a(*c, "SeqEvent_SequenceActivated") && self.db.ops[*c as usize].props.str("InputLabel").eq_ignore_ascii_case(&link.name))
                    });
                    if let Some(e) = target {
                        self.check_activate(host, e, Obj::Op(op), Obj::None, false, None, false);
                    }
                }
                self.ops[op as usize].active = false;
            }
            "SeqAct_Gate" => self.gate(op),
            "SeqAct_Toggle" => self.toggle(host, op),
            "SeqAct_Switch" => self.switch(op),
            "SeqAct_RandomSwitch" => self.random_switch(op),
            "SeqAct_Delay" => {
                let d = self.prop_float(op, "Duration") as f64;
                let now = self.time;
                let rt = &mut self.ops[op as usize];
                rt.t = d;
                rt.t2 = now;
            }
            "SeqAct_Timer" => {
                self.set_prop(op, "Time", Value::Float(0.0));
            }
            "SeqAct_FinishSequence" => self.finish_sequence(op),
            "SeqAct_ActivateRemoteEvent" => {
                let name = self.prop_str(op, "EventName");
                let inst = self.prop(op, "Instigator").and_then(|v| v.as_obj().cloned()).filter(|o| !o.is_none()).unwrap_or(Obj::Path("WorldInfo".into()));
                let n = self.remote_event(host, &name, inst);
                if n == 0 {
                    self.note(format!("remote event '{name}' has no enabled listener in the loaded levels ({})", self.db.describe(op)));
                }
            }
            "SeqAct_AttachToEvent" => self.attach_to_event(op),
            "SeqAct_SetBool" => {
                let vals = self.bools(op, "Value");
                let v = if vals.is_empty() { self.prop_bool(op, "DefaultValue") } else { vals.iter().all(|b| *b) };
                self.set_link(op, "Target", Value::Bool(v));
            }
            "SeqAct_SetInt" => {
                let sum: i32 = self.prop(op, "Value").map(|v| v.as_array().iter().filter_map(Value::as_int).sum()).unwrap_or(0);
                self.set_prop(op, "Target", Value::Int(sum));
            }
            "SeqAct_SetFloat" => {
                let sum: f32 = self.prop(op, "Value").map(|v| v.as_array().iter().filter_map(Value::as_float).sum()).unwrap_or(0.0);
                self.set_prop(op, "Target", Value::Float(sum));
            }
            "SeqAct_SetObject" => {
                let mut v = self.prop(op, "Value").and_then(|v| v.as_obj().cloned()).unwrap_or_default();
                if v.is_none() {
                    v = self.prop(op, "DefaultValue").and_then(|v| v.as_obj().cloned()).unwrap_or_default();
                    self.set_prop(op, "Value", Value::Obj(v.clone()));
                }
                let n = self.prop(op, "Targets").map(|t| t.as_array().len()).unwrap_or(0);
                self.set_prop(op, "Targets", Value::Array(vec![Value::Obj(v.clone()); n.max(1)]));
                self.set_link(op, "Target", Value::Obj(v));
            }
            "SeqAct_SetString" => {
                let v = self.prop_str(op, "Value");
                self.set_prop(op, "Target", Value::Str(v));
            }
            "SeqAct_AddInt" | "SeqAct_SubtractInt" => {
                let (a, b) = (self.prop_int(op, "ValueA"), self.prop_int(op, "ValueB"));
                let r = if class == "SeqAct_AddInt" { a + b } else { a - b };
                self.set_prop(op, "IntResult", Value::Int(r));
                self.set_prop(op, "FloatResult", Value::Float(r as f32));
                self.fire(op, 0);
            }
            "SeqAct_AddFloat" | "SeqAct_SubtractFloat" | "SeqAct_MultiplyFloat" | "SeqAct_DivideFloat" => {
                let (a, b) = (self.prop_float(op, "ValueA"), self.prop_float(op, "ValueB"));
                let r = match class {
                    "SeqAct_AddFloat" => a + b,
                    "SeqAct_SubtractFloat" => a - b,
                    "SeqAct_MultiplyFloat" => a * b,
                    _ => if b != 0.0 { a / b } else { 0.0 },
                };
                self.set_prop(op, "FloatResult", Value::Float(r));
                self.set_prop(op, "IntResult", Value::Int(r.round() as i32));
                self.fire(op, 0);
            }
            "SeqCond_CompareBool" => {
                let v = self.bools(op, "Bool").iter().all(|b| *b);
                self.set_prop(op, "bResult", Value::Bool(v));
                self.fire(op, if v { 0 } else { 1 });
            }
            "SeqCond_CompareInt" | "SeqCond_CompareFloat" => {
                let (a, b) = (self.prop_float(op, "ValueA"), self.prop_float(op, "ValueB"));
                let tests = [a <= b, a > b, a == b, a < b, a >= b];
                for (i, t) in tests.iter().enumerate() {
                    if *t {
                        self.fire(op, i);
                    }
                }
            }
            "SeqCond_CompareObject" => {
                let a = self.objects(op, "A");
                let b = self.objects(op, "B");
                let eq = a.iter().all(|x| b.iter().all(|y| x == y));
                self.fire(op, if eq { 0 } else { 1 });
            }
            "SeqCond_IsSameTeam" | "SeqCond_Increment" => return None,
            "SeqAct_Log" => {
                let mut parts: Vec<String> = Vec::new();
                for s in self.strings(op, "String") {
                    parts.push(s);
                }
                for l in self.op(op).vars.clone() {
                    if l.name == "String" {
                        continue;
                    }
                    for v in &l.vars {
                        let val = self.var_value(*v);
                        parts.push(format!("{val:?}"));
                    }
                }
                let text = format!("{}{}", self.op(op).comment, parts.join(" "));
                host.log(&text);
                self.note(format!("log: {text}"));
            }
            "SeqAct_Interp" => self.interp_activated(host, op),
            _ => return None,
        }
        Some(())
    }

    fn update(&mut self, host: &mut dyn Host, op: OpId, dt: f32) -> bool {
        match self.class(op) {
            "SeqAct_Delay" => self.delay_update(op, dt),
            "SeqAct_Timer" => {
                if self.has_impulse(op, 1) {
                    return true;
                }
                let t = self.prop_float(op, "Time") + dt;
                self.set_prop(op, "Time", Value::Float(t));
                self.publish(op);
                false
            }
            "SeqAct_Interp" => self.interp_update(host, op, dt),
            _ if self.ops[op as usize].host_latent => {
                let done = {
                    let mut cx = Ctx { k: self, op };
                    host.update(&mut cx, dt)
                };
                if done {
                    self.ops[op as usize].host_latent = false;
                }
                done
            }
            _ => true,
        }
    }

    /// `DeActivated` (vtable slot 94, decoded per class; see [`deactivation`]).
    fn deactivated(&mut self, op: OpId) {
        match deactivation(self.class(op)) {
            DeAct::None => {}
            DeAct::Latent => {
                let aborted = self.ops[op as usize].flags & 1 != 0;
                let n = self.ops[op as usize].out_imp.len();
                if n > 0 {
                    self.fire(op, if aborted && n >= 2 { 1 } else { 0 });
                }
                self.ops[op as usize].flags &= !1;
            }
            DeAct::AbortedFirst => {
                if self.ops[op as usize].flags & 1 != 0 {
                    self.fire(op, 0);
                }
                self.ops[op as usize].flags &= !1;
            }
            DeAct::Auto => {
                if self.prop_bool(op, "bAutoActivateOutputLinks") {
                    for i in 0..self.ops[op as usize].out_imp.len() {
                        self.fire(op, i);
                    }
                }
            }
        }
    }

    // ----- engine actions -----

    /// `SeqAct_Gate` (decoded): In passes while open; Open/Close/Toggle; `AutoCloseCount`
    /// closes after that many passes counted from the opening activation.
    fn gate(&mut self, op: OpId) {
        let ac = self.prop_int(op, "AutoCloseCount");
        let count = self.ops[op as usize].activate_count as i32;
        let in0 = self.has_impulse(op, 0);
        let mut open = self.prop(op, "bOpen").and_then(|v| v.as_bool()).unwrap_or(true);
        let mut cur = self.prop_int(op, "CurrentCloseCount");
        if self.has_impulse(op, 1) {
            open = true;
            cur = ac + count - in0 as i32;
        } else if self.has_impulse(op, 2) {
            open = false;
        } else if self.has_impulse(op, 3) {
            open = !open;
            if open {
                cur = ac + count - in0 as i32;
            }
        }
        if open && in0 {
            self.fire(op, 0);
            if ac > 0 && cur <= count {
                open = false;
            }
        }
        self.set_prop(op, "bOpen", Value::Bool(open));
        self.set_prop(op, "CurrentCloseCount", Value::Int(cur));
    }

    /// `SeqAct_Toggle` (decoded): bool variables, linked events (and their attached
    /// duplicates) on/off/toggle; actor targets go to the host.
    fn toggle(&mut self, host: &mut dyn Host, op: OpId) {
        let mode = if self.has_impulse(op, 0) {
            0u8
        } else if self.has_impulse(op, 1) {
            1
        } else if self.has_impulse(op, 2) {
            2
        } else {
            return;
        };
        for v in self.link_vars(op, "Bool") {
            let cur = self.var_value(v).as_bool().unwrap_or(false);
            let nv = match mode {
                0 => true,
                1 => false,
                _ => !cur,
            };
            self.write_var(v, &Value::Bool(nv));
        }
        let events: Vec<OpId> = self.op(op).events.first().map(|l| l.events.clone()).unwrap_or_default();
        for e in events {
            let nv = match mode {
                0 => true,
                1 => false,
                _ => !self.ops[e as usize].enabled,
            };
            self.ops[e as usize].enabled = nv;
            let n = self.n_static();
            for (i, d) in self.dups.iter().enumerate() {
                if d.source == e {
                    self.ops[n + i].enabled = nv;
                }
            }
        }
        let targets: Vec<Obj> = self.objects(op, "Target");
        if !targets.is_empty() {
            let mut cx = Ctx { k: self, op };
            host.toggle_actors(&mut cx, &targets, mode);
        }
    }

    fn indices(&self, op: OpId) -> Vec<i32> {
        self.prop(op, "Indices").map(|v| v.as_array().iter().filter_map(Value::as_int).collect()).unwrap_or_default()
    }

    /// `SeqAct_Switch` (decoded): fires output `index - 1` for each index (disabling it with
    /// `bAutoDisableLinks`), then advances each index by `IncrementAmount`, wrapping over the
    /// outputs when `bLooping`.
    fn switch(&mut self, op: OpId) {
        let mut idx = self.indices(op);
        let inc = self.prop_int(op, "IncrementAmount");
        let looping = self.prop_bool(op, "bLooping");
        let auto_disable = self.prop_bool(op, "bAutoDisableLinks");
        let n_out = self.ops[op as usize].out_imp.len() as i32;
        for i in idx.iter_mut() {
            let o = *i - 1;
            if o >= 0 && o < n_out && !self.ops[op as usize].out_disabled[o as usize] {
                self.ops[op as usize].out_imp[o as usize] = true;
                if auto_disable {
                    self.ops[op as usize].out_disabled[o as usize] = true;
                }
            }
            if inc != 0 {
                if !looping {
                    *i += inc;
                } else if n_out > 0 {
                    *i = (*i - 1 + inc).rem_euclid(n_out) + 1;
                }
            }
        }
        self.set_prop(op, "Indices", Value::Array(idx.into_iter().map(Value::Int).collect()));
    }

    /// `SeqAct_RandomSwitch` (decoded): Reset re-enables auto-disabled links; In picks one
    /// enabled output at random (re-enabling all when looping and none is left), disables it
    /// with `bAutoDisableLinks`, and stores it in `Indices`.
    fn random_switch(&mut self, op: OpId) {
        let mut disabled: Vec<i32> = self.prop(op, "AutoDisabledIndices").map(|v| v.as_array().iter().filter_map(Value::as_int).collect()).unwrap_or_default();
        if self.has_impulse(op, 1) {
            for d in &disabled {
                if let Some(x) = self.ops[op as usize].out_disabled.get_mut(*d as usize) {
                    *x = false;
                }
            }
            disabled.clear();
        }
        if self.has_impulse(op, 0) {
            let n = self.ops[op as usize].out_imp.len();
            let mut valid: Vec<usize> = (0..n).filter(|i| !self.ops[op as usize].out_disabled[*i]).collect();
            let looping = self.prop_bool(op, "bLooping");
            let auto_disable = self.prop_bool(op, "bAutoDisableLinks");
            if looping && auto_disable && valid.is_empty() {
                for d in &disabled {
                    if let Some(x) = self.ops[op as usize].out_disabled.get_mut(*d as usize) {
                        *x = false;
                    }
                    valid.push(*d as usize);
                }
                disabled.clear();
            }
            if !valid.is_empty() {
                let pick = valid[self.rand(valid.len())];
                self.ops[op as usize].out_imp[pick] = true;
                if auto_disable {
                    disabled.push(pick as i32);
                    self.ops[op as usize].out_disabled[pick] = true;
                }
                let n_idx = self.indices(op).len().max(1);
                self.set_prop(op, "Indices", Value::Array(vec![Value::Int(pick as i32 + 1); n_idx]));
                let name = self.op(op).outputs.get(pick).map(|o| o.name.clone()).unwrap_or_default();
                self.note(format!("random switch {} -> {name}", self.db.describe(op)));
            }
        }
        self.set_prop(op, "AutoDisabledIndices", Value::Array(disabled.into_iter().map(Value::Int).collect()));
    }

    /// `SeqAct_Delay` update (decoded): Start (re)arms, Stop finishes, Pause holds; counts
    /// down from the tick after it started.
    fn delay_update(&mut self, op: OpId, dt: f32) -> bool {
        let now = self.time;
        if self.has_impulse(op, 0) {
            if self.prop_bool(op, "bStartWillRestart") {
                let d = self.prop_float(op, "Duration") as f64;
                let rt = &mut self.ops[op as usize];
                rt.t = d;
                rt.t2 = now;
            }
            self.ops[op as usize].flags |= 4; // delay active
        } else if self.has_impulse(op, 1) {
            self.ops[op as usize].flags &= !4;
            return true;
        } else if self.has_impulse(op, 2) {
            self.ops[op as usize].flags &= !4;
        }
        let rt = &mut self.ops[op as usize];
        if rt.flags & 4 != 0 && rt.t2 != now {
            rt.t -= dt as f64;
            if rt.t <= 0.0 {
                rt.flags &= !4;
                self.fire(op, 0);
                return true;
            }
        }
        false
    }

    /// `SeqAct_FinishSequence` (decoded): fires the enclosing sequence's output linked to this
    /// op; its targets are queued in the sequence that contains that sequence.
    fn finish_sequence(&mut self, op: OpId) {
        let Some(seq) = self.parent(op) else { return };
        let label = self.prop_str(op, "OutputLabel");
        let outs = self.op(seq).outputs.clone();
        let Some(k) = outs.iter().position(|o| o.linked_op == Some(op) && !o.disabled).or_else(|| outs.iter().position(|o| !label.is_empty() && o.name.eq_ignore_ascii_case(&label))) else {
            return;
        };
        let grand = self.parent(seq).unwrap_or(seq);
        let link = outs[k].clone();
        for (t, idx) in link.links {
            let in_delay = self.op(t).inputs.get(idx as usize).map(|l| l.delay).unwrap_or(0.0);
            let delay = link.delay + in_delay;
            if delay <= 0.0 {
                if self.impulse(t, idx as usize) {
                    self.queue_op(grand, t, true);
                }
            } else {
                let s = self.seqs.entry(grand).or_default();
                s.delayed.push((t, idx, delay));
            }
        }
    }

    /// `SeqAct_AttachToEvent` (decoded): duplicates each linked event onto each attachee
    /// (unless the attachee is already its originator), in this op's sequence.
    fn attach_to_event(&mut self, op: OpId) {
        let targets = self.objects(op, "Attachee");
        let events: Vec<OpId> = self.op(op).events.first().map(|l| l.events.clone()).unwrap_or_default();
        let parent = self.parent(op).unwrap_or(op);
        for t in &targets {
            for &e in &events {
                let static_orig = if (e as usize) < self.n_static() { self.db.ops[e as usize].props.obj("Originator") } else { Obj::None };
                if &static_orig == t {
                    continue;
                }
                let src = if (e as usize) < self.n_static() { e } else { self.dups[e as usize - self.n_static()].source };
                self.dups.push(Dup { source: src, parent, originator: t.clone() });
                let o = self.db.op(src);
                self.ops.push(OpRt {
                    in_imp: vec![false; o.inputs.len()],
                    in_queued: vec![0; o.inputs.len()],
                    out_imp: vec![false; o.outputs.len()],
                    in_disabled: o.inputs.iter().map(|l| l.disabled).collect(),
                    out_disabled: o.outputs.iter().map(|l| l.disabled).collect(),
                    enabled: self.ops[e as usize].enabled,
                    ..Default::default()
                });
                self.note(format!("attached {} to {t:?}", self.db.describe(src)));
            }
        }
    }

    // ----- matinee -----

    /// The `InterpData` a matinee plays.
    pub fn interp_data(&self, op: OpId) -> Option<&InterpInfo> {
        let d = self.link_vars(op, "Data").into_iter().next()?;
        self.op(d).interp.as_ref()
    }

    fn interp_length(&self, op: OpId) -> f32 {
        self.interp_data(op).map(|i| i.length).unwrap_or(0.0)
    }

    /// Flags in `OpRt::flags` for matinees.
    const PLAYING: u32 = 1;
    const REVERSE: u32 = 0x100;
    const PAUSED: u32 = 4;
    const STOP_REQ: u32 = 0x20_0000;

    /// `SeqAct_Interp::Activated` (decoded): Play / Reverse / Change Dir start playback and
    /// fire "Started"; a matinee that cannot play fires output 2.
    fn interp_activated(&mut self, host: &mut dyn Host, op: OpId) {
        let start = self.has_impulse(op, 0) || self.has_impulse(op, 1) || self.has_impulse(op, 4);
        if self.ops[op as usize].flags & Self::PLAYING != 0 || !start {
            return;
        }
        if self.interp_data(op).is_none() {
            self.fire(op, 2);
            self.ops[op as usize].active = false;
            return;
        }
        self.fire_name(op, "Started");
        self.interp_play.insert(op, crate::interp::PlayState::default()); // [interp]
        self.ops[op as usize].flags &= !Self::STOP_REQ;
        if self.has_impulse(op, 0) {
            self.interp_play(op);
        } else if self.has_impulse(op, 1) {
            let f = self.ops[op as usize].flags;
            self.ops[op as usize].flags = (f & !Self::PAUSED) | Self::PLAYING | Self::REVERSE;
        } else {
            let f = self.ops[op as usize].flags;
            self.ops[op as usize].flags = (!f & Self::REVERSE) | (f & !(Self::REVERSE | Self::PAUSED)) | Self::PLAYING;
        }
        let length = self.interp_length(op);
        let reverse = self.ops[op as usize].flags & Self::REVERSE != 0;
        let mut cx = Ctx { k: self, op };
        host.matinee(&mut cx, Matinee::Started { length, reverse });
    }

    fn interp_play(&mut self, op: OpId) {
        let rewind = self.prop_bool(op, "bRewindOnPlay");
        let len = self.interp_length(op) as f64;
        let rt = &mut self.ops[op as usize];
        if (rewind && rt.flags & Self::PLAYING == 0) || rt.t >= len {
            rt.t = 0.0;
        }
        rt.flags = (rt.flags & !(Self::REVERSE | Self::PAUSED)) | Self::PLAYING;
    }

    /// `SeqAct_Interp::Update` (decoded): inputs, then one step of playback; finished once it
    /// is no longer playing.
    fn interp_update(&mut self, host: &mut dyn Host, op: OpId, dt: f32) -> bool {
        // [interp] `SCT_<pin>` inputs break the soiree control keys of that pin.
        for i in 5..self.db.ops[op as usize].inputs.len() {
            if self.has_impulse(op, i) {
                if let Some(pin) = self.db.ops[op as usize].inputs[i].name.strip_prefix("SCT_") {
                    self.interp_play.entry(op).or_default().break_pin(pin);
                }
            }
        }
        let f = self.ops[op as usize].flags;
        if f & Self::PLAYING != 0 && self.has_impulse(op, 3) {
            self.ops[op as usize].flags ^= Self::PAUSED;
        } else if self.has_impulse(op, 0) {
            self.interp_play(op);
        } else if self.has_impulse(op, 1) {
            self.ops[op as usize].flags = (f & !Self::PAUSED) | Self::PLAYING | Self::REVERSE;
        } else if self.has_impulse(op, 2) {
            self.ops[op as usize].flags |= Self::STOP_REQ;
            self.ops[op as usize].flags &= !Self::PLAYING;
        } else if self.has_impulse(op, 4) {
            self.ops[op as usize].flags = (!f & Self::REVERSE) | (f & !(Self::REVERSE | Self::PAUSED)) | Self::PLAYING;
        } else if f & Self::PLAYING == 0 {
            // Stopped (end reached or Stop input) on an earlier update: finish now.
            self.interp_finish(host, op);
            return true;
        }
        let f = self.ops[op as usize].flags;
        if f & Self::PLAYING == 0 || f & Self::PAUSED != 0 {
            return false;
        }
        // Step.
        // [interp] Borrow the data from the shared database (it now holds the whole matinee).
        let db = self.db.clone();
        let Some(info) = self.link_vars(op, "Data").into_iter().next().and_then(|d| db.ops[d as usize].interp.as_ref()) else {
            self.ops[op as usize].flags &= !Self::PLAYING;
            return false;
        };
        let rate = self.prop(op, "PlayRate").and_then(|v| v.as_float()).unwrap_or(1.0) as f64;
        let looping = self.prop_bool(op, "bLooping");
        let len = info.length as f64;
        let reverse = f & Self::REVERSE != 0;
        let old = self.ops[op as usize].t;
        let mut new = if reverse { old - rate * dt as f64 } else { old + rate * dt as f64 };
        let mut stop = false;
        let mut events: Vec<String> = Vec::new();
        // [interp] Soiree control (loops until their pin breaks, pauses, dialog waits): the
        // position may hold or jump back; keys are not fired across a jump.
        if !reverse {
            if let Some(m) = info.matinee.as_deref() {
                let st = self.interp_play.entry(op).or_default().step(m, old as f32, new as f32);
                if st.jumped {
                    self.ops[op as usize].t = st.pos as f64;
                    return false;
                }
                new = st.pos as f64;
            }
        }
        let cross = |from: f64, to: f64, events: &mut Vec<String>| {
            for (t, name, fwd, back) in &info.events {
                let t = *t as f64;
                if to >= from && *fwd && t > from && t <= to {
                    events.push(name.clone());
                }
                if to < from && *back && t < from && t >= to {
                    events.push(name.clone());
                }
            }
        };
        if !reverse && new >= len {
            cross(old, len, &mut events);
            if looping && len > 0.0 {
                new = (new - len) % len;
                cross(0.0, new, &mut events);
            } else {
                new = len;
                stop = true;
            }
        } else if reverse && new <= 0.0 {
            cross(old, 0.0, &mut events);
            if looping && len > 0.0 {
                new = len + new % len;
                cross(len, new, &mut events);
            } else {
                new = 0.0;
                stop = true;
            }
        } else {
            cross(old, new, &mut events);
        }
        self.ops[op as usize].t = new;
        for e in events {
            self.fire_name(op, &e);
            let mut cx = Ctx { k: self, op };
            host.matinee(&mut cx, Matinee::Event(e));
        }
        if stop {
            self.ops[op as usize].flags &= !Self::PLAYING;
        }
        false
    }

    /// `SeqAct_Interp::DeActivated` (decoded): 0 Completed at the end, 1 Reversed back at the
    /// start, 2 stopped part way, 3 stopped part way by the Stop input.
    fn interp_finish(&mut self, host: &mut dyn Host, op: OpId) {
        let len = self.interp_length(op) as f64;
        let pos = self.ops[op as usize].t;
        let stop_req = self.ops[op as usize].flags & Self::STOP_REQ != 0;
        let eps = 1e-4;
        let completed;
        if !stop_req && pos < eps {
            self.fire(op, 1);
            completed = false;
        } else if pos <= len - eps {
            self.fire(op, if stop_req { 3 } else { 2 });
            completed = false;
        } else {
            self.fire(op, 0);
            completed = true;
        }
        self.ops[op as usize].flags &= !(Self::STOP_REQ | Self::PLAYING | Self::PAUSED);
        let mut cx = Ctx { k: self, op };
        host.matinee(&mut cx, Matinee::Finished { position: pos as f32, completed });
    }

    /// Start a matinee by its `m_MatineeGUID` (a conversation's `DisConv_Soiree`), as if its
    /// Play input had an impulse. Returns the op.
    pub fn play_matinee_guid(&mut self, guid: &str) -> Option<OpId> {
        let g = guid.to_ascii_uppercase();
        let op = (0..self.n_static() as OpId).find(|&o| {
            self.is_loaded(o)
                && self.class(o) == "SeqAct_Interp"
                && matches!(self.db.ops[o as usize].props.get("m_MatineeGUID"), Some(Value::Guid(x)) if format!("{:08X}{:08X}{:08X}{:08X}", x[0], x[1], x[2], x[3]) == g)
        })?;
        self.activate_input(op, 0);
        Some(op)
    }

    // ----- state -----

    /// Matinees currently playing: (op, position, length).
    pub fn playing_matinees(&self) -> Vec<(OpId, f32, f32)> {
        (0..self.n_static() as OpId).filter(|o| self.class(*o) == "SeqAct_Interp" && self.ops[*o as usize].flags & Self::PLAYING != 0).map(|o| (o, self.ops[o as usize].t as f32, self.interp_length(o))).collect()
    }

    /// Ops waiting in sequences (diagnostics): (sequence, op).
    pub fn pending(&self) -> Vec<(OpId, OpId)> {
        self.seqs.iter().flat_map(|(s, r)| r.active.iter().map(move |o| (*s, *o))).collect()
    }

    /// Hash of the runtime state (determinism checks).
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |x: u64| {
            h ^= x;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        };
        for (i, o) in self.ops.iter().enumerate() {
            if o.activate_count == 0 && o.trigger_count == 0 && o.props.0.is_empty() {
                continue;
            }
            mix(i as u64);
            mix(o.activate_count as u64);
            mix(o.trigger_count as u64);
            mix(o.active as u64);
            mix(o.flags as u64);
            mix(o.t.to_bits());
            for (k, v) in &o.props.0 {
                for b in k.bytes() {
                    mix(b as u64);
                }
                for b in format!("{v:?}").bytes() {
                    mix(b as u64);
                }
            }
        }
        mix(self.dups.len() as u64);
        mix(self.rng.holdrand as u64);
        h
    }
}

impl Ctx<'_> {
    pub fn class(&self) -> &str {
        self.k.class(self.op)
    }
    pub fn data(&self) -> &Op {
        self.k.op(self.op)
    }
    pub fn prop(&self, name: &str) -> Option<Value> {
        self.k.prop(self.op, name)
    }
    pub fn prop_str(&self, name: &str) -> String {
        self.k.prop_str(self.op, name)
    }
    pub fn prop_bool(&self, name: &str) -> bool {
        self.k.prop_bool(self.op, name)
    }
    pub fn prop_int(&self, name: &str) -> i32 {
        self.k.prop_int(self.op, name)
    }
    pub fn prop_float(&self, name: &str) -> f32 {
        self.k.prop_float(self.op, name)
    }
    pub fn prop_obj(&self, name: &str) -> Obj {
        self.k.prop(self.op, name).and_then(|v| v.as_obj().cloned()).unwrap_or_default()
    }
    /// First input with an impulse.
    pub fn input(&self) -> Option<usize> {
        self.k.input(self.op)
    }
    pub fn has_impulse(&self, i: usize) -> bool {
        self.k.has_impulse(self.op, i)
    }
    pub fn fire(&mut self, out: usize) -> bool {
        self.k.fire(self.op, out)
    }
    pub fn fire_name(&mut self, name: &str) -> bool {
        self.k.fire_name(self.op, name)
    }
    pub fn objects(&self, link: &str) -> Vec<Obj> {
        self.k.objects(self.op, link)
    }
    /// Raw objects linked (spawner actors of `DisSeqVar_SpawnerPawn` instead of their pawns).
    pub fn linked_values(&self, link: &str) -> Vec<Obj> {
        self.k.link_vars(self.op, link).into_iter().map(|v| self.k.prop(v, "ObjValue").and_then(|x| x.as_obj().cloned()).unwrap_or(Obj::Op(v))).collect()
    }
    pub fn link_vars(&self, link: &str) -> Vec<OpId> {
        self.k.link_vars(self.op, link)
    }
    pub fn bools(&mut self, link: &str) -> Vec<bool> {
        self.k.bools(self.op, link)
    }
    pub fn ints(&mut self, link: &str) -> Vec<i32> {
        self.k.ints(self.op, link)
    }
    pub fn floats(&mut self, link: &str) -> Vec<f32> {
        self.k.floats(self.op, link)
    }
    pub fn strings(&mut self, link: &str) -> Vec<String> {
        self.k.strings(self.op, link)
    }
    pub fn set_link(&mut self, link: &str, v: Value) {
        self.k.set_link(self.op, link, v)
    }
    pub fn set_prop(&mut self, name: &str, v: Value) {
        self.k.set_prop(self.op, name, v)
    }
    pub fn describe(&self) -> String {
        self.k.db.describe(self.op)
    }
    pub fn package(&self) -> &str {
        self.k.db.package_name(self.k.op(self.op).package)
    }
    pub fn actor(&self, o: &Obj) -> Option<&Actor> {
        match o {
            Obj::Actor(a) => self.k.db.actors.get(*a as usize),
            _ => None,
        }
    }
    /// Abort flag of a latent action (fires output 1 "Aborted" when it finishes).
    pub fn set_aborted(&mut self) {
        self.k.ops[self.op as usize].flags |= 1;
    }
}

/// What `PublishLinkedVariableValues` writes into each of a link's `n_vars` variables from
/// the op's property value `v` (None = left alone). A single variable gets the value. An array
/// property goes back element by element: entry i into the i-th variable (UE3
/// `USeqVar_Object::PublishValue` copies `min(array, vars)` entries in order). Writing the whole
/// array into every variable gave each one entry 0, so an op with `Targets` linked to "Lord
/// Shaw", "Shaw_Elite" and "Shaw_Elite_Pistols" made all three Lord Shaw (U-221). `populate`
/// skips empty object variables, so when the counts differ the entries cannot be matched to
/// their variables and nothing is written back.
pub fn publish_values(v: &Value, n_vars: usize) -> Vec<Option<Value>> {
    match v {
        Value::Array(items) if n_vars > 1 => {
            if items.len() == n_vars {
                items.iter().cloned().map(Some).collect()
            } else {
                vec![None; n_vars]
            }
        }
        _ => vec![Some(v.clone()); n_vars],
    }
}

#[cfg(test)]
mod publish_tests {
    use super::*;

    #[test]
    fn array_property_goes_back_element_by_element() {
        let a = Value::Array(vec![Value::Obj(Obj::Pawn(7)), Value::Obj(Obj::Pawn(4)), Value::Obj(Obj::Pawn(8))]);
        assert_eq!(publish_values(&a, 3), vec![Some(Value::Obj(Obj::Pawn(7))), Some(Value::Obj(Obj::Pawn(4))), Some(Value::Obj(Obj::Pawn(8)))]);
        // An empty variable was skipped when populating: no way to match, nothing written.
        let short = Value::Array(vec![Value::Obj(Obj::Pawn(7)), Value::Obj(Obj::Pawn(8))]);
        assert_eq!(publish_values(&short, 3), vec![None, None, None]);
        // One variable takes the whole value (as before).
        assert_eq!(publish_values(&a, 1), vec![Some(a.clone())]);
        assert_eq!(publish_values(&Value::Bool(true), 2), vec![Some(Value::Bool(true)), Some(Value::Bool(true))]);
    }
}
