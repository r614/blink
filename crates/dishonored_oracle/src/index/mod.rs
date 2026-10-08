//! Phase 3 of the oracle: the static index of the original (research/dishonored/ORACLE.md).
//!
//! One index per exe hash under `<oracle cache>/index/<exe sha256, 16 hex>/`:
//! - `index.bin` (bincode, [`Index`]): the exe's functions, calls (both directions), string
//!   references, native function table (lookup tables and the `GNatives` array joined with the
//!   script's `FunctionDef`s), per-class vtables, trace call sites and their `TraceProfile`s,
//!   and the decompiled UnrealScript of every script class and function;
//! - `decomp.c` + the offsets in `index.bin`: Ghidra's decompiler output of every function,
//!   read by seeking, so a lookup by address is instant.
//!
//! Everything in it is derived from retail code and data: it lives in the cache, never in git.
//! Built by `oracle index build`, queried by `oracle index fn|class|xref|search|...` ([`cli`]).

pub mod cli;
pub mod codec;
pub mod ghidra;
pub mod script;
pub mod uscript;

use crate::query::TraceProfile;
use anyhow::{Context, Result};
use ghidra::{Call, ClassVtable, Func, StrRef, TraceSite};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Bump when [`Index`] changes.
pub const INDEX_VERSION: u32 = 1;

/// A script function, decompiled.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScriptFunc {
    pub class: String,
    pub state: Option<String>,
    pub name: String,
    pub flags: u32,
    pub native_index: u16,
    /// Operator symbol (`+`, `Dot`...) for operators.
    pub friendly: String,
    /// Serialized bytecode bytes.
    pub script_size: u32,
    /// Native implementation: (exec symbol, address).
    pub native: Option<(String, u32)>,
    pub text: String,
    pub error: Option<String>,
}

