//! In-memory layout of script classes and structs (UE3 `UStruct::Link` rules), for reading
//! the native code of the original executable: property offsets are not serialized, the
//! engine computes them when linking, so this recomputes them from the property exports.
//!
//! `prop_layout <CookedPCConsole dir> <Class|Outer.Struct> [...] [--grep <substr>]`
//!
//! Rules reproduced: a class starts at its super's size; each property is aligned to its
//! own alignment (byte 1, struct = struct alignment, everything else 4); consecutive bools
//! share one dword (one bit each); structs named Plane/Matrix/Quat/... align to 16, QWord 8;
//! struct sizes are rounded up to their alignment. Fields are linked in `UField::Next`
//! order. Native interface vtable pointers appear as the compiler-added `VfTable_I*`
//! properties.
use anyhow::{Context, Result};
use std::collections::HashMap;
use ue3_package::{ObjRef, Package};

struct Db {
    pkgs: Vec<Package>,
    /// lowercase "Outer.Name" or "Name" (top-level classes) -> (pkg, export)
    index: HashMap<String, (usize, usize)>,
    cache: HashMap<(usize, usize), Layout>,
}

#[derive(Clone, Default)]
struct Layout {
    size: u32,
    align: u32,
    fields: Vec<Field>,
}

#[derive(Clone)]
struct Field {
    owner: String,
    name: String,
    ty: String,
    offset: u32,
    size: u32,
    bit: Option<u32>,
}

const FORCE16: &[&str] = &["plane", "matrix", "quat", "shvector", "shvectorrgb", "vector4", "fourvectors", "linearcolor_aligned"];

impl Db {
    fn key(p: &Package, i: usize) -> String {
        let e = &p.exports[i];
        let name = p.name(e.object_name).to_ascii_lowercase();
        match ObjRef::from_index(e.outer) {
            ObjRef::Null => name,
            o => format!("{}.{}", p.obj_name(o).to_ascii_lowercase(), name),
        }
    }

    fn resolve(&self, pi: usize, r: i32) -> Option<(usize, usize)> {
        let p = &self.pkgs[pi];
        match ObjRef::from_index(r) {
            ObjRef::Null => None,
            ObjRef::Export(i) => Some((pi, i)),
            ObjRef::Import(i) => {
                let imp = &p.imports[i];
                let name = p.name(imp.object_name).to_ascii_lowercase();
                let key = match ObjRef::from_index(imp.outer) {
                    ObjRef::Null => name.clone(),
                    o => {
                        let outer_outer = p.obj_outer(o);
                        // Top-level classes are keyed by bare name (their outer is the package).
                        if outer_outer == ObjRef::Null && p.class_name(ObjRef::Import(i)) == "Class" {
                            name.clone()
                        } else {
                            format!("{}.{}", p.obj_name(o).to_ascii_lowercase(), name)
                        }
                    }
                };
                self.index.get(&key).copied().or_else(|| self.index.get(&name).copied())
            }
        }
    }

    fn i32_at(d: &[u8], o: usize) -> i32 {
        i32::from_le_bytes(d[o..o + 4].try_into().unwrap())
    }

