//! The original's UnrealScript as data: every script package's classes, functions, properties,
//! enums and constants, resolved across packages, with the in-memory property layout
//! (`UStruct::Link` rules, as `prop_layout`) so field offsets seen in the decompiled native code
//! can be named.
//!
//! Serialized layouts (file version 801, licensee 30), after the UObject header (NetIndex and the
//! tagged properties ending in `None`):
//! - `UField`: `Next`;
//! - `UStruct`: `SuperStruct`, `ScriptText`, `Children`, `CppText`, `Line`, `TextPos`,
//!   `ScriptBytecodeSize` (in-memory bytes, object references count 8), `ScriptStorageSize`,
//!   then the bytecode;
//! - `UFunction`: `iNative` (u16), `OperPrecedence` (u8), `FunctionFlags` (u32), `RepOffset`
//!   (u16, with `FUNC_Net`), `FriendlyName`;
//! - `UProperty`: `ArrayDim` (i32), `PropertyFlags` (u64), `Category` (FName), `ArraySizeEnum`
//!   (object), `RepOffset` (u16, with `CPF_Net`), then the type's references (property class,
//!   struct, inner property, enum, function...);
//! - `UEnum`: names (`TArray<FName>`); `UConst`: the value (`FString`).

use anyhow::{Context, Result, bail, ensure};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use ue3_package::properties::{RF_CLASS_DEFAULT_OBJECT, read_properties};
use ue3_package::reader::Reader;
use ue3_package::{ObjRef, Package};

pub const FUNC_FINAL: u32 = 0x1;
pub const FUNC_ITERATOR: u32 = 0x4;
pub const FUNC_LATENT: u32 = 0x8;
pub const FUNC_PRE_OPERATOR: u32 = 0x10;
pub const FUNC_SINGULAR: u32 = 0x20;
pub const FUNC_NET: u32 = 0x40;
pub const FUNC_NET_RELIABLE: u32 = 0x80;
pub const FUNC_SIMULATED: u32 = 0x100;
pub const FUNC_EXEC: u32 = 0x200;
pub const FUNC_NATIVE: u32 = 0x400;
pub const FUNC_EVENT: u32 = 0x800;
pub const FUNC_OPERATOR: u32 = 0x1000;
pub const FUNC_STATIC: u32 = 0x2000;
pub const FUNC_CONST: u32 = 0x8000;
pub const FUNC_PRIVATE: u32 = 0x40000;
pub const FUNC_PROTECTED: u32 = 0x80000;
pub const FUNC_DELEGATE: u32 = 0x10_0000;
pub const FUNC_NET_SERVER: u32 = 0x20_0000;
pub const FUNC_NET_CLIENT: u32 = 0x100_0000;

pub const CPF_EDIT: u64 = 0x1;
pub const CPF_CONST: u64 = 0x2;
pub const CPF_OPTIONAL: u64 = 0x10;
pub const CPF_NET: u64 = 0x20;
pub const CPF_PARM: u64 = 0x80;
pub const CPF_OUT: u64 = 0x100;
pub const CPF_RETURN: u64 = 0x400;
pub const CPF_COERCE: u64 = 0x2_0000;
pub const CPF_NATIVE: u64 = 0x4_0000;
pub const CPF_TRANSIENT: u64 = 0x2000;
pub const CPF_CONFIG: u64 = 0x4000;

/// Classes of property definitions.
pub const PROPERTY_CLASSES: &[&str] = &[
    "ByteProperty", "IntProperty", "BoolProperty", "FloatProperty", "ObjectProperty", "ClassProperty", "ComponentProperty", "NameProperty", "StrProperty",
    "StructProperty", "ArrayProperty", "MapProperty", "DelegateProperty", "InterfaceProperty",
];

/// (package, export) of an object.
pub type Key = (u32, u32);

