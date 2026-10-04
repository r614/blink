//! Arkane "tweak" objects (`DisTweaks_*`): the data-driven tuning for movement, powers,
//! weapons and AI.
//!
//! Cooked objects store only deltas against their archetype (the class default object),
//! and class defaults are themselves deltas against the superclass defaults. Struct
//! properties are deltas too (e.g. `m_HealthPotionPotencyBonus` sets only Easy and
//! VeryHard). Resolution therefore merges, in order:
//! `superclass CDO chain -> class CDO -> instance`, recursively for structs.

use crate::content::Content;
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use ue3_package::properties::{Property, Value};
use ue3_package::{ObjRef, Package};

#[derive(Debug, Clone, PartialEq)]
pub enum TVal {
    F(f32),
    I(i32),
    B(bool),
    Name(String),
    Str(String),
    Obj(String),
    Vec3([f32; 3]),
    Rot([i32; 3]),
    Color([u8; 4]),
    Struct(TMap),
    List(Vec<TMap>),
    Raw(String),
}

/// Property name (with `[i]` suffix for static-array index > 0) -> value.
pub type TMap = BTreeMap<String, TVal>;

/// Packages that hold tweak objects and the script classes they derive from.
pub const TWEAK_PACKAGES: &[&str] = &["Core", "Engine", "DishonoredGame", "Startup", "DishonoredGameFull_P"];

pub struct TweakDb {
    pkgs: Vec<Arc<Package>>,
    /// lowercase object path -> (pkg slot, export)
    objects: HashMap<String, (usize, usize)>,
    /// class name -> superclass name
    supers: HashMap<String, String>,
    /// class name -> CDO location
    cdos: HashMap<String, (usize, usize)>,
}

fn key(p: &Property) -> String {
    if p.array_index == 0 {
        p.name.clone()
    } else {
        format!("{}[{}]", p.name, p.array_index)
    }
}

fn to_tval(pkg: &Package, p: &Property) -> TVal {
    match &p.value {
        Value::Int(i) => TVal::I(*i),
        Value::Float(f) => TVal::F(*f),
        Value::Bool(b) => TVal::B(*b),
        Value::Byte(b) => TVal::I(*b as i32),
        Value::Enum(s) | Value::Name(s) => TVal::Name(s.clone()),
        Value::Str(s) => TVal::Str(s.clone()),
        Value::Object(i) => TVal::Obj(pkg.obj_path(ObjRef::from_index(*i))),
        Value::Vector(v) => TVal::Vec3(*v),
        Value::Rotator(r) => TVal::Rot(*r),
        Value::Color(c) => TVal::Color(*c),
        Value::LinearColor(c) => TVal::Vec3([c[0], c[1], c[2]]),
        Value::Vector2D(v) => TVal::Vec3([v[0], v[1], 0.0]),
        Value::Guid(_) => TVal::Raw("Guid".into()),
        Value::Struct(_, props) => TVal::Struct(props_to_map(pkg, props)),
        Value::Array(_) => match p.array_structs(pkg) {
            Ok(elems) if !elems.is_empty() && elems.iter().all(|e| !e.is_empty()) => {
                TVal::List(elems.iter().map(|e| props_to_map(pkg, e)).collect())
            }
            _ => match p.array_objects() {
                Ok(objs) => TVal::List(
                    objs.iter()
                        .map(|o| {
                            let mut m = TMap::new();
                            m.insert("obj".into(), TVal::Obj(pkg.obj_path(ObjRef::from_index(*o))));
                            m
                        })
                        .collect(),
                ),
                Err(_) => TVal::Raw("Array".into()),
            },
        },
        Value::Raw(t, _) => TVal::Raw(t.clone()),
    }
}

pub fn props_to_map(pkg: &Package, props: &[Property]) -> TMap {
    props.iter().map(|p| (key(p), to_tval(pkg, p))).collect()
}

