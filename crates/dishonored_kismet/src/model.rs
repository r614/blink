//! Engine-independent form of a level's Kismet.
//!
//! UE3 Kismet (`Sequence`, `SequenceOp` = events, actions and conditions, `SequenceVariable`)
//! as cooked in Dishonored's level packages, plus the always-loaded global sequence of
//! `DishonoredGameFull_P`. Every object keeps its original package, export index and object
//! path; properties are typed and complete (cooked objects omit values equal to their class
//! default object, so the loader merges the `Default__<Class>` chain read from the engine
//! packages); links are resolved to op ids; references to level actors are resolved to
//! [`Actor`]s with their transform and, for trigger volumes, their touch shape.
//!
//! Text the game shows (task text, HUD messages) is read from the install at runtime and is
//! never written to the repository.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type OpId = u32;

/// An object reference.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Obj {
    #[default]
    None,
    /// A Kismet object of the database (op, variable, sequence).
    Op(OpId),
    /// A level actor (index into [`KismetDb::actors`]).
    Actor(u32),
    /// Any other object by full path (`Boyle_Objectives.BoyleAssassination`,
    /// `Npc_Boyle.Materials.Boyle_red_inst`). Objects of a level package that are neither
    /// Kismet objects nor actors are written `Package:TheWorld.PersistentLevel...`.
    Path(String),
    /// Runtime only: a pawn of the simulation (player or NPC id).
    Pawn(u32),
}

impl Obj {
    pub fn is_none(&self) -> bool {
        matches!(self, Obj::None)
    }
}

/// A typed property value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Float(f32),
    Byte(u8),
    Enum(String),
    Name(String),
    Str(String),
    Obj(Obj),
    Vector([f32; 3]),
    Rotator([i32; 3]),
    Guid([u32; 4]),
    /// Struct name and fields.
    Struct(String, Vec<(String, Value)>),
    Array(Vec<Value>),
    /// Not decodable without the owning class layout (kept for diagnostics).
    Raw(String),
}

impl Value {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(i) => Some(*i != 0),
            Value::Byte(b) => Some(*b != 0),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i32> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Byte(b) => Some(*b as i32),
            Value::Bool(b) => Some(*b as i32),
            Value::Float(f) => Some(*f as i32),
            _ => None,
        }
    }
    pub fn as_float(&self) -> Option<f32> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f32),
            Value::Byte(b) => Some(*b as f32),
            _ => None,
        }
    }
    /// Names, strings and enum values.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Name(s) | Value::Str(s) | Value::Enum(s) => Some(s.as_str()),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&Obj> {
        match self {
            Value::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_array(&self) -> &[Value] {
        match self {
            Value::Array(a) => a,
            _ => &[],
        }
    }
    pub fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Struct(_, f) => f.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Property list (original names, case-insensitive lookup).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Props(pub Vec<(String, Value)>);

impl Props {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
    }
    pub fn set(&mut self, name: &str, v: Value) {
        match self.0.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            Some(e) => e.1 = v,
            None => self.0.push((name.to_string(), v)),
        }
    }
    pub fn bool(&self, name: &str) -> bool {
        self.get(name).and_then(Value::as_bool).unwrap_or(false)
    }
    pub fn int(&self, name: &str) -> i32 {
        self.get(name).and_then(Value::as_int).unwrap_or(0)
    }
    pub fn float(&self, name: &str) -> f32 {
        self.get(name).and_then(Value::as_float).unwrap_or(0.0)
    }
    pub fn str(&self, name: &str) -> &str {
        self.get(name).and_then(Value::as_str).unwrap_or("")
    }
    pub fn obj(&self, name: &str) -> Obj {
        self.get(name).and_then(Value::as_obj).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    Sequence,
    Event,
    Action,
    Condition,
    Variable,
    Other,
}

/// `FSeqOpInputLink`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct InputLink {
    pub name: String,
    /// Sequences: the `SeqEvent_SequenceActivated` this input starts.
    pub linked_op: Option<OpId>,
    pub disabled: bool,
    pub delay: f32,
}

/// `FSeqOpOutputLink`: targets are (op, input index).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputLink {
    pub name: String,
    pub links: Vec<(OpId, u16)>,
    pub disabled: bool,
    pub delay: f32,
    /// Sequences: the `SeqAct_FinishSequence` that fires this output.
    pub linked_op: Option<OpId>,
}

