//! UnrealScript bytecode decompiler for the original's script (UE3 file version 801).
//!
//! The token set is UE3's after the `RangeConst` removal (as UELib's `UnshiftTokens3`,
//! research/dishonored/EXISTING_WORK.md): 0x00-0x5A expression tokens, 0x60-0x6F extended
//! natives (index = (token - 0x60) << 8 | next byte), 0x70+ natives by index. Object references
//! are 4 bytes serialized and 8 in memory; jump offsets are in memory bytes, so the parser keeps
//! both positions and a function decompiles only when its serialized and in-memory sizes both
//! end exactly where the export says (`ScriptStorageSize`, `ScriptBytecodeSize`).
//!
//! Control flow is rebuilt from the jumps (if / else, while, do-until, switch, foreach, break,
//! continue); anything that does not fit a structure is kept as a labelled `goto`, so the output
//! is always complete and faithful even where it is not idiomatic.

use super::script::*;
use anyhow::{Result, bail, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

/// A rendered expression and its binding strength (0 = atom; operators use their UnrealScript
/// precedence, a higher number binds looser).
#[derive(Debug, Clone)]
struct E {
    s: String,
    p: u8,
    call: bool,
}

fn atom(s: impl Into<String>) -> E {
    E { s: s.into(), p: 0, call: false }
}

const P_TERNARY: u8 = 200;

/// One parsed top-level token.
#[derive(Debug, Clone)]
enum Node {
    E(E),
    Let(E, E),
    Return(Option<E>),
    Jump(u32),
    JumpIfNot(u32, E),
    /// The expression and, for an enum, its value names.
    Switch(E, Option<Vec<String>>),
    Case(u32, Option<E>),
    Iter(String, u32),
    IterNext,
    IterPop,
    Stop,
    Goto(E),
    Assert(E),
    Labels(Vec<(String, u32)>),
    EndOfScript,
    Nothing,
    DefaultParm(E),
    EndParm,
    EndParms,
    FilterEditorOnly(u32),
    EmptyParm,
}

#[derive(Debug, Clone)]
struct Stmt {
    /// In-memory position of the token.
    pos: u32,
    /// In-memory position after it.
    end: u32,
    node: Node,
}

struct Parser<'a> {
    db: &'a Db,
    pi: u32,
    d: &'a [u8],
    at: usize,
    mem: u32,
    /// Function being decompiled (for `super` calls).
    func_name: String,
    class_key: Option<Key>,
}

fn esc_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn fmt_f32(f: f32) -> String {
    let s = format!("{f}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") { s } else { format!("{s}.0") }
}

fn cast_name(c: u8) -> Option<&'static str> {
    Some(match c {
        0x36 | 0x46 => "",
        0x37 => "string",
        0x38 => "bool",
        0x39 => "Vector",
        0x3A => "int",
        0x3B => "bool",
        0x3C => "float",
        0x3D => "byte",
        0x3E => "bool",
        0x3F => "float",
        0x40 => "byte",
        0x41 => "int",
        0x42 => "float",
        0x43 => "byte",
        0x44 => "int",
        0x45 => "bool",
        0x47 | 0x48 => "bool",
        0x49 => "byte",
        0x4A => "int",
        0x4B => "bool",
        0x4C => "float",
        0x4D => "Vector",
        0x4E => "Rotator",
        0x4F => "bool",
        0x50 => "Rotator",
        0x51 => "bool",
        0x52..=0x5A => "string",
        0x60 => "name",
        _ => return None,
    })
}

