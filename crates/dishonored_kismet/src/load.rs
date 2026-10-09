//! Reading Kismet from the cooked packages (feature `load`).
//!
//! - Every export whose class derives from `SequenceObject` under `TheWorld` is an op.
//! - Class defaults: cooked objects omit values equal to their archetype, so each op's
//!   properties are its class default object chain (`Default__SequenceOp` ..
//!   `Default__<Class>`, read from `Core`, `Engine`, `GameFramework` and `DishonoredGame`)
//!   overlaid with its own. Link arrays (`InputLinks`, `OutputLinks`, `VariableLinks`,
//!   `EventLinks`) come from the same merge, so ops whose links equal the defaults (a gate's
//!   four inputs) still have them.
//! - `SeqVar_Named` resolves like the engine does at level start: the first variable whose
//!   `VarName` matches, searching the global sequence, then the map's levels in streaming
//!   order, each depth-first in `SequenceObjects` order.
//! - Level actors referenced by Kismet are collected with their transform and touch shape.
//! - Story-flag GUIDs (`m_StoryFlag`) gain their name (`m_StoryFlagName`) from `Startup.upk`.
//! - Localized strings (task text, HUD messages, target names) are read from
//!   `Localization/INT` and replace the package's own text.

use crate::model::*;
use anyhow::{Context, Result};
use dishonored_assets::Content;
use dishonored_assets::localization::Localization;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use ue3_package::properties::{Property, Value as PV, find};
use ue3_package::reader::Reader;
use ue3_package::{ObjRef, Package};

const CLASS_PACKAGES: &[&str] = &["Core", "Engine", "GameFramework", "DishonoredGame"];

/// Properties dropped from ops: link plumbing (decoded separately) and editor layout.
const DROP: &[&str] = &[
    "ObjPosX", "ObjPosY", "DrawWidth", "DrawHeight", "ObjComment", "OutputLinks", "InputLinks", "VariableLinks", "EventLinks", "ParentSequence", "ObjInstanceVersion", "ObjName", "ObjColor",
    "bDrawFirst", "bDrawLast", "bOutputObjCommentToScreen", "bSuppressAutoComment", "SequenceObjects", "SelectedSequenceObjects", "DefaultViewX", "DefaultViewY", "DefaultViewZoom", "DrawY",
    "FindVarName_Editor", "PinnedLinks", "ReferencedObjects", "bDeletedOnLoad", "PlayerIndex", "GamepadID", "SearchName", "bUseCompactMode",
];

/// Arrays of ints (not object references) among Kismet properties.
const INT_ARRAYS: &[&str] = &["Indices", "AutoDisabledIndices"];

/// The engine's class hierarchy and class default objects.
pub struct ClassDb {
    pkgs: Vec<Arc<Package>>,
    /// class name (lowercase) -> (package, export)
    classes: HashMap<String, (usize, usize)>,
    chains: std::sync::Mutex<HashMap<String, Vec<String>>>,
    defaults: std::sync::Mutex<HashMap<String, Arc<Vec<Property>>>>,
    /// full path (lowercase) -> (package, export), built on first use.
    paths: std::sync::OnceLock<HashMap<String, (usize, usize)>>,
}

impl ClassDb {
    pub fn load(content: &Content) -> Result<Self> {
        let mut pkgs = Vec::new();
        let mut classes = HashMap::new();
        for name in CLASS_PACKAGES {
            let pkg = content.package(name).with_context(|| format!("class package {name}"))?;
            let pi = pkgs.len();
            for i in 0..pkg.exports.len() {
                if pkg.exports[i].class == 0 {
                    classes.entry(pkg.obj_name(ObjRef::Export(i)).to_ascii_lowercase()).or_insert((pi, i));
                }
            }
            pkgs.push(pkg);
        }
        Ok(Self { pkgs, classes, chains: Default::default(), defaults: Default::default(), paths: Default::default() })
    }

    /// Class chain, the class itself first (`SeqAct_Gate`, `SequenceAction`, `SequenceOp`, ...).
    pub fn chain(&self, class: &str) -> Vec<String> {
        let key = class.to_ascii_lowercase();
        if let Some(c) = self.chains.lock().unwrap().get(&key) {
            return c.clone();
        }
        let mut out = vec![class.to_string()];
        let mut cur = key.clone();
        for _ in 0..64 {
            let Some(&(pi, ei)) = self.classes.get(&cur) else { break };
            let pkg = &self.pkgs[pi];
            let sup = pkg.exports[ei].super_;
            if sup == 0 {
                break;
            }
            let name = pkg.obj_name(ObjRef::from_index(sup));
            cur = name.to_ascii_lowercase();
            out.push(name);
        }
        self.chains.lock().unwrap().insert(key, out.clone());
        out
    }

