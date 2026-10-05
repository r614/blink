//! Interpreter semantics on small synthetic graphs (no game data needed): gates, toggles,
//! delays, switches, random switches, remote events, trigger limits, latent ops, sequences,
//! variables and queued activations, following the engine behaviour documented in
//! research/dishonored/KISMET.md.

use dishonored_kismet::runtime::Ctx;
use dishonored_kismet::*;
use std::collections::BTreeMap;
use std::sync::Arc;

const DT: f32 = 1.0 / 60.0;

/// Builds a database with one always-loaded package ("Global") and one map ("Map" with
/// package "MapPkg").
struct B {
    db: KismetDb,
}

impl B {
    fn new() -> Self {
        let mut db = KismetDb { packages: vec!["Global".into(), "MapPkg".into()], global: vec![0], maps: vec![("Map".into(), vec![1])], roots: vec![Vec::new(), Vec::new()], ..Default::default() };
        let chains: &[(&str, &[&str])] = &[
            ("Sequence", &["Sequence", "SequenceOp", "SequenceObject"]),
            ("SeqEvent_RemoteEvent", &["SeqEvent_RemoteEvent", "SequenceEvent", "SequenceOp"]),
            ("SeqEvent_Console", &["SeqEvent_Console", "SequenceEvent", "SequenceOp"]),
            ("SeqEvent_SequenceActivated", &["SeqEvent_SequenceActivated", "SequenceEvent", "SequenceOp"]),
            ("SeqEvent_Touch", &["SeqEvent_Touch", "SequenceEvent", "SequenceOp"]),
            ("SeqAct_Gate", &["SeqAct_Gate", "SequenceAction", "SequenceOp"]),
            ("SeqAct_Toggle", &["SeqAct_Toggle", "SequenceAction", "SequenceOp"]),
            ("SeqAct_Switch", &["SeqAct_Switch", "SequenceAction", "SequenceOp"]),
            ("SeqAct_RandomSwitch", &["SeqAct_RandomSwitch", "SeqAct_Switch", "SequenceAction", "SequenceOp"]),
            ("SeqAct_Delay", &["SeqAct_Delay", "SeqAct_Latent", "SequenceAction", "SequenceOp"]),
            ("SeqAct_Interp", &["SeqAct_Interp", "SeqAct_Latent", "SequenceAction", "SequenceOp"]),
            ("SeqAct_FinishSequence", &["SeqAct_FinishSequence", "SequenceAction", "SequenceOp"]),
            ("SeqAct_ActivateRemoteEvent", &["SeqAct_ActivateRemoteEvent", "SequenceAction", "SequenceOp"]),
            ("SeqAct_SetBool", &["SeqAct_SetBool", "SeqAct_SetSequenceVariable", "SequenceAction", "SequenceOp"]),
            ("SeqCond_CompareBool", &["SeqCond_CompareBool", "SequenceCondition", "SequenceOp"]),
            ("SeqVar_Bool", &["SeqVar_Bool", "SequenceVariable", "SequenceObject"]),
            ("InterpData", &["InterpData", "SequenceVariable", "SequenceObject"]),
            ("TestRecord", &["TestRecord", "SequenceAction", "SequenceOp"]),
        ];
        db.classes = chains.iter().map(|(c, ch)| (c.to_string(), ch.iter().map(|s| s.to_string()).collect())).collect();
        B { db }
    }