impl<'a> Parser<'a> {
    fn u8(&mut self) -> Result<u8> {
        ensure!(self.at < self.d.len(), "bytecode ends inside a token at {:#x}", self.at);
        let v = self.d[self.at];
        self.at += 1;
        self.mem += 1;
        Ok(v)
    }
    fn peek(&self) -> Result<u8> {
        ensure!(self.at < self.d.len(), "bytecode ends at {:#x}", self.at);
        Ok(self.d[self.at])
    }
    fn u16(&mut self) -> Result<u16> {
        ensure!(self.at + 2 <= self.d.len(), "bytecode ends inside a word");
        let v = u16::from_le_bytes([self.d[self.at], self.d[self.at + 1]]);
        self.at += 2;
        self.mem += 2;
        Ok(v)
    }
    fn i32(&mut self) -> Result<i32> {
        ensure!(self.at + 4 <= self.d.len(), "bytecode ends inside a dword");
        let v = i32::from_le_bytes(self.d[self.at..self.at + 4].try_into().unwrap());
        self.at += 4;
        self.mem += 4;
        Ok(v)
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.i32()? as u32))
    }
    /// Object reference: 4 bytes serialized, a pointer (8 bytes) in memory.
    fn obj(&mut self) -> Result<i32> {
        let v = self.i32()?;
        self.mem += 4;
        let n = self.db.pkgs[self.pi as usize].imports.len() as i32;
        let x = self.db.pkgs[self.pi as usize].exports.len() as i32;
        ensure!(v >= -n && v <= x, "object reference {v} out of range");
        Ok(v)
    }
    fn name(&mut self) -> Result<String> {
        let idx = self.i32()?;
        let num = self.i32()?;
        let p = &self.db.pkgs[self.pi as usize];
        ensure!(idx >= 0 && (idx as usize) < p.names.len(), "name index {idx} out of range");
        Ok(p.name(ue3_package::FName { index: idx, number: num }))
    }
    /// Value names of the enum a byte property holds.
    fn enum_names(&self, prop: i32) -> Option<Vec<String>> {
        let k = self.db.resolve(self.pi, prop)?;
        let p = self.db.props.get(&k)?;
        if p.class != "ByteProperty" {
            return None;
        }
        let e = self.db.resolve(k.0, *p.refs.first()?)?;
        self.db.enum_values(e).ok()
    }

    fn obj_name(&self, r: i32) -> String {
        if r == 0 { "None".into() } else { self.db.ref_name(self.pi, r) }
    }

    fn expr(&mut self) -> Result<E> {
        let at = self.at;
        match self.node()? {
            Node::E(e) => Ok(e),
            Node::Nothing | Node::EmptyParm => Ok(atom("")),
            other => bail!("statement token {other:?} inside an expression at {at:#x}"),
        }
    }

    /// Arguments up to `EndFunctionParms`.
    fn args(&mut self) -> Result<Vec<E>> {
        let mut out = Vec::new();
        loop {
            if self.peek()? == 0x16 {
                self.u8()?;
                break;
            }
            if self.peek()? == 0x41 {
                self.node()?;
                continue;
            }
            out.push(self.expr()?);
            ensure!(out.len() < 64, "runaway argument list");
        }
        while out.last().is_some_and(|e| e.s.is_empty()) {
            out.pop();
        }
        Ok(out)
    }

    fn call(name: &str, args: &[E]) -> E {
        let a: Vec<&str> = args.iter().map(|e| e.s.as_str()).collect();
        E { s: format!("{name}({})", a.join(", ")), p: 0, call: true }
    }

    fn operator(sym: &str, prec: u8, f: &FuncDef, args: Vec<E>) -> E {
        let wrap = |e: &E, limit: u8, strict: bool| -> String {
            if e.p == 0 || (strict && e.p < limit) || (!strict && e.p <= limit) { e.s.clone() } else { format!("({})", e.s) }
        };
        let alpha = sym.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        if f.flags & FUNC_PRE_OPERATOR != 0 && !args.is_empty() {
            let o = if args[0].p != 0 { format!("({})", args[0].s) } else { args[0].s.clone() };
            return E { s: if alpha { format!("{sym} {o}") } else { format!("{sym}{o}") }, p: 0, call: false };
        }
        if args.len() >= 2 {
            let p = if prec == 0 { 100 } else { prec };
            let l = wrap(&args[0], p, false);
            let r = wrap(&args[1], p, true);
            return E { s: format!("{l} {sym} {r}"), p, call: false };
        }
        if args.len() == 1 {
            let o = if args[0].p != 0 { format!("({})", args[0].s) } else { args[0].s.clone() };
            return E { s: if alpha { format!("{o} {sym}") } else { format!("{o}{sym}") }, p: 0, call: false };
        }
        Self::call(sym, &args)
    }

    fn native(&mut self, idx: u16) -> Result<E> {
        let args = self.args()?;
        let Some(k) = self.db.natives.get(&idx) else {
            return Ok(Self::call(&format!("__NFUN_{idx}__"), &args));
        };
        let f = &self.db.funcs[k];
        if f.is_operator() {
            Ok(Self::operator(&f.friendly, f.oper_precedence, f, args))
        } else {
            Ok(Self::call(&f.name, &args))
        }
    }

    fn node(&mut self) -> Result<Node> {
        let at = self.at;
        let op = self.u8()?;
        if op >= 0x70 {
            return Ok(Node::E(self.native(op as u16)?));
        }
        if op >= 0x60 {
            let lo = self.u8()?;
            return Ok(Node::E(self.native((((op - 0x60) as u16) << 8) | lo as u16)?));
        }
        Ok(match op {
            0x00 | 0x01 | 0x03 | 0x48 | 0x29 => {
                let r = self.obj()?;
                Node::E(atom(self.obj_name(r)))
            }
            0x02 => {
                let r = self.obj()?;
                Node::E(atom(format!("default.{}", self.obj_name(r))))
            }
            0x04 => {
                let e = self.expr()?;
                Node::Return(if e.s.is_empty() { None } else { Some(e) })
            }
            0x05 => {
                let prop = self.obj()?;
                let _size = self.u8()?;
                let names = self.enum_names(prop);
                Node::Switch(self.expr()?, names)
            }
            0x06 => Node::Jump(self.u16()? as u32),
            0x07 => {
                let t = self.u16()? as u32;
                Node::JumpIfNot(t, self.expr()?)
            }
            0x08 => Node::Stop,
            0x09 => {
                let _line = self.u16()?;
                let _debug = self.u8()?;
                Node::Assert(self.expr()?)
            }
            0x0A => {
                let t = self.u16()? as u32;
                if t == 0xFFFF { Node::Case(t, None) } else { Node::Case(t, Some(self.expr()?)) }
            }
            0x0B => Node::Nothing,
            0x0C => {
                let mut v = Vec::new();
                loop {
                    let n = self.name()?;
                    let off = self.i32()?;
                    if n.eq_ignore_ascii_case("None") {
                        break;
                    }
                    v.push((n, off as u32));
                    ensure!(v.len() < 1024, "runaway label table");
                }
                Node::Labels(v)
            }
            0x0D => Node::Goto(self.expr()?),
            0x0E => {
                let _ret = self.obj()?;
                Node::E(self.expr()?)
            }
            0x0F | 0x14 | 0x44 => {
                let l = self.expr()?;
                let r = self.expr()?;
                Node::Let(l, r)
            }
            0x10 | 0x1A => {
                let i = self.expr()?;
                let a = self.expr()?;
                Node::E(atom(format!("{}[{}]", a.s, i.s)))
            }
            0x11 => {
                let outer = self.expr()?;
                let name = self.expr()?;
                let flags = self.expr()?;
                let class = self.expr()?;
                let template = self.expr()?;
                let mut a: Vec<String> = vec![outer.s, name.s, flags.s];
                while a.last().is_some_and(|s| s.is_empty()) {
                    a.pop();
                }
                let head = if a.is_empty() { "new".to_string() } else { format!("new ({})", a.join(", ")) };
                let t = if template.s.is_empty() { String::new() } else { format!("({})", template.s) };
                Node::E(E { s: format!("{head} {}{t}", class.s), p: 0, call: true })
            }
            0x12 | 0x19 => {
                let o = self.expr()?;
                let _skip = self.u16()?;
                let _prop = self.obj()?;
                let _size = self.u8()?;
                let m = self.expr()?;
                let left = if o.p != 0 { format!("({})", o.s) } else { o.s };
                let s = if op == 0x12 && m.call { format!("{left}.static.{}", m.s) } else { format!("{left}.{}", m.s) };
                Node::E(E { s, p: 0, call: m.call })
            }
            0x13 => {
                let c = self.obj()?;
                let e = self.expr()?;
                Node::E(atom(format!("class<{}>({})", self.obj_name(c), e.s)))
            }
            0x15 => Node::EndParm,
            0x16 => Node::EndParms,
            0x17 => Node::E(atom("self")),
            0x18 => {
                let _size = self.u16()?;
                Node::E(self.expr()?)
            }
            0x1B => {
                let n = self.name()?;
                let a = self.args()?;
                Node::E(Self::call(&n, &a))
            }
            0x1C => {
                let r = self.obj()?;
                let a = self.args()?;
                let name = self.obj_name(r);
                match self.db.func(self.pi, r) {
                    Some(f) if f.is_operator() => Node::E(Self::operator(&f.friendly, f.oper_precedence, f, a)),
                    Some(f) => {
                        // A non-final, non-private, non-static function called by object: `super`.
                        let sup = f.flags & (FUNC_FINAL | FUNC_PRIVATE | FUNC_STATIC) == 0;
                        if sup {
                            let direct = self.class_key.and_then(|c| self.db.super_name(c)).is_some_and(|s| s.eq_ignore_ascii_case(&f.owner_class));
                            let head = if direct || f.state.is_some() && name == self.func_name { "super".to_string() } else { format!("super({})", f.owner_class) };
                            Node::E(Self::call(&format!("{head}.{name}"), &a))
                        } else {
                            Node::E(Self::call(&name, &a))
                        }
                    }
                    None => Node::E(Self::call(&name, &a)),
                }
            }
            0x1D => Node::E(atom(self.i32()?.to_string())),
            0x1E => Node::E(atom(fmt_f32(self.f32()?))),
            0x1F => {
                let start = self.at;
                while self.peek()? != 0 {
                    self.u8()?;
                }
                let s: String = self.d[start..self.at].iter().map(|&b| b as char).collect();
                self.u8()?;
                Node::E(atom(esc_str(&s)))
            }
            0x20 => {
                let r = self.obj()?;
                if r == 0 {
                    Node::E(atom("None"))
                } else {
                    let cls = self.db.ref_class(self.pi, r);
                    let p = &self.db.pkgs[self.pi as usize];
                    let path = p.obj_path(ue3_package::ObjRef::from_index(r));
                    let path = if cls == "Class" { self.obj_name(r) } else { path };
                    Node::E(atom(format!("{}'{}'", if cls == "Class" { "class".to_string() } else { cls }, path)))
                }
            }
            0x21 => Node::E(atom(format!("'{}'", self.name()?))),
            0x22 => {
                let (a, b, c) = (self.i32()?, self.i32()?, self.i32()?);
                Node::E(atom(format!("rot({a}, {b}, {c})")))
            }
            0x23 => {
                let (a, b, c) = (self.f32()?, self.f32()?, self.f32()?);
                Node::E(atom(format!("vect({}, {}, {})", fmt_f32(a), fmt_f32(b), fmt_f32(c))))
            }
            0x24 | 0x2C => Node::E(atom(self.u8()?.to_string())),
            0x25 => Node::E(atom("0")),
            0x26 => Node::E(atom("1")),
            0x27 => Node::E(atom("true")),
            0x28 => Node::E(atom("false")),
            0x2A | 0x3F => Node::E(atom("None")),
            0x2D => Node::E(self.expr()?),
            0x2E | 0x52 => {
                let c = self.obj()?;
                let e = self.expr()?;
                Node::E(atom(format!("{}({})", self.obj_name(c), e.s)))
            }
            0x2F => {
                let e = self.expr()?;
                let end = self.u16()? as u32;
                Node::Iter(e.s, end)
            }
            0x30 => Node::IterPop,
            0x31 => Node::IterNext,
            0x32 | 0x33 => {
                let _s = self.obj()?;
                let l = self.expr()?;
                let r = self.expr()?;
                Node::E(E { s: format!("{} {} {}", l.s, if op == 0x32 { "==" } else { "!=" }, r.s), p: 24, call: false })
            }
            0x34 => {
                let mut s = String::new();
                loop {
                    let c = self.u16()?;
                    if c == 0 {
                        break;
                    }
                    s.push(char::from_u32(c as u32).unwrap_or('?'));
                }
                Node::E(atom(esc_str(&s)))
            }
            0x35 => {
                let field = self.obj()?;
                let _st = self.obj()?;
                let _copy = self.u8()?;
                let _modify = self.u8()?;
                let e = self.expr()?;
                let left = if e.p != 0 { format!("({})", e.s) } else { e.s };
                Node::E(atom(format!("{left}.{}", self.obj_name(field))))
            }
            0x36 => {
                let e = self.expr()?;
                Node::E(atom(format!("{}.Length", e.s)))
            }
            0x37 => {
                let n = self.name()?;
                let a = self.args()?;
                Node::E(Self::call(&format!("global.{n}"), &a))
            }
            0x38 => {
                let c = self.u8()?;
                let e = self.expr()?;
                match cast_name(c) {
                    Some("") => Node::E(e),
                    Some(t) => Node::E(atom(format!("{t}({})", e.s))),
                    None => bail!("unknown primitive cast {c:#x}"),
                }
            }
            0x39 | 0x40 | 0x54 => {
                let a = self.expr()?;
                let args = self.args()?;
                let m = match op {
                    0x39 => "Insert",
                    0x40 => "Remove",
                    _ => "Add",
                };
                Node::E(Self::call(&format!("{}.{m}", a.s), &args))
            }
            0x3A => {
                let _r = self.obj()?;
                Node::E(atom(""))
            }
            0x3B..=0x3E => {
                let l = self.expr()?;
                let r = self.expr()?;
                let _end = self.args()?;
                let o = if op == 0x3B || op == 0x3D { "==" } else { "!=" };
                Node::E(E { s: format!("{} {o} {}", l.s, r.s), p: 24, call: false })
            }
            0x41 => {
                let _v = self.i32()?;
                let _line = self.i32()?;
                let _pos = self.i32()?;
                let _op = self.u8()?;
                return self.node();
            }
            0x42 => {
                let _local = self.u8()?;
                let _prop = self.obj()?;
                let n = self.name()?;
                let a = self.args()?;
                Node::E(Self::call(&n, &a))
            }
            0x43 => {
                let n = self.name()?;
                let _p = self.obj()?;
                Node::E(atom(n))
            }
            0x45 => {
                let c = self.expr()?;
                let _s1 = self.u16()?;
                let a = self.expr()?;
                let _s2 = self.u16()?;
                let b = self.expr()?;
                Node::E(E { s: format!("{} ? {} : {}", c.s, a.s, b.s), p: P_TERNARY, call: false })
            }
            0x46 | 0x47 | 0x55 | 0x56 | 0x57 | 0x59 => {
                let a = self.expr()?;
                let _skip = self.u16()?;
                let args = self.args()?;
                let m = match op {
                    0x46 | 0x47 => "Find",
                    0x55 => "AddItem",
                    0x56 => "RemoveItem",
                    0x57 => "InsertItem",
                    _ => "Sort",
                };
                Node::E(Self::call(&format!("{}.{m}", a.s), &args))
            }
            0x49 => {
                let _size = self.u16()?;
                let e = self.expr()?;
                let end = self.u8()?;
                ensure!(end == 0x15, "default parameter value without EndParmValue ({end:#x})");
                Node::DefaultParm(e)
            }
            0x4A => Node::EmptyParm,
            0x4B => Node::E(atom(self.name()?)),
            0x50 => Node::E(atom("/*undefined*/")),
            0x51 => Node::E(self.expr()?),
            0x53 => Node::EndOfScript,
            0x58 => {
                let arr = self.expr()?;
                let item = self.expr()?;
                let with_index = self.u8()?;
                let index = self.expr()?;
                let end = self.u16()? as u32;
                let s = if with_index != 0 { format!("{}({}, {})", arr.s, item.s, index.s) } else { format!("{}({})", arr.s, item.s) };
                Node::Iter(s, end)
            }
            0x5A => Node::FilterEditorOnly(self.u16()? as u32),
            other => bail!("unknown token {other:#04x} at {at:#x}"),
        })
    }
}

