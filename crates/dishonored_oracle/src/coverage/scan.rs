//! Static scan of one cooked package: what it contains, independent of the port.
//!
//! Results are cached per package (bincode) under the oracle cache, keyed by the scanner
//! version and the file's size and modification time, so a warm scan of the whole install
//! only reads the cache files.

use super::ufunc::{FunctionInfo, read_function};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use ue3_package::properties::{RF_CLASS_DEFAULT_OBJECT, Value, find};
use ue3_package::{ObjRef, Package};

/// Bump when [`PackageScan`] or the way it is filled changes.
pub const SCAN_VERSION: u32 = 1;

/// Script / metadata classes: not "NetIndex + tagged properties" objects.
pub const SCRIPT_CLASSES: &[&str] = &[
    "Class", "Function", "State", "ScriptStruct", "Struct", "Enum", "Const", "Package", "TextBuffer", "MetaData",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FunctionDef {
    /// Class that declares the function (through a state when `state` is set).
    pub class: String,
    pub state: Option<String>,
    pub name: String,
    pub native_index: u16,
    pub flags: u32,
    pub script_size: u32,
}

impl FunctionDef {
    pub fn info(&self) -> FunctionInfo {
        FunctionInfo { native_index: self.native_index, flags: self.flags, script_size: self.script_size }
    }
}

/// What one package holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PackageScan {
    pub name: String,
    /// `None` for the base game, else the DLC folder (`DLC05`, ...).
    pub dlc: Option<String>,
    pub file_size: u64,
    pub exports: u32,
    pub imports: u32,
    pub names: u32,
    /// Objects per class (exports that are not script definitions or class default objects).
    pub classes: BTreeMap<String, u32>,
    /// Actors placed in the package's `PersistentLevel`, per class.
    pub actors: BTreeMap<String, u32>,
    /// Up to three object paths per class (for the work-list dossiers).
    pub examples: BTreeMap<String, Vec<String>>,
    /// Per class: property name -> number of objects that set it (tagged properties are the
    /// values that differ from the archetype, so "set" means "authored or overridden").
    pub props: BTreeMap<String, BTreeMap<String, u32>>,
    /// The same for class default objects (script defaults), per class.
    pub cdo_props: BTreeMap<String, BTreeMap<String, u32>>,
    /// Enum-valued properties: `Class.Property` -> value -> count (material blend modes and
    /// lighting models, physics modes, door states...).
    pub enums: BTreeMap<String, BTreeMap<String, u32>>,
    /// Classes defined here: (name, super class name; empty for roots).
    pub class_defs: Vec<(String, String)>,
    pub functions: Vec<FunctionDef>,
    /// Functions this package references through its imports.
    pub imported_functions: Vec<String>,
    /// `WorldInfo.StreamingLevels` package names (persistent maps).
    pub streaming_levels: Vec<String>,
    /// Exports whose properties or function layout did not parse.
    pub parse_failures: u32,
    pub first_failure: Option<String>,
}

fn bump(m: &mut BTreeMap<String, u32>, k: &str) {
    if let Some(v) = m.get_mut(k) {
        *v += 1;
    } else {
        m.insert(k.to_string(), 1);
    }
}

fn owner_class(pkg: &Package, export: usize) -> (String, Option<String>) {
    let mut state = None;
    let mut cur = ObjRef::from_index(pkg.exports[export].outer);
    for _ in 0..16 {
        match cur {
            ObjRef::Export(o) => {
                let c = pkg.export_class(o);
                if c == "Class" {
                    return (pkg.obj_name(cur), state);
                }
                if c == "State" {
                    state = Some(pkg.obj_name(cur));
                }
                cur = ObjRef::from_index(pkg.exports[o].outer);
            }
            _ => break,
        }
    }
    (String::new(), state)
}