/// `FSeqVarLink`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VarLink {
    pub name: String,
    pub vars: Vec<OpId>,
    /// Property of the op this link reads/writes (`PropertyName`), empty if none.
    pub property: String,
    pub writeable: bool,
    /// `ExpectedType` class name.
    pub expected: String,
}

/// `FSeqEventLink`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EventLink {
    pub name: String,
    pub events: Vec<OpId>,
}

/// `InterpData` summary: length and the event track keys (each key name is also an output
/// link of the `SeqAct_Interp` playing it).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct InterpInfo {
    pub length: f32,
    /// (time, event name, fire when playing forwards, fire when playing backwards)
    pub events: Vec<(f32, String, bool, bool)>,
    /// [interp] The whole matinee (groups and tracks), when it could be read.
    #[serde(default)]
    pub matinee: Option<Box<crate::interp::Matinee>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Op {
    pub id: OpId,
    /// Index into [`KismetDb::packages`].
    pub package: u16,
    pub export: u32,
    pub name: String,
    /// Object path inside its package (`TheWorld.PersistentLevel.Main_Sequence.SeqAct_Gate_3`).
    pub path: String,
    pub class: String,
    pub kind: Kind,
    /// Enclosing sequence.
    pub parent: Option<OpId>,
    pub comment: String,
    /// Gameplay properties, class defaults merged in; link plumbing and editor layout removed.
    pub props: Props,
    pub inputs: Vec<InputLink>,
    pub outputs: Vec<OutputLink>,
    pub vars: Vec<VarLink>,
    pub events: Vec<EventLink>,
    /// Sequences: their Kismet objects in `SequenceObjects` order.
    pub children: Vec<OpId>,
    /// `SeqVar_Named`: the variable it stands for (resolved like the engine at level start).
    pub named: Option<OpId>,
    /// `InterpData`.
    pub interp: Option<InterpInfo>,
}

impl Op {
    pub fn output(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|o| o.name.eq_ignore_ascii_case(name))
    }
    pub fn input(&self, name: &str) -> Option<usize> {
        self.inputs.iter().position(|o| o.name.eq_ignore_ascii_case(name))
    }
    pub fn var_link(&self, name: &str) -> Option<&VarLink> {
        self.vars.iter().find(|v| v.name.eq_ignore_ascii_case(name))
    }
}

/// Touch shape of a trigger actor, in the actor's local frame (scaled, rotated by yaw, then
/// translated to `location`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Shape {
    /// `CylinderComponent` (Trigger, DisTrigger): radius, half height.
    Cylinder { radius: f32, height: f32 },
    /// Brush volumes: the convex pieces' boxes (`BrushComponent.BrushAggGeom.ConvexElems[].ElemBox`).
    Boxes(Vec<[[f32; 3]; 2]>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Actor {
    pub package: String,
    pub name: String,
    pub class: String,
    pub location: [f32; 3],
    /// Pitch, yaw, roll (65536 = 360 degrees).
    pub rotation: [i32; 3],
    pub scale: [f32; 3],
    pub tag: String,
    pub shape: Option<Shape>,
    /// Scalar gameplay properties of the actor (`m_pAbstractItem`, `bHidden`, ...).
    pub props: Props,
    /// [interp] Skeletal mesh actors: reference-pose bones in actor space (name, position,
    /// rotation quaternion xyzw), for matinee attachments to a bone.
    #[serde(default)]
    pub bones: Vec<(String, [f32; 3], [f32; 4])>,
}

impl Actor {
    pub fn yaw_deg(&self) -> f32 {
        self.rotation[1] as f32 * 360.0 / 65536.0
    }

    /// Whether a point (a pawn's centre, with `radius`/`half_height` of the pawn's cylinder)
    /// overlaps the touch shape.
    pub fn touches(&self, p: [f32; 3], radius: f32, half_height: f32) -> bool {
        let d = [p[0] - self.location[0], p[1] - self.location[1], p[2] - self.location[2]];
        match &self.shape {
            Some(Shape::Cylinder { radius: r, height: h }) => (d[0] * d[0] + d[1] * d[1]).sqrt() <= r + radius && d[2].abs() <= h + half_height,
            Some(Shape::Boxes(boxes)) => {
                // Into the local frame: undo yaw, then scale.
                let yaw = -(self.rotation[1] as f32) * std::f32::consts::TAU / 65536.0;
                let (s, c) = yaw.sin_cos();
                let l = [d[0] * c - d[1] * s, d[0] * s + d[1] * c, d[2]];
                let sc = [self.scale[0].abs().max(1e-3), self.scale[1].abs().max(1e-3), self.scale[2].abs().max(1e-3)];
                let pad = [radius / sc[0], radius / sc[1], half_height / sc[2]];
                let l = [l[0] / sc[0], l[1] / sc[1], l[2] / sc[2]];
                boxes.iter().any(|b| (0..3).all(|i| l[i] >= b[0][i] - pad[i] && l[i] <= b[1][i] + pad[i]))
            }
            None => false,
        }
    }
}

/// `DishonoredTask_Custom`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Task {
    /// `DishonoredTask_Custom_13`.
    pub name: String,
    /// Localized `m_Description`.
    pub description: String,
    /// `DTS_Inactive` .. `DTS_Failed`.
    pub initial: String,
    pub hidden: bool,
}

