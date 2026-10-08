//! Reflection on the client side: properties and functions by name through the DLL's `FIELDS`
//! op (research/dishonored/INSTRUMENTATION.md "By-name access"), typed values as JSON, property
//! paths, UFunction calls with JSON arguments and actor spawning.
//!
//! Object specs: `pawn`, `pc`, `camera` (the PlayerCamera), `worldinfo`, `input`
//! (PlayerInput), `engine`, `0x<addr>`, `Default__<Class>` (class default object), or an object
//! name / `Outer.Name` (the first live match; `class` narrows it).
//!
//! Property paths: dot-separated property names, `[i]` for static and dynamic array elements;
//! an object reference is followed when more segments come (`Controller.Pawn.Health`,
//! `WorldInfo.StreamingLevels[0].PackageName`, `Location.Z`).
//!
//! Values: Float / Int as numbers, Byte as a number (or the enum name for enum bytes; both are
//! accepted when writing), Bool as true / false, Name and Str as strings, objects as
//! `"<Class> <Outer.Name>@0x<addr>"` (or null), structs as objects, arrays as lists (at most
//! `ARRAY_LIMIT` elements are decoded, then a `"... N more"` string).
use anyhow::{Context, Result, bail};
use dishonored_probe::{Client, FieldInfo, StructInfo};
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub const ARRAY_LIMIT: usize = 64;
/// UObject.Class.
const OBJ_CLASS: u32 = 0x30;
/// PropertyFlags.
pub const CPF_PARM: u64 = 0x80;
pub const CPF_OUT_PARM: u64 = 0x100;
pub const CPF_RETURN_PARM: u64 = 0x400;
pub const CPF_OPTIONAL_PARM: u64 = 0x10;

/// Cached reflection data of one game session (addresses stay valid for its lifetime).
#[derive(Default)]
pub struct Reflect {
    structs: HashMap<u32, StructInfo>,
    enums: HashMap<u32, Vec<String>>,
    classes: HashMap<String, u32>,
    scratch: u32,
}

/// A resolved property location.
#[derive(Clone, Debug)]
pub struct Place {
    /// Address of the value (element 0 of a static array when `whole_array`).
    pub addr: u32,
    pub prop: FieldInfo,
    /// The path ended on a static array without an index: all `array_dim` elements.
    pub whole_array: bool,
}

pub fn hex(a: u32) -> String {
    format!("{a:#010x}")
}

pub fn parse_addr(s: &str) -> Option<u32> {
    s.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok())
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Status values of the session (`local_pc`, `pawn`, ...).
pub fn status_addr(c: &mut Client, key: &str) -> Result<u32> {
    let v = c.status_value(key)?.unwrap_or_default();
    Ok(u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap_or(0))
}

impl Reflect {
    pub fn info(&mut self, c: &mut Client, st: u32) -> Result<&StructInfo> {
        if !self.structs.contains_key(&st) {
            let i = c.fields(st).with_context(|| format!("fields of {st:#x}"))?;
            self.structs.insert(st, i);
        }
        Ok(&self.structs[&st])
    }

    /// Every property of a struct or class, the supers' first.
    pub fn properties(&mut self, c: &mut Client, st: u32) -> Result<Vec<FieldInfo>> {
        let mut chain = Vec::new();
        let mut s = st;
        while s != 0 && chain.len() < 64 {
            chain.push(s);
            s = self.info(c, s)?.super_struct;
        }
        let mut out = Vec::new();
        for s in chain.into_iter().rev() {
            out.extend(self.info(c, s)?.fields.iter().filter(|f| f.class.ends_with("Property")).cloned());
        }
        Ok(out)
    }