    fn add(&mut self, package: u16, class: &str, kind: Kind, parent: Option<OpId>, inputs: &[&str], outputs: &[&str], props: Vec<(&str, Value)>) -> OpId {
        let id = self.db.ops.len() as OpId;
        let mut p = Props::default();
        // Class defaults the loader would merge (the relevant ones).
        match class {
            "SeqAct_Gate" | "TestRecord" => p.set("bAutoActivateOutputLinks", Value::Bool(false)),
            _ if kind == Kind::Event || kind == Kind::Condition => p.set("bAutoActivateOutputLinks", Value::Bool(false)),
            _ => p.set("bAutoActivateOutputLinks", Value::Bool(true)),
        }
        if kind == Kind::Event {
            p.set("MaxTriggerCount", Value::Int(1));
            p.set("bPlayerOnly", Value::Bool(false));
        }
        for (k, v) in props {
            p.set(k, v);
        }
        self.db.ops.push(Op {
            id,
            package,
            export: id,
            name: format!("{class}_{id}"),
            path: format!("TheWorld.PersistentLevel.Main_Sequence.{class}_{id}"),
            class: class.to_string(),
            kind,
            parent,
            comment: String::new(),
            props: p,
            inputs: inputs.iter().map(|n| InputLink { name: n.to_string(), ..Default::default() }).collect(),
            outputs: outputs.iter().map(|n| OutputLink { name: n.to_string(), ..Default::default() }).collect(),
            vars: Vec::new(),
            events: Vec::new(),
            children: Vec::new(),
            named: None,
            interp: None,
        });
        if let Some(s) = parent {
            self.db.ops[s as usize].children.push(id);
        }
        id
    }

    fn root(&mut self, package: u16) -> OpId {
        let s = self.add(package, "Sequence", Kind::Sequence, None, &[], &[], vec![("bEnabled", Value::Bool(true))]);
        self.db.roots[package as usize].push(s);
        s
    }

    fn event(&mut self, seq: OpId, class: &str, props: Vec<(&str, Value)>) -> OpId {
        let pk = self.db.ops[seq as usize].package;
        self.add(pk, class, Kind::Event, Some(seq), &[], &["Out"], props)
    }

    fn record(&mut self, seq: OpId) -> OpId {
        let pk = self.db.ops[seq as usize].package;
        self.add(pk, "TestRecord", Kind::Action, Some(seq), &["In", "Second"], &["Out"], vec![])
    }

    fn link(&mut self, from: OpId, out: usize, to: OpId, input: u16) {
        self.db.ops[from as usize].outputs[out].links.push((to, input));
    }

    fn var_bool(&mut self, seq: OpId, v: bool) -> OpId {
        let pk = self.db.ops[seq as usize].package;
        self.add(pk, "SeqVar_Bool", Kind::Variable, Some(seq), &[], &[], vec![("bValue", Value::Int(v as i32))])
    }

    fn var_link(&mut self, op: OpId, name: &str, vars: Vec<OpId>, property: &str) {
        self.db.ops[op as usize].vars.push(VarLink { name: name.into(), vars, property: property.into(), writeable: true, expected: String::new() });
    }

    fn build(self, seed: u64) -> Kismet {
        let mut k = Kismet::new(Arc::new(self.db), seed);
        k.players = vec![1];
        k
    }
}

/// Records `TestRecord` activations: (tick, op, input).
#[derive(Default)]
struct Rec {
    tick: u32,
    hits: Vec<(u32, OpId, usize)>,
    flags: BTreeMap<String, bool>,
}

impl Host for Rec {
    fn flag(&mut self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }
    fn set_flag(&mut self, name: &str, value: bool) {
        self.flags.insert(name.into(), value);
    }
    fn is_player(&self, o: &Obj) -> bool {
        matches!(o, Obj::Pawn(1))
    }
    fn activated(&mut self, cx: &mut Ctx) -> HostResult {
        if cx.class() == "TestRecord" {
            self.hits.push((self.tick, cx.op, cx.input().unwrap_or(99)));
            return HostResult::Done;
        }
        HostResult::Unhandled
    }
}

fn tick(k: &mut Kismet, h: &mut Rec, n: u32) {
    for _ in 0..n {
        h.tick += 1;
        k.tick(h, DT);
    }
}

fn fire(k: &mut Kismet, h: &mut Rec, ev: OpId) -> bool {
    k.check_activate(h, ev, Obj::Path("WorldInfo".into()), Obj::Pawn(1), false, None, false)
}