/// Parse a whole bytecode blob into statements; checks both sizes.
fn parse(db: &Db, key: Key, h: &StructHeader, d: &[u8], func_name: &str, class_key: Option<Key>) -> Result<Vec<Stmt>> {
    let mut p = Parser { db, pi: key.0, d, at: 0, mem: 0, func_name: func_name.to_string(), class_key };
    let mut out = Vec::new();
    while p.at < d.len() {
        let pos = p.mem;
        let node = p.node()?;
        out.push(Stmt { pos, end: p.mem, node });
    }
    ensure!(p.at == d.len(), "parse ended at {:#x} of {:#x}", p.at, d.len());
    ensure!(p.mem == h.bytecode_size, "in-memory size {:#x}, the export says {:#x}", p.mem, h.bytecode_size);
    Ok(out)
}

// ------------------------------------------------------------------------- structuring

#[derive(Clone, Copy)]
struct Loop {
    brk: u32,
    cont: Option<u32>,
    /// A `for` loop's step: `continue` jumps there.
    step: Option<u32>,
    /// The loop's back jump: a jump there is a `continue` too (or the end of an if/else).
    edge: Option<u32>,
}

impl Loop {
    fn is_cont(&self, t: u32) -> bool {
        self.cont == Some(t) || self.step == Some(t)
    }
}