    pub fn is_a(&self, class: &str, ancestor: &str) -> bool {
        self.chain(class).iter().any(|c| c.eq_ignore_ascii_case(ancestor))
    }

    /// Package and export of a class default object.
    fn cdo(&self, class: &str) -> Option<(usize, usize)> {
        let &(pi, _) = self.classes.get(&class.to_ascii_lowercase())?;
        let pkg = &self.pkgs[pi];
        let want = format!("Default__{class}");
        (0..pkg.exports.len()).find(|&i| pkg.obj_name(ObjRef::Export(i)).eq_ignore_ascii_case(&want) && pkg.exports[i].outer == 0).map(|i| (pi, i))
    }

    /// Merged class defaults (root class first, each subclass overriding), as the raw
    /// properties of the class default objects with object references rewritten to paths.
    pub fn defaults(&self, class: &str) -> Arc<Vec<Property>> {
        let key = class.to_ascii_lowercase();
        if let Some(d) = self.defaults.lock().unwrap().get(&key) {
            return d.clone();
        }
        let mut merged: Vec<Property> = Vec::new();
        let chain = self.chain(class);
        for c in chain.iter().rev() {
            let Some((pi, ei)) = self.cdo(c) else { continue };
            let Ok(h) = self.pkgs[pi].read_object(ei) else { continue };
            for p in h.properties {
                let p = self.localize_refs(pi, p);
                match merged.iter_mut().find(|m| m.name.eq_ignore_ascii_case(&p.name) && m.array_index == p.array_index) {
                    Some(m) => *m = p,
                    None => merged.push(p),
                }
            }
        }
        let d = Arc::new(merged);
        self.defaults.lock().unwrap().insert(key, d.clone());
        d
    }

    /// Class default objects reference objects of their own package; tag them so values
    /// decode against the right package (`Value::Object` carries the index of the owning
    /// package, so we keep a side marker in the type name).
    fn localize_refs(&self, pi: usize, mut p: Property) -> Property {
        p.type_name = format!("{}@{}", p.type_name, pi);
        p
    }

    /// Exported object by full path in the class packages (for archetypes / templates).
    pub fn find_object(&self, path: &str) -> Option<(Arc<Package>, usize)> {
        let idx = self.paths.get_or_init(|| {
            let mut m = HashMap::new();
            for (pi, pkg) in self.pkgs.iter().enumerate() {
                for i in 0..pkg.exports.len() {
                    m.entry(format!("{}.{}", pkg.name, pkg.obj_path(ObjRef::Export(i))).to_ascii_lowercase()).or_insert((pi, i));
                }
            }
            m
        });
        idx.get(&path.to_ascii_lowercase()).map(|&(pi, i)| (self.pkgs[pi].clone(), i))
    }

    fn pkg(&self, i: usize) -> &Arc<Package> {
        &self.pkgs[i]
    }
}

/// Where a raw property's object references point (its own level package, or a class package
/// for class defaults).
#[derive(Clone, Copy)]
enum Src {
    Level(usize),
    Class(usize),
}

struct Builder<'a> {
    content: &'a Content,
    classes: &'a ClassDb,
    loc: &'a Localization,
    flags: dishonored_conversation::load::StoryFlags,
    db: KismetDb,
    pkgs: Vec<Arc<Package>>,
    /// (package, export) -> op id
    op_ids: HashMap<(usize, usize), OpId>,
    /// (package, export) -> actor index
    actor_ids: HashMap<(usize, usize), u32>,
}

/// Read the Kismet of the global packages and the given maps.
///
/// `maps`: (persistent map name, its packages in streaming order); `global`: always-loaded
/// packages (normally `DishonoredGameFull_P`).
pub fn load(content: &Content, global: &[String], maps: &[(String, Vec<String>)]) -> Result<KismetDb> {
    let classes = ClassDb::load(content)?;
    let loc = Localization::load(content, "INT");
    load_with(content, &classes, &loc, global, maps)
}

