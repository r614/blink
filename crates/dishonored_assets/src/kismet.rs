//! Reader for the original mission logic: UE3 Kismet sequences (`Sequence`, `SeqEvent_*`,
//! `SeqAct_*`, `SeqCond_*`, `SeqVar_*` plus Arkane's `DisSeq*` nodes) and Arkane's objective
//! data (`DishonoredObjective` with `DishonoredTask_Custom` tasks). Research and inspiration
//! for the custom mission framework; nothing here is executed.

use ue3_package::properties::{Property, Value, find, format_value};
use ue3_package::{ObjRef, Package};

/// One Kismet node.
#[derive(Debug, Clone)]
pub struct KismetOp {
    pub export: usize,
    pub path: String,
    pub class: String,
    /// Enclosing sequence path (e.g. `TheWorld.PersistentLevel.Main_Sequence.Pistols_in_the_Yard`).
    pub sequence: String,
    pub comment: Option<String>,
    /// Gameplay properties (editor layout and link plumbing removed), formatted.
    pub props: Vec<(String, String)>,
    /// Output links: (description, [(target export, target input index)]).
    pub outputs: Vec<(String, Vec<(usize, i32)>)>,
    /// Variable links: (description, [variable exports]).
    pub variables: Vec<(String, Vec<usize>)>,
}