struct Out {
    lines: Vec<(usize, String, Option<u32>)>,
    gotos: BTreeSet<u32>,
}

impl Out {
    fn line(&mut self, ind: usize, s: String, pos: Option<u32>) {
        self.lines.push((ind, s, pos));
    }
}

struct Ctx<'s> {
    st: &'s [Stmt],
    /// pos -> index
    at: HashMap<u32, usize>,
}

impl<'s> Ctx<'s> {
    fn idx(&self, pos: u32, lo: usize, hi: usize) -> Option<usize> {
        if hi < self.st.len() && pos == self.st[hi].pos || hi == self.st.len() && pos == self.st.last().map(|s| s.end).unwrap_or(0) {
            return Some(hi);
        }
        self.at.get(&pos).copied().filter(|&i| i >= lo && i <= hi)
    }
    fn end_pos(&self, hi: usize) -> u32 {
        if hi < self.st.len() { self.st[hi].pos } else { self.st.last().map(|s| s.end).unwrap_or(0) }
    }

    fn jump(&self, out: &mut Out, ind: usize, t: u32, here: &Stmt, loops: &[Loop], prefix: &str, next_pos: u32) {
        if let Some(l) = loops.last() {
            if t == l.brk {
                out.line(ind, format!("{prefix}break;"), Some(here.pos));
                return;
            }
            if l.is_cont(t) || l.edge == Some(t) {
                out.line(ind, format!("{prefix}continue;"), Some(here.pos));
                return;
            }
        }
        // `continue` of an enclosing loop through switches.
        for l in loops.iter().rev() {
            if l.is_cont(t) {
                out.line(ind, format!("{prefix}continue;"), Some(here.pos));
                return;
            }
            if l.cont.is_some() {
                break;
            }
        }
        if t == next_pos && prefix.is_empty() {
            return;
        }
        out.gotos.insert(t);
        out.line(ind, format!("{prefix}goto J0x{t:X};"), Some(here.pos));
    }