#[derive(Debug, Clone)]
pub struct StructHeader {
    pub next: i32,
    pub super_struct: i32,
    pub children: i32,
    pub line: i32,
    pub text_pos: i32,
    /// In-memory size of the bytecode (jump offsets are in this space).
    pub bytecode_size: u32,
    /// Byte range of the serialized bytecode inside the export data.
    pub script: std::ops::Range<usize>,
}

/// One property definition (`UProperty` export).
#[derive(Debug, Clone)]
pub struct PropDef {
    pub name: String,
    /// `IntProperty`, `StructProperty`...
    pub class: String,
    pub array_dim: u32,
    pub flags: u64,
    pub category: String,
    /// The type's references (class, struct, inner property, enum, function...).
    pub refs: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: String,
    pub owner_class: String,
    pub state: Option<String>,
    pub native_index: u16,
    pub oper_precedence: u8,
    pub flags: u32,
    pub friendly: String,
    /// Parameters (no return value), in order.
    pub params: Vec<Key>,
    pub ret: Option<Key>,
    pub locals: Vec<Key>,
    pub super_func: i32,
}

impl FuncDef {
    pub fn is_operator(&self) -> bool {
        self.flags & FUNC_OPERATOR != 0
    }
}

pub struct Db {
    pub pkgs: Vec<Package>,
    /// Lowercase global path (`core.object.add_intint`) -> object.
    pub paths: HashMap<String, Key>,
    /// Lowercase class name -> class export.
    pub classes: HashMap<String, Key>,
    pub funcs: HashMap<Key, FuncDef>,
    pub props: HashMap<Key, PropDef>,
    /// Native index -> function.
    pub natives: HashMap<u16, Key>,
    /// Per struct / class: its fields in `Next` order (properties, functions, states, enums...).
    pub children: HashMap<Key, Vec<Key>>,
    pub failures: Vec<String>,
    layouts: std::sync::Mutex<HashMap<Key, Layout>>,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub size: u32,
    pub align: u32,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub owner: String,
    pub name: String,
    pub ty: String,
    pub offset: u32,
    pub size: u32,
    pub bit: Option<u32>,
}