impl ScriptFunc {
    pub fn path(&self) -> String {
        match &self.state {
            Some(s) => format!("{}.{}.{}", self.class, s, self.name),
            None => format!("{}.{}", self.class, self.name),
        }
    }
    pub fn is_native(&self) -> bool {
        self.flags & script::FUNC_NATIVE != 0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldRow {
    pub owner: String,
    pub name: String,
    pub ty: String,
    pub offset: u32,
    pub size: u32,
    pub bit: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScriptClass {
    pub name: String,
    pub package: String,
    pub super_name: String,
    /// Declarations (constants, enums, structs, variables with offsets).
    pub header: String,
    /// The fields this class adds to the in-memory layout (inherited ones: [`Index::all_fields`]).
    pub fields: Vec<FieldRow>,
    pub size: u32,
    /// (state, code, error)
    pub states: Vec<(String, String, Option<String>)>,
    pub defaults: Vec<String>,
}

/// One row of the native function table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NativeRow {
    /// `iNative` (0 for natives bound by name).
    pub index: u16,
    /// `Class.Function` (or `Class.State.Function`).
    pub script: String,
    pub exec: String,
    pub addr: Option<u32>,
    /// Where the address came from: `lookup` (FNativeFunctionLookup), `gnatives` (the array).
    pub via: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub exe_sha256: String,
    pub built: String,
    pub functions: Vec<Func>,
    /// Sorted by caller (`xref.bin`, loaded with [`Parts::xref`]).
    #[serde(skip)]
    pub calls: Vec<Call>,
    /// (callee, index into `calls`), sorted by callee: direct callers.
    #[serde(skip)]
    pub callers: Vec<(u32, u32)>,
    #[serde(skip)]
    pub strings: Vec<StrRef>,
    pub execs: Vec<(String, u32, u32)>,
    /// `GNatives` base address (voted from the script's native indices) and its entries.
    pub gnatives_base: Option<u32>,
    pub natives: Vec<NativeRow>,
    pub vtables: Vec<ClassVtable>,
    pub traces: Vec<TraceSite>,
    /// (class, super class) of every script class (in the core: vtable overrides need it).
    pub class_supers: Vec<(String, String)>,
    /// `script.bin`, loaded with [`Parts::script`].
    #[serde(skip)]
    pub script_classes: Vec<ScriptClass>,
    #[serde(skip)]
    pub script_funcs: Vec<ScriptFunc>,
    /// Decompiled C: (address, byte offset, byte length) into `decomp.c`, sorted.
    pub decomp: Vec<(u32, u64, u32)>,
    /// Build measurements and counts.
    pub stats: Vec<(String, String)>,
    /// Addresses cited in the research docs: (address, `file:line`, excerpt).
    pub notes: Vec<(u32, String, String)>,
    /// Names the research docs give addresses (`UWorld::SingleLineCheck`, 0x0064E7A0).
    pub doc_names: Vec<(u32, String)>,
}

/// `<cache>/index`.
pub fn index_root(cache_root: &Path) -> PathBuf {
    cache_root.join("index")
}

pub fn index_dir(cache_root: &Path, exe_hash: &str) -> PathBuf {
    index_root(cache_root).join(&exe_hash[..16.min(exe_hash.len())])
}

/// Which sections of the index a query needs (the core is always loaded).
#[derive(Debug, Clone, Copy, Default)]
pub struct Parts {
    /// Calls, callers and string references (`xref.bin`).
    pub xref: bool,
    /// Script classes and decompiled script (`script.bin`).
    pub script: bool,
}

#[derive(Default)]
struct XrefPart {
    calls: Vec<Call>,
    callers: Vec<(u32, u32)>,
    strings: Vec<StrRef>,
}

#[derive(Default)]
struct ScriptPart {
    classes: Vec<ScriptClass>,
    funcs: Vec<ScriptFunc>,
}

crate::codec!(XrefPart { calls, callers, strings });
crate::codec!(ScriptPart { classes, funcs });
crate::codec!(ScriptFunc { class, state, name, flags, native_index, friendly, script_size, native, text, error });
crate::codec!(FieldRow { owner, name, ty, offset, size, bit });
crate::codec!(ScriptClass { name, package, super_name, header, fields, size, states, defaults });
crate::codec!(NativeRow { index, script, exec, addr, via });
crate::codec!(Index { version, exe_sha256, built, functions, execs, gnatives_base, natives, vtables, traces, class_supers, decomp, stats, notes, doc_names });

const MAGIC_CORE: &[u8; 8] = b"BLKIDX01";
const MAGIC_XREF: &[u8; 8] = b"BLKXRF01";
const MAGIC_SCRIPT: &[u8; 8] = b"BLKSCR01";

fn magic_of(path: &Path) -> &'static [u8; 8] {
    match path.file_name().and_then(|n| n.to_str()) {
        Some("xref.bin") => MAGIC_XREF,
        Some("script.bin") => MAGIC_SCRIPT,
        _ => MAGIC_CORE,
    }
}

fn read_bin<T: codec::Codec>(path: &Path) -> Result<T> {
    let t = std::time::Instant::now();
    let bytes = std::fs::read(path).with_context(|| format!("{} (run `oracle index build`)", path.display()))?;
    let v: T = codec::decode(magic_of(path), &bytes).with_context(|| format!("decoding {} (rebuild with `oracle index build`)", path.display()))?;
    if std::env::var_os("ORACLE_INDEX_TIMING").is_some() {
        eprintln!("{}: {:.1} MB in {:?}", path.display(), bytes.len() as f64 / 1e6, t.elapsed());
    }
    Ok(v)
}

fn write_bin<T: codec::Codec>(path: &Path, v: &T) -> Result<()> {
    let bytes = codec::encode(magic_of(path), v);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl Index {
    /// Load the core and the requested parts.
    pub fn load_parts(dir: &Path, parts: Parts) -> Result<Self> {
        let (a, b, c) = std::thread::scope(|s| {
            let x = s.spawn(|| parts.xref.then(|| read_bin::<XrefPart>(&dir.join("xref.bin"))).transpose());
            let y = s.spawn(|| parts.script.then(|| read_bin::<ScriptPart>(&dir.join("script.bin"))).transpose());
            let core = read_bin::<Index>(&dir.join("index.bin"));
            (core, x.join().unwrap(), y.join().unwrap())
        });
        let mut ix = a?;
        anyhow::ensure!(ix.version == INDEX_VERSION, "index version {} (expected {INDEX_VERSION}): rebuild with `oracle index build`", ix.version);
        if let Some(x) = b? {
            ix.calls = x.calls;
            ix.callers = x.callers;
            ix.strings = x.strings;
        }
        if let Some(s) = c? {
            ix.script_classes = s.classes;
            ix.script_funcs = s.funcs;
        }
        Ok(ix)
    }

    /// Everything.
    pub fn load(dir: &Path) -> Result<Self> {
        Self::load_parts(dir, Parts { xref: true, script: true })
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        write_bin(&dir.join("xref.bin"), &XrefPart { calls: self.calls.clone(), callers: self.callers.clone(), strings: self.strings.clone() })?;
        write_bin(&dir.join("script.bin"), &ScriptPart { classes: self.script_classes.clone(), funcs: self.script_funcs.clone() })?;
        write_bin(&dir.join("index.bin"), self)?;
        Ok(())
    }

    pub fn func_at(&self, addr: u32) -> Option<&Func> {
        self.functions.binary_search_by_key(&addr, |f| f.addr).ok().map(|i| &self.functions[i])
    }

    /// The function containing an address.
    pub fn func_containing(&self, addr: u32) -> Option<&Func> {
        let i = self.functions.partition_point(|f| f.addr <= addr);
        let f = self.functions.get(i.checked_sub(1)?)?;
        (addr < f.addr + f.size.max(1)).then_some(f)
    }

    /// Exec names bound to an address.
    pub fn exec_names(&self, addr: u32) -> Vec<&str> {
        self.execs.iter().filter(|e| e.1 == addr).map(|e| e.0.as_str()).collect()
    }

    /// Best display name of a function address: its exec name, the research docs' name, then
    /// Ghidra's.
    pub fn name_of(&self, addr: u32) -> String {
        if let Some(e) = self.execs.iter().find(|e| e.1 == addr) {
            return e.0.clone();
        }
        if let Some(d) = self.doc_names.iter().find(|d| d.0 == addr) {
            return d.1.clone();
        }
        match self.func_at(addr) {
            Some(f) => f.name.clone(),
            None => format!("{addr:08x}"),
        }
    }

    pub fn calls_from(&self, caller: u32) -> &[Call] {
        let lo = self.calls.partition_point(|c| c.caller < caller);
        let hi = self.calls.partition_point(|c| c.caller <= caller);
        &self.calls[lo..hi]
    }

    pub fn callers_of(&self, callee: u32) -> Vec<&Call> {
        let lo = self.callers.partition_point(|c| c.0 < callee);
        let hi = self.callers.partition_point(|c| c.0 <= callee);
        self.callers[lo..hi].iter().map(|c| &self.calls[c.1 as usize]).collect()
    }

    /// Decompiled C of the function at `addr`.
    pub fn decompiled(&self, dir: &Path, addr: u32) -> Option<String> {
        use std::io::{Read, Seek, SeekFrom};
        let i = self.decomp.binary_search_by_key(&addr, |d| d.0).ok()?;
        let (_, off, len) = self.decomp[i];
        let mut f = std::fs::File::open(dir.join("decomp.c")).ok()?;
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut b = vec![0u8; len as usize];
        f.read_exact(&mut b).ok()?;
        Some(String::from_utf8_lossy(&b).into_owned())
    }

    /// (class, slot) pairs whose vtable holds `addr`, with whether the class introduced or
    /// overrode it (its parent's vtable has another function there or is shorter).
    pub fn vtable_slots(&self, addr: u32, parent: &dyn Fn(&str) -> Option<String>) -> Vec<(String, usize, bool)> {
        let by_name: HashMap<&str, &ClassVtable> = self.vtables.iter().map(|v| (v.name.as_str(), v)).collect();
        let mut out = Vec::new();
        for v in &self.vtables {
            for (i, &s) in v.slots.iter().enumerate() {
                if s != addr {
                    continue;
                }
                let own = match parent(&v.name).and_then(|p| by_name.get(p.as_str()).copied()) {
                    Some(pv) => pv.slots.get(i) != Some(&s),
                    None => true,
                };
                out.push((v.name.clone(), i, own));
            }
        }
        out
    }

    /// Name of a vtable slot when known (for classes deriving from the slot's class).
    pub fn slot_name(&self, class: &str, slot: usize) -> Option<&'static str> {
        if !self.exe_sha256.to_ascii_lowercase().starts_with(GOG_EXE) {
            return None;
        }
        let mut chain = vec![class.to_string()];
        while let Some(p) = self.parent(chain.last().unwrap()) {
            if chain.len() > 64 {
                break;
            }
            chain.push(p);
        }
        GOG_SLOTS.iter().find(|(c, off, _)| *off as usize == slot * 4 && chain.iter().any(|x| x.eq_ignore_ascii_case(c))).map(|x| x.2)
    }

    pub fn script_class(&self, name: &str) -> Option<&ScriptClass> {
        self.script_classes.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Every field of a class's layout, inherited ones first.
    pub fn all_fields(&self, class: &str) -> Vec<&FieldRow> {
        let mut chain = Vec::new();
        let mut cur = self.script_class(class);
        while let Some(c) = cur {
            if chain.len() > 64 {
                break;
            }
            chain.push(c);
            cur = (!c.super_name.is_empty()).then(|| self.script_class(&c.super_name)).flatten();
        }
        chain.iter().rev().flat_map(|c| c.fields.iter()).collect()
    }

    pub fn parent(&self, class: &str) -> Option<String> {
        let i = self.class_supers.binary_search_by(|(c, _)| c.to_ascii_lowercase().cmp(&class.to_ascii_lowercase())).ok()?;
        Some(self.class_supers[i].1.clone()).filter(|s| !s.is_empty())
    }

    pub fn vtable(&self, class: &str) -> Option<&ClassVtable> {
        self.vtables.iter().find(|v| v.name.eq_ignore_ascii_case(class))
    }

    /// Trace profiles enumerated from the trace call sites: one per distinct (flags, extent)
    /// with constant flags; the call site is the first one using it.
    pub fn trace_profiles(&self) -> Vec<TraceProfile> {
        let mut seen: Vec<(u32, [u32; 3])> = Vec::new();
        let mut out = Vec::new();
        for t in &self.traces {
            let Some(ext) = t.extent_value() else { continue };
            for &fl in &t.flags {
                let key = (fl, ext.map(f32::to_bits));
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                let box_ = ext != [0.0; 3];
                out.push(TraceProfile {
                    name: format!("{}_{:08x}_{}", if box_ { "box" } else { "line" }, fl, self.name_of(t.caller)),
                    tag: format!("X{}", out.len()),
                    flags: fl,
                    extent: ext,
                    call_site: Some(t.site),
                    source: format!("index:{}@{:08x}", t.kind, t.site),
                });
            }
        }
        out
    }

    /// Trace profiles for `Profiles::Enumerated`: the constant ones ([`Index::trace_profiles`])
    /// plus, when `pawn` is given, one box profile per constant flag set whose call sites pass a
    /// run-time extent (mostly the tracing actor's cylinder), with the pawn's extent assumed and
    /// said so in `source`.
    pub fn enumerated_profiles(&self, pawn: Option<[f32; 3]>) -> Vec<TraceProfile> {
        let mut out = self.trace_profiles();
        if let Some(ext) = pawn {
            let mut seen: Vec<u32> = out.iter().filter(|p| p.extent == ext).map(|p| p.flags).collect();
            for t in &self.traces {
                if t.extent_value().is_some() {
                    continue;
                }
                for &fl in &t.flags {
                    if seen.contains(&fl) {
                        continue;
                    }
                    seen.push(fl);
                    out.push(TraceProfile {
                        name: format!("pawnbox_{fl:08x}_{}", self.name_of(t.caller)),
                        tag: format!("X{}", out.len()),
                        flags: fl,
                        extent: ext,
                        call_site: Some(t.site),
                        source: format!("index:{}@{:08x}: run-time extent `{}`, the pawn's assumed", t.kind, t.site, t.extent),
                    });
                }
            }
        }
        out
    }

    /// Code pointers into the original for a class: its natives, vtable and script functions.
    pub fn code_pointers(&self, class: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(v) = self.vtable(class) {
            out.push(format!("original native class `{}`: constructor {:08x}, vtable {:08x} ({} slots); `oracle index class {}`", v.name, v.ctor, v.vtable, v.slots.len(), v.name));
        }
        let natives: Vec<String> = self.natives.iter().filter(|n| n.script.split('.').next().is_some_and(|c| c.eq_ignore_ascii_case(class)) && n.addr.is_some()).take(8).map(|n| format!("{} @ {:08x}", n.exec, n.addr.unwrap())).collect();
        if !natives.is_empty() {
            out.push(format!("natives: {}", natives.join(", ")));
        }
        let script: Vec<String> = self.script_funcs.iter().filter(|f| f.class.eq_ignore_ascii_case(class) && !f.is_native() && f.script_size > 0).take(12).map(|f| f.path()).collect();
        if !script.is_empty() {
            out.push(format!("script: {} (`oracle index uscript {class}`)", script.join(", ")));
        }
        out
    }
}

/// `Profiles::Enumerated` of a sweep: the index's profiles (see [`Index::enumerated_profiles`]).
pub fn enumerated_profiles(pawn: Option<[f32; 3]>) -> Result<Vec<TraceProfile>> {
    let (ix, _) = cli::open(Parts::default())?;
    Ok(ix.enumerated_profiles(pawn))
}

/// Add code pointers into the original to work items when the index is built: for coverage
/// items the class's natives, vtable and script functions (and a property's offset); for
/// collision items the engine's trace entry point and the trace call sites.
pub fn annotate(items: &mut [crate::report::WorkItem]) {
    let Ok((ix, _)) = cli::open(Parts { xref: false, script: true }) else { return };
    for it in items.iter_mut() {
        let mut ptrs: Vec<String> = Vec::new();
        match it.probe.as_str() {
            "coverage" => {
                let key = it.id.splitn(3, ':').nth(2).unwrap_or("");
                let (class, prop) = match key.split_once('.') {
                    Some((c, p)) => (c, Some(p)),
                    None => (key, None),
                };
                if let Some(p) = prop {
                    if let Some(f) = ix.all_fields(class).into_iter().find(|f| f.name.eq_ignore_ascii_case(p)) {
                        ptrs.push(format!("`{}.{}` is at +0x{:x} ({}{}) in the original's objects: search the decompiled C for that offset (`oracle index field {class} 0x{:x}`)", f.owner, f.name, f.offset, f.ty, f.bit.map(|b| format!(", bit {b}")).unwrap_or_default(), f.offset));
                    }
                }
                ptrs.extend(ix.code_pointers(class));
            }
            "collision" => {
                ptrs.push("`UWorld::SingleLineCheck` 0064e7a0 and `UWorld::MultiLineCheck` 0064a860 (`oracle index fn 0064e7a0`); every call site with its flags and extent: `oracle index traces`".into());
            }
            _ => {}
        }
        for p in ptrs {
            let line = format!("original: {p}");
            if !it.code.contains(&line) {
                it.code.push(line);
            }
        }
    }
}

/// Every 8-digit address the research docs cite, with the line it is cited on, and the names
/// they give (`` `Class::Function` `` right before the address).
pub fn research_notes(repo: &Path) -> (Vec<(u32, String, String)>, Vec<(u32, String)>) {
    let addr_re = regex::Regex::new(r"0x([0-9A-Fa-f]{8})\b").unwrap();
    let name_re = regex::Regex::new(r"`(~?[A-Za-z_]\w*::~?[A-Za-z_]\w*)`\s*[(,]?\s*(?:at\s+)?0x([0-9A-Fa-f]{8})\b").unwrap();
    let mut notes = Vec::new();
    let mut names = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(repo.join("research").join("dishonored")).into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
    files.push(repo.join("research").join("LOG.md"));
    files.sort();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let fname = f.file_name().unwrap().to_string_lossy().into_owned();
        for (n, line) in text.lines().enumerate() {
            for m in addr_re.captures_iter(line) {
                let Ok(a) = u32::from_str_radix(&m[1], 16) else { continue };
                if !(0x0040_0000..0x0200_0000).contains(&a) {
                    continue;
                }
                let excerpt: String = line.trim().chars().take(220).collect();
                notes.push((a, format!("{fname}:{}", n + 1), excerpt));
            }
            for m in name_re.captures_iter(line) {
                // Port modules (`dishonored_sim::...`) are not names of the original's code.
                if m[1].starts_with(|c: char| c.is_ascii_lowercase()) {
                    continue;
                }
                if let Ok(a) = u32::from_str_radix(&m[2], 16) {
                    if !names.iter().any(|(x, _)| *x == a) {
                        names.push((a, m[1].to_string()));
                    }
                }
            }
        }
    }
    notes.sort();
    notes.dedup();
    names.sort();
    (notes, names)
}

/// Names the engine hook relies on in the GOG exe (`70da9613...`; INSTRUMENTATION.md,
/// COLLISION.md, `dishonored_probe_dll`), for exes with that hash only.
pub const GOG_EXE: &str = "70da96138fe2171c";
pub const GOG_NAMES: &[(u32, &str)] = &[
    (0x0064_E7A0, "UWorld::SingleLineCheck"),
    (0x0064_A860, "UWorld::MultiLineCheck"),
    (0x0047_0640, "UObject::ProcessEvent"),
    (0x0056_7A20, "AActor::ProcessEvent"),
    (0x006B_D150, "APawn::physWalking"),
];

/// Virtual functions identified by vtable byte offset in the GOG exe: (class that has the slot,
/// offset, name). INSTRUMENTATION.md (ProcessEvent), COLLISION.md (ShouldTrace), and the
/// physWalking override found by this index.
pub const GOG_SLOTS: &[(&str, u32, &str)] = &[("Object", 0xEC, "ProcessEvent"), ("Actor", 0x1C4, "ShouldTrace"), ("Pawn", 0x234, "physWalking")];

pub fn known_names(exe_sha: &str) -> Vec<(u32, String)> {
    if exe_sha.to_ascii_lowercase().starts_with(GOG_EXE) { GOG_NAMES.iter().map(|&(a, n)| (a, n.to_string())).collect() } else { Vec::new() }
}

/// Native table: every native script function joined with the exe's lookup tables (by exec
/// name) and the `GNatives` array (by index; its base voted from the indices that resolve by
/// name).
pub fn native_table(funcs: &[ScriptFunc], execs: &[(String, u32, u32)], arrays: &[(u32, u32)]) -> (Vec<NativeRow>, Option<u32>) {
    let mut by_name: HashMap<String, (String, u32)> = HashMap::new();
    for (n, a, _) in execs {
        // `UObjectexecAdd_IntInt` -> `object.add_intint`.
        if let Some(pos) = n.find("exec") {
            let class = &n[1..pos];
            let func = &n[pos + 4..];
            by_name.entry(format!("{class}.{func}").to_ascii_lowercase()).or_insert((n.clone(), *a));
        }
    }
    // Vote the GNatives base: address - 4 * index over natives whose function is known by name.
    let mut votes: HashMap<u32, u32> = HashMap::new();
    let by_addr: HashMap<u32, Vec<u32>> = arrays.iter().fold(HashMap::new(), |mut m, &(at, f)| {
        m.entry(f).or_default().push(at);
        m
    });
    for f in funcs.iter().filter(|f| f.native_index != 0) {
        let Some((_, a)) = by_name.get(&format!("{}.{}", f.class, f.name).to_ascii_lowercase()) else { continue };
        for &at in by_addr.get(a).map(|v| v.as_slice()).unwrap_or(&[]) {
            *votes.entry(at.wrapping_sub(4 * f.native_index as u32)).or_default() += 1;
        }
    }
    // A statically initialized GNatives would put most indexed natives at base + 4 * index; the
    // GOG exe fills it at run time, so a weak vote is noise, not a table.
    let candidates = funcs.iter().filter(|f| f.native_index != 0 && by_name.contains_key(&format!("{}.{}", f.class, f.name).to_ascii_lowercase())).count() as u32;
    let base = votes.into_iter().max_by_key(|&(b, n)| (n, std::cmp::Reverse(b))).filter(|&(_, n)| n >= 8 && 2 * n >= candidates).map(|(b, _)| b);
    let slot: HashMap<u32, u32> = arrays.iter().map(|&(at, f)| (at, f)).collect();
    let mut rows = Vec::new();
    for f in funcs.iter().filter(|f| f.is_native()) {
        let key = format!("{}.{}", f.class, f.name).to_ascii_lowercase();
        let prefix = if by_name.keys().any(|k| k.starts_with(&format!("{}.", f.class.to_ascii_lowercase()))) { "" } else { "?" };
        let (exec, addr, via) = match by_name.get(&key) {
            Some((n, a)) => (n.clone(), Some(*a), "lookup".to_string()),
            None => match base.and_then(|b| (f.native_index != 0).then(|| slot.get(&(b + 4 * f.native_index as u32)).copied()).flatten()) {
                Some(a) => (format!("{prefix}exec{}", f.name), Some(a), "gnatives".to_string()),
                None => (format!("{prefix}exec{}", f.name), None, String::new()),
            },
        };
        // Cross-check: an indexed native found by name must sit in its GNatives slot.
        let via = match (base, addr, f.native_index) {
            (Some(b), Some(a), i) if i != 0 && via == "lookup" => match slot.get(&(b + 4 * i as u32)) {
                Some(&s) if s == a => "lookup+gnatives".to_string(),
                Some(&s) => format!("lookup (gnatives slot holds {s:08x})"),
                None => via,
            },
            _ => via,
        };
        rows.push(NativeRow { index: f.native_index, script: f.path(), exec, addr, via });
    }
    rows.sort_by(|a, b| (a.index == 0, a.index, &a.script).cmp(&(b.index == 0, b.index, &b.script)));
    (rows, base)
}

/// Decompile every script class of the install into the index's script part. Returns
/// (classes, functions, failures).
pub fn build_script(db: &script::Db, execs: &[(String, u32, u32)]) -> (Vec<ScriptClass>, Vec<ScriptFunc>, Vec<String>) {
    use rayon::prelude::*;
    let mut keys: Vec<script::Key> = db.classes.values().copied().collect();
    keys.sort();
    let by_exec: HashMap<String, (String, u32)> = execs
        .iter()
        .filter_map(|(n, a, _)| {
            let pos = n.find("exec")?;
            Some((format!("{}.{}", &n[1..pos], &n[pos + 4..]).to_ascii_lowercase(), (n.clone(), *a)))
        })
        .collect();
    let per: Vec<(ScriptClass, Vec<ScriptFunc>)> = keys
        .par_iter()
        .map(|&k| {
            let src = uscript::class(db, k);
            let layout = db.layout(k);
            let class = ScriptClass {
                name: src.name.clone(),
                package: src.package.clone(),
                super_name: src.super_name.clone(),
                header: src.header.clone(),
                fields: layout.fields.iter().filter(|f| f.owner == src.name).map(|f| FieldRow { owner: f.owner.clone(), name: f.name.clone(), ty: f.ty.clone(), offset: f.offset, size: f.size, bit: f.bit }).collect(),
                size: layout.size,
                states: src.states.clone(),
                defaults: src.defaults.clone(),
            };
            let mut funcs = Vec::new();
            let kids = db.children.get(&k).cloned().unwrap_or_default();
            let mut fkeys: Vec<script::Key> = Vec::new();
            for c in kids {
                match db.class_of(c).as_str() {
                    "Function" => fkeys.push(c),
                    "State" => fkeys.extend(db.children.get(&c).into_iter().flatten().copied().filter(|&x| db.class_of(x) == "Function")),
                    _ => {}
                }
            }
            for (fk, (state, name, text, error, size)) in fkeys.iter().zip(src.functions) {
                let f = &db.funcs[fk];
                let native = (f.flags & script::FUNC_NATIVE != 0).then(|| by_exec.get(&format!("{}.{}", f.owner_class, f.name).to_ascii_lowercase()).cloned()).flatten();
                funcs.push(ScriptFunc { class: src.name.clone(), state, name, flags: f.flags, native_index: f.native_index, friendly: f.friendly.clone(), script_size: size, native, text, error });
            }
            (class, funcs)
        })
        .collect();
    let mut classes = Vec::new();
    let mut funcs = Vec::new();
    let mut failures = Vec::new();
    for (c, f) in per {
        for (s, _, e) in &c.states {
            if let Some(e) = e {
                failures.push(format!("{}.{s} (state code): {e}", c.name));
            }
        }
        for x in &f {
            if let Some(e) = &x.error {
                failures.push(format!("{}: {e}", x.path()));
            }
        }
        classes.push(c);
        funcs.extend(f);
    }
    (classes, funcs, failures)
}

/// Assemble the index from the Ghidra tables and the script.
pub fn assemble(t: ghidra::Tables, classes: Vec<ScriptClass>, funcs: Vec<ScriptFunc>, decomp: Vec<(u32, u64, u32)>) -> Index {
    let mut calls = t.calls;
    calls.sort_by_key(|c| (c.caller, c.site));
    let mut callers: Vec<(u32, u32)> = calls.iter().enumerate().filter(|(_, c)| c.kind == 'd').map(|(i, c)| (c.target, i as u32)).collect();
    callers.sort();
    let (natives, base) = native_table(&funcs, &t.execs, &t.native_arrays);
    let mut class_supers: Vec<(String, String)> = classes.iter().map(|c| (c.name.clone(), c.super_name.clone())).collect();
    // Class records whose name is not a script class are other UTF-16 records (package names).
    let known: std::collections::HashSet<String> = classes.iter().map(|c| c.name.to_ascii_lowercase()).collect();
    let mut vtables = t.classes;
    vtables.retain(|v| known.contains(&v.name.to_ascii_lowercase()));
    class_supers.sort_by_key(|(c, _)| c.to_ascii_lowercase());
    Index {
        class_supers,
        version: INDEX_VERSION,
        exe_sha256: t.sha256,
        built: String::new(),
        functions: t.functions,
        calls,
        callers,
        strings: t.strings,
        execs: t.execs,
        gnatives_base: base,
        natives,
        vtables,
        traces: t.traces,
        script_classes: classes,
        script_funcs: funcs,
        decomp,
        stats: Vec::new(),
        notes: Vec::new(),
        doc_names: Vec::new(),
    }
}

/// Read `decomp.idx` (address, offset, length per line).
pub fn read_decomp_index(path: &Path) -> Result<Vec<(u32, u64, u32)>> {
    let mut v = Vec::new();
    for l in std::fs::read_to_string(path)?.lines() {
        let c: Vec<&str> = l.split('\t').collect();
        if c.len() == 3 {
            v.push((u32::from_str_radix(c[0], 16)?, c[1].parse()?, c[2].parse()?));
        }
    }
    v.sort();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_roundtrips_the_index() {
        let mut ix = Index { version: INDEX_VERSION, exe_sha256: "ab".into(), ..Default::default() };
        ix.functions.push(Func { addr: 0x401000, size: 22, name: "FUN_00401000".into(), source: "DEFAULT".into(), thunk: Some(5) });
        ix.vtables.push(ClassVtable { name: "Actor".into(), record: 1, ctor: 2, vtable: 3, slots: vec![4, 5], others: vec![(6, vec![7])] });
        ix.traces.push(TraceSite { kind: "single".into(), caller: 1, site: 2, flags: vec![0x20df], flags_dynamic: true, extent: "stack:1,2,3".into(), note: String::new(), statement: "x".into() });
        ix.notes.push((0x401000, "A.md:1".into(), "text".into()));
        let b = codec::encode(b"TESTTEST", &ix);
        let back: Index = codec::decode(b"TESTTEST", &b).unwrap();
        assert_eq!(back.functions[0].thunk, Some(5));
        assert_eq!(back.vtables[0].others, vec![(6, vec![7])]);
        assert_eq!(back.traces[0].extent_value(), Some([1.0, 2.0, 3.0]));
        assert_eq!(back.notes, ix.notes);
        assert!(codec::decode::<Index>(b"OTHERMAG", &b).is_err());
    }

    #[test]
    fn native_table_joins_by_exec_name_and_ignores_a_weak_gnatives_vote() {
        let f = |c: &str, n: &str, i: u16| ScriptFunc { class: c.into(), name: n.into(), native_index: i, flags: script::FUNC_NATIVE, ..Default::default() };
        let funcs = vec![f("Object", "Add_IntInt", 146), f("Actor", "Trace", 277), f("Foo", "Bar", 0)];
        let execs = vec![("UObjectexecAdd_IntInt".to_string(), 0x406b10, 1), ("AActorexecTrace".to_string(), 0x6d0ed0, 2)];
        let (rows, base) = native_table(&funcs, &execs, &[(0x1000, 0x406b10)]);
        assert_eq!(base, None);
        let add = rows.iter().find(|r| r.script == "Object.Add_IntInt").unwrap();
        assert_eq!((add.addr, add.exec.as_str(), add.via.as_str()), (Some(0x406b10), "UObjectexecAdd_IntInt", "lookup"));
        assert!(rows.iter().find(|r| r.script == "Foo.Bar").unwrap().addr.is_none());
    }

    #[test]
    fn trace_profiles_dedupe_constant_sites() {
        let mut ix = Index::default();
        let t = |site, fl: Vec<u32>, ext: &str| TraceSite { kind: "single".into(), caller: 0x500000, site, flags: fl, extent: ext.into(), ..Default::default() };
        ix.traces = vec![t(1, vec![0x20df], "stack:0.0,0.0,0.0"), t(2, vec![0x20df], "stack:0.0,0.0,0.0"), t(3, vec![0x2086], "dyn"), t(4, vec![0x8023086], "global:01000000:1.0,2.0,3.0")];
        let p = ix.trace_profiles();
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].extent, [1.0, 2.0, 3.0]);
        let q = ix.enumerated_profiles(Some([35.0, 35.0, 88.0]));
        assert_eq!(q.len(), 3);
        assert!(q[2].source.contains("assumed") && q[2].flags == 0x2086);
    }
}