    /// A field (property or function) by name, searching the struct then its supers.
    pub fn field(&mut self, c: &mut Client, st: u32, name: &str, want_function: bool) -> Result<Option<FieldInfo>> {
        let mut s = st;
        let mut guard = 0;
        while s != 0 && guard < 64 {
            let info = self.info(c, s)?;
            if let Some(f) = info.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name) && (f.class == "Function") == want_function) {
                return Ok(Some(f.clone()));
            }
            s = info.super_struct;
            guard += 1;
        }
        Ok(None)
    }

    pub fn enum_names(&mut self, c: &mut Client, e: u32) -> Result<&Vec<String>> {
        if !self.enums.contains_key(&e) {
            let v = c.enum_names(e)?;
            self.enums.insert(e, v);
        }
        Ok(&self.enums[&e])
    }

    pub fn class_named(&mut self, c: &mut Client, name: &str) -> Result<u32> {
        if let Some(&a) = self.classes.get(&name.to_ascii_lowercase()) {
            return Ok(a);
        }
        let a = c.find(name, "Class")?.first().copied().with_context(|| format!("no loaded class {name}"))?;
        self.classes.insert(name.to_ascii_lowercase(), a);
        Ok(a)
    }

    pub fn class_of(&mut self, c: &mut Client, obj: u32) -> Result<u32> {
        let cls = c.read_u32(obj + OBJ_CLASS)?;
        if cls == 0 {
            bail!("{obj:#x} has no class");
        }
        Ok(cls)
    }

    /// Resolve an object spec (module docs).
    pub fn object(&mut self, c: &mut Client, spec: &str, class: Option<&str>) -> Result<u32> {
        let a = match spec {
            "pawn" => status_addr(c, "pawn")?,
            "pc" => status_addr(c, "local_pc")?,
            "engine" => status_addr(c, "gengine")?,
            "input" => status_addr(c, "player_input")?,
            "camera" | "worldinfo" => {
                let pc = status_addr(c, "local_pc")?;
                if pc == 0 {
                    bail!("no player controller");
                }
                let cls = self.class_of(c, pc)?;
                let prop = if spec == "camera" { "PlayerCamera" } else { "WorldInfo" };
                let f = self.field(c, cls, prop, false)?.with_context(|| format!("no {prop}"))?;
                c.read_u32(pc + f.offset)?
            }
            s => match parse_addr(s) {
                Some(a) => a,
                None => {
                    let found = c.find(s, class.unwrap_or(""))?;
                    *found.first().with_context(|| format!("no live object {s}{}", class.map(|c| format!(" of class {c}")).unwrap_or_default()))?
                }
            },
        };
        if a == 0 {
            bail!("object {spec} is not available (no level / pawn yet?)");
        }
        Ok(a)
    }

    /// Resolve a property path on an object.
    pub fn place(&mut self, c: &mut Client, obj: u32, path: &str) -> Result<Place> {
        let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
        if segs.is_empty() {
            bail!("empty property path");
        }
        let mut base = obj;
        let mut st = self.class_of(c, obj)?;
        let mut i = 0;
        loop {
            let seg = segs[i];
            let (name, index) = match seg.split_once('[') {
                Some((n, rest)) => (n, Some(rest.trim_end_matches(']').parse::<usize>().with_context(|| format!("bad index in {seg}"))?)),
                None => (seg, None),
            };
            let prop = self.field(c, st, name, false)?.with_context(|| format!("no property {name} in {}", self.info(c, st).map(|s| s.name.clone()).unwrap_or_default()))?;
            let mut addr = base + prop.offset;
            let mut elem = prop.clone();
            let mut whole = false;
            match index {
                Some(k) if prop.class == "ArrayProperty" => {
                    let arr = c.read(addr, 12)?;
                    let (data, num) = (u32_at(&arr, 0), u32_at(&arr, 4) as usize);
                    if k >= num {
                        bail!("{name}[{k}] out of range ({num})");
                    }
                    let inner = self.inner_of(c, &prop)?;
                    addr = data + k as u32 * inner.element_size;
                    elem = inner;
                }
                Some(k) => {
                    if k >= prop.array_dim as usize {
                        bail!("{name}[{k}] out of range ({})", prop.array_dim);
                    }
                    addr += k as u32 * prop.element_size;
                }
                None => whole = prop.array_dim > 1,
            }
            i += 1;
            if i == segs.len() {
                return Ok(Place { addr, prop: elem, whole_array: whole });
            }
            match elem.class.as_str() {
                "StructProperty" => {
                    base = addr;
                    st = elem.aux;
                }
                "ObjectProperty" | "ComponentProperty" | "ClassProperty" | "InterfaceProperty" => {
                    let o = c.read_u32(addr)?;
                    if o == 0 {
                        bail!("{} is None", segs[..i].join("."));
                    }
                    base = o;
                    st = self.class_of(c, o)?;
                }
                k => bail!("cannot descend into {name} ({k})"),
            }
        }
    }

    /// The inner property of an ArrayProperty (its `aux`), as a field.
    pub fn inner_of(&mut self, c: &mut Client, arr: &FieldInfo) -> Result<FieldInfo> {
        let inner = arr.aux;
        let b = c.read(inner, 0x74)?;
        let cls = c.name_of(inner)?.split(' ').next().unwrap_or("").to_string();
        Ok(FieldInfo {
            addr: inner,
            name: arr.name.clone(),
            class: cls,
            offset: 0,
            element_size: u32_at(&b, 0x40),
            array_dim: 1,
            flags: u32_at(&b, 0x44) as u64 | (u32_at(&b, 0x48) as u64) << 32,
            aux: u32_at(&b, 0x6c),
            aux2: u32_at(&b, 0x70),
        })
    }

    /// Decode the value of a property from bytes that start at the value.
    pub fn decode(&mut self, c: &mut Client, p: &FieldInfo, b: &[u8], depth: u32) -> Result<Value> {
        Ok(match p.class.as_str() {
            "FloatProperty" => json!(f32_at(b, 0)),
            "IntProperty" => json!(u32_at(b, 0) as i32),
            "ByteProperty" => {
                if p.aux != 0 {
                    let names = self.enum_names(c, p.aux)?;
                    match names.get(b[0] as usize) {
                        Some(n) => json!(n),
                        None => json!(b[0]),
                    }
                } else {
                    json!(b[0])
                }
            }
            "BoolProperty" => json!(u32_at(b, 0) & p.aux != 0),
            "NameProperty" => {
                let (i, n) = (u32_at(b, 0), u32_at(b, 4));
                json!(name_text(c, i, n)?)
            }
            "StrProperty" => json!(read_fstring(c, u32_at(b, 0), u32_at(b, 4))?),
            "ObjectProperty" | "ComponentProperty" | "ClassProperty" | "InterfaceProperty" => object_value(c, u32_at(b, 0))?,
            "DelegateProperty" => {
                let o = object_value(c, u32_at(b, 0))?;
                json!({"object": o, "function": name_text(c, u32_at(b, 4), u32_at(b, 8))?})
            }
            "StructProperty" => {
                if depth > 6 {
                    return Ok(json!("<struct>"));
                }
                let mut m = Map::new();
                for f in self.properties(c, p.aux)? {
                    let o = f.offset as usize;
                    let len = (f.element_size * f.array_dim.max(1)) as usize;
                    if o + len > b.len() {
                        continue;
                    }
                    let v = self.decode_dim(c, &f, &b[o..o + len], depth + 1)?;
                    m.insert(f.name.clone(), v);
                }
                Value::Object(m)
            }
            "ArrayProperty" => {
                let (data, num) = (u32_at(b, 0), u32_at(b, 4) as usize);
                let inner = self.inner_of(c, p)?;
                let n = num.min(ARRAY_LIMIT);
                let mut out = Vec::with_capacity(n + 1);
                if n > 0 && depth <= 6 {
                    let es = inner.element_size as usize;
                    let raw = c.read(data, (n * es) as u32)?;
                    for k in 0..n {
                        out.push(self.decode(c, &inner, &raw[k * es..(k + 1) * es], depth + 1)?);
                    }
                }
                if num > n {
                    out.push(json!(format!("... {} more", num - n)));
                }
                Value::Array(out)
            }
            k => json!(format!("<{k}>")),
        })
    }

    /// Decode a property including its static array dimension.
    pub fn decode_dim(&mut self, c: &mut Client, p: &FieldInfo, b: &[u8], depth: u32) -> Result<Value> {
        if p.array_dim <= 1 {
            return self.decode(c, p, b, depth);
        }
        let es = p.element_size as usize;
        let mut v = Vec::new();
        for k in 0..p.array_dim as usize {
            v.push(self.decode(c, p, &b[k * es..(k + 1) * es], depth)?);
        }
        Ok(Value::Array(v))
    }

    pub fn read_place(&mut self, c: &mut Client, pl: &Place) -> Result<Value> {
        let dim = if pl.whole_array { pl.prop.array_dim } else { 1 };
        let b = c.read(pl.addr, pl.prop.element_size * dim)?;
        let mut p = pl.prop.clone();
        if !pl.whole_array {
            p.array_dim = 1;
        }
        self.decode_dim(c, &p, &b, 0)
    }

    /// Encode a JSON value for a property into `out` (exactly `element_size` bytes; `old` is
    /// the current value, kept for bools and fields a struct value leaves out).
    pub fn encode(&mut self, c: &mut Client, p: &FieldInfo, v: &Value, old: &[u8]) -> Result<Vec<u8>> {
        let mut out = old.to_vec();
        out.resize(p.element_size as usize, 0);
        let num = |v: &Value| -> Result<f64> { v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).with_context(|| format!("{}: expected a number, got {v}", p.name)) };
        match p.class.as_str() {
            "FloatProperty" => out[..4].copy_from_slice(&(num(v)? as f32).to_le_bytes()),
            "IntProperty" => out[..4].copy_from_slice(&(num(v)? as i64 as i32).to_le_bytes()),
            "ByteProperty" => {
                out[0] = match v.as_str() {
                    Some(s) if p.aux != 0 && s.parse::<u8>().is_err() => {
                        let names = self.enum_names(c, p.aux)?;
                        names.iter().position(|n| n.eq_ignore_ascii_case(s)).with_context(|| format!("{}: no enum value {s} ({names:?})", p.name))? as u8
                    }
                    _ => num(v)? as u8,
                }
            }
            "BoolProperty" => {
                let on = v.as_bool().or_else(|| v.as_f64().map(|x| x != 0.0)).with_context(|| format!("{}: expected a bool", p.name))?;
                let w = u32_at(&out, 0);
                let w = if on { w | p.aux } else { w & !p.aux };
                out[..4].copy_from_slice(&w.to_le_bytes());
            }
            "NameProperty" => {
                let s = v.as_str().context("expected a name string")?;
                let (i, n) = c.name_index(s)?;
                out[..4].copy_from_slice(&i.to_le_bytes());
                out[4..8].copy_from_slice(&n.to_le_bytes());
            }
            "StrProperty" => {
                let s = v.as_str().context("expected a string")?;
                let fs = c.fstring(s)?;
                for (k, x) in fs.iter().enumerate() {
                    out[k * 4..k * 4 + 4].copy_from_slice(&x.to_le_bytes());
                }
            }
            "ObjectProperty" | "ComponentProperty" | "ClassProperty" | "InterfaceProperty" => {
                let a = match v {
                    Value::Null => 0,
                    Value::Number(n) => n.as_u64().context("object address")? as u32,
                    Value::String(s) => {
                        // `...@0xADDR`, `0xADDR`, or an object spec.
                        match s.rsplit_once('@').and_then(|x| parse_addr(x.1)).or_else(|| parse_addr(s)) {
                            Some(a) => a,
                            None if p.class == "ClassProperty" => self.class_named(c, s)?,
                            None => self.object(c, s, None)?,
                        }
                    }
                    _ => bail!("{}: expected an object", p.name),
                };
                out[..4].copy_from_slice(&a.to_le_bytes());
            }
            "StructProperty" => {
                let fields = self.properties(c, p.aux)?;
                match v {
                    Value::Object(m) => {
                        for (k, x) in m {
                            let f = fields.iter().find(|f| f.name.eq_ignore_ascii_case(k)).with_context(|| format!("{}: no field {k}", p.name))?.clone();
                            let o = f.offset as usize;
                            let b = self.encode(c, &f, x, &out[o..o + f.element_size as usize].to_vec())?;
                            out[o..o + b.len()].copy_from_slice(&b);
                        }
                    }
                    // A list fills the fields in order ([x, y, z] for a Vector).
                    Value::Array(a) => {
                        for (f, x) in fields.iter().zip(a) {
                            let o = f.offset as usize;
                            let b = self.encode(c, f, x, &out[o..o + f.element_size as usize].to_vec())?;
                            out[o..o + b.len()].copy_from_slice(&b);
                        }
                    }
                    _ => bail!("{}: expected an object or a list", p.name),
                }
            }
            k => bail!("{}: writing {k} is not supported", p.name),
        }
        Ok(out)
    }

    pub fn write_place(&mut self, c: &mut Client, pl: &Place, v: &Value) -> Result<()> {
        if pl.whole_array {
            let arr = v.as_array().context("a static array takes a list")?;
            for (k, x) in arr.iter().enumerate().take(pl.prop.array_dim as usize) {
                let a = pl.addr + k as u32 * pl.prop.element_size;
                let old = c.read(a, pl.prop.element_size)?;
                let b = self.encode(c, &pl.prop, x, &old)?;
                c.write(a, &b)?;
            }
            return Ok(());
        }
        let old = c.read(pl.addr, pl.prop.element_size)?;
        let b = self.encode(c, &pl.prop, v, &old)?;
        c.write(pl.addr, &b)?;
        Ok(())
    }

    /// Every property of an object (optionally only names containing `filter`), as JSON.
    pub fn dump_object(&mut self, c: &mut Client, obj: u32, filter: Option<&str>) -> Result<Value> {
        let cls = self.class_of(c, obj)?;
        let size = self.info(c, cls)?.size;
        let raw = c.read(obj, size.max(0x38))?;
        let mut m = Map::new();
        let f = filter.map(|s| s.to_ascii_lowercase());
        for p in self.properties(c, cls)? {
            if f.as_ref().is_some_and(|f| !p.name.to_ascii_lowercase().contains(f.as_str())) {
                continue;
            }
            let o = p.offset as usize;
            let len = (p.element_size * p.array_dim.max(1)) as usize;
            if o + len > raw.len() {
                continue;
            }
            let v = self.decode_dim(c, &p, &raw[o..o + len], 1).unwrap_or_else(|e| json!(format!("<error: {e}>")));
            m.insert(p.name.clone(), v);
        }
        Ok(Value::Object(m))
    }

    /// Call a function by name on an object with JSON arguments (an object by parameter name or
    /// a positional list). Returns the return value and the out parameters.
    pub fn call(&mut self, c: &mut Client, obj: u32, func: &str, args: &Value) -> Result<Value> {
        let (class_hint, fname) = match func.rsplit_once('.') {
            Some((cl, f)) => (Some(cl), f),
            None => (None, func),
        };
        let st = match class_hint {
            Some(cl) => self.class_named(c, cl)?,
            None => self.class_of(c, obj)?,
        };
        let f = self.field(c, st, fname, true)?.with_context(|| format!("no function {func} on {}", c.name_of(obj).unwrap_or_default()))?;
        let parms_size = (f.aux2 & 0xffff) as usize;
        let params: Vec<FieldInfo> = self.info(c, f.addr)?.fields.iter().filter(|p| p.flags & CPF_PARM != 0).cloned().collect();
        let mut buf = vec![0u8; parms_size.max(4)];
        let inputs: Vec<&FieldInfo> = params.iter().filter(|p| p.flags & CPF_RETURN_PARM == 0).collect();
        match args {
            Value::Null => {}
            Value::Array(a) => {
                if a.len() > inputs.len() {
                    bail!("{func} takes {} arguments ({})", inputs.len(), inputs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
                }
                for (p, v) in inputs.iter().zip(a) {
                    let o = p.offset as usize;
                    let b = self.encode(c, p, v, &buf[o..o + p.element_size as usize].to_vec())?;
                    buf[o..o + b.len()].copy_from_slice(&b);
                }
            }
            Value::Object(m) => {
                for (k, v) in m {
                    let p = inputs.iter().find(|p| p.name.eq_ignore_ascii_case(k)).with_context(|| format!("{func} has no parameter {k} ({})", inputs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")))?;
                    let o = p.offset as usize;
                    let b = self.encode(c, p, v, &buf[o..o + p.element_size as usize].to_vec())?;
                    buf[o..o + b.len()].copy_from_slice(&b);
                }
            }
            _ => bail!("args: an object by parameter name or a list"),
        }
        let after = if f.element_size != 0 {
            // A function with a native index (operators, `SetLocation`...): `ProcessEvent`
            // returns early for these, so run its exec thunk directly with a frame like the
            // one ProcessEvent builds: Code = the function's own parameter script
            // (`EX_NativeParm` per parameter), Locals = our parameter block.
            self.call_native_exec(c, obj, f.addr, &buf, (f.aux2 >> 16) as usize)?
        } else {
            c.call_func(obj, f.addr, &buf)?
        };
        let mut out = Map::new();
        for p in &params {
            if p.flags & (CPF_OUT_PARM | CPF_RETURN_PARM) == 0 {
                continue;
            }
            let o = p.offset as usize;
            let len = (p.element_size * p.array_dim.max(1)) as usize;
            let v = self.decode_dim(c, p, &after[o..o + len], 0)?;
            let key = if p.flags & CPF_RETURN_PARM != 0 { "return".to_string() } else { p.name.clone() };
            out.insert(key, v);
        }
        Ok(Value::Object(out))
    }

    /// Scratch memory in the game for native calls (one block per client session, reused).
    fn scratch(&mut self, c: &mut Client) -> Result<u32> {
        if self.scratch == 0 {
            self.scratch = c.alloc(0x2000)?;
        }
        Ok(self.scratch)
    }

    /// `UFunction::Func(this, FFrame&, Result)` with a frame over our parameter block.
    /// FFrame (this build, seen at execSpawn's entry): FOutputDevice {vtable 0x00FB944C,
    /// bAllowSuppression 1, bSuppressEventTag 0, bAutoEmitLineTerminator 1}, Node +0x10,
    /// Object +0x14, Code +0x18, Locals +0x1C, PreviousFrame +0x20, OutParms +0x24.
    fn call_native_exec(&mut self, c: &mut Client, obj: u32, func: u32, parms: &[u8], ret_off: usize) -> Result<Vec<u8>> {
        if parms.len() > 0x1000 {
            bail!("parameter block too large");
        }
        let base = self.scratch(c)?;
        let (frame, locals) = (base, base + 0x100);
        let script = c.read(func + 0x50, 4)?;
        let code = u32::from_le_bytes(script[..4].try_into().unwrap());
        let thunk = c.read_u32(func + 0x9c)?;
        if code == 0 || thunk < 0x401000 {
            bail!("native function {func:#x} has no parameter script or thunk");
        }
        let mut fr = Vec::new();
        for w in [0x00fb_944c, 1, 0, 1, func, obj, code, locals, 0, 0, 0, 0, 0, 0, 0, 0u32] {
            fr.extend(w.to_le_bytes());
        }
        c.write(frame, &fr)?;
        c.write(locals, parms)?;
        c.native_call(thunk, obj, &[frame, locals + ret_off as u32], false)?;
        Ok(c.read(locals, parms.len() as u32)?)
    }

    /// Describe a function's parameters (name, type, flags).
    pub fn signature(&mut self, c: &mut Client, func: u32) -> Result<Value> {
        let params: Vec<FieldInfo> = self.info(c, func)?.fields.iter().filter(|p| p.flags & CPF_PARM != 0).cloned().collect();
        Ok(Value::Array(
            params
                .iter()
                .map(|p| {
                    let mut kind = vec![];
                    if p.flags & CPF_OPTIONAL_PARM != 0 {
                        kind.push("optional");
                    }
                    if p.flags & CPF_OUT_PARM != 0 {
                        kind.push("out");
                    }
                    if p.flags & CPF_RETURN_PARM != 0 {
                        kind.push("return");
                    }
                    json!({"name": p.name, "type": p.class, "flags": kind})
                })
                .collect(),
        ))
    }
}

/// Text of an FName (index, number): `Base_N` for number N + 1.
pub fn name_text(c: &mut Client, idx: u32, number: u32) -> Result<String> {
    let s = name_str(c, idx)?;
    Ok(if number > 0 { format!("{s}_{}", number - 1) } else { s })
}

/// Text of an FName index (GNames read through the pipe).
pub fn name_str(c: &mut Client, idx: u32) -> Result<String> {
    thread_local! {
        static GN: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
    }
    let (mut gnames, mut off) = GN.with(|g| g.get());
    if gnames == 0 {
        gnames = status_addr(c, "gnames")?;
        off = status_addr(c, "name_text_off")?;
        GN.with(|g| g.set((gnames, off)));
    }
    let data = c.read_u32(gnames)?;
    let e = c.read_u32(data + 4 * idx)?;
    let b = c.read(e + off, 64).or_else(|_| c.read(e + off, 16))?;
    Ok(b.iter().take_while(|&&x| x != 0).map(|&x| x as char).collect())
}

pub fn read_fstring(c: &mut Client, data: u32, num: u32) -> Result<String> {
    if data == 0 || num == 0 || num > 1 << 16 {
        return Ok(String::new());
    }
    let b = c.read(data, num * 2)?;
    let w: Vec<u16> = b.chunks(2).map(|x| u16::from_le_bytes([x[0], x[1]])).take_while(|&x| x != 0).collect();
    Ok(String::from_utf16_lossy(&w))
}

pub fn object_value(c: &mut Client, a: u32) -> Result<Value> {
    if a == 0 {
        return Ok(Value::Null);
    }
    Ok(json!(format!("{}@{}", c.name_of(a)?, hex(a))))
}
