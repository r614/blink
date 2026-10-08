//! Semantics of the engine ops the interpreter runs itself (and `Kismet` plumbing they use):
//! `SeqAct_AndGate`, `SeqAct_CastToInt` / `CastToFloat`, `SeqCond_Increment` /
//! `IncrementFloat`, the object-list ops and `SeqVar_External`. Each expectation is the
//! decompiled behaviour named in the op's comment in `runtime.rs`; the graphs are built by hand
//! (no retail data).

use dishonored_kismet::host::NullHost;
use dishonored_kismet::runtime::in_object_list;
use dishonored_kismet::testkit::GraphBuilder;
use dishonored_kismet::*;
use std::sync::Arc;

type G = GraphBuilder;

fn run(g: G) -> Kismet {
    Kismet::new(Arc::new(g.db), 1)
}

fn step(k: &mut Kismet) {
    k.tick(&mut NullHost::default(), 0.1);
}

fn ran(k: &Kismet, op: OpId) -> u32 {
    k.coverage.activations.get(&op).copied().unwrap_or(0)
}

#[test]
fn and_gate_waits_for_every_linked_output_then_closes() {
    let mut g = G::new();
    let (a, b) = (g.sink(), g.sink());
    let gate = g.op("SeqAct_AndGate");
    g.inputs(gate, &["In"]);
    g.outputs(gate, &["Out"]);
    g.set(gate, "bOpen", Value::Bool(true));
    g.set(gate, "bAutoActivateOutputLinks", Value::Bool(false));
    let out = g.sink();
    g.link(a, 0, gate, 0);
    g.link(b, 0, gate, 0);
    g.link(gate, 0, out, 0);
    let mut k = run(g);
    k.activate_input(a, 0);
    step(&mut k);
    assert_eq!(ran(&k, gate), 1, "the gate is activated by the first impulse");
    assert_eq!(ran(&k, out), 0, "but fires nothing until b has delivered too");
    k.activate_input(b, 0);
    step(&mut k);
    assert_eq!(ran(&k, out), 1, "all linked outputs have delivered");
    // Closed for good: nothing reopens the gate.
    k.activate_input(a, 0);
    k.activate_input(b, 0);
    step(&mut k);
    assert_eq!(ran(&k, out), 1);
    assert_eq!(k.prop(gate, "bOpen"), Some(Value::Bool(false)));
}

#[test]
fn and_gate_counts_a_source_once_however_often_it_fires() {
    let mut g = G::new();
    let (a, b) = (g.sink(), g.sink());
    let gate = g.op("SeqAct_AndGate");
    g.inputs(gate, &["In"]);
    g.outputs(gate, &["Out"]);
    g.set(gate, "bOpen", Value::Bool(true));
    g.set(gate, "bAutoActivateOutputLinks", Value::Bool(false));
    let out = g.sink();
    g.link(a, 0, gate, 0);
    g.link(b, 0, gate, 0);
    g.link(gate, 0, out, 0);
    let mut k = run(g);
    for _ in 0..3 {
        k.activate_input(a, 0);
        step(&mut k);
    }
    assert_eq!(ran(&k, out), 0, "a alone never opens it");
}

#[test]
fn cast_to_int_rounds_or_truncates() {
    for (v, trunc, want) in [(2.5, false, 3), (2.4, false, 2), (2.6, false, 3), (-0.5, false, 0), (-1.5, false, -1), (2.9, true, 2), (-2.9, true, -2)] {
        let mut g = G::new();
        let c = g.op("SeqAct_CastToInt");
        g.inputs(c, &["In"]);
        g.outputs(c, &["Out"]);
        g.set(c, "Value", Value::Float(v));
        g.set(c, "bTruncate", Value::Bool(trunc));
        let out = g.sink();
        g.link(c, 0, out, 0);
        let mut k = run(g);
        k.activate_input(c, 0);
        step(&mut k);
        assert_eq!(k.prop_int(c, "IntResult"), want, "cast {v} (truncate {trunc})");
        assert_eq!(ran(&k, out), 1);
    }
}

#[test]
fn cast_to_float_converts_the_int() {
    let mut g = G::new();
    let c = g.op("SeqAct_CastToFloat");
    g.inputs(c, &["In"]);
    g.outputs(c, &["Out"]);
    g.set(c, "Value", Value::Int(-7));
    let out = g.sink();
    g.link(c, 0, out, 0);
    let mut k = run(g);
    k.activate_input(c, 0);
    step(&mut k);
    assert_eq!(k.prop_float(c, "FloatResult"), -7.0);
    assert_eq!(ran(&k, out), 1);
}