impl KismetOp {
    pub fn kind(&self) -> &'static str {
        let c = self.class.as_str();
        if c == "Sequence" || c == "SequenceFrame" || c == "SequenceFrameWrapped" {
            "sequence"
        } else if c.contains("SeqEvent") {
            "event"
        } else if c.contains("SeqCond") {
            "condition"
        } else if c.contains("SeqVar") {
            "variable"
        } else if c.contains("SeqAct") {
            "action"
        } else {
            "other"
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KismetGraph {
    pub ops: Vec<KismetOp>,
}

impl KismetGraph {
    pub fn op(&self, export: usize) -> Option<&KismetOp> {
        self.ops.iter().find(|o| o.export == export)
    }
}

#[derive(Debug, Clone)]
pub struct OriginalTask {
    pub name: String,
    pub description: String,
    pub initial_state: String,
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub struct OriginalObjective {
    pub path: String,
    pub description: String,
    pub tasks: Vec<OriginalTask>,
}

const EDITOR_PROPS: &[&str] = &[
    "ObjPosX", "ObjPosY", "DrawWidth", "DrawHeight", "ObjComment", "OutputLinks", "InputLinks", "VariableLinks", "EventLinks", "ParentSequence", "ObjInstanceVersion",
    "ObjName", "ObjColor", "bDrawFirst", "bDrawLast", "bOutputObjCommentToScreen", "bSuppressAutoComment", "SearchName", "SequenceObjects", "SelectedSequenceObjects", "DefaultViewX",
    "DefaultViewY", "DefaultViewZoom", "bAutoActivateOutputLinks", "DrawY", "VarName", "bHidden", "FindVarName", "PinnedLinks", "ReferencedObjects",
];

fn is_kismet_class(c: &str) -> bool {
    c.starts_with("Seq") || c.starts_with("DisSeq") || c.starts_with("Sequence") || c.starts_with("DisConv_Kismet")
}

fn linked_op(pkg: &Package, props: &[Property]) -> Option<usize> {
    match find(props, "LinkedOp").map(|p| &p.value) {
        Some(Value::Object(r)) => match ObjRef::from_index(*r) {
            ObjRef::Export(i) => Some(i),
            _ => None,
        },
        _ => {
            let _ = pkg;
            None
        }
    }
}

fn str_prop(props: &[Property], name: &str) -> Option<String> {
    match find(props, name).map(|p| &p.value) {
        Some(Value::Str(s)) => Some(s.clone()),
        Some(Value::Name(s)) if s != "None" => Some(s.clone()),
        _ => None,
    }
}

/// Read every Kismet node of a package (all sequences, all levels in the package).
pub fn read_kismet(pkg: &Package) -> KismetGraph {
    let mut ops = Vec::new();
    for i in 0..pkg.exports.len() {
        let class = pkg.export_class(i);
        if !is_kismet_class(&class) {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        let path = pkg.obj_path(ObjRef::Export(i));
        let sequence = path.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_default();
        let mut op = KismetOp { export: i, path, class: class.to_string(), sequence, comment: str_prop(&h.properties, "ObjComment").filter(|s| !s.is_empty()), props: Vec::new(), outputs: Vec::new(), variables: Vec::new() };
        if let Some(p) = find(&h.properties, "OutputLinks") {
            for link in p.array_structs(pkg).unwrap_or_default() {
                let desc = str_prop(&link, "LinkDesc").unwrap_or_default();
                let mut targets = Vec::new();
                if let Some(l) = find(&link, "Links") {
                    for t in l.array_structs(pkg).unwrap_or_default() {
                        if let Some(op) = linked_op(pkg, &t) {
                            let idx = match find(&t, "InputLinkIdx").map(|p| &p.value) {
                                Some(Value::Int(v)) => *v,
                                _ => 0,
                            };
                            targets.push((op, idx));
                        }
                    }
                }
                op.outputs.push((desc, targets));
            }
        }
        if let Some(p) = find(&h.properties, "VariableLinks") {
            for link in p.array_structs(pkg).unwrap_or_default() {
                let desc = str_prop(&link, "LinkDesc").unwrap_or_default();
                let vars: Vec<usize> = find(&link, "LinkedVariables")
                    .and_then(|v| v.array_objects().ok())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|r| match ObjRef::from_index(r) {
                        ObjRef::Export(i) => Some(i),
                        _ => None,
                    })
                    .collect();
                op.variables.push((desc, vars));
            }
        }
        for p in &h.properties {
            if EDITOR_PROPS.contains(&p.name.as_str()) {
                continue;
            }
            if matches!(p.value, Value::Struct(..)) {
                continue;
            }
            let v = format_value(pkg, &p.value);
            if v.len() < 200 {
                op.props.push((p.name.clone(), v));
            }
        }
        ops.push(op);
    }
    KismetGraph { ops }
}

/// Arkane objective data (`DishonoredObjective` -> `m_Tasks` of `DishonoredTask_Custom`).
pub fn read_objectives(pkg: &Package) -> Vec<OriginalObjective> {
    let mut out = Vec::new();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "DishonoredObjective" {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        let path = pkg.obj_path(ObjRef::Export(i));
        let mut obj = OriginalObjective { path: path.clone(), description: str_prop(&h.properties, "m_Description").unwrap_or_default(), tasks: Vec::new() };
        // Tasks are subobjects named `<objective>.DishonoredTask_Custom_N`.
        let prefix = format!("{path}.");
        let mut tasks: Vec<(u32, OriginalTask)> = Vec::new();
        for t in 0..pkg.exports.len() {
            let tp = pkg.obj_path(ObjRef::Export(t));
            if pkg.export_class(t) != "DishonoredTask_Custom" || !tp.starts_with(&prefix) {
                continue;
            }
            let Ok(th) = pkg.read_object(t) else { continue };
            let name = tp[prefix.len()..].to_string();
            let n = name.rsplit('_').next().and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
            tasks.push((
                n,
                OriginalTask {
                    name,
                    description: str_prop(&th.properties, "m_Description").unwrap_or_default(),
                    initial_state: match find(&th.properties, "m_InitialState").map(|p| &p.value) {
                        Some(Value::Enum(e)) => e.clone(),
                        _ => "DTS_Active".into(),
                    },
                    hidden: matches!(find(&th.properties, "m_bInitiallyHidden").map(|p| &p.value), Some(Value::Bool(true))),
                },
            ));
        }
        tasks.sort_by_key(|t| t.0);
        obj.tasks = tasks.into_iter().map(|t| t.1).collect();
        out.push(obj);
    }
    out
}
