//! `class_layout <CookedPCConsole dir> <package> <Class or Outer.Struct> ...`: native memory
//! layout of UE3 classes and script structs, computed like `UStruct::Link` from the cooked
//! reflection data (property order from the `Children`/`Next` chain, alignment, bitfield bools,
//! super class size). Used to map the field offsets seen in the decompiled executable to
//! property names (research; output is derived from retail data, keep it out of the repo).
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use ue3_package::{ObjRef, Package};

struct Ctx {
    dir: String,
    pkgs: HashMap<String, Package>,
    sizes: HashMap<(String, usize), (u32, u32)>,
}

#[derive(Clone)]
struct Obj {
    pkg: String,
    idx: usize,
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn i32_at(d: &[u8], o: usize) -> i32 {
    u32_at(d, o) as i32
}

impl Ctx {
    fn pkg(&mut self, name: &str) -> Result<&Package> {
        if !self.pkgs.contains_key(name) {
            let p = Package::open(format!("{}/{}.upk", self.dir, name))?;
            self.pkgs.insert(name.to_string(), p);
        }
        Ok(&self.pkgs[name])
    }

    /// Resolve an object reference of `pkg` to the export holding it.
    fn resolve(&mut self, pkg: &str, r: i32) -> Result<Option<Obj>> {
        match ObjRef::from_index(r) {
            ObjRef::Null => Ok(None),
            ObjRef::Export(i) => Ok(Some(Obj { pkg: pkg.to_string(), idx: i })),
            ObjRef::Import(i) => {
                let p = self.pkg(pkg)?;
                let full = p.obj_path(ObjRef::Import(i));
                let package = p.import_package(i);
                let rest = full.strip_prefix(&format!("{package}.")).unwrap_or(&full).to_string();
                let q = self.pkg(&package)?;
                for (j, _) in q.exports.iter().enumerate() {
                    if q.obj_path(ObjRef::Export(j)) == rest {
                        return Ok(Some(Obj { pkg: package, idx: j }));
                    }
                }
                bail!("import {full} not found in {package}")
            }
        }
    }

    fn ref_name(&mut self, pkg: &str, r: i32) -> Result<String> {
        let p = self.pkg(pkg)?;
        Ok(if r == 0 { String::new() } else { p.obj_name(ObjRef::from_index(r)) })
    }

    fn header(&mut self, o: &Obj) -> Result<(Vec<u8>, usize)> {
        let p = self.pkg(&o.pkg)?;
        let d = p.export_data(o.idx)?.to_vec();
        let none = p.find_name("None").unwrap_or(-1) as u32;
        let h = if d.len() >= 12 && u32_at(&d, 4) == none && u32_at(&d, 8) == 0 { 12 } else { 4 };
        Ok((d, h))
    }