#[test]
fn gate_auto_close_open_close_toggle() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![("MaxTriggerCount", Value::Int(0))]);
    let g = b.add(0, "SeqAct_Gate", Kind::Action, Some(s), &["In", "Open", "Close", "Toggle"], &["Out"], vec![("bOpen", Value::Bool(true)), ("AutoCloseCount", Value::Int(1)), ("CurrentCloseCount", Value::Int(1))]);
    let r = b.record(s);
    b.link(e, 0, g, 0);
    b.link(g, 0, r, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    for _ in 0..3 {
        assert!(fire(&mut k, &mut h, e));
        tick(&mut k, &mut h, 1);
    }
    assert_eq!(h.hits.len(), 1, "AutoCloseCount 1: the gate closes after one pass");
    // Open again: one more pass.
    k.activate_input(g, 1);
    tick(&mut k, &mut h, 1);
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    assert_eq!(h.hits.len(), 2);
    // Toggle opens a closed gate.
    k.activate_input(g, 3);
    tick(&mut k, &mut h, 1);
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    assert_eq!(h.hits.len(), 3);
}

#[test]
fn gate_without_auto_close_and_close_input() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![("MaxTriggerCount", Value::Int(0))]);
    let g = b.add(0, "SeqAct_Gate", Kind::Action, Some(s), &["In", "Open", "Close", "Toggle"], &["Out"], vec![("bOpen", Value::Bool(true))]);
    let r = b.record(s);
    b.link(e, 0, g, 0);
    b.link(g, 0, r, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    for _ in 0..3 {
        fire(&mut k, &mut h, e);
        tick(&mut k, &mut h, 1);
    }
    assert_eq!(h.hits.len(), 3);
    k.activate_input(g, 2);
    tick(&mut k, &mut h, 1);
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    assert_eq!(h.hits.len(), 3, "closed gate blocks");
}

#[test]
fn toggle_disables_and_enables_events_and_flips_bools() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![("MaxTriggerCount", Value::Int(0))]);
    let r = b.record(s);
    b.link(e, 0, r, 0);
    let t = b.add(0, "SeqAct_Toggle", Kind::Action, Some(s), &["Turn On", "Turn Off", "Toggle"], &["Out"], vec![]);
    b.db.ops[t as usize].events.push(EventLink { name: "Event".into(), events: vec![e] });
    let v = b.var_bool(s, false);
    b.var_link(t, "Bool", vec![v], "");
    let mut k = b.build(1);
    let mut h = Rec::default();
    k.activate_input(t, 1); // off
    tick(&mut k, &mut h, 1);
    assert!(!k.ops[e as usize].enabled);
    assert!(fire(&mut k, &mut h, e), "a disabled event still passes CheckActivate");
    tick(&mut k, &mut h, 1);
    assert!(h.hits.is_empty(), "but does not activate");
    k.activate_input(t, 2); // toggle -> on
    tick(&mut k, &mut h, 1);
    assert!(k.ops[e as usize].enabled);
    assert_eq!(k.var_value(v), Value::Bool(true), "Toggle flips the bool (off -> toggle = on)");
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    assert_eq!(h.hits.len(), 1);
}

#[test]
fn delay_fires_after_its_duration_counted_from_the_next_tick() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![]);
    let d = b.add(0, "SeqAct_Delay", Kind::Action, Some(s), &["Start", "Stop", "Pause"], &["Finished"], vec![("Duration", Value::Float(0.5))]);
    let r = b.record(s);
    b.link(e, 0, d, 0);
    b.link(d, 0, r, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 29);
    assert!(h.hits.is_empty());
    tick(&mut k, &mut h, 2);
    assert_eq!(h.hits.len(), 1);
    assert_eq!(h.hits[0].0, 31, "activated on tick 1, counts down from tick 2: 30 ticks of 1/60 s");
    tick(&mut k, &mut h, 60);
    assert_eq!(h.hits.len(), 1, "a latent op finishes once");
}

