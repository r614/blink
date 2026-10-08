//! `oracle index ...`: build and query the static index. Every query prints text, or JSON with
//! `--json`.
//!
//! ```text
//! oracle index build [--project DIR]... [--tables DIR] [--no-decomp] [--keep-tables]
//! oracle index fn <address|name> [--no-c] [--json]       decompiled C, callers, callees, strings
//! oracle index class <Class> [--brief] [--json]          layout, vtable, natives, script
//! oracle index uscript <Class>[.<Function>] [--json]     decompiled UnrealScript
//! oracle index xref <address|text> [--json]              who calls / references it
//! oracle index search <regex> [--limit N] [--json]       functions, classes, script, strings
//! oracle index natives [<Class>] [--json]                native function table
//! oracle index traces [--json]                           trace call sites and profiles
//! oracle index field <Class> <offset> [--json]           the property at an offset
//! oracle index vcall <Class> <slot|0xoffset> [--json]    the function in a vtable slot
//! oracle index stats [--json]
//! oracle index raw <Package> <Class> [<Function>]       parsed bytecode statements (decompiler check)
//! ```

use super::*;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::time::Instant;

struct Args(Vec<String>);

impl Args {
    fn flag(&self, f: &str) -> bool {
        self.0.iter().any(|a| a == f)
    }
    fn opt(&self, f: &str) -> Option<String> {
        self.0.iter().position(|a| a == f).and_then(|i| self.0.get(i + 1)).cloned()
    }
    fn opts(&self, f: &str) -> Vec<String> {
        self.0.iter().enumerate().filter(|(_, a)| *a == f).filter_map(|(i, _)| self.0.get(i + 1).cloned()).collect()
    }
    /// Positional arguments (not flags or their values).
    fn pos(&self) -> Vec<String> {
        const VALUED: &[&str] = &["--project", "--tables", "--limit", "--repo", "--out"];
        let mut out = Vec::new();
        let mut skip = false;
        for a in &self.0 {
            if skip {
                skip = false;
                continue;
            }
            if VALUED.contains(&a.as_str()) {
                skip = true;
                continue;
            }
            if a.starts_with("--") {
                continue;
            }
            out.push(a.clone());
        }
        out
    }
}

fn parse_addr(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("FUN_").trim_start_matches("LAB_");
    if t.len() >= 5 && t.len() <= 8 && t.chars().all(|c| c.is_ascii_hexdigit()) { u32::from_str_radix(t, 16).ok() } else { None }
}

fn hx(a: u32) -> String {
    format!("{a:08x}")
}

/// The index of the installed exe.
pub fn open(parts: Parts) -> Result<(Index, PathBuf)> {
    let root = crate::cache::root();
    let exe = crate::cache::exe_path(&crate::install_dir());
    let t = Instant::now();
    let sha = crate::cache::file_sha256(&root, &exe)?;
    let dir = index_dir(&root, &crate::cache::hex(&sha));
    let t1 = t.elapsed();
    let ix = Index::load_parts(&dir, parts)?;
    if std::env::var_os("ORACLE_INDEX_TIMING").is_some() {
        eprintln!("exe hash {:?}, load {:?}", t1, t.elapsed() - t1);
    }
    Ok((ix, dir))
}