/// Packages that define script classes, by name (from the coverage scan or a directory listing).
pub fn script_packages(install: &Path, scans: &[crate::coverage::scan::PackageScan]) -> Vec<std::path::PathBuf> {
    let dir = install.join("DishonoredGame").join("CookedPCConsole");
    let mut out: Vec<_> = scans.iter().filter(|s| s.dlc.is_none() && !s.class_defs.is_empty()).map(|s| dir.join(format!("{}.upk", s.name))).collect();
    out.sort();
    out
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

impl Db {
    pub fn load(paths: &[std::path::PathBuf]) -> Result<Self> {
        let pkgs: Vec<Package> = paths.par_iter().map(|p| Package::open(p).with_context(|| p.display().to_string())).collect::<Result<_>>()?;
        let mut db = Db {
            pkgs,
            paths: HashMap::new(),
            classes: HashMap::new(),
            funcs: HashMap::new(),
            props: HashMap::new(),
            natives: HashMap::new(),
            children: HashMap::new(),
            failures: Vec::new(),
            layouts: Default::default(),
        };
        for (pi, p) in db.pkgs.iter().enumerate() {
            for i in 0..p.exports.len() {
                let path = format!("{}.{}", p.name, p.obj_path(ObjRef::Export(i))).to_ascii_lowercase();
                db.paths.entry(path).or_insert((pi as u32, i as u32));
                if p.exports[i].class == 0 {
                    db.classes.entry(p.obj_name(ObjRef::Export(i)).to_ascii_lowercase()).or_insert((pi as u32, i as u32));
                }
            }
        }
        // Properties and functions.
        let mut failures = Vec::new();
        for (pi, p) in db.pkgs.iter().enumerate() {
            for i in 0..p.exports.len() {
                let class = p.export_class(i);
                let k = (pi as u32, i as u32);
                // Zero-size property exports are cooked out (nothing to read).
                if PROPERTY_CLASSES.contains(&class.as_str()) && p.exports[i].serial_size > 0 {
                    match read_prop(p, i) {
                        Ok(d) => {
                            db.props.insert(k, d);
                        }
                        Err(e) => failures.push(format!("{}.{}: {e:#}", p.name, p.obj_path(ObjRef::Export(i)))),
                    }
                }
            }
        }
        for (pi, p) in db.pkgs.iter().enumerate() {
            for i in 0..p.exports.len() {
                let class = p.export_class(i);
                if matches!(class.as_str(), "Class" | "Function" | "State" | "ScriptStruct") && p.exports[i].serial_size > 0 {
                    let k = (pi as u32, i as u32);
                    let kids = match struct_header(p, i) {
                        Ok(h) => chain(p, pi as u32, h.children),
                        Err(e) => {
                            failures.push(format!("{}.{}: {e:#}", p.name, p.obj_path(ObjRef::Export(i))));
                            continue;
                        }
                    };
                    db.children.insert(k, kids);
                }
            }
        }
        for (pi, p) in db.pkgs.iter().enumerate() {
            for i in 0..p.exports.len() {
                if p.export_class(i) != "Function" || p.exports[i].serial_size == 0 {
                    continue;
                }
                let k = (pi as u32, i as u32);
                match read_func(p, i) {
                    Ok(mut f) => {
                        for &c in db.children.get(&k).map(|v| v.as_slice()).unwrap_or(&[]) {
                            let Some(pd) = db.props.get(&c) else { continue };
                            if pd.flags & CPF_RETURN != 0 {
                                f.ret = Some(c);
                            } else if pd.flags & CPF_PARM != 0 {
                                f.params.push(c);
                            } else {
                                f.locals.push(c);
                            }
                        }
                        if f.native_index != 0 {
                            db.natives.entry(f.native_index).or_insert(k);
                        }
                        db.funcs.insert(k, f);
                    }
                    Err(e) => failures.push(format!("{}.{}: {e:#}", p.name, p.obj_path(ObjRef::Export(i)))),
                }
            }
        }
        db.failures = failures;
        Ok(db)
    }

    pub fn pkg(&self, k: Key) -> &Package {
        &self.pkgs[k.0 as usize]
    }

    pub fn name(&self, k: Key) -> String {
        self.pkg(k).obj_name(ObjRef::Export(k.1 as usize))
    }

    pub fn class_of(&self, k: Key) -> String {
        self.pkg(k).export_class(k.1 as usize)
    }

    /// The export a package-local reference points at (imports resolved by path).
    pub fn resolve(&self, pi: u32, r: i32) -> Option<Key> {
        let p = &self.pkgs[pi as usize];
        match ObjRef::from_index(r) {
            ObjRef::Null => None,
            ObjRef::Export(i) => Some((pi, i as u32)),
            ObjRef::Import(_) => self.paths.get(&p.obj_path(ObjRef::from_index(r)).to_ascii_lowercase()).copied(),
        }
    }

    /// Name of a package-local reference (without resolving it).
    pub fn ref_name(&self, pi: u32, r: i32) -> String {
        self.pkgs[pi as usize].obj_name(ObjRef::from_index(r))
    }

    pub fn ref_class(&self, pi: u32, r: i32) -> String {
        self.pkgs[pi as usize].class_name(ObjRef::from_index(r))
    }

    pub fn func(&self, pi: u32, r: i32) -> Option<&FuncDef> {
        self.resolve(pi, r).and_then(|k| self.funcs.get(&k))
    }

    pub fn class_key(&self, name: &str) -> Option<Key> {
        self.classes.get(&name.to_ascii_lowercase()).copied()
    }

    /// Super class / struct of a struct export.
    pub fn super_of(&self, k: Key) -> Option<Key> {
        let e = &self.pkg(k).exports[k.1 as usize];
        self.resolve(k.0, e.super_)
    }

    pub fn super_name(&self, k: Key) -> Option<String> {
        let e = &self.pkg(k).exports[k.1 as usize];
        (e.super_ != 0).then(|| self.pkg(k).obj_name(ObjRef::from_index(e.super_)))
    }

    /// Class chain from `k` up to `Object`.
    pub fn ancestry(&self, k: Key) -> Vec<Key> {
        let mut out = vec![k];
        let mut cur = k;
        while let Some(s) = self.super_of(cur) {
            if out.contains(&s) || out.len() > 64 {
                break;
            }
            out.push(s);
            cur = s;
        }
        out
    }

    /// UnrealScript type of a property.
    pub fn type_of(&self, k: Key) -> String {
        let Some(p) = self.props.get(&k) else { return "?".into() };
        let r = |i: usize| p.refs.get(i).copied().unwrap_or(0);
        let nm = |x: i32| self.ref_name(k.0, x);
        match p.class.as_str() {
            "IntProperty" => "int".into(),
            "FloatProperty" => "float".into(),
            "BoolProperty" => "bool".into(),
            "NameProperty" => "name".into(),
            "StrProperty" => "string".into(),
            "ByteProperty" => {
                if r(0) != 0 {
                    nm(r(0))
                } else {
                    "byte".into()
                }
            }
            "ObjectProperty" | "ComponentProperty" => nm(r(0)),
            "ClassProperty" => format!("class<{}>", nm(r(1))),
            "StructProperty" => nm(r(0)),
            "InterfaceProperty" => nm(r(0)),
            "DelegateProperty" => format!("delegate<{}>", nm(r(0))),
            "ArrayProperty" => match self.resolve(k.0, r(0)) {
                Some(inner) => format!("array<{}>", self.type_of(inner)),
                None => "array<?>".into(),
            },
            "MapProperty" => "map<?>".into(),
            other => format!("?{other}"),
        }
    }

    /// In-memory layout of a class or struct (as `prop_layout`).
    pub fn layout(&self, k: Key) -> Layout {
        if let Some(l) = self.layouts.lock().unwrap().get(&k) {
            return l.clone();
        }
        let l = self.compute_layout(k, 0);
        self.layouts.lock().unwrap().insert(k, l.clone());
        l
    }

    fn compute_layout(&self, k: Key, depth: u32) -> Layout {
        if depth > 64 {
            return Layout::default();
        }
        let owner = self.name(k);
        let mut l = match self.super_of(k) {
            Some(s) => self.layout(s),
            None => Layout { size: 0, align: 1, fields: Vec::new() },
        };
        const FORCE16: &[&str] = &["plane", "matrix", "quat", "shvector", "shvectorrgb", "vector4", "fourvectors", "linearcolor_aligned"];
        let mut prev_bool: Option<(u32, u32)> = None;
        for &c in self.children.get(&k).map(|v| v.as_slice()).unwrap_or(&[]) {
            let Some(p) = self.props.get(&c) else { continue };
            let dim = p.array_dim.max(1);
            let (size, align) = match p.class.as_str() {
                "ByteProperty" => (1, 1),
                "IntProperty" | "FloatProperty" | "ObjectProperty" | "ComponentProperty" | "ClassProperty" | "BoolProperty" => (4, 4),
                "NameProperty" | "InterfaceProperty" => (8, 4),
                "StrProperty" | "ArrayProperty" | "DelegateProperty" => (12, 4),
                "MapProperty" => (60, 4),
                "StructProperty" => match p.refs.first().and_then(|&r| self.resolve(c.0, r)) {
                    Some(s) => {
                        let sl = self.layout(s);
                        (sl.size, sl.align.max(1))
                    }
                    None => (0, 4),
                },
                _ => (4, 4),
            };
            let ty = self.type_of(c);
            if p.class == "BoolProperty" {
                if let Some((off, bit)) = prev_bool {
                    if bit < 31 {
                        prev_bool = Some((off, bit + 1));
                        l.fields.push(Field { owner: owner.clone(), name: p.name.clone(), ty, offset: off, size: 4, bit: Some(bit + 1) });
                        continue;
                    }
                }
            }
            let off = l.size.div_ceil(align) * align;
            l.size = off + size * dim;
            l.align = l.align.max(align);
            prev_bool = if p.class == "BoolProperty" { Some((off, 0)) } else { None };
            let ty = if dim > 1 { format!("{ty}[{dim}]") } else { ty };
            l.fields.push(Field { owner: owner.clone(), name: p.name.clone(), ty, offset: off, size: size * dim, bit: (p.class == "BoolProperty").then_some(0) });
        }
        if self.class_of(k) == "ScriptStruct" {
            let lname = owner.to_ascii_lowercase();
            if FORCE16.contains(&lname.as_str()) {
                l.align = 16;
            } else if lname == "qword" {
                l.align = 8;
            }
            l.size = l.size.div_ceil(l.align.max(1)) * l.align.max(1);
        }
        l
    }

    /// Names of an enum export.
    pub fn enum_values(&self, k: Key) -> Result<Vec<String>> {
        let p = self.pkg(k);
        let d = p.export_data(k.1 as usize)?;
        let mut r = Reader::new(d);
        let _net = r.i32()?;
        let _props = read_properties(p, &mut r)?;
        let _next = r.i32()?;
        let n = r.count(8)?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let f = ue3_package::FName::read(&mut r)?;
            out.push(p.name(f));
        }
        Ok(out)
    }

    /// Value text of a `Const` export.
    pub fn const_value(&self, k: Key) -> Result<String> {
        let p = self.pkg(k);
        let d = p.export_data(k.1 as usize)?;
        let mut r = Reader::new(d);
        let _net = r.i32()?;
        let _props = read_properties(p, &mut r)?;
        let _next = r.i32()?;
        r.fstring()
    }

    /// The class default object's properties (`Default__<Class>`), as `name=value` lines.
    pub fn defaults(&self, class: Key) -> Vec<String> {
        let p = self.pkg(class);
        let want = format!("Default__{}", self.name(class));
        let Some(i) = (0..p.exports.len()).find(|&i| p.exports[i].object_flags & RF_CLASS_DEFAULT_OBJECT != 0 && p.obj_name(ObjRef::Export(i)) == want) else { return Vec::new() };
        match p.read_object(i) {
            Ok(h) => h
                .properties
                .iter()
                .map(|pr| {
                    let idx = if pr.array_index != 0 { format!("({})", pr.array_index) } else { String::new() };
                    format!("{}{idx}={}", pr.name, ue3_package::properties::format_value(p, &pr.value))
                })
                .collect(),
            Err(e) => vec![format!("// defaults not read: {e:#}")],
        }
    }

    /// The bytecode of a struct export (function, state, class).
    pub fn bytecode(&self, k: Key) -> Result<(StructHeader, &[u8])> {
        let p = self.pkg(k);
        let h = struct_header(p, k.1 as usize)?;
        let d = p.export_data(k.1 as usize)?;
        Ok((h.clone(), &d[h.script.clone()]))
    }
}