#[test]
fn delay_stop_finishes_without_output() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![]);
    let d = b.add(0, "SeqAct_Delay", Kind::Action, Some(s), &["Start", "Stop", "Pause"], &["Finished"], vec![("Duration", Value::Float(0.5))]);
    let r = b.record(s);
    b.link(e, 0, d, 0);
    b.link(d, 0, r, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 10);
    k.activate_input(d, 1);
    tick(&mut k, &mut h, 60);
    assert!(h.hits.is_empty(), "Stop ends the delay; its DeActivated fires nothing");
    assert!(!k.ops[d as usize].active);
}

#[test]
fn switch_steps_through_links_and_loops() {
    for looping in [false, true] {
        let mut b = B::new();
        let s = b.root(0);
        let e = b.event(s, "SeqEvent_Console", vec![("MaxTriggerCount", Value::Int(0))]);
        let sw = b.add(0, "SeqAct_Switch", Kind::Action, Some(s), &["In"], &["Link 1", "Link 2", "Link 3"], vec![("LinkCount", Value::Int(3)), ("IncrementAmount", Value::Int(1)), ("Indices", Value::Array(vec![Value::Int(1)])), ("bLooping", Value::Bool(looping))]);
        let rs: Vec<OpId> = (0..3).map(|_| b.record(s)).collect();
        b.link(e, 0, sw, 0);
        for (i, r) in rs.iter().enumerate() {
            b.link(sw, i, *r, 0);
        }
        let mut k = b.build(1);
        let mut h = Rec::default();
        for _ in 0..4 {
            fire(&mut k, &mut h, e);
            tick(&mut k, &mut h, 1);
        }
        let order: Vec<OpId> = h.hits.iter().map(|x| x.1).collect();
        if looping {
            assert_eq!(order, vec![rs[0], rs[1], rs[2], rs[0]]);
        } else {
            assert_eq!(order, vec![rs[0], rs[1], rs[2]], "past the last link nothing fires");
        }
    }
}

#[test]
fn random_switch_with_auto_disable_picks_each_link_once_and_is_reproducible() {
    let run = |seed: u64| {
        let mut b = B::new();
        let s = b.root(0);
        let e = b.event(s, "SeqEvent_Console", vec![("MaxTriggerCount", Value::Int(0))]);
        let sw = b.add(0, "SeqAct_RandomSwitch", Kind::Action, Some(s), &["In", "Reset"], &["Link 1", "Link 2", "Link 3"], vec![("LinkCount", Value::Int(3)), ("bAutoDisableLinks", Value::Bool(true))]);
        let rs: Vec<OpId> = (0..3).map(|_| b.record(s)).collect();
        b.link(e, 0, sw, 0);
        for (i, r) in rs.iter().enumerate() {
            b.link(sw, i, *r, 0);
        }
        let mut k = b.build(seed);
        let mut h = Rec::default();
        for _ in 0..4 {
            fire(&mut k, &mut h, e);
            tick(&mut k, &mut h, 1);
        }
        let picked: Vec<usize> = h.hits.iter().map(|x| rs.iter().position(|r| *r == x.1).unwrap()).collect();
        // Reset re-enables the links.
        k.activate_input(sw, 1);
        tick(&mut k, &mut h, 1);
        fire(&mut k, &mut h, e);
        tick(&mut k, &mut h, 1);
        (picked, h.hits.len())
    };
    let (a, n) = run(7);
    let mut sorted = a.clone();
    sorted.sort();
    assert_eq!(sorted, vec![0, 1, 2], "each link once, then none left (not looping)");
    assert_eq!(n, 4, "Reset makes a link available again");
    assert_eq!(run(7).0, a, "same seed, same picks");
    let other: Vec<Vec<usize>> = (1..20).map(|s| run(s).0).collect();
    assert!(other.iter().any(|o| *o != a), "different seeds give different orders");
}