pub fn load_with(content: &Content, classes: &ClassDb, loc: &Localization, global: &[String], maps: &[(String, Vec<String>)]) -> Result<KismetDb> {
    let flags = dishonored_conversation::load::StoryFlags::load(&content.cooked).unwrap_or_default();
    let mut b = Builder { content, classes, loc, flags, db: KismetDb::default(), pkgs: Vec::new(), op_ids: HashMap::new(), actor_ids: HashMap::new() };
    let add_pkg = |b: &mut Builder, name: &str| -> Result<Option<u16>> {
        if let Some(i) = b.db.packages.iter().position(|p| p.eq_ignore_ascii_case(name)) {
            return Ok(Some(i as u16));
        }
        if !content.has_package(name) {
            return Ok(None);
        }
        let pkg = content.package(name)?;
        b.db.packages.push(name.to_string());
        b.pkgs.push(pkg);
        Ok(Some((b.db.packages.len() - 1) as u16))
    };
    for g in global {
        if let Some(i) = add_pkg(&mut b, g)? {
            b.db.global.push(i);
        }
    }
    for (m, pkgs) in maps {
        let mut ids = Vec::new();
        for p in pkgs {
            if let Some(i) = add_pkg(&mut b, p)? {
                if !b.db.global.contains(&i) {
                    ids.push(i);
                }
            }
        }
        b.db.maps.push((m.clone(), ids));
    }
    b.collect_ops();
    b.build_ops();
    b.resolve_named();
    b.read_objectives();
    // [interp] Animated attachment bones and AI root tracks of the matinees.
    let src: HashMap<u32, (usize, usize)> = b.actor_ids.iter().map(|(k, v)| (*v, *k)).collect();
    crate::interp_bake::bake(content, &b.pkgs, &mut b.db, &src);
    let mut chains = BTreeMap::new();
    for o in &b.db.ops {
        chains.entry(o.class.clone()).or_insert_with(|| classes.chain(&o.class));
    }
    b.db.classes = chains;
    Ok(b.db)
}

fn is_under_world(pkg: &Package, i: usize) -> bool {
    let mut cur = ObjRef::Export(i);
    let mut top = cur;
    for _ in 0..64 {
        let o = pkg.obj_outer(cur);
        if o == ObjRef::Null {
            break;
        }
        top = o;
        cur = o;
    }
    pkg.obj_name(top) == "TheWorld"
}

fn kind_of(classes: &ClassDb, class: &str) -> Kind {
    let chain = classes.chain(class);
    let has = |c: &str| chain.iter().any(|x| x.eq_ignore_ascii_case(c));
    if has("Sequence") {
        Kind::Sequence
    } else if has("SequenceEvent") {
        Kind::Event
    } else if has("SequenceCondition") {
        Kind::Condition
    } else if has("SequenceAction") {
        Kind::Action
    } else if has("SequenceVariable") {
        Kind::Variable
    } else {
        Kind::Other
    }
}