pub fn scan_package(path: &Path, dlc: Option<&str>) -> Result<PackageScan> {
    let pkg = Package::open(path)?;
    let mut s = PackageScan {
        name: pkg.name.clone(),
        dlc: dlc.map(str::to_string),
        file_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        exports: pkg.exports.len() as u32,
        imports: pkg.imports.len() as u32,
        names: pkg.names.len() as u32,
        ..Default::default()
    };
    let fail = |s: &mut PackageScan, what: String| {
        s.parse_failures += 1;
        if s.first_failure.is_none() {
            s.first_failure = Some(what);
        }
    };
    for (i, imp) in pkg.imports.iter().enumerate() {
        if pkg.name(imp.class_name) == "Function" {
            s.imported_functions.push(pkg.obj_path(ObjRef::Import(i)));
        }
    }
    s.imported_functions.sort();
    s.imported_functions.dedup();
    for i in 0..pkg.exports.len() {
        let e = &pkg.exports[i];
        let class = pkg.export_class(i);
        if e.class == 0 {
            let sup = if e.super_ == 0 { String::new() } else { pkg.obj_name(ObjRef::from_index(e.super_)) };
            s.class_defs.push((pkg.obj_name(ObjRef::Export(i)), sup));
            continue;
        }
        if class == "Function" {
            match read_function(&pkg, i) {
                Ok(f) => {
                    let (c, state) = owner_class(&pkg, i);
                    s.functions.push(FunctionDef { class: c, state, name: pkg.obj_name(ObjRef::Export(i)), native_index: f.native_index, flags: f.flags, script_size: f.script_size });
                }
                Err(err) => fail(&mut s, format!("function {}: {err:#}", pkg.obj_path(ObjRef::Export(i)))),
            }
            continue;
        }
        if SCRIPT_CLASSES.contains(&class.as_str()) || class.ends_with("Property") {
            continue;
        }
        let cdo = e.object_flags & RF_CLASS_DEFAULT_OBJECT != 0;
        if !cdo {
            bump(&mut s.classes, &class);
            let ex = s.examples.entry(class.clone()).or_default();
            if ex.len() < 3 {
                ex.push(pkg.obj_path(ObjRef::Export(i)));
            }
            if let ObjRef::Export(o) = ObjRef::from_index(e.outer) {
                if pkg.obj_name(ObjRef::Export(o)) == "PersistentLevel" && pkg.export_class(o) == "Level" {
                    bump(&mut s.actors, &class);
                }
            }
        }
        let props = match pkg.read_object(i) {
            Ok(h) => h.properties,
            Err(err) => {
                fail(&mut s, format!("{} ({class}): {err:#}", pkg.obj_path(ObjRef::Export(i))));
                continue;
            }
        };
        let table = if cdo { &mut s.cdo_props } else { &mut s.props };
        let per = table.entry(class.clone()).or_default();
        for p in &props {
            if p.array_index == 0 {
                bump(per, &p.name);
            }
            if let Value::Enum(v) = &p.value {
                if !cdo {
                    bump(s.enums.entry(format!("{class}.{}", p.name)).or_default(), v);
                }
            }
        }
        if !cdo && class == "WorldInfo" {
            if let Ok(refs) = find(&props, "StreamingLevels").map(|p| p.array_objects()).transpose().map(Option::unwrap_or_default) {
                for r in refs {
                    let ObjRef::Export(x) = ObjRef::from_index(r) else { continue };
                    let Ok(h) = pkg.read_object(x) else { continue };
                    if let Some(Value::Name(n)) = find(&h.properties, "PackageName").map(|p| &p.value) {
                        if !s.streaming_levels.iter().any(|l| l.eq_ignore_ascii_case(n)) {
                            s.streaming_levels.push(n.clone());
                        }
                    }
                }
            }
        }
    }
    Ok(s)
}

/// One package file of the install.
#[derive(Debug, Clone)]
pub struct PackageFile {
    pub path: PathBuf,
    pub name: String,
    pub dlc: Option<String>,
}

/// Every `.upk` of the install: `CookedPCConsole`, plus `DLC/PCConsole/DLC*` with `dlc`.
pub fn list_packages(install: &Path, dlc: bool) -> Result<Vec<PackageFile>> {
    let game = install.join("DishonoredGame");
    let mut out = Vec::new();
    let mut add = |dir: &Path, tag: Option<String>| -> Result<()> {
        for e in std::fs::read_dir(dir).with_context(|| format!("{}", dir.display()))? {
            let p = e?.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("upk")) {
                let name = p.file_stem().unwrap().to_string_lossy().into_owned();
                out.push(PackageFile { path: p, name, dlc: tag.clone() });
            }
        }
        Ok(())
    };
    add(&game.join("CookedPCConsole"), None)?;
    if dlc {
        let root = game.join("DLC").join("PCConsole");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
        dirs.sort();
        for d in dirs {
            let tag = d.file_name().unwrap().to_string_lossy().into_owned();
            add(&d, Some(tag))?;
        }
    }
    out.sort_by(|a, b| (a.dlc.is_some(), &a.dlc, a.name.to_ascii_lowercase()).cmp(&(b.dlc.is_some(), &b.dlc, b.name.to_ascii_lowercase())));
    Ok(out)
}

fn cache_path(cache: &Path, f: &PackageFile) -> Option<PathBuf> {
    let m = std::fs::metadata(&f.path).ok()?;
    let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let tag = f.dlc.as_deref().unwrap_or("base");
    Some(cache.join(format!("v{SCAN_VERSION}")).join(format!("{tag}.{}.{}_{mtime}.bin", f.name, m.len())))
}

/// Scan one package, through the cache when `cache` is given. Returns the scan and whether it
/// came from the cache.
pub fn scan_cached(f: &PackageFile, cache: Option<&Path>) -> Result<(PackageScan, bool)> {
    let cp = cache.and_then(|c| cache_path(c, f));
    if let Some(p) = &cp {
        if let Ok(bytes) = std::fs::read(p) {
            if let Ok((s, _)) = bincode::serde::decode_from_slice::<PackageScan, _>(&bytes, bincode::config::standard()) {
                return Ok((s, true));
            }
        }
    }
    let s = scan_package(&f.path, f.dlc.as_deref()).with_context(|| f.name.clone())?;
    if let Some(p) = &cp {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).ok();
        }
        if let Ok(bytes) = bincode::serde::encode_to_vec(&s, bincode::config::standard()) {
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                std::fs::rename(&tmp, p).ok();
            }
        }
    }
    Ok((s, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_result_roundtrips_through_the_cache_encoding() {
        let mut s = PackageScan { name: "P".into(), dlc: Some("DLC05".into()), first_failure: Some("x".into()), ..Default::default() };
        s.classes.insert("A".into(), 2);
        s.props.entry("A".into()).or_default().insert("p".into(), 1);
        s.class_defs.push(("A".into(), "Actor".into()));
        s.functions.push(FunctionDef { class: "A".into(), state: Some("S".into()), name: "F".into(), native_index: 3, flags: 0x400, script_size: 9 });
        let bytes = bincode::serde::encode_to_vec(&s, bincode::config::standard()).unwrap();
        let (back, _): (PackageScan, usize) = bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(back, s);
    }
}