#[test]
fn remote_events_reach_enabled_listeners_in_loaded_levels_only() {
    let mut b = B::new();
    let g = b.root(0);
    let m = b.root(1);
    let start = b.event(g, "SeqEvent_Console", vec![]);
    let act = b.add(0, "SeqAct_ActivateRemoteEvent", Kind::Action, Some(g), &["In"], &["Out"], vec![("EventName", Value::Name("Ping".into()))]);
    b.link(start, 0, act, 0);
    let l_global = b.event(g, "SeqEvent_RemoteEvent", vec![("EventName", Value::Name("Ping".into())), ("MaxTriggerCount", Value::Int(0))]);
    let l_map = b.event(m, "SeqEvent_RemoteEvent", vec![("EventName", Value::Name("Ping".into())), ("MaxTriggerCount", Value::Int(0))]);
    let l_off = b.event(g, "SeqEvent_RemoteEvent", vec![("EventName", Value::Name("Ping".into())), ("bEnabled", Value::Bool(false))]);
    let rg = b.record(g);
    let rm = b.record(m);
    let ro = b.record(g);
    b.link(l_global, 0, rg, 0);
    b.link(l_map, 0, rm, 0);
    b.link(l_off, 0, ro, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    fire(&mut k, &mut h, start);
    tick(&mut k, &mut h, 2);
    let hit: Vec<OpId> = h.hits.iter().map(|x| x.1).collect();
    assert_eq!(hit, vec![rg], "the map is not loaded; the disabled listener is skipped");
    // Load the map: the next remote event reaches it too (in the same tick, after the global
    // sequence).
    k.load_map(&mut h, "Map");
    h.hits.clear();
    k.remote_event(&mut h, "Ping", Obj::None);
    tick(&mut k, &mut h, 1);
    let hit: Vec<OpId> = h.hits.iter().map(|x| x.1).collect();
    assert_eq!(hit, vec![rg, rm]);
    assert!(h.hits.iter().all(|x| x.0 == h.tick));
}

#[test]
fn max_trigger_count_and_retrigger_delay() {
    let mut b = B::new();
    let s = b.root(0);
    let twice = b.event(s, "SeqEvent_Console", vec![("MaxTriggerCount", Value::Int(2))]);
    let slow = b.event(s, "SeqEvent_Console", vec![("MaxTriggerCount", Value::Int(0)), ("ReTriggerDelay", Value::Float(0.5))]);
    let r1 = b.record(s);
    let r2 = b.record(s);
    b.link(twice, 0, r1, 0);
    b.link(slow, 0, r2, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    for _ in 0..3 {
        fire(&mut k, &mut h, twice);
        tick(&mut k, &mut h, 1);
    }
    assert_eq!(h.hits.iter().filter(|x| x.1 == r1).count(), 2, "MaxTriggerCount 2");
    assert!(fire(&mut k, &mut h, slow));
    tick(&mut k, &mut h, 1);
    assert!(!fire(&mut k, &mut h, slow), "within ReTriggerDelay");
    tick(&mut k, &mut h, 31);
    assert!(fire(&mut k, &mut h, slow), "after ReTriggerDelay");
    tick(&mut k, &mut h, 1);
    assert_eq!(h.hits.iter().filter(|x| x.1 == r2).count(), 2);
}

#[test]
fn player_only_events_need_a_player_instigator() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Touch", vec![("bPlayerOnly", Value::Bool(true)), ("MaxTriggerCount", Value::Int(0))]);
    let mut k = b.build(1);
    let mut h = Rec::default();
    assert!(!k.check_activate(&mut h, e, Obj::Path("W".into()), Obj::Pawn(5), false, None, false));
    assert!(k.check_activate(&mut h, e, Obj::Path("W".into()), Obj::Pawn(1), false, None, false));
}

#[test]
fn touch_fires_touched_untouched_and_empty() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.add(0, "SeqEvent_Touch", Kind::Event, Some(s), &[], &["Touched", "UnTouched", "Empty"], vec![("MaxTriggerCount", Value::Int(0)), ("bPlayerOnly", Value::Bool(false)), ("bAutoActivateOutputLinks", Value::Bool(false))]);
    b.db.ops[e as usize].props.set("Originator", Value::Obj(Obj::Path("Trigger".into())));
    let rs: Vec<OpId> = (0..3).map(|_| b.record(s)).collect();
    for (i, r) in rs.iter().enumerate() {
        b.link(e, i, *r, 0);
    }
    let mut k = b.build(1);
    let mut h = Rec::default();
    assert!(k.touch(&mut h, e, Obj::Pawn(1), true));
    tick(&mut k, &mut h, 1);
    assert!(k.touch(&mut h, e, Obj::Pawn(2), true));
    tick(&mut k, &mut h, 1);
    assert!(k.touch(&mut h, e, Obj::Pawn(1), false));
    tick(&mut k, &mut h, 1);
    assert!(k.touch(&mut h, e, Obj::Pawn(2), false));
    tick(&mut k, &mut h, 1);
    let order: Vec<usize> = h.hits.iter().map(|x| rs.iter().position(|r| *r == x.1).unwrap()).collect();
    assert_eq!(order, vec![0, 0, 1, 1, 2], "two touches, two untouches, then empty");
}