/// Which of the five outputs fired for a counter op: 0 `<=`, 1 `>`, 2 `==`, 3 `<`, 4 `>=`.
fn increment_outputs(class: &str, a: Value, inc: Value, b: Value) -> (Vec<bool>, Value) {
    let mut g = G::new();
    let c = g.cond(class);
    g.inputs(c, &["In"]);
    g.outputs(c, &["A<=B", "A>B", "A==B", "A<B", "A>=B"]);
    g.set(c, "ValueA", a);
    g.set(c, "IncrementAmount", inc);
    g.set(c, "ValueB", b);
    g.set(c, "bAutoActivateOutputLinks", Value::Bool(false));
    let sinks: Vec<OpId> = (0..5).map(|_| g.sink()).collect();
    for (i, s) in sinks.iter().enumerate() {
        g.link(c, i, *s, 0);
    }
    let mut k = run(g);
    k.activate_input(c, 0);
    step(&mut k);
    (sinks.iter().map(|s| ran(&k, *s) == 1).collect(), k.prop(c, "ValueA").unwrap())
}

#[test]
fn increment_counts_up_and_compares() {
    // 2 + 1 against 3: equal.
    let (fired, a) = increment_outputs("SeqCond_Increment", Value::Int(2), Value::Int(1), Value::Int(3));
    assert_eq!(fired, [true, false, true, false, true]);
    assert_eq!(a, Value::Int(3));
    // Below the target.
    let (fired, _) = increment_outputs("SeqCond_Increment", Value::Int(0), Value::Int(1), Value::Int(3));
    assert_eq!(fired, [true, false, false, true, false]);
    // Past it.
    let (fired, a) = increment_outputs("SeqCond_Increment", Value::Int(3), Value::Int(1), Value::Int(3));
    assert_eq!(fired, [false, true, false, false, true]);
    assert_eq!(a, Value::Int(4));
}

#[test]
fn increment_float_counts_up_and_compares() {
    let (fired, a) = increment_outputs("SeqCond_IncrementFloat", Value::Float(0.5), Value::Float(0.25), Value::Float(1.0));
    assert_eq!(fired, [true, false, false, true, false]);
    assert_eq!(a, Value::Float(0.75));
    let (fired, _) = increment_outputs("SeqCond_IncrementFloat", Value::Float(0.75), Value::Float(0.25), Value::Float(1.0));
    assert_eq!(fired, [true, false, true, false, true]);
    let (fired, _) = increment_outputs("SeqCond_IncrementFloat", Value::Float(1.0), Value::Float(0.25), Value::Float(1.0));
    assert_eq!(fired, [false, true, false, false, true]);
}

#[test]
fn in_object_list_keeps_the_decoded_last_test_object_quirk() {
    let (a, b, c) = (Obj::Actor(1), Obj::Actor(2), Obj::Actor(3));
    // Any: found when the test object is in the list.
    assert!(in_object_list(false, &[a.clone(), b.clone()], &[b.clone()]));
    assert!(!in_object_list(false, &[a.clone(), b.clone()], &[c.clone()]));
    // The decoded loop leaves the result of the last test object only.
    assert!(!in_object_list(false, &[a.clone()], &[a.clone(), b.clone()]));
    assert!(in_object_list(false, &[a.clone()], &[b.clone(), a.clone()]));
    // All: the test object must equal every list entry.
    assert!(in_object_list(true, &[a.clone(), a.clone()], &[a.clone()]));
    assert!(!in_object_list(true, &[a.clone(), b.clone()], &[a.clone()]));
    // Nothing to test, or an empty list: not found.
    assert!(!in_object_list(false, &[a.clone()], &[]));
    assert!(!in_object_list(false, &[], &[a]));
}

/// Object-list graph: Modify (Add / Remove / Empty) and IsIn / Access on one list variable.
struct Lists {
    k: Kismet,
    list: OpId,
    objref: OpId,
    count: OpId,
    modify: OpId,
    isin: OpId,
    in_list: OpId,
    not_in_list: OpId,
    access: OpId,
    out_obj: OpId,
}

fn lists() -> Lists {
    let mut g = G::new();
    let list = g.var("SeqVar_ObjectList");
    let objref = g.var("SeqVar_Object");
    let count = g.var("SeqVar_Int");
    let out_obj = g.var("SeqVar_Object");
    let modify = g.op("SeqAct_ModifyObjectList");
    g.inputs(modify, &["Add To List", "Remove From List", "Empty List"]);
    g.outputs(modify, &["Out"]);
    g.vars(modify, "ObjectRef", "", &[objref]);
    g.vars(modify, "ObjectListVar", "", &[list]);
    g.vars(modify, "ListEntriesCount", "ListEntriesCount", &[count]);
    let isin = g.op("SeqAct_IsInObjectList");
    g.inputs(isin, &["Test if in List"]);
    g.outputs(isin, &["In List", "Not in List"]);
    g.vars(isin, "Object(s)ToTest", "", &[objref]);
    g.vars(isin, "ObjectListVar", "", &[list]);
    let (in_list, not_in_list) = (g.sink(), g.sink());
    g.link(isin, 0, in_list, 0);
    g.link(isin, 1, not_in_list, 0);
    let access = g.op("SeqAct_AccessObjectList");
    g.inputs(access, &["Random", "First", "Last", "At Index"]);
    g.outputs(access, &["Out"]);
    g.vars(access, "Object List", "", &[list]);
    g.vars(access, "Index", "ObjectIndex", &[]);
    g.vars(access, "Output Object", "OutputObject", &[out_obj]);
    let mut k = run(g);
    k.set_prop(objref, "ObjValue", Value::Obj(Obj::None));
    Lists { k, list, objref, count, modify, isin, in_list, not_in_list, access, out_obj }
}