    fn layout(&mut self, pi: usize, ei: usize) -> Result<Layout> {
        if let Some(l) = self.cache.get(&(pi, ei)) {
            return Ok(l.clone());
        }
        let owner = self.pkgs[pi].name(self.pkgs[pi].exports[ei].object_name);
        let sup = self.pkgs[pi].exports[ei].super_;
        let mut l = match self.resolve(pi, sup) {
            Some((sp, se)) => self.layout(sp, se)?,
            None => Layout { size: 0, align: 1, fields: Vec::new() },
        };
        // Children of this struct: exports whose outer is it, chained by UField::Next (+12).
        let me = (ei + 1) as i32;
        let p = &self.pkgs[pi];
        let kids: Vec<usize> = (0..p.exports.len()).filter(|&i| p.exports[i].outer == me).collect();
        let mut next: HashMap<usize, i32> = HashMap::new();
        for &k in &kids {
            let d = p.export_data(k)?;
            if d.len() >= 16 {
                next.insert(k, Self::i32_at(d, 12));
            }
        }
        let pointed: std::collections::HashSet<i32> = next.values().copied().collect();
        let head = kids.iter().copied().find(|k| !pointed.contains(&((*k + 1) as i32)));
        let mut chain = Vec::new();
        let mut cur = head;
        while let Some(c) = cur {
            if chain.contains(&c) {
                break;
            }
            chain.push(c);
            cur = match next.get(&c) {
                Some(&n) if n > 0 => Some(n as usize - 1),
                _ => None,
            };
        }
        let mut prev_bool: Option<(u32, u32)> = None; // (offset, bit)
        for c in chain {
            let p = &self.pkgs[pi];
            let cls = p.export_class(c);
            if !cls.ends_with("Property") {
                continue;
            }
            let name = p.name(p.exports[c].object_name);
            let d = p.export_data(c)?.to_vec();
            let dim = Self::i32_at(&d, 16).max(1) as u32;
            let last_ref = Self::i32_at(&d, d.len() - 4);
            let (size, align, ty) = match cls.as_str() {
                "ByteProperty" => (1, 1, "byte".to_string()),
                "IntProperty" => (4, 4, "int".into()),
                "FloatProperty" => (4, 4, "float".into()),
                "ObjectProperty" | "ComponentProperty" | "ClassProperty" => (4, 4, "object".into()),
                "NameProperty" => (8, 4, "name".into()),
                "StrProperty" => (12, 4, "string".into()),
                "ArrayProperty" => (12, 4, "array".into()),
                "DelegateProperty" => (12, 4, "delegate".into()),
                "InterfaceProperty" => (8, 4, "interface".into()),
                "MapProperty" => (60, 4, "map(?)".into()),
                "BoolProperty" => (4, 4, "bool".into()),
                "StructProperty" => {
                    let s = self.resolve(pi, last_ref).with_context(|| format!("struct of {owner}.{name}"))?;
                    let sname = self.pkgs[s.0].name(self.pkgs[s.0].exports[s.1].object_name);
                    let sl = self.layout(s.0, s.1)?;
                    (sl.size, sl.align, format!("struct {sname}"))
                }
                other => (4, 4, format!("?{other}")),
            };
            if cls == "BoolProperty" {
                if let Some((off, bit)) = prev_bool {
                    if bit < 31 {
                        prev_bool = Some((off, bit + 1));
                        l.fields.push(Field { owner: owner.clone(), name, ty, offset: off, size: 4, bit: Some(bit + 1) });
                        continue;
                    }
                }
            }
            let off = l.size.div_ceil(align) * align;
            l.size = off + size * dim;
            l.align = l.align.max(align);
            prev_bool = if cls == "BoolProperty" { Some((off, 0)) } else { None };
            let ty = if dim > 1 { format!("{ty}[{dim}]") } else { ty };
            l.fields.push(Field { owner: owner.clone(), name, ty, offset: off, size: size * dim, bit: (cls == "BoolProperty").then_some(0) });
        }
        let p = &self.pkgs[pi];
        if p.export_class(ei) == "ScriptStruct" {
            let lname = owner.to_ascii_lowercase();
            if FORCE16.contains(&lname.as_str()) {
                l.align = 16;
            } else if lname == "qword" {
                l.align = 8;
            }
            l.size = l.size.div_ceil(l.align.max(1)) * l.align.max(1);
        }
        self.cache.insert((pi, ei), l.clone());
        Ok(l)
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(args.first().context("usage: prop_layout <dir> <Class>... [--grep s]")?);
    let grep = args.iter().position(|a| a == "--grep").map(|i| args[i + 1].to_ascii_lowercase());
    let mut db = Db { pkgs: Vec::new(), index: HashMap::new(), cache: HashMap::new() };
    for n in ["Core", "Engine", "GameFramework", "IpDrv", "AkAudio", "DishonoredGame"] {
        let p = Package::open(dir.join(format!("{n}.upk")))?;
        let pi = db.pkgs.len();
        for i in 0..p.exports.len() {
            let c = p.export_class(i);
            if c == "Class" || c == "ScriptStruct" {
                db.index.entry(Db::key(&p, i)).or_insert((pi, i));
                if c == "Class" {
                    db.index.entry(p.name(p.exports[i].object_name).to_ascii_lowercase()).or_insert((pi, i));
                }
            }
        }
        db.pkgs.push(p);
    }
    let mut skip = false;
    for a in &args[1..] {
        if skip {
            skip = false;
            continue;
        }
        if a == "--grep" {
            skip = true;
            continue;
        }
        let Some(&(pi, ei)) = db.index.get(&a.to_ascii_lowercase()) else {
            println!("{a}: not found");
            continue;
        };
        let l = db.layout(pi, ei)?;
        println!("== {a}: size {:#x} align {}", l.size, l.align);
        for f in &l.fields {
            if let Some(g) = &grep {
                if !f.name.to_ascii_lowercase().contains(g.as_str()) {
                    continue;
                }
            }
            let bit = f.bit.map(|b| format!(" bit{b} (mask {:#x})", 1u32 << b)).unwrap_or_default();
            println!("  +{:#06x} {:>4}  {}.{} : {}{}", f.offset, f.size, f.owner, f.name, f.ty, bit);
        }
    }
    Ok(())
}