#[test]
fn latent_matinee_fires_event_keys_then_completed() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![]);
    let data = b.add(0, "InterpData", Kind::Variable, Some(s), &[], &[], vec![]);
    b.db.ops[data as usize].interp = Some(InterpInfo { length: 1.0, events: vec![(0.5, "Half".into(), true, true)] });
    let m = b.add(0, "SeqAct_Interp", Kind::Action, Some(s), &["Play", "Reverse", "Stop", "Pause", "Change Dir"], &["Completed", "Reversed", "Interrupted", "Stopped", "Started", "OnSkipWhole", "Half"], vec![("PlayRate", Value::Float(1.0))]);
    b.var_link(m, "Data", vec![data], "");
    let rs: Vec<OpId> = (0..7).map(|_| b.record(s)).collect();
    for (i, r) in rs.iter().enumerate() {
        b.link(m, i, *r, 0);
    }
    b.link(e, 0, m, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 120);
    let got: Vec<(u32, usize)> = h.hits.iter().map(|x| (x.0, rs.iter().position(|r| *r == x.1).unwrap())).collect();
    assert_eq!(got.iter().map(|g| g.1).collect::<Vec<_>>(), vec![4, 6, 0], "Started, the event key, Completed");
    assert_eq!(got[0].0, 1);
    assert!((29..=31).contains(&got[1].0), "event key at 0.5 s: tick {}", got[1].0);
    assert!((60..=62).contains(&got[2].0), "Completed the update after reaching the end: tick {}", got[2].0);
    // Stop part way: output 3 ("Stopped").
    h.hits.clear();
    k.activate_input(m, 0);
    tick(&mut k, &mut h, 10);
    k.activate_input(m, 2);
    tick(&mut k, &mut h, 3);
    let got: Vec<usize> = h.hits.iter().map(|x| rs.iter().position(|r| *r == x.1).unwrap()).collect();
    assert_eq!(got, vec![4, 3]);
}

