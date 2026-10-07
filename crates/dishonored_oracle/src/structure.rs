//! Structural diff: the primary equivalence proof of a system. The original's live objects
//! (from the hook's reflection dump, phase 2) and the port's view of the same objects are
//! compared field by field: collision flags, trace channels, shapes, transforms, AI settings...
//! Queries and traces then sample behaviour on top of a structure already shown equal.
//!
//! Both sides are a [`StructureDump`]: objects keyed by a stable path (`<package>:<actor>` or
//! `<package>:<actor>.<component>`, the names the probe and the port share), each with named
//! fields. The port side names its fields after the original's properties (`CollideActors`,
//! `BlockRigidBody`, `m_CollisionTraceTypes`...) so the diff needs no mapping table; a field the
//! port does not model is simply absent and reported as such, never guessed.
//!
//! Stored in the oracle cache under [`crate::cache::Probe::State`] (original side).

use crate::report::{Table, WorkItem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Vectors, rotators, matrices, shape parameters.
    Floats(Vec<f64>),
    Str(String),
    /// Object references by stable path.
    Ref(String),
}

impl FieldValue {
    /// Equal within `tol` (floats) or exactly.
    pub fn same(&self, other: &FieldValue, tol: f64) -> bool {
        match (self, other) {
            (FieldValue::Float(a), FieldValue::Float(b)) => (a - b).abs() <= tol,
            (FieldValue::Floats(a), FieldValue::Floats(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol),
            (FieldValue::Int(a), FieldValue::Float(b)) | (FieldValue::Float(b), FieldValue::Int(a)) => (*a as f64 - b).abs() <= tol,
            (FieldValue::Str(a), FieldValue::Str(b)) | (FieldValue::Ref(a), FieldValue::Ref(b)) => a.eq_ignore_ascii_case(b),
            (a, b) => a == b,
        }
    }
    pub fn text(&self) -> String {
        match self {
            FieldValue::Bool(b) => b.to_string(),
            FieldValue::Int(i) => format!("{i} ({i:#x})"),
            FieldValue::Float(f) => format!("{f:.4}"),
            FieldValue::Floats(v) => format!("[{}]", v.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(", ")),
            FieldValue::Str(s) => s.clone(),
            FieldValue::Ref(s) => format!("-> {s}"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ObjectRecord {
    pub class: String,
    pub fields: BTreeMap<String, FieldValue>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StructureDump {
    /// `collision`, `ai`, `kismet`...: which system's fields the dump holds.
    pub system: String,
    pub side: String,
    pub objects: BTreeMap<String, ObjectRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldDiff {
    Differs { original: FieldValue, port: FieldValue },
    /// The port does not model the field.
    MissingInPort(FieldValue),
    /// The port has a field the original's dump lacks (a port-side name or a dump gap).
    ExtraInPort(FieldValue),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StructureDiff {
    pub objects_compared: u64,
    pub fields_compared: u64,
    pub fields_equal: u64,
    pub only_original: Vec<String>,
    pub only_port: Vec<String>,
    /// (object, field) -> difference.
    pub diffs: BTreeMap<(String, String), FieldDiff>,
}

impl StructureDiff {
    pub fn rate(&self) -> f64 {
        if self.fields_compared == 0 { 1.0 } else { self.fields_equal as f64 / self.fields_compared as f64 }
    }
}

/// Compare two dumps; `tol(field)` is the float tolerance per field name.
pub fn diff(orig: &StructureDump, port: &StructureDump, tol: &dyn Fn(&str) -> f64) -> StructureDiff {
    let mut d = StructureDiff::default();
    for (path, o) in &orig.objects {
        let Some(p) = port.objects.get(path) else {
            d.only_original.push(path.clone());
            continue;
        };
        d.objects_compared += 1;
        for (f, ov) in &o.fields {
            d.fields_compared += 1;
            match p.fields.get(f) {
                Some(pv) if ov.same(pv, tol(f)) => d.fields_equal += 1,
                Some(pv) => {
                    d.diffs.insert((path.clone(), f.clone()), FieldDiff::Differs { original: ov.clone(), port: pv.clone() });
                }
                None => {
                    d.diffs.insert((path.clone(), f.clone()), FieldDiff::MissingInPort(ov.clone()));
                }
            }
        }
        for (f, pv) in &p.fields {
            if !o.fields.contains_key(f) {
                d.diffs.insert((path.clone(), f.clone()), FieldDiff::ExtraInPort(pv.clone()));
            }
        }
    }
    d.only_port = port.objects.keys().filter(|k| !orig.objects.contains_key(*k)).cloned().collect();
    d
}

/// Work items: one per (class, field) with the objects that differ, so a rule-level difference
/// (every `BlockingVolume` lacks a trace channel) is one item, not hundreds.
pub fn work_items(subject: &str, orig: &StructureDump, d: &StructureDiff, examples: usize) -> Vec<WorkItem> {
    let mut groups: BTreeMap<(String, String, &'static str), Vec<(&String, &FieldDiff)>> = BTreeMap::new();
    for ((path, field), fd) in &d.diffs {
        let class = orig.objects.get(path).map(|o| o.class.clone()).unwrap_or_default();
        let kind = match fd {
            FieldDiff::Differs { .. } => "differs",
            FieldDiff::MissingInPort(_) => "missing in port",
            FieldDiff::ExtraInPort(_) => "extra in port",
        };
        groups.entry((class, field.clone(), kind)).or_default().push((path, fd));
    }
    let mut out = Vec::new();
    for ((class, field, kind), v) in groups {
        let mut ex = String::from("| object | original | port |\n|---|---|---|\n");
        for (path, fd) in v.iter().take(examples) {
            let (a, b) = match fd {
                FieldDiff::Differs { original, port } => (original.text(), port.text()),
                FieldDiff::MissingInPort(o) => (o.text(), "(absent)".into()),
                FieldDiff::ExtraInPort(p) => ("(absent)".into(), p.text()),
            };
            ex.push_str(&format!("| {path} | {a} | {b} |\n"));
        }
        out.push(WorkItem {
            id: format!("structure:{subject}:{class}.{field}:{kind}"),
            title: format!("Structure: `{class}.{field}` {kind} ({} objects)", v.len()),
            probe: "structure".into(),
            system: orig.system.clone(),
            count: v.len() as u64,
            impact: 5.0 * v.len() as f64,
            maps: vec![subject.to_string()],
            evidence: vec![format!("{} of the {} compared objects", v.len(), d.objects_compared), ex],
            repro: vec![format!("oracle structure --map {subject}")],
            code: Vec::new(),
            cause: None,
        });
    }
    out.sort_by(|a, b| b.impact.total_cmp(&a.impact).then(a.id.cmp(&b.id)));
    out
}

pub fn table(subject: &str, d: &StructureDiff) -> Table {
    let mut t = Table::new(&format!("{subject}: structural diff"), &["objects compared", "only original", "only port", "fields compared", "equal", "rate"]);
    t.row(vec![d.objects_compared.to_string(), d.only_original.len().to_string(), d.only_port.len().to_string(), d.fields_compared.to_string(), d.fields_equal.to_string(), format!("{:.2} %", 100.0 * d.rate())]);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(side: &str, objs: &[(&str, &str, &[(&str, FieldValue)])]) -> StructureDump {
        StructureDump {
            system: "collision".into(),
            side: side.into(),
            objects: objs.iter().map(|(p, c, f)| (p.to_string(), ObjectRecord { class: c.to_string(), fields: f.iter().map(|(k, v)| (k.to_string(), v.clone())).collect() })).collect(),
        }
    }

    #[test]
    fn field_by_field_and_grouped_items() {
        use FieldValue::*;
        let o = dump(
            "original",
            &[
                ("L:Vol_1", "BlockingVolume", &[("CollideActors", Bool(true)), ("TraceMask", Int(0x20df)), ("Location", Floats(vec![1.0, 2.0, 3.0]))]),
                ("L:Vol_2", "BlockingVolume", &[("CollideActors", Bool(true)), ("TraceMask", Int(0x20df))]),
                ("L:Gone", "StaticMeshActor", &[]),
            ],
        );
        let p = dump(
            "port",
            &[
                ("L:Vol_1", "BlockingVolume", &[("CollideActors", Bool(true)), ("Location", Floats(vec![1.0, 2.0, 3.001])), ("Channels", Int(1))]),
                ("L:Vol_2", "BlockingVolume", &[("CollideActors", Bool(false))]),
                ("L:Extra", "X", &[]),
            ],
        );
        let d = diff(&o, &p, &|_| 0.01);
        assert_eq!((d.objects_compared, d.fields_compared, d.fields_equal), (2, 5, 2));
        assert_eq!(d.only_original, vec!["L:Gone".to_string()]);
        assert_eq!(d.only_port, vec!["L:Extra".to_string()]);
        assert!(matches!(d.diffs[&("L:Vol_2".to_string(), "CollideActors".to_string())], FieldDiff::Differs { .. }));
        let items = work_items("M", &o, &d, 3);
        // TraceMask missing on both volumes: one item of two objects, ranked first.
        assert_eq!(items[0].title, "Structure: `BlockingVolume.TraceMask` missing in port (2 objects)");
        assert_eq!(items.len(), 3);
    }
}