/// Children of a struct: the chain starting at `children`, following each field's `Next`.
fn chain(p: &Package, pi: u32, first: i32) -> Vec<Key> {
    let mut out = Vec::new();
    let mut cur = first;
    while let ObjRef::Export(i) = ObjRef::from_index(cur) {
        if out.len() > 4096 || out.contains(&(pi, i as u32)) {
            break;
        }
        out.push((pi, i as u32));
        cur = match field_next(p, i) {
            Ok(n) => n,
            Err(_) => break,
        };
    }
    out
}

fn field_next(p: &Package, i: usize) -> Result<i32> {
    let d = p.export_data(i)?;
    let mut r = Reader::new(d);
    let _net = r.i32()?;
    let _props = read_properties(p, &mut r)?;
    r.i32()
}

pub fn struct_header(p: &Package, i: usize) -> Result<StructHeader> {
    let d = p.export_data(i)?;
    let mut r = Reader::new(d);
    let _net = r.i32()?;
    // `UObject::Serialize` writes no tagged properties for UClass objects.
    if p.exports[i].class != 0 {
        let _props = read_properties(p, &mut r)?;
    }
    let next = r.i32()?;
    let super_struct = r.i32()?;
    let _script_text = r.i32()?;
    let children = r.i32()?;
    let _cpp_text = r.i32()?;
    let line = r.i32()?;
    let text_pos = r.i32()?;
    let bytecode_size = r.i32()?;
    let storage = r.i32()?;
    ensure!(storage >= 0 && r.remaining() >= storage as usize, "implausible script size {storage}");
    ensure!(bytecode_size >= storage, "bytecode size {bytecode_size} below storage {storage}");
    let start = r.pos();
    Ok(StructHeader { next, super_struct, children, line, text_pos, bytecode_size: bytecode_size as u32, script: start..start + storage as usize })
}