#[test]
fn sequences_start_on_their_input_and_finish_into_the_parent_next_tick() {
    let mut b = B::new();
    let root = b.root(0);
    let e = b.event(root, "SeqEvent_Console", vec![]);
    let sub = b.add(0, "Sequence", Kind::Sequence, Some(root), &["In"], &["Done"], vec![("bEnabled", Value::Bool(true))]);
    let act = b.event(sub, "SeqEvent_SequenceActivated", vec![("InputLabel", Value::Str("In".into())), ("MaxTriggerCount", Value::Int(0))]);
    let inner = b.record(sub);
    let fin = b.add(0, "SeqAct_FinishSequence", Kind::Action, Some(sub), &["In"], &[], vec![("OutputLabel", Value::Str("Done".into()))]);
    let after = b.record(root);
    b.db.ops[sub as usize].inputs[0].linked_op = Some(act);
    b.db.ops[sub as usize].outputs[0].linked_op = Some(fin);
    b.link(e, 0, sub, 0);
    b.link(act, 0, inner, 0);
    b.link(act, 0, fin, 0);
    b.link(sub, 0, after, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    assert_eq!(h.hits, vec![(1, inner, 0)], "the nested sequence runs after its parent, in the same tick");
    tick(&mut k, &mut h, 1);
    assert_eq!(h.hits.last(), Some(&(2, after, 0)), "FinishSequence queues the parent's targets; the parent already ran this tick");
}

#[test]
fn output_links_run_depth_first_in_link_order_and_queue_repeated_impulses() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![("MaxTriggerCount", Value::Int(0))]);
    let a = b.record(s);
    let c = b.record(s);
    b.link(e, 0, a, 0);
    b.link(e, 0, c, 0);
    b.link(e, 0, a, 0); // a second impulse on the same input: a queued activation
    let mut k = b.build(1);
    let mut h = Rec::default();
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    let order: Vec<OpId> = h.hits.iter().map(|x| x.1).collect();
    // Targets get their impulses in reverse link order and are pushed on top unless already
    // queued: the last link (a) is pushed first, then c on top; the first link's impulse finds
    // a already queued and only counts a queued activation. So c runs, then a twice.
    assert_eq!(order, vec![c, a, a], "pushed in reverse; a repeated impulse is a queued activation, not a second push");
}

#[test]
fn output_links_run_in_link_order() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![]);
    let a = b.record(s);
    let c = b.record(s);
    let d = b.record(s);
    b.link(e, 0, a, 0);
    b.link(e, 0, c, 0);
    b.link(e, 0, d, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    let order: Vec<OpId> = h.hits.iter().map(|x| x.1).collect();
    assert_eq!(order, vec![a, c, d], "first link first");
}

#[test]
fn set_bool_and_compare_bool_through_variables() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![]);
    let src = b.var_bool(s, true);
    let dst = b.var_bool(s, false);
    let set = b.add(0, "SeqAct_SetBool", Kind::Action, Some(s), &["In"], &["Out"], vec![]);
    b.var_link(set, "Value", vec![src], "");
    b.var_link(set, "Target", vec![dst], "");
    let cmp = b.add(0, "SeqCond_CompareBool", Kind::Condition, Some(s), &["In"], &["True", "False"], vec![]);
    b.var_link(cmp, "Bool", vec![dst], "");
    let t = b.record(s);
    let f = b.record(s);
    b.link(e, 0, set, 0);
    b.link(set, 0, cmp, 0);
    b.link(cmp, 0, t, 0);
    b.link(cmp, 1, f, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    assert_eq!(k.var_value(dst), Value::Bool(true));
    assert_eq!(h.hits.iter().map(|x| x.1).collect::<Vec<_>>(), vec![t]);
}

#[test]
fn unsupported_ops_are_counted_and_continue_with_their_automatic_outputs() {
    let mut b = B::new();
    let s = b.root(0);
    let e = b.event(s, "SeqEvent_Console", vec![]);
    let x = b.add(0, "DisSeqAct_SomethingNew", Kind::Action, Some(s), &["In"], &["Out"], vec![]);
    let r = b.record(s);
    b.link(e, 0, x, 0);
    b.link(x, 0, r, 0);
    let mut k = b.build(1);
    let mut h = Rec::default();
    fire(&mut k, &mut h, e);
    tick(&mut k, &mut h, 1);
    assert_eq!(k.coverage.unhandled.get("DisSeqAct_SomethingNew"), Some(&1));
    assert!(k.log.iter().any(|l| l.contains("unsupported Kismet class DisSeqAct_SomethingNew")));
    assert_eq!(h.hits.len(), 1);
}
