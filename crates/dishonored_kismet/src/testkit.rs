//! Hand-built Kismet graphs for tests (here and in the game crates): a one-package database
//! whose package is the always-loaded global one, with a root sequence (op 0). No retail data.

use crate::model::*;

pub struct GraphBuilder {
    pub db: KismetDb,
}

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphBuilder {
    pub fn new() -> Self {
        let mut db = KismetDb { packages: vec!["T".into()], global: vec![0], roots: vec![vec![0]], ..Default::default() };
        let chain = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        db.classes.insert("SeqVar_ObjectList".into(), chain(&["SeqVar_ObjectList", "SeqVar_Object", "SequenceVariable", "SequenceObject", "Object"]));
        db.classes.insert("SeqVar_Object".into(), chain(&["SeqVar_Object", "SequenceVariable", "SequenceObject", "Object"]));
        db.classes.insert("SeqVar_Int".into(), chain(&["SeqVar_Int", "SequenceVariable", "SequenceObject", "Object"]));
        db.classes.insert("SeqVar_Float".into(), chain(&["SeqVar_Float", "SequenceVariable", "SequenceObject", "Object"]));
        db.classes.insert("SeqVar_Vector".into(), chain(&["SeqVar_Vector", "SequenceVariable", "SequenceObject", "Object"]));
        db.classes.insert("SeqVar_Player".into(), chain(&["SeqVar_Player", "SeqVar_Object", "SequenceVariable", "SequenceObject", "Object"]));
        db.classes.insert("SeqEvent_RemoteEvent".into(), chain(&["SeqEvent_RemoteEvent", "SequenceEvent", "SequenceOp", "SequenceObject", "Object"]));
        let mut g = GraphBuilder { db };
        g.add("Sequence", Kind::Sequence, None);
        g
    }

    /// An op of the given class and kind under `parent`; returns its id.
    pub fn add(&mut self, class: &str, kind: Kind, parent: Option<OpId>) -> OpId {
        let id = self.db.ops.len() as OpId;
        let mut props = Props::default();
        if kind == Kind::Action || kind == Kind::Condition {
            props.set("bAutoActivateOutputLinks", Value::Bool(true));
        }
        self.db.ops.push(Op {
            id,
            package: 0,
            export: id,
            name: format!("{class}_{id}"),
            path: String::new(),
            class: class.into(),
            kind,
            parent,
            comment: String::new(),
            props,
            inputs: Vec::new(),
            outputs: Vec::new(),
            vars: Vec::new(),
            events: Vec::new(),
            children: Vec::new(),
            named: None,
            interp: None,
        });
        if let Some(p) = parent {
            self.db.ops[p as usize].children.push(id);
        }
        id
    }

    /// An action in the root sequence.
    pub fn op(&mut self, class: &str) -> OpId {
        self.add(class, Kind::Action, Some(0))
    }

    /// A condition in the root sequence.
    pub fn cond(&mut self, class: &str) -> OpId {
        self.add(class, Kind::Condition, Some(0))
    }

    /// A variable in the root sequence.
    pub fn var(&mut self, class: &str) -> OpId {
        self.add(class, Kind::Variable, Some(0))
    }

    pub fn inputs(&mut self, op: OpId, names: &[&str]) {
        self.db.ops[op as usize].inputs = names.iter().map(|n| InputLink { name: n.to_string(), ..Default::default() }).collect();
    }

    pub fn outputs(&mut self, op: OpId, names: &[&str]) {
        self.db.ops[op as usize].outputs = names.iter().map(|n| OutputLink { name: n.to_string(), ..Default::default() }).collect();
    }

    /// Output `out` of `from` leads to input `input` of `to`.
    pub fn link(&mut self, from: OpId, out: usize, to: OpId, input: u16) {
        self.db.ops[from as usize].outputs[out].links.push((to, input));
    }

    /// A variable link of an op (`property` empty for none).
    pub fn vars(&mut self, op: OpId, link: &str, property: &str, vars: &[OpId]) {
        self.db.ops[op as usize].vars.push(VarLink { name: link.into(), vars: vars.to_vec(), property: property.into(), writeable: false, expected: String::new() });
    }

    pub fn set(&mut self, op: OpId, name: &str, v: Value) {
        self.db.ops[op as usize].props.set(name, v);
    }

    /// A logging action with an input and an output: a sink the coverage counts when it runs.
    pub fn sink(&mut self) -> OpId {
        let s = self.op("SeqAct_Log");
        self.inputs(s, &["In"]);
        self.outputs(s, &["Out"]);
        s
    }

    /// A `SeqEvent_RemoteEvent` named `name` in the root sequence with one output.
    pub fn remote_event(&mut self, name: &str) -> OpId {
        let e = self.add("SeqEvent_RemoteEvent", Kind::Event, Some(0));
        self.set(e, "EventName", Value::Str(name.into()));
        self.outputs(e, &["Out"]);
        e
    }

    /// A level actor.
    pub fn actor(&mut self, class: &str, name: &str, location: [f32; 3], yaw: i32) -> u32 {
        self.db.actors.push(Actor {
            package: "T".into(),
            name: name.into(),
            class: class.into(),
            location,
            rotation: [0, yaw, 0],
            scale: [1.0; 3],
            tag: String::new(),
            shape: None,
            props: Props::default(),
            bones: Vec::new(),
        });
        self.db.actors.len() as u32 - 1
    }
}