    /// Structure statements `lo..hi` at indent `ind`.
    fn block(&self, out: &mut Out, lo: usize, hi: usize, ind: usize, loops: &mut Vec<Loop>) {
        let st = self.st;
        let mut i = lo;
        while i < hi {
            let s = &st[i];
            // do { } until(): a backward JumpIfNot further on that targets this statement.
            if let Some(j) = (i + 1..hi).rev().find(|&j| matches!(&st[j].node, Node::JumpIfNot(t, _) if *t == s.pos)) {
                let Node::JumpIfNot(_, c) = &st[j].node else { unreachable!() };
                out.line(ind, "do".into(), Some(s.pos));
                out.line(ind, "{".into(), None);
                loops.push(Loop { brk: st[j].end, cont: Some(st[j].pos), step: None, edge: None });
                self.block(out, i, j, ind + 1, loops);
                loops.pop();
                out.line(ind, format!("}} until({});", c.s), Some(st[j].pos));
                i = j + 1;
                continue;
            }
            let next_pos = self.end_pos(i + 1);
            match &s.node {
                Node::JumpIfNot(t, c) => {
                    let t = *t;
                    if t > s.pos {
                        if let Some(k) = self.idx(t, i + 1, hi) {
                            // while: the block ends with a jump back to the condition.
                            if k > i + 1 {
                                if let Node::Jump(back) = st[k - 1].node {
                                    if back == s.pos {
                                        // for: a step statement before the back jump that a `continue` jumps to.
                                        let step = (k >= i + 3)
                                            .then(|| k - 2)
                                            .filter(|&p| matches!(st[p].node, Node::Let(..) | Node::E(_)))
                                            .filter(|&p| (i + 1..p).any(|x| matches!(st[x].node, Node::Jump(j) if j == st[p].pos)));
                                        let body_end = match step {
                                            Some(p) => {
                                                let text = simple(&st[p].node).unwrap_or_default();
                                                out.line(ind, format!("for (; {}; {})", c.s, text.trim_end_matches(';')), Some(s.pos));
                                                p
                                            }
                                            None => {
                                                out.line(ind, format!("while ({})", c.s), Some(s.pos));
                                                k - 1
                                            }
                                        };
                                        out.line(ind, "{".into(), None);
                                        loops.push(Loop { brk: t, cont: Some(s.pos), step: step.map(|p| st[p].pos), edge: if step.is_none() { Some(st[k - 1].pos) } else { None } });
                                        self.block(out, i + 1, body_end, ind + 1, loops);
                                        loops.pop();
                                        out.line(ind, "}".into(), None);
                                        i = k;
                                        continue;
                                    }
                                    // if / else: the then-block jumps over the else-block.
                                    let is_loop_exit = loops.last().is_some_and(|l| l.brk == back || l.is_cont(back));
                                    if back > t && !is_loop_exit {
                                        if let Some(k2) = self.idx(back, k, hi) {
                                            out.line(ind, format!("if ({})", c.s), Some(s.pos));
                                            out.line(ind, "{".into(), None);
                                            self.block(out, i + 1, k - 1, ind + 1, loops);
                                            out.line(ind, "}".into(), None);
                                            // else if
                                            if k2 == k + 1 || !matches!(st[k].node, Node::JumpIfNot(..)) || !self.single_if(k, k2) {
                                                out.line(ind, "else".into(), None);
                                                out.line(ind, "{".into(), None);
                                                self.block(out, k, k2, ind + 1, loops);
                                                out.line(ind, "}".into(), None);
                                            } else {
                                                let mark = out.lines.len();
                                                self.block(out, k, k2, ind, loops);
                                                if mark < out.lines.len() {
                                                    let l = &mut out.lines[mark];
                                                    l.1 = format!("else {}", l.1);
                                                }
                                            }
                                            i = k2;
                                            continue;
                                        }
                                    }
                                }
                            }
                            out.line(ind, format!("if ({})", c.s), Some(s.pos));
                            out.line(ind, "{".into(), None);
                            self.block(out, i + 1, k, ind + 1, loops);
                            out.line(ind, "}".into(), None);
                            i = k;
                            continue;
                        }
                    }
                    self.jump(out, ind, t, s, loops, &format!("if (!({})) ", c.s), next_pos);
                }
                Node::Jump(t) => self.jump(out, ind, *t, s, loops, "", next_pos),
                Node::Iter(e, t) => {
                    let t = *t;
                    if let Some(mut k) = self.idx(t, i + 1, hi) {
                        let mut body_end = k;
                        if body_end > i + 1 && matches!(st[body_end - 1].node, Node::IterNext) {
                            body_end -= 1;
                        }
                        let cont = st.get(body_end).map(|x| x.pos);
                        // `break` jumps to the IteratorPop the loop ends on.
                        let brk = t;
                        out.line(ind, format!("foreach {e}"), Some(s.pos));
                        out.line(ind, "{".into(), None);
                        loops.push(Loop { brk, cont: if body_end < k { cont } else { None }, step: None, edge: None });
                        self.block(out, i + 1, body_end, ind + 1, loops);
                        loops.pop();
                        out.line(ind, "}".into(), None);
                        if k < hi && matches!(st[k].node, Node::IterPop) {
                            k += 1;
                        }
                        i = k;
                        continue;
                    }
                    out.line(ind, format!("foreach {e} // ends at J0x{t:X}"), Some(s.pos));
                    out.gotos.insert(t);
                }
                Node::Switch(e, names) => {
                    if let Some(n) = self.switch(out, i, hi, ind, e, names.as_deref(), loops) {
                        i = n;
                        continue;
                    }
                    out.line(ind, format!("switch ({}) // unstructured", e.s), Some(s.pos));
                }
                Node::Case(t, v) => match v {
                    Some(v) => out.line(ind, format!("case {}: // next J0x{t:X}", v.s), Some(s.pos)),
                    None => out.line(ind, "default:".into(), Some(s.pos)),
                },
                Node::IterNext => {
                    // IteratorNext + a jump to the loop's end: `break`.
                    if let (Some(Node::Jump(t)), Some(l)) = (st.get(i + 1).map(|x| &x.node), loops.last()) {
                        if *t == l.brk && i + 1 < hi {
                            out.line(ind, "break;".into(), Some(s.pos));
                            i += 2;
                            continue;
                        }
                    }
                    out.line(ind, "continue;".into(), Some(s.pos));
                }
                Node::IterPop => {
                    // Before a return or a jump out of the loop: implicit.
                    let silent = matches!(st.get(i + 1).map(|x| &x.node), Some(Node::Return(_)) | Some(Node::Jump(_)) | None);
                    if !silent {
                        out.line(ind, "// iterator pop".into(), Some(s.pos));
                    }
                }
                Node::FilterEditorOnly(t) => {
                    if let Some(k) = self.idx(*t, i + 1, hi) {
                        out.line(ind, "filtereditoronly".into(), Some(s.pos));
                        out.line(ind, "{".into(), None);
                        self.block(out, i + 1, k, ind + 1, loops);
                        out.line(ind, "}".into(), None);
                        i = k;
                        continue;
                    }
                    out.line(ind, format!("filtereditoronly // ends at J0x{t:X}"), Some(s.pos));
                }
                other => {
                    if let Some(text) = simple(other) {
                        out.line(ind, text, Some(s.pos));
                    }
                }
            }
            i += 1;
        }
    }