fn read_func(p: &Package, i: usize) -> Result<FuncDef> {
    let h = struct_header(p, i)?;
    let d = p.export_data(i)?;
    let mut r = Reader::at(d, h.script.end);
    let native_index = r.u16()?;
    let oper_precedence = r.u8()?;
    let flags = r.u32()?;
    if flags & FUNC_NET != 0 {
        let _rep = r.u16()?;
    }
    let friendly = ue3_package::FName::read(&mut r)?;
    ensure!(r.remaining() == 0, "{} bytes left after the function", r.remaining());
    // Owner: the class (through a state).
    let mut state = None;
    let mut owner = String::new();
    let mut cur = ObjRef::from_index(p.exports[i].outer);
    for _ in 0..16 {
        let ObjRef::Export(o) = cur else { break };
        let c = p.export_class(o);
        if c == "Class" {
            owner = p.obj_name(cur);
            break;
        }
        if c == "State" {
            state = Some(p.obj_name(cur));
        }
        cur = ObjRef::from_index(p.exports[o].outer);
    }
    Ok(FuncDef {
        name: p.obj_name(ObjRef::Export(i)),
        owner_class: owner,
        state,
        native_index,
        oper_precedence,
        flags,
        friendly: p.name(friendly),
        params: Vec::new(),
        ret: None,
        locals: Vec::new(),
        super_func: h.super_struct,
    })
}