impl Lists {
    fn point_at(&mut self, o: Obj) {
        self.k.set_prop(self.objref, "ObjValue", Value::Obj(o));
    }
    fn go(&mut self, op: OpId, input: usize) {
        self.k.activate_input(op, input);
        step(&mut self.k);
    }
}

#[test]
fn modify_object_list_adds_unique_removes_and_empties() {
    let mut l = lists();
    l.point_at(Obj::Actor(5));
    l.go(l.modify, 0);
    assert_eq!(l.k.list_entries(l.list), vec![Obj::Actor(5)]);
    assert_eq!(l.k.prop_int(l.count, "IntValue"), 1, "ListEntriesCount reaches its variable");
    // Adding the same object again keeps one entry (AddUniqueItem).
    l.go(l.modify, 0);
    assert_eq!(l.k.list_entries(l.list), vec![Obj::Actor(5)]);
    l.point_at(Obj::Actor(6));
    l.go(l.modify, 0);
    assert_eq!(l.k.list_entries(l.list), vec![Obj::Actor(5), Obj::Actor(6)]);
    assert_eq!(l.k.prop_int(l.count, "IntValue"), 2);
    l.go(l.modify, 1);
    assert_eq!(l.k.list_entries(l.list), vec![Obj::Actor(5)]);
    l.go(l.modify, 2);
    assert!(l.k.list_entries(l.list).is_empty());
    assert_eq!(l.k.prop_int(l.count, "IntValue"), 0);
    assert_eq!(l.k.coverage.activations.get(&l.modify).copied(), Some(5));
}

#[test]
fn is_in_object_list_arms_found_or_not_found() {
    let mut l = lists();
    l.point_at(Obj::Actor(5));
    l.go(l.isin, 0);
    assert_eq!((ran(&l.k, l.in_list), ran(&l.k, l.not_in_list)), (0, 1));
    l.go(l.modify, 0);
    l.go(l.isin, 0);
    assert_eq!((ran(&l.k, l.in_list), ran(&l.k, l.not_in_list)), (1, 1));
    assert_eq!(l.k.prop_bool(l.isin, "bObjectFound"), true);
}

#[test]
fn access_object_list_picks_first_last_index_and_none() {
    let mut l = lists();
    for n in [4, 5, 6] {
        l.point_at(Obj::Actor(n));
        l.go(l.modify, 0);
    }
    let out = |l: &mut Lists| l.k.objects_of(l.out_obj);
    l.go(l.access, 1);
    assert_eq!(out(&mut l), vec![Obj::Actor(4)]);
    l.go(l.access, 2);
    assert_eq!(out(&mut l), vec![Obj::Actor(6)]);
    l.k.set_prop(l.access, "ObjectIndex", Value::Int(1));
    l.go(l.access, 3);
    assert_eq!(out(&mut l), vec![Obj::Actor(5)]);
    // Out of range: no object.
    l.k.set_prop(l.access, "ObjectIndex", Value::Int(3));
    l.go(l.access, 3);
    assert!(out(&mut l).is_empty());
    // Random picks one of the entries.
    l.go(l.access, 0);
    let got = out(&mut l);
    assert_eq!(got.len(), 1);
    assert!([Obj::Actor(4), Obj::Actor(5), Obj::Actor(6)].contains(&got[0]));
    // An empty list gives none.
    l.go(l.modify, 2);
    l.go(l.access, 1);
    assert!(out(&mut l).is_empty());
}

#[test]
fn external_variable_stands_for_the_sequence_link_it_is_labelled_with() {
    let mut g = G::new();
    let value = g.var("SeqVar_Object");
    let sub = g.add("Sequence", Kind::Sequence, Some(0));
    g.vars(sub, "Actor A", "", &[value]);
    let ext = g.add("SeqVar_External", Kind::Variable, Some(sub));
    g.set(ext, "VariableLabel", Value::Str("Actor A".into()));
    let user = g.add("SeqAct_Log", Kind::Action, Some(sub));
    g.vars(user, "Objects", "", &[ext]);
    // An external nobody connected stays itself.
    let loose = g.add("SeqVar_External", Kind::Variable, Some(sub));
    g.set(loose, "VariableLabel", Value::Str("Nothing".into()));
    g.vars(user, "Loose", "", &[loose]);
    let mut k = run(g);
    k.set_prop(value, "ObjValue", Value::Obj(Obj::Actor(9)));
    assert_eq!(k.objects(user, "Objects"), vec![Obj::Actor(9)]);
    assert_eq!(k.link_vars(user, "Objects"), vec![value]);
    assert_eq!(k.link_vars(user, "Loose"), vec![loose]);
}