impl Builder<'_> {
    fn collect_ops(&mut self) {
        for pi in 0..self.pkgs.len() {
            let pkg = self.pkgs[pi].clone();
            for i in 0..pkg.exports.len() {
                if pkg.exports[i].class == 0 {
                    continue;
                }
                let class = pkg.export_class(i);
                if !self.classes.is_a(&class, "SequenceObject") || !is_under_world(&pkg, i) {
                    continue;
                }
                let id = self.db.ops.len() as OpId;
                self.op_ids.insert((pi, i), id);
                let path = pkg.obj_path(ObjRef::Export(i));
                self.db.ops.push(Op {
                    id,
                    package: pi as u16,
                    export: i as u32,
                    name: pkg.obj_name(ObjRef::Export(i)),
                    path,
                    kind: kind_of(self.classes, &class),
                    class,
                    parent: None,
                    comment: String::new(),
                    props: Props::default(),
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    vars: Vec::new(),
                    events: Vec::new(),
                    children: Vec::new(),
                    named: None,
                    interp: None,
                });
            }
        }
        self.db.roots = vec![Vec::new(); self.pkgs.len()];
    }

    fn pkg_of(&self, src: Src) -> Arc<Package> {
        match src {
            Src::Level(i) => self.pkgs[i].clone(),
            Src::Class(i) => self.classes.pkg(i).clone(),
        }
    }

    /// Object reference -> model reference.
    fn obj(&mut self, src: Src, idx: i32) -> Obj {
        let pkg = self.pkg_of(src);
        match ObjRef::from_index(idx) {
            ObjRef::Null => Obj::None,
            ObjRef::Import(_) => Obj::Path(pkg.obj_path(ObjRef::from_index(idx))),
            ObjRef::Export(e) => {
                let Src::Level(pi) = src else { return Obj::Path(format!("{}.{}", pkg.name, pkg.obj_path(ObjRef::Export(e)))) };
                if let Some(&id) = self.op_ids.get(&(pi, e)) {
                    return Obj::Op(id);
                }
                if pkg.obj_name(ObjRef::from_index(pkg.exports[e].outer)) == "PersistentLevel" && pkg.exports[e].class != 0 {
                    return Obj::Actor(self.actor(pi, e));
                }
                if is_under_world(&pkg, e) {
                    Obj::Path(format!("{}:{}", pkg.name, pkg.obj_path(ObjRef::Export(e))))
                } else {
                    Obj::Path(pkg.obj_path(ObjRef::Export(e)))
                }
            }
        }
    }

    fn value(&mut self, src: Src, p: &Property) -> Value {
        let pkg = self.pkg_of(src);
        match &p.value {
            PV::Int(v) => Value::Int(*v),
            PV::Float(v) => Value::Float(*v),
            PV::Bool(v) => Value::Bool(*v),
            PV::Byte(v) => Value::Byte(*v),
            PV::Enum(v) => Value::Enum(v.clone()),
            PV::Name(v) => Value::Name(v.clone()),
            PV::Str(v) => Value::Str(v.clone()),
            PV::Object(i) => {
                // Class references (`ExpectedType`, `ClassProximityTypes`) are kept as names.
                if p.type_name.starts_with("ClassProperty") {
                    return Value::Name(pkg.obj_name(ObjRef::from_index(*i)));
                }
                Value::Obj(self.obj(src, *i))
            }
            PV::Vector(v) => Value::Vector(*v),
            PV::Rotator(v) => Value::Rotator(*v),
            PV::Color(c) => Value::Array(c.iter().map(|x| Value::Byte(*x)).collect()),
            PV::LinearColor(c) => Value::Array(c.iter().map(|x| Value::Float(*x)).collect()),
            PV::Vector2D(v) => Value::Array(v.iter().map(|x| Value::Float(*x)).collect()),
            PV::Guid(g) => Value::Guid(*g),
            PV::Struct(name, fields) => {
                let f = fields.iter().map(|f| (f.name.clone(), self.value(src, f))).collect();
                Value::Struct(name.clone(), f)
            }
            PV::Array(_) => self.array(src, p),
            PV::Raw(t, b) => Value::Raw(format!("{t} {} bytes", b.len())),
        }
    }

    fn array(&mut self, src: Src, p: &Property) -> Value {
        let pkg = self.pkg_of(src);
        let Some((n, body)) = p.array_raw() else { return Value::Array(Vec::new()) };
        if n == 0 {
            return Value::Array(Vec::new());
        }
        if let Ok(elems) = p.array_structs(&pkg) {
            if elems.len() == n && elems.iter().all(|e| !e.is_empty()) {
                let out = elems.iter().map(|e| Value::Struct(String::new(), e.iter().map(|f| (f.name.clone(), self.value(src, f))).collect())).collect();
                return Value::Array(out);
            }
        }
        if body.len() == 4 * n {
            let ints: Vec<i32> = body.chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            if INT_ARRAYS.iter().any(|x| x.eq_ignore_ascii_case(&p.name)) {
                return Value::Array(ints.into_iter().map(Value::Int).collect());
            }
            let valid = ints.iter().all(|&i| match ObjRef::from_index(i) {
                ObjRef::Null => true,
                ObjRef::Export(e) => e < pkg.exports.len(),
                ObjRef::Import(m) => m < pkg.imports.len(),
            });
            if valid {
                return Value::Array(ints.into_iter().map(|i| Value::Obj(self.obj(src, i))).collect());
            }
            return Value::Array(ints.into_iter().map(Value::Int).collect());
        }
        if body.len() == 8 * n {
            let mut r = Reader::new(body);
            let mut out = Vec::new();
            for _ in 0..n {
                match ue3_package::FName::read(&mut r) {
                    Ok(f) if (f.index as usize) < pkg.names.len() => out.push(Value::Name(pkg.name(f))),
                    _ => return Value::Raw(format!("array {n} x 8 bytes")),
                }
            }
            return Value::Array(out);
        }
        Value::Raw(format!("array {n} elems, {} bytes", body.len()))
    }

    /// Instance properties over the class defaults: (src, property) pairs.
    fn merged(&self, pi: usize, class: &str, own: Vec<Property>) -> Vec<(Src, Property)> {
        let mut out: Vec<(Src, Property)> = Vec::new();
        for p in self.classes.defaults(class).iter() {
            let (t, ci) = p.type_name.split_once('@').map(|(t, c)| (t.to_string(), c.parse().unwrap_or(0))).unwrap_or((p.type_name.clone(), 0));
            let mut p = p.clone();
            p.type_name = t;
            out.push((Src::Class(ci), p));
        }
        for p in own {
            match out.iter_mut().find(|m| m.1.name.eq_ignore_ascii_case(&p.name) && m.1.array_index == p.array_index) {
                Some(m) => *m = (Src::Level(pi), p),
                None => out.push((Src::Level(pi), p)),
            }
        }
        out
    }

    fn op_ref(&mut self, src: Src, idx: i32) -> Option<OpId> {
        match self.obj(src, idx) {
            Obj::Op(id) => Some(id),
            _ => None,
        }
    }

    fn build_ops(&mut self) {
        for id in 0..self.db.ops.len() {
            let (pi, ei, class) = {
                let o = &self.db.ops[id];
                (o.package as usize, o.export as usize, o.class.clone())
            };
            let pkg = self.pkgs[pi].clone();
            let own = match pkg.read_object(ei) {
                Ok(h) => h.properties,
                Err(_) => Vec::new(),
            };
            let merged = self.merged(pi, &class, own);
            let mut op = self.db.ops[id].clone();
            for (src, p) in &merged {
                let pk = self.pkg_of(*src);
                match p.name.as_str() {
                    "InputLinks" => {
                        op.inputs = p
                            .array_structs(&pk)
                            .unwrap_or_default()
                            .iter()
                            .map(|l| InputLink {
                                name: str_of(l, "LinkDesc"),
                                linked_op: find(l, "LinkedOp").and_then(|v| v.as_object()).and_then(|i| self.op_ref(*src, i)),
                                disabled: bool_of(l, "bDisabled"),
                                delay: find(l, "ActivateDelay").and_then(|v| v.as_float()).unwrap_or(0.0),
                            })
                            .collect();
                    }
                    "OutputLinks" => {
                        let links = p.array_structs(&pk).unwrap_or_default();
                        op.outputs = links
                            .iter()
                            .map(|l| {
                                let mut targets = Vec::new();
                                if let Some(ls) = find(l, "Links") {
                                    for t in ls.array_structs(&pk).unwrap_or_default() {
                                        let Some(target) = find(&t, "LinkedOp").and_then(|v| v.as_object()).and_then(|i| self.op_ref(*src, i)) else { continue };
                                        let idx = find(&t, "InputLinkIdx").and_then(|v| match v.value {
                                            PV::Int(i) => Some(i),
                                            _ => None,
                                        });
                                        targets.push((target, idx.unwrap_or(0).max(0) as u16));
                                    }
                                }
                                OutputLink {
                                    name: str_of(l, "LinkDesc"),
                                    links: targets,
                                    disabled: bool_of(l, "bDisabled"),
                                    delay: find(l, "ActivateDelay").and_then(|v| v.as_float()).unwrap_or(0.0),
                                    linked_op: find(l, "LinkedOp").and_then(|v| v.as_object()).and_then(|i| self.op_ref(*src, i)),
                                }
                            })
                            .collect();
                    }
                    "VariableLinks" => {
                        let links = p.array_structs(&pk).unwrap_or_default();
                        op.vars = links
                            .iter()
                            .map(|l| VarLink {
                                name: str_of(l, "LinkDesc"),
                                vars: find(l, "LinkedVariables").and_then(|v| v.array_objects().ok()).unwrap_or_default().into_iter().filter_map(|i| self.op_ref(*src, i)).collect(),
                                property: str_of(l, "PropertyName"),
                                writeable: bool_of(l, "bWriteable"),
                                expected: find(l, "ExpectedType").and_then(|v| v.as_object()).map(|i| pk.obj_name(ObjRef::from_index(i))).unwrap_or_default(),
                            })
                            .collect();
                    }
                    "EventLinks" => {
                        let links = p.array_structs(&pk).unwrap_or_default();
                        op.events = links
                            .iter()
                            .map(|l| EventLink {
                                name: str_of(l, "LinkDesc"),
                                events: find(l, "LinkedEvents").and_then(|v| v.array_objects().ok()).unwrap_or_default().into_iter().filter_map(|i| self.op_ref(*src, i)).collect(),
                            })
                            .collect();
                    }
                    "SequenceObjects" => {
                        let objs = p.array_objects().unwrap_or_default();
                        op.children = objs.into_iter().filter_map(|i| self.op_ref(*src, i)).collect();
                    }
                    "ParentSequence" => {
                        op.parent = p.as_object().and_then(|i| self.op_ref(*src, i));
                    }
                    "ObjComment" => {
                        if let PV::Str(s) = &p.value {
                            op.comment = s.clone();
                        }
                    }
                    n if DROP.iter().any(|d| d.eq_ignore_ascii_case(n)) => {}
                    _ => {
                        let key = if p.array_index == 0 { p.name.clone() } else { format!("{}[{}]", p.name, p.array_index) };
                        let mut v = self.value(*src, p);
                        // Localized text (`L_<Package>.int` sections keyed by the object path).
                        if let (Value::Str(_), Src::Level(_)) = (&v, src) {
                            if let Some(t) = self.loc.get_object(&pkg.name, &op.path, &key) {
                                v = Value::Str(t.to_string());
                            }
                        }
                        op.props.0.push((key, v));
                    }
                }
            }
            if let Some(Value::Guid(g)) = op.props.get("m_StoryFlag").cloned() {
                op.props.set("m_StoryFlagName", Value::Name(self.flags.name(&g)));
            }
            if op.class == "InterpData" {
                op.interp = Some(self.interp_info(pi, ei));
            }
            self.db.ops[id] = op;
        }
        // Parent links and top-level sequences.
        for id in 0..self.db.ops.len() {
            if self.db.ops[id].kind == Kind::Sequence {
                for c in self.db.ops[id].children.clone() {
                    if self.db.ops[c as usize].parent.is_none() {
                        self.db.ops[c as usize].parent = Some(id as OpId);
                    }
                }
            }
        }
        for id in 0..self.db.ops.len() {
            let o = &self.db.ops[id];
            if o.kind == Kind::Sequence && o.parent.is_none() {
                self.db.roots[o.package as usize].push(id as OpId);
            }
        }
    }

    /// `InterpData`: length and event keys. Dishonored keeps the matinee in a `MatineeData`
    /// object (`m_Data`, cooked into the level package) whose `m_RunData` holds `InterpLength`
    /// and `InterpGroups`; plain UE3 `InterpData` holds them itself. Events come from
    /// `InterpGroups[].InterpTracks[]` of class `InterpTrackEvent` (`EventTrack` keys).
    fn interp_info(&mut self, pi: usize, export: usize) -> InterpInfo {
        let pkg = self.pkgs[pi].clone();
        let own = pkg.read_object(export).map(|h| h.properties).unwrap_or_default();
        let mut length = find(&own, "InterpLength").and_then(|p| p.as_float());
        let mut groups: Vec<i32> = find(&own, "InterpGroups").and_then(|p| p.array_objects().ok()).unwrap_or_default();
        if let Some(ObjRef::Export(m)) = find(&own, "m_Data").and_then(|p| p.as_object()).map(ObjRef::from_index) {
            if let Ok(h) = pkg.read_object(m) {
                if let Some(PV::Struct(_, run)) = find(&h.properties, "m_RunData").map(|p| &p.value) {
                    length = find(run, "InterpLength").and_then(|p| p.as_float()).or(length);
                    groups = find(run, "InterpGroups").and_then(|p| p.array_objects().ok()).unwrap_or(groups);
                }
            }
        }
        let mut info = InterpInfo { length: length.unwrap_or(5.0), events: Vec::new(), matinee: crate::interp::load(&pkg, export).map(Box::new) };
        let d = self.classes.defaults("InterpTrackEvent");
        for g in groups {
            let ObjRef::Export(gi) = ObjRef::from_index(g) else { continue };
            let Ok(gh) = pkg.read_object(gi) else { continue };
            let tracks = find(&gh.properties, "InterpTracks").and_then(|p| p.array_objects().ok()).unwrap_or_default();
            for t in tracks {
                let ObjRef::Export(ti) = ObjRef::from_index(t) else { continue };
                if !self.classes.is_a(&pkg.export_class(ti), "InterpTrackEvent") {
                    continue;
                }
                let Ok(th) = pkg.read_object(ti) else { continue };
                let flag = |n: &str| match find(&th.properties, n).or_else(|| find(&d, n)).map(|p| &p.value) {
                    Some(PV::Bool(b)) => *b,
                    _ => true,
                };
                let (fwd, back) = (flag("bFireEventsWhenForwards"), flag("bFireEventsWhenBackwards"));
                if let Some(keys) = find(&th.properties, "EventTrack") {
                    for k in keys.array_structs(&pkg).unwrap_or_default() {
                        let time = find(&k, "Time").and_then(|v| v.as_float()).unwrap_or(0.0);
                        let name = match find(&k, "EventName").map(|p| &p.value) {
                            Some(PV::Name(n)) => n.clone(),
                            _ => continue,
                        };
                        info.events.push((time, name, fwd, back));
                    }
                }
            }
        }
        info.events.sort_by(|a, b| a.0.total_cmp(&b.0));
        info
    }

    /// Register a level actor (transform, tag, touch shape, scalar properties).
    fn actor(&mut self, pi: usize, e: usize) -> u32 {
        if let Some(&a) = self.actor_ids.get(&(pi, e)) {
            return a;
        }
        let pkg = self.pkgs[pi].clone();
        let class = pkg.export_class(e);
        let name = pkg.obj_name(ObjRef::Export(e));
        let idx = self.db.actors.len() as u32;
        self.actor_ids.insert((pi, e), idx);
        self.db.actors.push(Actor { package: pkg.name.clone(), name: name.clone(), class: class.clone(), location: [0.0; 3], rotation: [0; 3], scale: [1.0; 3], tag: String::new(), shape: None, props: Props::default(), bones: Vec::new() });
        let props = pkg.read_object(e).map(|h| h.properties).unwrap_or_default();
        let mut a = self.db.actors[idx as usize].clone();
        if let Some(v) = find(&props, "Location").and_then(|p| p.as_vector()) {
            a.location = v;
        }
        if let Some(r) = find(&props, "Rotation").and_then(|p| p.as_rotator()) {
            a.rotation = r;
        }
        let ds = find(&props, "DrawScale").and_then(|p| p.as_float()).unwrap_or(1.0);
        let ds3 = match find(&props, "DrawScale3D").map(|p| &p.value) {
            Some(PV::Vector(v)) => *v,
            _ => [1.0; 3],
        };
        a.scale = [ds * ds3[0], ds * ds3[1], ds * ds3[2]];
        a.tag = match find(&props, "Tag").map(|p| &p.value) {
            Some(PV::Name(n)) => n.clone(),
            _ => class.clone(),
        };
        let pre = match find(&props, "PrePivot").map(|p| &p.value) {
            Some(PV::Vector(v)) => *v,
            _ => [0.0; 3],
        };
        // Touch shape from the actor's components.
        let mut boxes = Vec::new();
        let mut cyl: Option<(f32, f32)> = None;
        for c in 0..pkg.exports.len() {
            if pkg.exports[c].outer != (e as i32 + 1) {
                continue;
            }
            let cc = pkg.export_class(c);
            if cc == "CylinderComponent" {
                let Ok(h) = pkg.read_object(c) else { continue };
                // Values equal to the class template are omitted: take the template's.
                let tmpl = template_props(self.classes, &pkg, c);
                let get = |n: &str| find(&h.properties, n).and_then(|p| p.as_float()).or_else(|| tmpl.as_ref().and_then(|t| find(t, n)).and_then(|p| p.as_float()));
                let r = get("CollisionRadius").unwrap_or(22.0);
                let hh = get("CollisionHeight").unwrap_or(22.0);
                // A trigger's own cylinder is its CollisionComponent; keep the largest.
                if cyl.is_none_or(|x| r * hh > x.0 * x.1) {
                    cyl = Some((r, hh));
                }
            } else if cc == "BrushComponent" {
                let Ok(h) = pkg.read_object(c) else { continue };
                if let Some(PV::Struct(_, agg)) = find(&h.properties, "BrushAggGeom").map(|p| &p.value) {
                    if let Some(elems) = find(agg, "ConvexElems") {
                        for el in elems.array_structs(&pkg).unwrap_or_default() {
                            if let Some(PV::Raw(_, b)) = find(&el, "ElemBox").map(|p| &p.value) {
                                if b.len() >= 24 {
                                    let f = |i: usize| f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
                                    boxes.push([[f(0) - pre[0], f(1) - pre[1], f(2) - pre[2]], [f(3) - pre[0], f(4) - pre[1], f(5) - pre[2]]]);
                                }
                            }
                        }
                    }
                }
            }
        }
        a.shape = if !boxes.is_empty() {
            Some(Shape::Boxes(boxes))
        } else {
            cyl.map(|(radius, height)| Shape::Cylinder { radius, height })
        };
        for p in &props {
            if matches!(p.name.as_str(), "Location" | "Rotation" | "DrawScale" | "DrawScale3D" | "Tag" | "PrePivot") {
                continue;
            }
            if matches!(p.value, PV::Int(_) | PV::Float(_) | PV::Bool(_) | PV::Byte(_) | PV::Enum(_) | PV::Name(_) | PV::Str(_) | PV::Object(_)) {
                // Object references of actors are kept as paths (no further actor recursion).
                let v = match &p.value {
                    PV::Object(i) => Value::Obj(match ObjRef::from_index(*i) {
                        ObjRef::Null => Obj::None,
                        r @ ObjRef::Import(_) => Obj::Path(pkg.obj_path(r)),
                        r @ ObjRef::Export(x) => {
                            if is_under_world(&pkg, x) {
                                Obj::Path(format!("{}:{}", pkg.name, pkg.obj_path(r)))
                            } else {
                                Obj::Path(pkg.obj_path(r))
                            }
                        }
                    }),
                    _ => self.value(Src::Level(pi), p),
                };
                a.props.0.push((p.name.clone(), v));
            }
        }
        // [interp] Skeletal mesh actors (matinee props such as Samuel's boat): reference bones
        // in actor space, R(RotOrigin) * T(MeshOrigin) * bind pose, for attachments to a bone.
        if class.starts_with("SkeletalMeshActor") {
            a.bones = self.actor_bones(&pkg, &props).unwrap_or_default();
        }
        self.db.actors[idx as usize] = a;
        idx
    }

    /// [interp] Reference-pose bones of a skeletal mesh actor's mesh, in actor space.
    fn actor_bones(&self, pkg: &Arc<Package>, props: &[Property]) -> Option<Vec<(String, [f32; 3], [f32; 4])>> {
        let skel = crate::interp_bake::actor_skeleton(self.content, pkg, props)?;
        let to_entry = |name: String, m: glam::Mat4| {
            let (_, q, t) = m.to_scale_rotation_translation();
            (name, t.to_array(), q.to_array())
        };
        // Bones, then sockets (`SkeletalMeshSocket`: bone, relative location / rotation):
        // attachments name them like bones (`Player_Attach`, `Npc_attach` on Samuel's boat).
        let names = skel.bones.iter().map(|b| b.name.clone()).chain(skel.sockets.iter().map(|s| s.0.clone()));
        Some(names.filter_map(|n| skel.point(&skel.bones, &n).map(|m| to_entry(n, m))).collect())
    }

    fn resolve_named(&mut self) {
        // VarName -> first variable, per search scope (package list).
        let order_for = |db: &KismetDb, p: u16| -> Vec<u16> {
            let mut v = db.global.clone();
            if let Some(m) = db.maps.iter().find(|m| m.1.contains(&p)) {
                v.extend(m.1.iter().copied());
            } else if !v.contains(&p) {
                v.push(p);
            }
            v
        };
        let mut cache: HashMap<(Vec<u16>, String), Option<OpId>> = HashMap::new();
        for id in 0..self.db.ops.len() {
            if self.db.ops[id].class != "SeqVar_Named" {
                continue;
            }
            let want = self.db.ops[id].props.str("FindVarName").to_string();
            if want.is_empty() || want == "None" {
                continue;
            }
            let scope = order_for(&self.db, self.db.ops[id].package);
            let key = (scope.clone(), want.to_ascii_lowercase());
            let found = *cache.entry(key).or_insert_with(|| {
                for p in &scope {
                    for r in &self.db.roots[*p as usize] {
                        if let Some(v) = find_var(&self.db, *r, &want) {
                            return Some(v);
                        }
                    }
                }
                None
            });
            self.db.ops[id].named = found;
        }
    }

    fn read_objectives(&mut self) {
        let mut seen = std::collections::BTreeSet::new();
        for pi in 0..self.pkgs.len() {
            let pkg = self.pkgs[pi].clone();
            for o in dishonored_assets::kismet::read_objectives(&pkg) {
                if !seen.insert(o.path.clone()) {
                    continue;
                }
                let (opkg, rest) = o.path.split_once('.').unwrap_or(("", o.path.as_str()));
                let desc = self.loc.get_object(opkg, rest, "m_Description").map(str::to_string).unwrap_or(o.description.clone());
                let tasks = o
                    .tasks
                    .iter()
                    .map(|t| Task {
                        name: t.name.clone(),
                        description: self.loc.get_object(opkg, &format!("{rest}.{}", t.name), "m_Description").map(str::to_string).unwrap_or(t.description.clone()),
                        initial: t.initial_state.clone(),
                        hidden: t.hidden,
                    })
                    .collect();
                self.db.objectives.push(Objective { path: o.path.clone(), description: desc, tasks, complete_when_all_tasks: o.complete_when_all_tasks });
            }
        }
        let _ = self.content;
    }
}