fn read_prop(p: &Package, i: usize) -> Result<PropDef> {
    let d = p.export_data(i)?;
    let mut r = Reader::new(d);
    let _net = r.i32()?;
    let _props = read_properties(p, &mut r)?;
    let _next = r.i32()?;
    let array_dim = r.i32()?;
    let flags = r.u64()?;
    let category = ue3_package::FName::read(&mut r)?;
    let _array_enum = r.i32()?;
    if flags & CPF_NET != 0 {
        let _rep = r.u16()?;
    }
    let class = p.export_class(i);
    let want = match class.as_str() {
        "ObjectProperty" | "ComponentProperty" | "StructProperty" | "ArrayProperty" | "ByteProperty" | "InterfaceProperty" => 1,
        "ClassProperty" | "DelegateProperty" | "MapProperty" => 2,
        _ => 0,
    };
    if r.remaining() != want * 4 {
        bail!("{class}: {} bytes of type data, expected {}", r.remaining(), want * 4);
    }
    let refs = (0..want).map(|_| r.i32()).collect::<Result<Vec<_>>>()?;
    let _ = u16_at;
    Ok(PropDef { name: p.obj_name(ObjRef::Export(i)), class, array_dim: array_dim.max(0) as u32, flags, category: p.name(category), refs })
}