pub fn run(argv: &[String]) -> Result<()> {
    let a = Args(argv.to_vec());
    let pos = a.pos();
    let cmd = pos.first().map(String::as_str).unwrap_or("help");
    if cmd == "build" {
        return build(&a);
    }
    if cmd == "raw" {
        let install = crate::install_dir();
        let dir = install.join("DishonoredGame").join("CookedPCConsole");
        let db = script::Db::load(&[dir.join(format!("{}.upk", pos[1]))])?;
        let k = db.class_key(&pos[2]).context("class")?;
        let p = db.pkg(k);
        println!("{:?} flags {:x} hdr {:?}", k, p.exports[k.1 as usize].object_flags, script::struct_header(p, k.1 as usize));
        let d = p.export_data(k.1 as usize)?;
        println!("{:02x?}", &d[..64.min(d.len())]);
        println!("kids {:?}", db.children.get(&k).map(|v| v.iter().map(|&x| db.name(x)).collect::<Vec<_>>()));
        if let Some(fname) = pos.get(3) {
            for &x in db.children.get(&k).into_iter().flatten() {
                if db.name(x).eq_ignore_ascii_case(fname) && db.funcs.contains_key(&x) {
                    print!("{}", uscript::raw(&db, x));
                    print!("{}", uscript::function(&db, x).text);
                }
            }
        }
        return Ok(());
    }
    if cmd == "help" {
        println!("{}", include_str!("cli.rs").lines().skip_while(|l| !l.starts_with("//! ```text")).skip(1).take_while(|l| !l.starts_with("//! ```")).map(|l| l.trim_start_matches("//! ")).collect::<Vec<_>>().join("\n"));
        return Ok(());
    }
    let t0 = Instant::now();
    let parts = match cmd {
        "fn" | "xref" => Parts { xref: true, script: false },
        "class" | "uscript" | "field" => Parts { xref: false, script: true },
        "search" => Parts { xref: true, script: true },
        _ => Parts::default(),
    };
    let (ix, dir) = open(parts)?;
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let arg = |i: usize| pos.get(i).cloned().with_context(|| format!("oracle index {cmd}: missing argument"));
    let (text, js) = match cmd {
        "fn" => q_fn(&ix, &dir, &arg(1)?, !a.flag("--no-c"))?,
        "class" => q_class(&ix, &arg(1)?, a.flag("--brief"))?,
        "uscript" => q_uscript(&ix, &arg(1)?)?,
        "xref" => q_xref(&ix, &arg(1)?)?,
        "search" => q_search(&ix, &arg(1)?, a.opt("--limit").and_then(|s| s.parse().ok()).unwrap_or(60))?,
        "natives" => q_natives(&ix, pos.get(1).map(String::as_str)),
        "traces" => q_traces(&ix),
        "field" => q_field(&ix, &arg(1)?, &arg(2)?)?,
        "vcall" => q_vcall(&ix, &arg(1)?, &arg(2)?)?,
        "stats" => q_stats(&ix),
        other => bail!("unknown `oracle index {other}` (oracle index help)"),
    };
    if a.flag("--json") {
        println!("{}", serde_json::to_string_pretty(&js)?);
    } else {
        print!("{text}");
    }
    if a.flag("--time") {
        eprintln!("index load {load_ms:.0} ms, total {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(())
}

// ------------------------------------------------------------------------- build

fn repo_root() -> PathBuf {
    let mut d = std::env::current_dir().unwrap_or_default();
    loop {
        if d.join("tools").join("ghidra_index").join("scripts").join("BlinkIndex.java").exists() {
            return d;
        }
        if !d.pop() {
            return PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        }
    }
}

fn build(a: &Args) -> Result<()> {
    let t0 = Instant::now();
    let root = crate::cache::root();
    let install = crate::install_dir();
    let exe = crate::cache::exe_path(&install);
    let sha = crate::cache::hex(&crate::cache::file_sha256(&root, &exe)?);
    let dir = index_dir(&root, &sha);
    std::fs::create_dir_all(&dir)?;
    let mut stats: Vec<(String, String)> = Vec::new();

    // 1. Script: every class of the install's script packages, decompiled.
    let t = Instant::now();
    let files = crate::coverage::scan::list_packages(&install, false)?;
    let (scans, _, errs) = crate::coverage::scan_all(&files, Some(&root.join("coverage")));
    for e in errs {
        eprintln!("scan: {e}");
    }
    let pkgs = script::script_packages(&install, &scans);
    let db = script::Db::load(&pkgs)?;
    let load_s = t.elapsed().as_secs_f64();
    eprintln!("script: {} packages, {} classes, {} functions loaded in {load_s:.2} s ({} layout failures)", pkgs.len(), db.classes.len(), db.funcs.len(), db.failures.len());
    for f in db.failures.iter().take(10) {
        eprintln!("  {f}");
    }
    stats.push(("script packages".into(), pkgs.len().to_string()));
    stats.push(("script load s".into(), format!("{load_s:.2}")));
    stats.push(("script definition failures".into(), db.failures.len().to_string()));

    // 2. Ghidra tables (exported now, or given).
    let tables_dir = match a.opt("--tables") {
        Some(t) => PathBuf::from(t),
        None => {
            let out = dir.join("tables");
            let classes_file = dir.join("classes.txt");
            let mut names: Vec<String> = db.classes.values().map(|&k| db.name(k)).collect();
            names.sort();
            std::fs::write(&classes_file, names.join("\n"))?;
            let mut g = ghidra::GhidraSetup::from_env();
            let projects = a.opts("--project");
            if !projects.is_empty() {
                g.projects = projects.into_iter().map(PathBuf::from).collect();
            }
            let scripts = repo_root().join("tools").join("ghidra_index").join("scripts");
            let t = Instant::now();
            eprintln!("ghidra: exporting into {} (decompiling every function takes a while)", out.display());
            ghidra::run_export(&g, &scripts, &out, Some(&classes_file), ghidra::DEFAULT_TRACES, !a.flag("--no-decomp"))?;
            stats.push(("ghidra export s".into(), format!("{:.1}", t.elapsed().as_secs_f64())));
            out
        }
    };
    let t = Instant::now();
    let tables = ghidra::read_tables(&tables_dir)?;
    stats.push(("tables read s".into(), format!("{:.2}", t.elapsed().as_secs_f64())));
    if !tables.sha256.is_empty() && !tables.sha256.eq_ignore_ascii_case(&sha) {
        bail!("the Ghidra tables are of exe {} but the install's is {sha}", tables.sha256);
    }
    for l in tables.summary.lines() {
        stats.push(("ghidra".into(), l.to_string()));
    }

    // 3. Decompile the script.
    let t = Instant::now();
    let (classes, funcs, failures) = build_script(&db, &tables.execs);
    let us_s = t.elapsed().as_secs_f64();
    let script_funcs = funcs.iter().filter(|f| !f.is_native()).count();
    let script_bytes: u64 = funcs.iter().filter(|f| !f.is_native()).map(|f| f.script_size as u64).sum();
    let bytes: u64 = funcs.iter().map(|f| f.script_size as u64).sum();
    let states = classes.iter().map(|c| c.states.iter().filter(|s| !s.1.is_empty()).count()).sum::<usize>();
    eprintln!("uscript: {} classes, {} functions ({} script, {} KiB; {} native), {states} state code blocks, all bytecode {} KiB, {} failures, {us_s:.2} s", classes.len(), funcs.len(), script_funcs, script_bytes / 1024, funcs.len() - script_funcs, bytes / 1024, failures.len());
    stats.push(("uscript script functions".into(), format!("{script_funcs} ({} KiB)", script_bytes / 1024)));
    stats.push(("uscript state code blocks".into(), states.to_string()));
    for f in failures.iter().take(20) {
        eprintln!("  FAIL {f}");
    }
    std::fs::write(dir.join("uscript_failures.txt"), failures.join("\n"))?;
    stats.push(("uscript classes".into(), classes.len().to_string()));
    stats.push(("uscript functions".into(), funcs.len().to_string()));
    stats.push(("uscript functions with bytecode".into(), funcs.iter().filter(|f| f.script_size > 0).count().to_string()));
    stats.push(("uscript bytecode KiB".into(), (bytes / 1024).to_string()));
    stats.push(("uscript failures".into(), failures.len().to_string()));
    stats.push(("uscript s".into(), format!("{us_s:.2}")));

    // 4. Decompiled C: moved next to the index.
    let decomp = if tables_dir.join("decomp.idx").exists() {
        if tables_dir != dir {
            let src = tables_dir.join("decomp.c");
            let dst = dir.join("decomp.c");
            if src != dst && src.exists() {
                if a.flag("--keep-tables") || std::fs::rename(&src, &dst).is_err() {
                    std::fs::copy(&src, &dst)?;
                }
            }
        }
        read_decomp_index(&tables_dir.join("decomp.idx"))?
    } else if dir.join("decomp.c").exists() {
        // Rebuilt from tables without C: keep the previous decompiled C.
        Index::load(&dir).map(|old| old.decomp).unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut ix = assemble(tables, classes, funcs, decomp);
    ix.built = format!("{} s since the epoch", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let (notes, mut doc_names) = research_notes(&repo_root());
    for (a, n) in known_names(&ix.exe_sha256) {
        if !doc_names.iter().any(|d| d.0 == a) {
            doc_names.push((a, n));
        }
    }
    doc_names.sort();
    stats.push(("research notes".into(), format!("{} citations, {} names", notes.len(), doc_names.len())));
    ix.notes = notes;
    ix.doc_names = doc_names;
    stats.push(("functions".into(), ix.functions.len().to_string()));
    stats.push(("calls".into(), ix.calls.len().to_string()));
    stats.push(("string refs".into(), ix.strings.len().to_string()));
    stats.push(("native rows".into(), ix.natives.len().to_string()));
    stats.push(("natives with address".into(), ix.natives.iter().filter(|n| n.addr.is_some()).count().to_string()));
    stats.push(("gnatives base".into(), ix.gnatives_base.map(hx).unwrap_or_default()));
    stats.push(("vtables".into(), ix.vtables.len().to_string()));
    stats.push(("trace sites".into(), ix.traces.len().to_string()));
    stats.push(("trace profiles".into(), ix.trace_profiles().len().to_string()));
    stats.push(("decompiled functions".into(), ix.decomp.len().to_string()));
    stats.push(("build total s".into(), format!("{:.1}", t0.elapsed().as_secs_f64())));
    ix.stats = stats;
    if std::env::var_os("ORACLE_INDEX_TIMING").is_some() {
        fn tm<T: codec::Codec>(n: &str, v: &T) {
            let b = codec::encode(b"TIMING00", v);
            let t = Instant::now();
            let _: T = codec::decode(b"TIMING00", &b).unwrap();
            eprintln!("{n}: {} bytes, decode {:?}", b.len(), t.elapsed());
        }
        tm("functions", &ix.functions);
        tm("vtables", &ix.vtables);
        tm("natives", &ix.natives);
        tm("execs", &ix.execs);
        tm("traces", &ix.traces);
        tm("decomp", &ix.decomp);
        tm("supers", &ix.class_supers);
        tm("calls", &ix.calls);
        tm("strings", &ix.strings);
    }
    ix.save(&dir)?;
    let size = std::fs::metadata(dir.join("index.bin")).map(|m| m.len()).unwrap_or(0);
    let csize = std::fs::metadata(dir.join("decomp.c")).map(|m| m.len()).unwrap_or(0);
    println!("index {}: {} functions, {} calls, {} strings, {} natives ({} with address), {} vtables, {} trace sites, {} decompiled; index.bin {:.1} MB, decomp.c {:.1} MB; {:.1} s", dir.display(), ix.functions.len(), ix.calls.len(), ix.strings.len(), ix.natives.len(), ix.natives.iter().filter(|n| n.addr.is_some()).count(), ix.vtables.len(), ix.traces.len(), ix.decomp.len(), size as f64 / 1e6, csize as f64 / 1e6, t0.elapsed().as_secs_f64());
    Ok(())
}

// ------------------------------------------------------------------------- queries

/// Resolve a function argument: an address, an exec name, a script `Class.Function`, a Ghidra name.
fn resolve_fn(ix: &Index, s: &str) -> Result<u32> {
    if let Some(a) = parse_addr(s) {
        if let Some(f) = ix.func_containing(a) {
            return Ok(f.addr);
        }
        return Ok(a);
    }
    let l = s.to_ascii_lowercase();
    if let Some(e) = ix.execs.iter().find(|e| e.0.eq_ignore_ascii_case(s)) {
        return Ok(e.1);
    }
    if let Some(d) = ix.doc_names.iter().find(|d| d.1.eq_ignore_ascii_case(s)) {
        return Ok(d.0);
    }
    if let Some(n) = ix.natives.iter().find(|n| n.script.eq_ignore_ascii_case(s) && n.addr.is_some()) {
        return Ok(n.addr.unwrap());
    }
    if let Some(f) = ix.functions.iter().find(|f| f.name.eq_ignore_ascii_case(s)) {
        return Ok(f.addr);
    }
    // `Class::vslotN` / `Class.vslotN`.
    if let Some((c, n)) = l.split_once("::vslot").or_else(|| l.split_once(".vslot")) {
        if let (Some(v), Ok(n)) = (ix.vtable(c), n.parse::<usize>()) {
            if let Some(&a) = v.slots.get(n) {
                return Ok(a);
            }
        }
    }
    let cands: Vec<String> = ix.execs.iter().filter(|e| e.0.to_ascii_lowercase().contains(&l)).map(|e| format!("{} {}", hx(e.1), e.0)).chain(ix.functions.iter().filter(|f| f.source != "DEFAULT" && f.name.to_ascii_lowercase().contains(&l)).map(|f| format!("{} {}", hx(f.addr), f.name))).take(30).collect();
    let mut seen = std::collections::HashSet::new();
    let cands: Vec<String> = cands.into_iter().filter(|c| seen.insert(c[..8].to_string())).collect();
    let exact: Vec<&String> = cands.iter().filter(|c| c.to_ascii_lowercase().ends_with(&l)).collect();
    if exact.len() == 1 {
        return parse_addr(&exact[0][..8]).context("candidate");
    }
    if cands.len() == 1 {
        return parse_addr(&cands[0][..8]).context("candidate");
    }
    if cands.is_empty() {
        bail!("no function matches `{s}` (try `oracle index search {s}`)");
    }
    bail!("`{s}` is ambiguous:\n  {}", cands.join("\n  "))
}

fn q_fn(ix: &Index, dir: &Path, s: &str, with_c: bool) -> Result<(String, Value)> {
    let addr = resolve_fn(ix, s)?;
    let f = ix.func_at(addr);
    let mut t = String::new();
    let parent = |c: &str| ix.parent(c);
    let slots = ix.vtable_slots(addr, &parent);
    let natives: Vec<&NativeRow> = ix.natives.iter().filter(|n| n.addr == Some(addr)).collect();
    let _ = writeln!(t, "function {} {} (size {})", hx(addr), ix.name_of(addr), f.map(|f| f.size).unwrap_or(0));
    for n in &natives {
        let _ = writeln!(t, "  native of script {} (iNative {}, {})", n.script, n.index, n.via);
    }
    let own: Vec<String> = slots.iter().filter(|s| s.2).map(|s| format!("{}::vslot{} (+0x{:x}{})", s.0, s.1, s.1 * 4, ix.slot_name(&s.0, s.1).map(|n| format!(", {n}")).unwrap_or_default())).collect();
    if !own.is_empty() {
        let _ = writeln!(t, "  vtable slot (introduced/overridden in): {}", own.join(", "));
    }
    let inherited = slots.iter().filter(|s| !s.2).count();
    if inherited > 0 {
        let _ = writeln!(t, "  inherited by {inherited} more vtables");
    }
    let callers = ix.callers_of(addr);
    let _ = writeln!(t, "callers ({}):", callers.len());
    for c in callers.iter().take(60) {
        let _ = writeln!(t, "  {} in {} {}", hx(c.site), hx(c.caller), ix.name_of(c.caller));
    }
    let callees = ix.calls_from(addr);
    let _ = writeln!(t, "callees ({}):", callees.len());
    for c in callees.iter().take(120) {
        let what = match c.kind {
            'd' => format!("{} {}", hx(c.target), ix.name_of(c.target)),
            'v' => format!("virtual +0x{:x} (slot {})", c.target, c.target / 4),
            _ => c.text.clone(),
        };
        let _ = writeln!(t, "  {} -> {what}", hx(c.site));
    }
    let strs: Vec<&StrRef> = ix.strings.iter().filter(|r| r.func == addr).collect();
    if !strs.is_empty() {
        let _ = writeln!(t, "strings ({}):", strs.len());
        for r in &strs {
            let _ = writeln!(t, "  {} {}{:?}", hx(r.site), if r.utf16 { "L" } else { "" }, r.text);
        }
    }
    let notes: Vec<&(u32, String, String)> = ix.notes.iter().filter(|n| n.0 == addr).collect();
    if !notes.is_empty() {
        let _ = writeln!(t, "research notes ({}):", notes.len());
        for n in notes.iter().take(20) {
            let _ = writeln!(t, "  {}: {}", n.1, n.2);
        }
    }
    let traces: Vec<&TraceSite> = ix.traces.iter().filter(|x| x.caller == addr).collect();
    for x in &traces {
        let _ = writeln!(t, "trace {} at {}: flags {:x?}{} extent {}", x.kind, hx(x.site), x.flags, if x.flags_dynamic { " +dynamic" } else { "" }, x.extent);
    }
    let c = if with_c { ix.decompiled(dir, addr) } else { None };
    if let Some(c) = &c {
        let _ = writeln!(t, "\n{c}");
    } else if with_c {
        let _ = writeln!(t, "\n(no decompiled C for {})", hx(addr));
    }
    let js = json!({
        "address": hx(addr), "name": ix.name_of(addr), "size": f.map(|f| f.size),
        "natives": natives.iter().map(|n| json!({"script": n.script, "index": n.index, "via": n.via})).collect::<Vec<_>>(),
        "vtable_slots": slots.iter().map(|s| json!({"class": s.0, "slot": s.1, "own": s.2})).collect::<Vec<_>>(),
        "callers": callers.iter().map(|c| json!({"site": hx(c.site), "caller": hx(c.caller), "name": ix.name_of(c.caller)})).collect::<Vec<_>>(),
        "callees": callees.iter().map(|c| json!({"site": hx(c.site), "kind": c.kind.to_string(), "target": if c.kind == 'd' { hx(c.target) } else if c.kind == 'v' { format!("+0x{:x}", c.target) } else { c.text.clone() }, "name": if c.kind == 'd' { ix.name_of(c.target) } else { String::new() }})).collect::<Vec<_>>(),
        "strings": strs.iter().map(|r| json!({"site": hx(r.site), "addr": hx(r.addr), "text": r.text})).collect::<Vec<_>>(),
        "traces": traces.iter().map(|x| json!({"kind": x.kind, "site": hx(x.site), "flags": x.flags.iter().map(|f| format!("0x{f:x}")).collect::<Vec<_>>(), "dynamic": x.flags_dynamic, "extent": x.extent})).collect::<Vec<_>>(),
        "research_notes": notes.iter().map(|n| json!({"at": n.1, "text": n.2})).collect::<Vec<_>>(),
        "c": c,
    });
    Ok((t, js))
}

fn q_class(ix: &Index, name: &str, brief: bool) -> Result<(String, Value)> {
    let sc = ix.script_class(name);
    let vt = ix.vtable(name);
    if sc.is_none() && vt.is_none() {
        let near: Vec<&str> = ix.script_classes.iter().filter(|c| c.name.to_ascii_lowercase().contains(&name.to_ascii_lowercase())).map(|c| c.name.as_str()).take(20).collect();
        bail!("no class `{name}`; similar: {}", near.join(", "));
    }
    let cname = sc.map(|c| c.name.clone()).or_else(|| vt.map(|v| v.name.clone())).unwrap();
    let mut chain = vec![cname.clone()];
    while let Some(p) = ix.parent(chain.last().unwrap()) {
        if chain.contains(&p) || chain.len() > 40 {
            break;
        }
        chain.push(p);
    }
    let mut t = String::new();
    let _ = writeln!(t, "class {} ({})", cname, chain.join(" < "));
    if let Some(c) = sc {
        let _ = writeln!(t, "package {}, instance size 0x{:x}", c.package, c.size);
    }
    // Vtable with the slots this class introduces or overrides.
    let parent_vt = ix.parent(&cname).and_then(|p| ix.vtable(&p).cloned());
    let mut slots_js = Vec::new();
    if let Some(v) = vt {
        let _ = writeln!(t, "\nnative: constructor {}, vtable {} ({} slots, parent {})", hx(v.ctor), hx(v.vtable), v.slots.len(), parent_vt.as_ref().map(|p| format!("{} slots", p.slots.len())).unwrap_or("-".into()));
        for (i, &s) in v.slots.iter().enumerate() {
            let own = parent_vt.as_ref().is_none_or(|p| p.slots.get(i) != Some(&s));
            let vname = ix.slot_name(&cname, i);
            slots_js.push(json!({"slot": i, "offset": format!("0x{:x}", i * 4), "address": hx(s), "name": ix.name_of(s), "virtual": vname, "own": own}));
            if own {
                let _ = writeln!(t, "  slot {i:>3} +0x{:03x} {} {}{}", i * 4, hx(s), ix.name_of(s), vname.map(|v| format!("  [{v}]")).unwrap_or_default());
            }
        }
        for (a, sl) in &v.others {
            let _ = writeln!(t, "  interface vtable {} ({} slots)", hx(*a), sl.len());
        }
    } else {
        let _ = writeln!(t, "\nnative: no class record with a vtable (script-only class, or abstract without a constructor)");
    }
    let natives: Vec<&NativeRow> = ix.natives.iter().filter(|n| n.script.split('.').next().is_some_and(|c| c.eq_ignore_ascii_case(&cname))).collect();
    if !natives.is_empty() {
        let _ = writeln!(t, "\nnatives ({}):", natives.len());
        for n in &natives {
            let _ = writeln!(t, "  {:>5} {:<48} {} {}", if n.index != 0 { n.index.to_string() } else { "-".into() }, n.script, n.addr.map(hx).unwrap_or("--------".into()), n.exec);
        }
    }
    let funcs: Vec<&ScriptFunc> = ix.script_funcs.iter().filter(|f| f.class.eq_ignore_ascii_case(&cname)).collect();
    if let Some(c) = sc {
        if brief {
            let _ = writeln!(t, "\nfields:");
            for f in c.fields.iter().filter(|f| f.owner.eq_ignore_ascii_case(&cname)) {
                let _ = writeln!(t, "  +0x{:04x} {} {}{}", f.offset, f.ty, f.name, f.bit.map(|b| format!(" (bit {b})")).unwrap_or_default());
            }
            let _ = writeln!(t, "\nscript functions:");
            for f in &funcs {
                let _ = writeln!(t, "  {}{}", f.path(), if f.is_native() { " [native]" } else { "" });
            }
        } else {
            let _ = writeln!(t, "\n{}", c.header);
            for f in &funcs {
                let st = f.state.as_ref().map(|s| format!("// state {s}\n")).unwrap_or_default();
                let _ = writeln!(t, "{st}{}", f.text);
            }
            for (s, code, err) in &c.states {
                if !code.is_empty() || err.is_some() {
                    let _ = writeln!(t, "state {s} code:\n{code}{}", err.as_ref().map(|e| format!("// FAILED: {e}\n")).unwrap_or_default());
                }
            }
            if !c.defaults.is_empty() {
                let _ = writeln!(t, "defaultproperties\n{{\n    {}\n}}", c.defaults.join("\n    "));
            }
        }
    }
    let js = json!({
        "class": cname, "chain": chain,
        "package": sc.map(|c| c.package.clone()), "size": sc.map(|c| c.size),
        "fields": sc.map(|c| ix.all_fields(&c.name).into_iter().map(|f| json!({"owner": f.owner, "name": f.name, "type": f.ty, "offset": format!("0x{:x}", f.offset), "size": f.size, "bit": f.bit})).collect::<Vec<_>>()),
        "constructor": vt.map(|v| hx(v.ctor)), "vtable": vt.map(|v| hx(v.vtable)), "slots": slots_js,
        "natives": natives.iter().map(|n| json!({"index": n.index, "script": n.script, "exec": n.exec, "address": n.addr.map(hx)})).collect::<Vec<_>>(),
        "functions": funcs.iter().map(|f| json!({"path": f.path(), "native": f.is_native(), "flags": format!("0x{:x}", f.flags), "text": if brief { Value::Null } else { Value::String(f.text.clone()) }, "error": f.error})).collect::<Vec<_>>(),
        "states": sc.map(|c| c.states.iter().map(|s| json!({"state": s.0, "code": s.1, "error": s.2})).collect::<Vec<_>>()),
        "defaults": sc.map(|c| c.defaults.clone()),
    });
    Ok((t, js))
}

fn q_uscript(ix: &Index, s: &str) -> Result<(String, Value)> {
    let (class, func) = match s.split_once('.') {
        Some((c, f)) => (c, Some(f)),
        None => (s, None),
    };
    let funcs: Vec<&ScriptFunc> = ix
        .script_funcs
        .iter()
        .filter(|f| f.class.eq_ignore_ascii_case(class))
        .filter(|f| func.is_none_or(|n| f.name.eq_ignore_ascii_case(n) || f.state.as_deref().is_some_and(|st| format!("{st}.{}", f.name).eq_ignore_ascii_case(n))))
        .collect();
    if funcs.is_empty() && (func.is_some() || ix.script_class(class).is_none()) {
        bail!("no script function `{s}`");
    }
    let mut t = String::new();
    if func.is_none() {
        if let Some(c) = ix.script_class(class) {
            let _ = writeln!(t, "{}", c.header);
        }
    }
    for f in &funcs {
        if let Some(st) = &f.state {
            let _ = writeln!(t, "// state {st}");
        }
        if let Some((e, a)) = &f.native {
            let _ = writeln!(t, "// native: {e} @ {}", hx(*a));
        }
        let _ = writeln!(t, "{}", f.text);
    }
    if func.is_none() {
        if let Some(c) = ix.script_class(class) {
            for (st, code, _) in &c.states {
                if !code.is_empty() {
                    let _ = writeln!(t, "state {st} code:\n{code}");
                }
            }
        }
    }
    let js = json!(funcs.iter().map(|f| json!({"path": f.path(), "flags": format!("0x{:x}", f.flags), "native": f.native.as_ref().map(|(e, a)| json!({"exec": e, "address": hx(*a)})), "text": f.text, "error": f.error})).collect::<Vec<_>>());
    Ok((t, js))
}

fn q_xref(ix: &Index, s: &str) -> Result<(String, Value)> {
    let mut t = String::new();
    if let Some(a) = parse_addr(s) {
        let f = ix.func_containing(a);
        let target = f.map(|f| f.addr).unwrap_or(a);
        let callers = ix.callers_of(target);
        let _ = writeln!(t, "{} {}: {} direct callers", hx(target), ix.name_of(target), callers.len());
        for c in &callers {
            let _ = writeln!(t, "  call {} in {} {}", hx(c.site), hx(c.caller), ix.name_of(c.caller));
        }
        let parent = |c: &str| ix.parent(c);
        let slots = ix.vtable_slots(target, &parent);
        for (c, i, own) in &slots {
            if *own {
                let _ = writeln!(t, "  vtable {c} slot {i} (+0x{:x})", i * 4);
            }
        }
        let vc: Vec<&Call> = slots.iter().filter(|s| s.2).flat_map(|s| ix.calls.iter().filter(move |c| c.kind == 'v' && c.target as usize == s.1 * 4)).take(40).collect();
        if !vc.is_empty() {
            let _ = writeln!(t, "  virtual calls through the same slot offset (any class; first 40):");
            for c in &vc {
                let _ = writeln!(t, "    {} in {} {}", hx(c.site), hx(c.caller), ix.name_of(c.caller));
            }
        }
        let strs: Vec<&StrRef> = ix.strings.iter().filter(|r| r.addr == a).collect();
        for r in &strs {
            let _ = writeln!(t, "  string {:?} used at {} in {} {}", r.text, hx(r.site), hx(r.func), ix.name_of(r.func));
        }
        let js = json!({"address": hx(target), "name": ix.name_of(target),
            "callers": callers.iter().map(|c| json!({"site": hx(c.site), "caller": hx(c.caller), "name": ix.name_of(c.caller)})).collect::<Vec<_>>(),
            "vtable_slots": slots.iter().map(|s| json!({"class": s.0, "slot": s.1, "own": s.2})).collect::<Vec<_>>(),
            "string_uses": strs.iter().map(|r| json!({"site": hx(r.site), "func": hx(r.func)})).collect::<Vec<_>>()});
        return Ok((t, js));
    }
    let l = s.to_ascii_lowercase();
    let hits: Vec<&StrRef> = ix.strings.iter().filter(|r| r.text.to_ascii_lowercase().contains(&l)).take(400).collect();
    let _ = writeln!(t, "{} string references containing {s:?}", hits.len());
    for r in &hits {
        let _ = writeln!(t, "  {} {:?} at {} in {} {}", hx(r.addr), r.text, hx(r.site), hx(r.func), ix.name_of(r.func));
    }
    let js = json!(hits.iter().map(|r| json!({"string": r.text, "addr": hx(r.addr), "site": hx(r.site), "func": hx(r.func), "name": ix.name_of(r.func)})).collect::<Vec<_>>());
    Ok((t, js))
}

fn q_search(ix: &Index, pat: &str, limit: usize) -> Result<(String, Value)> {
    let re = regex::RegexBuilder::new(pat).case_insensitive(true).build()?;
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for c in &ix.script_classes {
        if re.is_match(&c.name) {
            rows.push(("class".into(), c.name.clone(), format!("{} < {}", c.package, c.super_name)));
        }
    }
    for f in &ix.script_funcs {
        if re.is_match(&f.path()) {
            rows.push(("script".into(), f.path(), f.native.as_ref().map(|(e, a)| format!("native {e} @ {}", hx(*a))).unwrap_or_else(|| if f.is_native() { "native".into() } else { format!("{} bytes", f.script_size) })));
        }
    }
    for (n, a, _) in &ix.execs {
        if re.is_match(n) {
            rows.push(("exec".into(), n.clone(), hx(*a)));
        }
    }
    for (a, n) in &ix.doc_names {
        if re.is_match(n) {
            rows.push(("doc-name".into(), n.clone(), hx(*a)));
        }
    }
    for f in &ix.functions {
        if f.source != "DEFAULT" && re.is_match(&f.name) {
            rows.push(("function".into(), f.name.clone(), hx(f.addr)));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for r in &ix.strings {
        if re.is_match(&r.text) && seen.insert((r.addr, r.func)) {
            rows.push(("string".into(), r.text.clone(), format!("{} in {} {}", hx(r.addr), hx(r.func), ix.name_of(r.func))));
        }
    }
    // Script source text: functions whose body matches.
    for f in &ix.script_funcs {
        if !re.is_match(&f.path()) && re.is_match(&f.text) {
            let line = f.text.lines().find(|l| re.is_match(l)).unwrap_or("").trim().to_string();
            rows.push(("script-body".into(), f.path(), line));
        }
    }
    let total = rows.len();
    rows.truncate(limit);
    let mut t = String::new();
    for (k, n, d) in &rows {
        let _ = writeln!(t, "{k:<11} {n}  {d}");
    }
    if total > limit {
        let _ = writeln!(t, "... {} more (--limit N)", total - limit);
    }
    let js = json!({"total": total, "rows": rows.iter().map(|(k, n, d)| json!({"kind": k, "name": n, "detail": d})).collect::<Vec<_>>()});
    Ok((t, js))
}

fn q_natives(ix: &Index, class: Option<&str>) -> (String, Value) {
    let rows: Vec<&NativeRow> = ix.natives.iter().filter(|n| class.is_none_or(|c| n.script.split('.').next().is_some_and(|x| x.eq_ignore_ascii_case(c)))).collect();
    let mut t = format!("GNatives base {}; {} natives, {} with an address\n", ix.gnatives_base.map(hx).unwrap_or("?".into()), rows.len(), rows.iter().filter(|n| n.addr.is_some()).count());
    for n in &rows {
        let _ = writeln!(t, "{:>5} {:<56} {} {} {}", if n.index != 0 { n.index.to_string() } else { "-".into() }, n.script, n.addr.map(hx).unwrap_or("--------".into()), n.exec, n.via);
    }
    let js = json!(rows.iter().map(|n| json!({"index": n.index, "script": n.script, "exec": n.exec, "address": n.addr.map(hx), "via": n.via})).collect::<Vec<_>>());
    (t, js)
}

fn q_traces(ix: &Index) -> (String, Value) {
    let mut t = format!("{} trace call sites\n", ix.traces.len());
    for x in &ix.traces {
        let _ = writeln!(t, "{:<6} {} in {} {:<40} flags {:<28} extent {}", x.kind, hx(x.site), hx(x.caller), ix.name_of(x.caller), format!("{}{}", x.flags.iter().map(|f| format!("{f:#x}")).collect::<Vec<_>>().join("|"), if x.flags_dynamic { "|dyn" } else { "" }), x.extent);
    }
    let profiles = ix.trace_profiles();
    let _ = writeln!(t, "\n{} enumerated trace profiles (constant flags and extent):", profiles.len());
    for p in &profiles {
        let _ = writeln!(t, "  {:<6} flags {:#010x} extent {:?} {}", p.tag, p.flags, p.extent, p.name);
    }
    let js = json!({
        "sites": ix.traces.iter().map(|x| json!({"kind": x.kind, "site": hx(x.site), "caller": hx(x.caller), "name": ix.name_of(x.caller), "flags": x.flags.iter().map(|f| format!("0x{f:x}")).collect::<Vec<_>>(), "dynamic": x.flags_dynamic, "extent": x.extent, "note": x.note, "statement": x.statement})).collect::<Vec<_>>(),
        "profiles": profiles,
    });
    (t, js)
}

fn q_field(ix: &Index, class: &str, off: &str) -> Result<(String, Value)> {
    let c = ix.script_class(class).with_context(|| format!("no script class `{class}`"))?;
    let o = parse_addr(&format!("{:0>8}", off.trim_start_matches("0x"))).or_else(|| off.parse().ok()).context("offset")?;
    let hits: Vec<&FieldRow> = ix.all_fields(&c.name).into_iter().filter(|f| o >= f.offset && o < f.offset + f.size.max(1)).collect();
    let mut t = String::new();
    for f in &hits {
        let _ = writeln!(t, "{}+0x{:x}: {}.{} : {} (+0x{:x}, {} bytes{})", c.name, o, f.owner, f.name, f.ty, f.offset, f.size, f.bit.map(|b| format!(", bit {b} mask 0x{:x}", 1u32 << b)).unwrap_or_default());
    }
    if hits.is_empty() {
        let _ = writeln!(t, "{}+0x{o:x}: no script property there (instance size 0x{:x}; native-only member or padding)", c.name, c.size);
    }
    Ok((t, json!(hits.iter().map(|f| json!({"owner": f.owner, "name": f.name, "type": f.ty, "offset": format!("0x{:x}", f.offset), "size": f.size, "bit": f.bit})).collect::<Vec<_>>())))
}

fn q_vcall(ix: &Index, class: &str, slot: &str) -> Result<(String, Value)> {
    let v = ix.vtable(class).with_context(|| format!("no vtable for `{class}`"))?;
    let i = if let Some(h) = slot.strip_prefix("0x").or_else(|| slot.strip_prefix('+')) { usize::from_str_radix(h.trim_start_matches("0x"), 16)? / 4 } else { slot.parse()? };
    let a = *v.slots.get(i).with_context(|| format!("{class} has {} slots", v.slots.len()))?;
    let t = format!("{class} slot {i} (+0x{:x}) -> {} {}\n", i * 4, hx(a), ix.name_of(a));
    Ok((t, json!({"class": class, "slot": i, "address": hx(a), "name": ix.name_of(a)})))
}

fn q_stats(ix: &Index) -> (String, Value) {
    let mut t = format!("exe {}\nbuilt {}\n", ix.exe_sha256, ix.built);
    for (k, v) in &ix.stats {
        let _ = writeln!(t, "{k:<34} {v}");
    }
    (t, json!({"exe": ix.exe_sha256, "stats": ix.stats}))
}