/// `DishonoredObjective`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Objective {
    /// `Boyle_Objectives.BoyleAssassination`.
    pub path: String,
    pub description: String,
    pub tasks: Vec<Task>,
    /// `m_bCompleteWhenAllTasksComplete`.
    #[serde(default)]
    pub complete_when_all_tasks: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct KismetDb {
    pub packages: Vec<String>,
    /// Packages always loaded (the global game sequence of `DishonoredGameFull_P`).
    pub global: Vec<u16>,
    /// Persistent maps and their packages (persistent level first, then its sublevels).
    pub maps: Vec<(String, Vec<u16>)>,
    pub ops: Vec<Op>,
    pub actors: Vec<Actor>,
    /// Top-level sequences of each package (index = package).
    pub roots: Vec<Vec<OpId>>,
    /// Class -> its class chain (itself first, up to `Object`), for every Kismet class used.
    pub classes: BTreeMap<String, Vec<String>>,
    pub objectives: Vec<Objective>,
}

impl KismetDb {
    #[track_caller]
    pub fn op(&self, id: OpId) -> &Op {
        &self.ops[id as usize]
    }

    /// `IsA` over the original class chain.
    pub fn is_a(&self, class: &str, ancestor: &str) -> bool {
        match self.classes.get(class) {
            Some(chain) => chain.iter().any(|c| c.eq_ignore_ascii_case(ancestor)),
            None => class.eq_ignore_ascii_case(ancestor),
        }
    }

    pub fn op_is_a(&self, id: OpId, ancestor: &str) -> bool {
        self.is_a(&self.ops[id as usize].class, ancestor)
    }

    /// Op by package and export index.
    pub fn find_export(&self, package: &str, export: u32) -> Option<OpId> {
        let p = self.packages.iter().position(|x| x.eq_ignore_ascii_case(package))? as u16;
        self.ops.iter().find(|o| o.package == p && o.export == export).map(|o| o.id)
    }

    /// Actor by `Name` or `Package:Name`.
    pub fn find_actor(&self, reference: &str) -> Option<u32> {
        let (pkg, name) = match reference.split_once(':') {
            Some((p, n)) => (Some(p), n),
            None => (None, reference),
        };
        self.actors.iter().position(|a| a.name.eq_ignore_ascii_case(name) && pkg.is_none_or(|p| a.package.eq_ignore_ascii_case(p))).map(|i| i as u32)
    }

    pub fn package_name(&self, p: u16) -> &str {
        self.packages.get(p as usize).map(|s| s.as_str()).unwrap_or("")
    }

    /// `Package:ObjectName [export]` for logs.
    pub fn describe(&self, id: OpId) -> String {
        let o = self.op(id);
        format!("{}:{}[{}]", self.package_name(o.package), o.name, o.export)
    }

    /// Map whose packages include package `p`.
    pub fn map_of_package(&self, p: u16) -> Option<&str> {
        self.maps.iter().find(|m| m.1.contains(&p)).map(|m| m.0.as_str())
    }

    /// Op class histogram (classes of ops in the given packages, all if empty).
    pub fn class_counts(&self, packages: &[u16]) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for o in &self.ops {
            if packages.is_empty() || packages.contains(&o.package) {
                *m.entry(o.class.clone()).or_insert(0) += 1;
            }
        }
        m
    }
}