    /// (size, alignment) of a class or script struct; prints the fields when `print`.
    fn layout(&mut self, o: &Obj, print: bool, prefix: &str) -> Result<(u32, u32)> {
        let key = (o.pkg.clone(), o.idx);
        if !print {
            if let Some(s) = self.sizes.get(&key) {
                return Ok(*s);
            }
        }
        let (d, h) = self.header(o)?;
        let class = self.pkg(&o.pkg)?.export_class(o.idx);
        let name = self.pkg(&o.pkg)?.obj_path(ObjRef::Export(o.idx));
        let sup = i32_at(&d, h + 4);
        let children = i32_at(&d, h + 12);
        let (mut off, mut align) = match self.resolve(&o.pkg, sup)? {
            Some(s) => self.layout(&s, false, prefix)?,
            None => (0, 4),
        };
        if print {
            println!("{prefix}{name} ({class}) super size {off:#x}");
        }
        let mut cur = self.resolve(&o.pkg, children)?;
        let mut last_bool: Option<u32> = None; // bit of the previous bool
        while let Some(c) = cur {
            let ccls = self.pkg(&c.pkg)?.export_class(c.idx);
            let cname = self.pkg(&c.pkg)?.obj_name(ObjRef::Export(c.idx));
            let (cd, chh) = self.header(&c)?;
            let next = i32_at(&cd, chh);
            if ccls.ends_with("Property") {
                let dim = u32_at(&cd, chh + 4);
                let flags = u32_at(&cd, chh + 8) as u64 | ((u32_at(&cd, chh + 12) as u64) << 32);
                let mut p = chh + 4 + 4 + 8 + 8 + 4;
                if flags & 0x20 != 0 {
                    p += 2;
                }
                let (sz, al, extra) = self.prop_size(&c, &ccls, &cd, p)?;
                if ccls == "BoolProperty" {
                    if let Some(bit) = last_bool.filter(|b| *b < 31) {
                        last_bool = Some(bit + 1);
                        if print {
                            println!("{prefix}  {:#06x} bit{:<2} {cname} : Bool", off - 4, bit + 1);
                        }
                        cur = self.resolve(&c.pkg, next)?;
                        continue;
                    }
                    off = off.div_ceil(4) * 4;
                    last_bool = Some(0);
                    if print {
                        println!("{prefix}  {off:#06x} bit0  {cname} : Bool");
                    }
                    off += 4;
                    align = align.max(4);
                } else {
                    last_bool = None;
                    off = off.div_ceil(al) * al;
                    if print {
                        let dims = if dim > 1 { format!("[{dim}]") } else { String::new() };
                        println!("{prefix}  {off:#06x} {cname}{dims} : {} {extra} (size {:#x})", ccls.trim_end_matches("Property"), sz);
                    }
                    off += sz * dim.max(1);
                    align = align.max(al);
                }
            }
            cur = self.resolve(&c.pkg, next)?;
        }
        if class == "ScriptStruct" {
            let n = name.rsplit('.').next().unwrap_or("");
            if matches!(n, "Plane" | "Quat" | "Matrix" | "Vector4" | "LinearColor4") {
                align = align.max(16);
            }
            off = off.div_ceil(align) * align;
        }
        if print {
            println!("{prefix}  size {off:#x} align {align}");
        }
        self.sizes.insert(key, (off, align));
        Ok((off, align))
    }

    fn prop_size(&mut self, c: &Obj, cls: &str, d: &[u8], p: usize) -> Result<(u32, u32, String)> {
        Ok(match cls {
            "ByteProperty" => (1, 1, self.ref_name(&c.pkg, i32_at(d, p))?),
            "IntProperty" | "FloatProperty" | "BoolProperty" => (4, 4, String::new()),
            "NameProperty" => (8, 4, String::new()),
            "StrProperty" => (12, 4, String::new()),
            "ObjectProperty" | "ComponentProperty" | "ClassProperty" => (4, 4, self.ref_name(&c.pkg, i32_at(d, p))?),
            "InterfaceProperty" => (8, 4, String::new()),
            "DelegateProperty" => (12, 4, String::new()),
            "MapProperty" => (60, 4, "(map size guessed)".into()),
            "ArrayProperty" => {
                let inner = self.resolve(&c.pkg, i32_at(d, p))?;
                let mut desc = String::new();
                if let Some(i) = inner {
                    let icls = self.pkg(&i.pkg)?.export_class(i.idx);
                    let (id, ih) = self.header(&i)?;
                    let mut ip = ih + 4 + 4 + 8 + 8 + 4;
                    let fl = u32_at(&id, ih + 8);
                    if fl & 0x20 != 0 {
                        ip += 2;
                    }
                    let (isz, _, iextra) = self.prop_size(&i, &icls, &id, ip)?;
                    desc = format!("of {} {iextra} (elem {isz:#x})", icls.trim_end_matches("Property"));
                }
                (12, 4, desc)
            }
            "StructProperty" => {
                let s = self.resolve(&c.pkg, i32_at(d, p))?.context("struct")?;
                let sn = self.pkg(&s.pkg)?.obj_name(ObjRef::Export(s.idx));
                let (sz, al) = self.layout(&s, false, "")?;
                (sz, al, sn)
            }
            other => bail!("unknown property class {other}"),
        })
    }
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut ctx = Ctx { dir: a[0].clone(), pkgs: HashMap::new(), sizes: HashMap::new() };
    let pkg = a[1].clone();
    for want in &a[2..] {
        let found = {
            let p = ctx.pkg(&pkg)?;
            (0..p.exports.len()).find(|&i| {
                let c = p.export_class(i);
                (c == "Class" || c == "ScriptStruct") && {
                    let path = p.obj_path(ObjRef::Export(i));
                    path == *want || path.ends_with(&format!(".{want}"))
                }
            })
        };
        match found {
            Some(i) => {
                ctx.layout(&Obj { pkg: pkg.clone(), idx: i }, true, "")?;
            }
            None => println!("{want}: not found"),
        }
        println!();
    }
    Ok(())
}