/// Depth-first search of a sequence for a variable named `name` (not a `SeqVar_Named`).
fn find_var(db: &KismetDb, seq: OpId, name: &str) -> Option<OpId> {
    let s = db.op(seq);
    for &c in &s.children {
        let o = db.op(c);
        if o.kind == Kind::Variable && o.class != "SeqVar_Named" && o.props.str("VarName").eq_ignore_ascii_case(name) {
            return Some(c);
        }
    }
    for &c in &s.children {
        if db.op(c).kind == Kind::Sequence {
            if let Some(v) = find_var(db, c, name) {
                return Some(v);
            }
        }
    }
    None
}

/// Properties of a component's template (its archetype in the class packages).
fn template_props(classes: &ClassDb, pkg: &Package, c: usize) -> Option<Vec<Property>> {
    let arch = pkg.exports[c].archetype;
    if arch == 0 {
        return None;
    }
    let path = pkg.obj_path(ObjRef::from_index(arch));
    let (p, i) = classes.find_object(&path)?;
    p.read_object(i).ok().map(|h| h.properties)
}

fn str_of(props: &[Property], name: &str) -> String {
    match find(props, name).map(|p| &p.value) {
        Some(PV::Str(s)) => s.clone(),
        Some(PV::Name(s)) if s != "None" => s.clone(),
        _ => String::new(),
    }
}

fn bool_of(props: &[Property], name: &str) -> bool {
    matches!(find(props, name).map(|p| &p.value), Some(PV::Bool(true)))
}

/// Packages of a persistent map (persistent level, then its streaming levels). Boyle's
/// exterior also streams `L_BoyleStreet_Ext_Script`, listed in its `StreamingLevels`.
pub fn map_packages(content: &Content, map: &str) -> Vec<String> {
    dishonored_assets::map_packages(content, map)
}