/// Merge `over` onto `base`; structs merge field-wise, everything else replaces.
pub fn merge(base: &mut TMap, over: &TMap) {
    for (k, v) in over {
        match (base.get_mut(k), v) {
            (Some(TVal::Struct(b)), TVal::Struct(o)) => merge(b, o),
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

impl TweakDb {
    pub fn load(content: &Content) -> Result<Self> {
        let mut db = TweakDb {
            pkgs: Vec::new(),
            objects: HashMap::new(),
            supers: HashMap::new(),
            cdos: HashMap::new(),
        };
        for name in TWEAK_PACKAGES {
            if !content.has_package(name) {
                continue;
            }
            let pkg = content.package(name)?;
            let slot = db.pkgs.len();
            for i in 0..pkg.exports.len() {
                let e = &pkg.exports[i];
                let oname = pkg.name(e.object_name);
                if e.class == 0 {
                    // A class: record its superclass.
                    let sup = ObjRef::from_index(e.super_);
                    if sup != ObjRef::Null {
                        db.supers.entry(oname.clone()).or_insert_with(|| pkg.obj_name(sup));
                    }
                    continue;
                }
                if let Some(class) = oname.strip_prefix("Default__") {
                    if e.object_flags & ue3_package::properties::RF_CLASS_DEFAULT_OBJECT != 0 {
                        db.cdos.entry(class.to_string()).or_insert((slot, i));
                    }
                }
                let path = pkg.obj_path(ObjRef::Export(i)).to_ascii_lowercase();
                db.objects.entry(path).or_insert((slot, i));
            }
            db.pkgs.push(pkg);
        }
        Ok(db)
    }

    fn read(&self, loc: (usize, usize)) -> Result<TMap> {
        let pkg = &self.pkgs[loc.0];
        let h = pkg.read_object(loc.1)?;
        Ok(props_to_map(pkg, &h.properties))
    }

    /// Fully merged defaults for a class (superclass chain first).
    pub fn class_defaults(&self, class: &str) -> Result<TMap> {
        let mut chain = vec![class.to_string()];
        let mut cur = class.to_string();
        while let Some(s) = self.supers.get(&cur) {
            if chain.len() > 32 {
                break;
            }
            chain.push(s.clone());
            cur = s.clone();
        }
        let mut out = TMap::new();
        for c in chain.iter().rev() {
            if let Some(loc) = self.cdos.get(c) {
                // Engine-level component CDOs (e.g. Default__Component) use a header
                // variant we do not parse yet; they carry no gameplay tuning, so skip them.
                match self.read(*loc) {
                    Ok(m) => merge(&mut out, &m),
                    Err(_) if c != class => {}
                    Err(e) => return Err(e.context(format!("CDO of {c}"))),
                }
            }
        }
        Ok(out)
    }

    pub fn class_of(&self, path: &str) -> Option<String> {
        let loc = self.objects.get(&path.to_ascii_lowercase())?;
        Some(self.pkgs[loc.0].export_class(loc.1))
    }

    /// Resolve an object by full path (e.g. `Twk_Pawn_Corvo.Twk_Pawn_Corvo_Release`).
    pub fn resolve(&self, path: &str) -> Result<TMap> {
        let Some(loc) = self.objects.get(&path.to_ascii_lowercase()).copied() else {
            bail!("tweak object {path} not found");
        };
        let class = self.pkgs[loc.0].export_class(loc.1);
        let mut out = self.class_defaults(&class)?;
        let inst = self.read(loc)?;
        merge(&mut out, &inst);
        Ok(out)
    }

    pub fn paths_with_class(&self, class: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .objects
            .iter()
            .filter(|(_, l)| self.pkgs[l.0].export_class(l.1) == class)
            .map(|(_, l)| self.pkgs[l.0].obj_path(ObjRef::Export(l.1)))
            .collect();
        v.sort();
        v
    }
}

/// Dotted lookup: `m_JumpZ.m_fBaseValue2_Normal`, `m_Levels[1].m_HorizDistance`.
pub fn get<'a>(m: &'a TMap, path: &str) -> Option<&'a TVal> {
    let mut cur = m;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, p) in parts.iter().enumerate() {
        let v = cur.get(*p)?;
        if i == parts.len() - 1 {
            return Some(v);
        }
        match v {
            TVal::Struct(s) => cur = s,
            _ => return None,
        }
    }
    None
}

pub fn get_f32(m: &TMap, path: &str) -> Option<f32> {
    match get(m, path)? {
        TVal::F(f) => Some(*f),
        TVal::I(i) => Some(*i as f32),
        _ => None,
    }
}

pub fn get_obj<'a>(m: &'a TMap, path: &str) -> Option<&'a str> {
    match get(m, path)? {
        TVal::Obj(s) => Some(s),
        _ => None,
    }
}

pub fn dump(m: &TMap, indent: usize, out: &mut String) {
    use std::fmt::Write;
    for (k, v) in m {
        let pad = " ".repeat(indent);
        match v {
            TVal::Struct(s) => {
                let _ = writeln!(out, "{pad}{k}:");
                dump(s, indent + 2, out);
            }
            TVal::List(l) => {
                for (i, e) in l.iter().enumerate() {
                    let _ = writeln!(out, "{pad}{k}({i}):");
                    dump(e, indent + 2, out);
                }
            }
            other => {
                let _ = writeln!(out, "{pad}{k} = {other:?}");
            }
        }
    }
}