    /// Whether `k..k2` is one `if` (with or without else) that ends exactly at `k2` (else-if chain).
    fn single_if(&self, k: usize, k2: usize) -> bool {
        let Node::JumpIfNot(t, _) = self.st[k].node else { return false };
        let Some(e) = self.at.get(&t).copied().or_else(|| (self.end_pos(k2) == t).then_some(k2)) else { return false };
        if e == k2 {
            return true;
        }
        if e > k + 1 && e < k2 {
            if let Node::Jump(back) = self.st[e - 1].node {
                return back == self.end_pos(k2);
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn switch(&self, out: &mut Out, i: usize, hi: usize, ind: usize, e: &E, names: Option<&[String]>, loops: &mut Vec<Loop>) -> Option<usize> {
        let st = self.st;
        // The cases: each Case points at the next one.
        let mut cases: Vec<usize> = Vec::new();
        let mut j = i + 1;
        loop {
            if j >= hi || !matches!(st[j].node, Node::Case(..)) {
                return None;
            }
            cases.push(j);
            let Node::Case(t, _) = st[j].node else { unreachable!() };
            if t == 0xFFFF {
                break;
            }
            match self.idx(t, j + 1, hi) {
                Some(n) if n < hi && matches!(st[n].node, Node::Case(..)) => j = n,
                Some(n) => {
                    // The last case points past the switch.
                    return Some(self.emit_switch(out, i, &cases, n, ind, e, names, loops));
                }
                None => return None,
            }
        }
        // With a default: the end is where the cases' breaks jump.
        let last = *cases.last().unwrap();
        let mut end: Option<u32> = None;
        for &c in &cases[..cases.len() - 1] {
            let _ = c;
        }
        for x in st.iter().take(last).skip(i + 1) {
            if let Node::Jump(t) = x.node {
                if t > st[last].pos && loops.last().is_none_or(|l| l.brk != t && !l.is_cont(t)) {
                    end = Some(end.map_or(t, |e: u32| e.min(t)));
                }
            }
        }
        let n = match end.and_then(|t| self.idx(t, last, hi)) {
            Some(n) => n,
            None => hi,
        };
        Some(self.emit_switch(out, i, &cases, n, ind, e, names, loops))
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_switch(&self, out: &mut Out, i: usize, cases: &[usize], end: usize, ind: usize, e: &E, names: Option<&[String]>, loops: &mut Vec<Loop>) -> usize {
        let st = self.st;
        out.line(ind, format!("switch ({})", e.s), Some(st[i].pos));
        out.line(ind, "{".into(), None);
        loops.push(Loop { brk: self.end_pos(end), cont: None, step: None, edge: None });
        for (n, &c) in cases.iter().enumerate() {
            let body_end = cases.get(n + 1).copied().unwrap_or(end);
            match &st[c].node {
                Node::Case(_, Some(v)) => {
                    let label = match (names, v.s.parse::<usize>()) {
                        (Some(n), Ok(x)) if x < n.len() => n[x].clone(),
                        _ => v.s.clone(),
                    };
                    out.line(ind + 1, format!("case {label}:"), Some(st[c].pos))
                }
                _ => out.line(ind + 1, "default:".into(), Some(st[c].pos)),
            }
            self.block(out, c + 1, body_end, ind + 2, loops);
        }
        loops.pop();
        out.line(ind, "}".into(), None);
        end
    }
}

fn simple(n: &Node) -> Option<String> {
    Some(match n {
        Node::E(e) => {
            if e.s.is_empty() {
                return None;
            }
            format!("{};", e.s)
        }
        Node::Let(l, r) => format!("{} = {};", l.s, r.s),
        Node::Return(None) => "return;".into(),
        Node::Return(Some(e)) => format!("return {};", e.s),
        Node::Stop => "stop;".into(),
        Node::Goto(e) => format!("goto {};", e.s),
        Node::Assert(e) => format!("assert({});", e.s),
        Node::DefaultParm(e) => format!("// default value: {};", e.s),
        Node::Labels(_) | Node::EndOfScript | Node::Nothing | Node::EndParm | Node::EndParms | Node::EmptyParm => return None,
        _ => format!("// {n:?}"),
    })
}

/// Render statements as structured source lines (indent with 4 spaces from `base`).
fn render(st: &[Stmt], base: usize) -> String {
    let mut at = HashMap::new();
    for (i, s) in st.iter().enumerate() {
        at.entry(s.pos).or_insert(i);
    }
    // Labels of state code.
    let mut labels: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for s in st {
        if let Node::Labels(v) = &s.node {
            for (n, off) in v {
                labels.entry(*off).or_default().push(n.clone());
            }
        }
    }
    // The final implicit return.
    let mut hi = st.len();
    while hi > 0 && matches!(st[hi - 1].node, Node::EndOfScript | Node::Labels(_)) {
        hi -= 1;
    }
    let trailing_return = hi > 0 && matches!(st[hi - 1].node, Node::Return(None)) && !st.iter().any(|s| matches!(s.node, Node::Jump(t) | Node::JumpIfNot(t, _) if t == st[hi - 1].pos));
    let cx = Ctx { st, at };
    let mut out = Out { lines: Vec::new(), gotos: BTreeSet::new() };
    let mut loops = Vec::new();
    cx.block(&mut out, 0, if trailing_return { hi - 1 } else { hi }, base, &mut loops);
    // Labels at goto targets and state labels: before the first line at or after the position.
    let mut targets: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for t in &out.gotos {
        targets.entry(*t).or_default().push(format!("J0x{t:X}:"));
    }
    for (t, v) in labels {
        targets.entry(t).or_default().extend(v.into_iter().map(|n| format!("{n}:")));
    }
    let mut s = String::new();
    let mut pending: Vec<(u32, Vec<String>)> = targets.into_iter().collect();
    pending.reverse();
    for (ind, text, pos) in &out.lines {
        if let Some(p) = pos {
            while pending.last().is_some_and(|(t, _)| t <= p) {
                for l in pending.pop().unwrap().1 {
                    let _ = writeln!(s, "{}{l}", "    ".repeat(base.saturating_sub(1)));
                }
            }
        }
        let _ = writeln!(s, "{}{text}", "    ".repeat(*ind));
    }
    while let Some((_, v)) = pending.pop() {
        for l in v {
            let _ = writeln!(s, "{}{l}", "    ".repeat(base.saturating_sub(1)));
        }
    }
    s
}

// ------------------------------------------------------------------------- declarations

fn func_keyword(f: &FuncDef) -> String {
    let mut w: Vec<String> = Vec::new();
    if f.flags & FUNC_NATIVE != 0 {
        w.push(if f.native_index != 0 { format!("native({})", f.native_index) } else { "native".into() });
    }
    for (bit, kw) in [
        (FUNC_PRIVATE, "private"),
        (FUNC_PROTECTED, "protected"),
        (FUNC_STATIC, "static"),
        (FUNC_FINAL, "final"),
        (FUNC_SIMULATED, "simulated"),
        (FUNC_SINGULAR, "singular"),
        (FUNC_EXEC, "exec"),
        (FUNC_LATENT, "latent"),
        (FUNC_ITERATOR, "iterator"),
    ] {
        if f.flags & bit != 0 {
            w.push(kw.into());
        }
    }
    if f.flags & FUNC_NET != 0 {
        let rel = if f.flags & FUNC_NET_RELIABLE != 0 { "reliable" } else { "unreliable" };
        let side = if f.flags & FUNC_NET_SERVER != 0 { " server" } else if f.flags & FUNC_NET_CLIENT != 0 { " client" } else { "" };
        w.push(format!("{rel}{side}"));
    }
    let kind = if f.flags & FUNC_DELEGATE != 0 {
        "delegate".to_string()
    } else if f.flags & FUNC_OPERATOR != 0 {
        if f.flags & FUNC_PRE_OPERATOR != 0 {
            "preoperator".into()
        } else if f.params.len() == 1 {
            "postoperator".into()
        } else {
            format!("operator({})", f.oper_precedence)
        }
    } else if f.flags & FUNC_EVENT != 0 {
        "event".into()
    } else {
        "function".into()
    };
    w.push(kind);
    w.join(" ")
}

fn param_text(db: &Db, k: Key, default: Option<&str>) -> String {
    let p = &db.props[&k];
    let mut s = String::new();
    if p.flags & CPF_OPTIONAL != 0 {
        s.push_str("optional ");
    }
    if p.flags & CPF_OUT != 0 {
        s.push_str("out ");
    }
    if p.flags & CPF_COERCE != 0 {
        s.push_str("coerce ");
    }
    if p.flags & CPF_CONST != 0 {
        s.push_str("const ");
    }
    let _ = write!(s, "{} {}", db.type_of(k), p.name);
    if p.array_dim > 1 {
        let _ = write!(s, "[{}]", p.array_dim);
    }
    if let Some(d) = default {
        let _ = write!(s, " = {d}");
    }
    s
}

/// Result of decompiling one function or state.
#[derive(Debug, Clone)]
pub struct Decompiled {
    pub text: String,
    pub error: Option<String>,
    /// Functions it calls by name (virtual, final, native).
    pub tokens: usize,
}

/// The parsed statements of a function, one per line (`pos..end node`), for debugging.
pub fn raw(db: &Db, k: Key) -> String {
    let f = &db.funcs[&k];
    let class_key = db.class_key(&f.owner_class);
    match db.bytecode(k).and_then(|(h, d)| parse(db, k, &h, d, &f.name, class_key)) {
        Ok(st) => st.iter().map(|s| format!("{:04x}..{:04x} {:?}\n", s.pos, s.end, s.node)).collect(),
        Err(e) => format!("error: {e:#}"),
    }
}

/// Decompile a function export into UnrealScript-like source.
pub fn function(db: &Db, k: Key) -> Decompiled {
    let f = &db.funcs[&k];
    let class_key = db.class_key(&f.owner_class);
    let ret = f.ret.map(|r| format!("{} ", db.type_of(r))).unwrap_or_default();
    let mut body = String::new();
    let mut error = None;
    let mut tokens = 0;
    let mut defaults: Vec<Option<String>> = vec![None; f.params.len()];
    if f.flags & FUNC_NATIVE == 0 || db.bytecode(k).map(|(h, _)| h.bytecode_size > 0).unwrap_or(false) {
        match db.bytecode(k).and_then(|(h, d)| parse(db, k, &h, d, &f.name, class_key)) {
            Ok(st) => {
                tokens = st.len();
                // Leading default values belong to the optional parameters, in order.
                let opt: Vec<usize> = f.params.iter().enumerate().filter(|(_, p)| db.props[p].flags & CPF_OPTIONAL != 0).map(|(i, _)| i).collect();
                let mut skip = 0;
                let mut oi = 0;
                for s in &st {
                    match &s.node {
                        Node::DefaultParm(e) if oi < opt.len() => {
                            defaults[opt[oi]] = Some(e.s.clone());
                            oi += 1;
                            skip += 1;
                        }
                        Node::Nothing if oi < opt.len() => {
                            oi += 1;
                            skip += 1;
                        }
                        _ => break,
                    }
                }
                body = render(&st[skip..], 1);
            }
            Err(e) => error = Some(format!("{e:#}")),
        }
    }
    let params: Vec<String> = f.params.iter().zip(&defaults).map(|(&p, d)| param_text(db, p, d.as_deref())).collect();
    let name = if f.is_operator() { f.friendly.clone() } else { f.name.clone() };
    let mut s = format!("{} {ret}{name}({})", func_keyword(f), params.join(", "));
    let has_body = f.flags & FUNC_NATIVE == 0 || !body.trim().is_empty();
    if !has_body && error.is_none() {
        s.push_str(";\n");
        return Decompiled { text: s, error, tokens };
    }
    s.push_str("\n{\n");
    for &l in &f.locals {
        let _ = writeln!(s, "    local {};", param_text(db, l, None));
    }
    if !f.locals.is_empty() && !body.is_empty() {
        s.push('\n');
    }
    match &error {
        Some(e) => {
            let _ = writeln!(s, "    // DECOMPILE FAILED: {e}");
        }
        None => s.push_str(&body),
    }
    s.push_str("}\n");
    Decompiled { text: s, error, tokens }
}

/// Decompile the code of a state (its labels and latent code).
pub fn state_code(db: &Db, k: Key, class_key: Option<Key>) -> Decompiled {
    match db.bytecode(k) {
        Ok((h, _)) if h.bytecode_size == 0 => Decompiled { text: String::new(), error: None, tokens: 0 },
        Ok((h, d)) => match parse(db, k, &h, d, "", class_key) {
            Ok(st) => Decompiled { tokens: st.len(), text: render(&st, 1), error: None },
            Err(e) => Decompiled { text: String::new(), error: Some(format!("{e:#}")), tokens: 0 },
        },
        Err(e) => Decompiled { text: String::new(), error: Some(format!("{e:#}")), tokens: 0 },
    }
}

/// Per-class decompilation: the declaration text and every function separately.
#[derive(Debug, Clone, Default)]
pub struct ClassSource {
    pub name: String,
    pub package: String,
    pub super_name: String,
    /// Class header, constants, enums, structs and variables (with in-memory offsets).
    pub header: String,
    /// (state, function name, text, error)
    pub functions: Vec<(Option<String>, String, String, Option<String>, u32)>,
    /// (state, code text, error)
    pub states: Vec<(String, String, Option<String>)>,
    pub defaults: Vec<String>,
}

fn struct_decl(db: &Db, k: Key, ind: usize) -> String {
    let l = db.layout(k);
    let own = db.name(k);
    let pad = "    ".repeat(ind);
    let mut s = format!("{pad}struct {}{}\n{pad}{{\n", own, db.super_name(k).map(|n| format!(" extends {n}")).unwrap_or_default());
    for f in l.fields.iter().filter(|f| f.owner == own) {
        let bit = f.bit.map(|b| format!(" bit {b}")).unwrap_or_default();
        let _ = writeln!(s, "{pad}    var {} {}; // +0x{:x}{bit}", f.ty, f.name, f.offset);
    }
    let _ = writeln!(s, "{pad}}}; // size 0x{:x}", l.size);
    s
}

pub fn class(db: &Db, k: Key) -> ClassSource {
    let name = db.name(k);
    let mut out = ClassSource { name: name.clone(), package: db.pkg(k).name.clone(), super_name: db.super_name(k).unwrap_or_default(), ..Default::default() };
    let mut h = format!("class {}{};\n\n", name, if out.super_name.is_empty() { String::new() } else { format!(" extends {}", out.super_name) });
    let kids = db.children.get(&k).cloned().unwrap_or_default();
    for &c in &kids {
        match db.class_of(c).as_str() {
            "Const" => {
                let _ = writeln!(h, "const {} = {};", db.name(c), db.const_value(c).unwrap_or_else(|e| format!("?{e}")));
            }
            "Enum" => {
                let v = db.enum_values(c).unwrap_or_default();
                let _ = writeln!(h, "enum {}\n{{\n    {}\n}};", db.name(c), v.join(",\n    "));
            }
            "ScriptStruct" => h.push_str(&struct_decl(db, c, 0)),
            _ => {}
        }
    }
    let l = db.layout(k);
    h.push('\n');
    for f in l.fields.iter().filter(|f| f.owner == name) {
        let flags = db.props.iter().find(|_| false).map(|_| 0u64).unwrap_or(0);
        let _ = flags;
        let bit = f.bit.map(|b| format!(" bit {b} (mask 0x{:x})", 1u32 << b)).unwrap_or_default();
        let _ = writeln!(h, "var {} {}; // +0x{:x}{bit}", f.ty, f.name, f.offset);
    }
    let _ = writeln!(h, "// instance size 0x{:x}", l.size);
    out.header = h;
    let ck = Some(k);
    for &c in &kids {
        match db.class_of(c).as_str() {
            "Function" => {
                let d = function(db, c);
                let size = db.bytecode(c).map(|(h, _)| h.script.len() as u32).unwrap_or(0);
                out.functions.push((None, db.name(c), d.text, d.error, size));
            }
            "State" => {
                let sname = db.name(c);
                for &sf in db.children.get(&c).map(|v| v.as_slice()).unwrap_or(&[]) {
                    if db.class_of(sf) == "Function" {
                        let d = function(db, sf);
                        let size = db.bytecode(sf).map(|(h, _)| h.script.len() as u32).unwrap_or(0);
                        out.functions.push((Some(sname.clone()), db.name(sf), d.text, d.error, size));
                    }
                }
                let d = state_code(db, c, ck);
                out.states.push((sname, d.text, d.error));
            }
            _ => {}
        }
    }
    out.defaults = db.defaults(k);
    out
}

impl ClassSource {
    /// The whole class as one source text.
    pub fn text(&self) -> String {
        let mut s = self.header.clone();
        s.push('\n');
        for (st, _, t, _, _) in &self.functions {
            if st.is_none() {
                s.push_str(t);
                s.push('\n');
            }
        }
        for (st, code, err) in &self.states {
            let _ = writeln!(s, "state {st}\n{{");
            for (fs, _, t, _, _) in &self.functions {
                if fs.as_deref() == Some(st.as_str()) {
                    for l in t.lines() {
                        let _ = writeln!(s, "    {l}");
                    }
                    s.push('\n');
                }
            }
            if let Some(e) = err {
                let _ = writeln!(s, "    // STATE CODE DECOMPILE FAILED: {e}");
            } else if !code.is_empty() {
                s.push_str(code);
            }
            s.push_str("}\n\n");
        }
        if !self.defaults.is_empty() {
            s.push_str("defaultproperties\n{\n");
            for d in &self.defaults {
                let _ = writeln!(s, "    {d}");
            }
            s.push_str("}\n");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: &str) -> E {
        atom(s)
    }
    fn st(v: Vec<(u32, Node)>) -> Vec<Stmt> {
        let mut out: Vec<Stmt> = Vec::new();
        for (i, (pos, node)) in v.iter().enumerate() {
            let end = v.get(i + 1).map(|x| x.0).unwrap_or(pos + 1);
            out.push(Stmt { pos: *pos, end, node: node.clone() });
        }
        out
    }
    fn norm(s: &str) -> String {
        s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn if_else_and_while_and_for() {
        // if (a) { x; } else { y; }  while (c) { z; }
        let s = st(vec![
            (0, Node::JumpIfNot(10, e("a"))),
            (4, Node::E(e("x()"))),
            (7, Node::Jump(13)),
            (10, Node::E(e("y()"))),
            (13, Node::JumpIfNot(23, e("c"))),
            (17, Node::E(e("z()"))),
            (20, Node::Jump(13)),
            (23, Node::Return(None)),
            (25, Node::EndOfScript),
        ]);
        assert_eq!(norm(&render(&s, 1)), "if (a) { x(); } else { y(); } while (c) { z(); } return;");
        // for with a continue: I = 0; for (; I < N; I++) { if (b) continue; w(); }
        let s = st(vec![
            (0, Node::JumpIfNot(30, e("I < N"))),
            (4, Node::JumpIfNot(11, e("b"))),
            (8, Node::Jump(20)),
            (11, Node::E(e("w()"))),
            (20, Node::E(e("I++"))),
            (27, Node::Jump(0)),
            (30, Node::Return(None)),
            (32, Node::EndOfScript),
        ]);
        assert_eq!(norm(&render(&s, 1)), "for (; I < N; I++) { if (b) { continue; } w(); } return;");
    }

    #[test]
    fn foreach_break_and_switch() {
        // foreach It(A) { f(); break; }  switch (x) { case 1: g(); break; default: h(); }
        let s = st(vec![
            (0, Node::Iter("It(A)".into(), 12)),
            (5, Node::E(e("f()"))),
            (8, Node::Jump(12)),
            (11, Node::IterNext),
            (12, Node::IterPop),
            (13, Node::Switch(e("x"), None)),
            (16, Node::Case(25, Some(e("1")))),
            (19, Node::E(e("g()"))),
            (22, Node::Jump(30)),
            (25, Node::Case(0xFFFF, None)),
            (27, Node::E(e("h()"))),
            (30, Node::Return(None)),
            (32, Node::EndOfScript),
        ]);
        assert_eq!(norm(&render(&s, 1)), "foreach It(A) { f(); break; } switch (x) { case 1: g(); break; default: h(); } return;");
    }

    #[test]
    fn unstructured_jumps_become_labelled_gotos() {
        let s = st(vec![(0, Node::Jump(9)), (3, Node::E(e("a()"))), (6, Node::Jump(3)), (9, Node::Return(None)), (11, Node::EndOfScript)]);
        let r = norm(&render(&s, 1));
        assert!(r.contains("goto J0x9;") && r.contains("J0x9:") && r.contains("goto J0x3;") && r.contains("J0x3:"), "{r}");
    }
}
