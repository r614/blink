//! The Ghidra side of the static index: running `analyzeHeadless` with
//! `tools/ghidra_index/scripts/BlinkIndex.java` and reading the tables it writes.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Trace entry points of the GOG exe (`70da9613...`): `UWorld::SingleLineCheck` (COLLISION.md,
/// the hook's `LINE_CHECKS`) and `UWorld::MultiLineCheck` (physWalking's floor check).
pub const DEFAULT_TRACES: &str = "0x0064E7A0/single+0x0064A860/multi";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Func {
    pub addr: u32,
    pub size: u32,
    pub name: String,
    /// Symbol source (`DEFAULT`, `ANALYSIS`, `IMPORTED`...).
    pub source: String,
    pub thunk: Option<u32>,
}

/// One call instruction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Call {
    pub caller: u32,
    pub site: u32,
    /// `d` direct, `v` through a vtable (target = slot byte offset), `i` import or pointer, `r` register.
    pub kind: char,
    pub target: u32,
    /// Import name or register text for `i` / `r`.
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StrRef {
    pub func: u32,
    pub site: u32,
    pub addr: u32,
    pub utf16: bool,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassVtable {
    pub name: String,
    pub record: u32,
    pub ctor: u32,
    pub vtable: u32,
    pub slots: Vec<u32>,
    /// Interface (secondary) vtables the constructor also stores: (address, slots).
    pub others: Vec<(u32, Vec<u32>)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraceSite {
    /// `single` / `multi`.
    pub kind: String,
    pub caller: u32,
    pub site: u32,
    /// Constant trace flags the call can pass (through copies, ORs, phis and callers' arguments).
    pub flags: Vec<u32>,
    /// Whether some path passes flags that are not constant.
    pub flags_dynamic: bool,
    /// `stack:x,y,z` / `global:<addr>:x,y,z` (constant extent), `null`, `dyn`...
    pub extent: String,
    pub note: String,
    /// The decompiled call statement.
    pub statement: String,
}

impl TraceSite {
    /// The constant extent, when resolved.
    pub fn extent_value(&self) -> Option<[f32; 3]> {
        let v = self.extent.strip_prefix("stack:").or_else(|| self.extent.strip_prefix("global:").and_then(|r| r.split_once(':').map(|x| x.1)))?;
        let p: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
        (p.len() == 3).then(|| [p[0], p[1], p[2]])
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tables {
    pub sha256: String,
    pub functions: Vec<Func>,
    pub calls: Vec<Call>,
    pub strings: Vec<StrRef>,
    /// FNativeFunctionLookup entries: (exec name, function, table).
    pub execs: Vec<(String, u32, u32)>,
    /// Exec function pointers outside the lookup tables: (data address, function).
    pub native_arrays: Vec<(u32, u32)>,
    pub classes: Vec<ClassVtable>,
    pub traces: Vec<TraceSite>,
    pub summary: String,
}

fn hex(s: &str) -> Result<u32> {
    u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).with_context(|| format!("bad hex `{s}`"))
}

fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('t') => o.push('\t'),
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            Some('\\') => o.push('\\'),
            Some('x') => {
                let h: String = it.by_ref().take(2).collect();
                if let Ok(v) = u8::from_str_radix(&h, 16) {
                    o.push(v as char);
                }
            }
            Some(x) => {
                o.push('\\');
                o.push(x);
            }
            None => o.push('\\'),
        }
    }
    o
}

fn lines(dir: &Path, name: &str) -> Result<String> {
    std::fs::read_to_string(dir.join(name)).with_context(|| format!("{}", dir.join(name).display()))
}

/// Read the tables `BlinkIndex.java` wrote into `dir`.
pub fn read_tables(dir: &Path) -> Result<Tables> {
    let mut t = Tables { summary: lines(dir, "summary.txt").unwrap_or_default(), ..Default::default() };
    t.sha256 = t.summary.lines().next().and_then(|l| l.split(" sha256 ").nth(1)).unwrap_or_default().trim().to_string();
    for l in lines(dir, "functions.tsv")?.lines() {
        let c: Vec<&str> = l.split('\t').collect();
        if c.len() < 5 {
            continue;
        }
        let thunk = (c[4] != "-").then(|| c[4].split(':').next().and_then(|a| hex(a).ok())).flatten();
        t.functions.push(Func { addr: hex(c[0])?, size: c[1].parse().unwrap_or(0), name: c[2].to_string(), source: c[3].to_string(), thunk });
    }
    t.functions.sort_by_key(|f| f.addr);
    t.functions.dedup_by_key(|f| f.addr);
    for l in lines(dir, "calls.tsv")?.lines() {
        let c: Vec<&str> = l.split('\t').collect();
        if c.len() < 4 {
            continue;
        }
        let kind = c[1].chars().next().unwrap_or('?');
        let (target, text) = match kind {
            'd' => (hex(c[3])?, String::new()),
            'v' => (u32::from_str_radix(c[3], 16).unwrap_or(0), String::new()),
            _ => (0, c[3].to_string()),
        };
        t.calls.push(Call { caller: hex(c[0])?, site: hex(c[2])?, kind, target, text });
    }
    for l in lines(dir, "strings.tsv")?.lines() {
        let c: Vec<&str> = l.splitn(5, '\t').collect();
        if c.len() < 5 {
            continue;
        }
        t.strings.push(StrRef { func: hex(c[0])?, site: hex(c[1])?, addr: hex(c[2])?, utf16: c[3] == "u", text: unesc(c[4]) });
    }
    for l in lines(dir, "natives.tsv")?.lines() {
        let c: Vec<&str> = l.split('\t').collect();
        if c.len() >= 3 {
            t.execs.push((c[1].to_string(), hex(c[2])?, hex(c[0])?));
        }
    }
    if let Ok(s) = lines(dir, "native_arrays.tsv") {
        for l in s.lines() {
            let c: Vec<&str> = l.split('\t').collect();
            if c.len() >= 2 {
                t.native_arrays.push((hex(c[0])?, hex(c[1])?));
            }
        }
    }
    let mut others: std::collections::HashMap<String, Vec<(u32, Vec<u32>)>> = Default::default();
    if let Ok(s) = lines(dir, "ivtables.tsv") {
        for l in s.lines() {
            let c: Vec<&str> = l.split('\t').collect();
            if c.len() >= 4 {
                let slots = c[3].split(',').filter(|x| !x.is_empty()).map(hex).collect::<Result<Vec<_>>>()?;
                others.entry(c[0].to_string()).or_default().push((hex(c[1])?, slots));
            }
        }
    }
    for l in lines(dir, "classes.tsv")?.lines() {
        let c: Vec<&str> = l.split('\t').collect();
        if c.len() < 6 {
            continue;
        }
        let slots = c[5].split(',').filter(|x| !x.is_empty()).map(hex).collect::<Result<Vec<_>>>()?;
        t.classes.push(ClassVtable { name: c[0].to_string(), record: hex(c[1])?, ctor: hex(c[2])?, vtable: hex(c[3])?, slots, others: others.remove(c[0]).unwrap_or_default() });
    }
    if let Ok(s) = lines(dir, "traces.tsv") {
        for l in s.lines() {
            let c: Vec<&str> = l.split('\t').collect();
            if c.len() < 8 {
                continue;
            }
            let mut flags = Vec::new();
            let mut dynamic = false;
            for f in c[3].split('|') {
                if f == "dyn" || f == "?" {
                    dynamic = true;
                } else if let Ok(v) = hex(f) {
                    flags.push(v);
                }
            }
            t.traces.push(TraceSite { kind: c[0].to_string(), caller: hex(c[1])?, site: hex(c[2])?, flags, flags_dynamic: dynamic, extent: c[4].to_string(), note: c[5].to_string(), statement: unesc(c[7]) });
        }
    }
    Ok(t)
}

crate::codec!(Func { addr, size, name, source, thunk });
crate::codec!(Call { caller, site, kind, target, text });
crate::codec!(StrRef { func, site, addr, utf16, text });
crate::codec!(ClassVtable { name, record, ctor, vtable, slots, others });
crate::codec!(TraceSite { kind, caller, site, flags, flags_dynamic, extent, note, statement });

/// Where Ghidra lives and which analysed project to use.
#[derive(Debug, Clone)]
pub struct GhidraSetup {
    pub ghidra: PathBuf,
    pub jdk: PathBuf,
    /// Candidate project directories (each holding `<name>.gpr`); the first unlocked one is used.
    pub projects: Vec<PathBuf>,
    pub project_name: String,
    pub program: String,
    pub max_mem: String,
}

impl GhidraSetup {
    pub fn from_env() -> Self {
        let dev = std::env::var_os("BLINK_TOOLS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\Roshan\DEV\tools"));
        let projects = std::env::var("BLINK_GHIDRA_PROJECTS").map(|s| s.split(';').filter(|x| !x.is_empty()).map(PathBuf::from).collect()).unwrap_or_default();
        GhidraSetup {
            ghidra: std::env::var_os("GHIDRA_HOME").map(PathBuf::from).unwrap_or_else(|| dev.join("ghidra_12.1.4_PUBLIC")),
            jdk: std::env::var_os("GHIDRA_JDK").map(PathBuf::from).unwrap_or_else(|| dev.join("jdk-21.0.12.1+1")),
            projects,
            project_name: std::env::var("BLINK_GHIDRA_PROJECT_NAME").unwrap_or_else(|_| "dishonored".into()),
            program: "Dishonored.exe".into(),
            max_mem: std::env::var("BLINK_GHIDRA_MAXMEM").unwrap_or_else(|_| "8G".into()),
        }
    }
}

/// Run the export into `out`. Tries each project until one is not locked by another session.
/// `scripts` is `tools/ghidra_index/scripts`.
pub fn run_export(g: &GhidraSetup, scripts: &Path, out: &Path, classes: Option<&Path>, traces: &str, decompile: bool) -> Result<String> {
    std::fs::create_dir_all(out)?;
    if g.projects.is_empty() {
        bail!("no analysed Ghidra project given: pass --project <dir> (holding {}.gpr) or set BLINK_GHIDRA_PROJECTS; a fresh one is made with `analyzeHeadless <dir> {} -import Dishonored.exe` (about 15 min)", g.project_name, g.project_name);
    }
    let headless = g.ghidra.join("support").join("analyzeHeadless.bat");
    for _round in 0..240 {
        for p in &g.projects {
            if p.join(format!("{}.lock", g.project_name)).exists() {
                continue;
            }
            let mut args: Vec<String> = vec![
                p.display().to_string(),
                g.project_name.clone(),
                "-process".into(),
                g.program.clone(),
                "-noanalysis".into(),
                "-readOnly".into(),
                "-scriptPath".into(),
                scripts.display().to_string(),
                "-postScript".into(),
                "BlinkIndex.java".into(),
                out.display().to_string(),
                format!("traces:{traces}"),
            ];
            if let Some(c) = classes {
                args.push(format!("classes:{}", c.display()));
            }
            if !decompile {
                args.push("nodecomp".into());
            }
            let o = Command::new("cmd")
                .arg("/c")
                .arg(&headless)
                .args(&args)
                .env("JAVA_HOME", &g.jdk)
                .env("PATH", format!("{};{}", g.jdk.join("bin").display(), std::env::var("PATH").unwrap_or_default()))
                .env("GHIDRA_HEADLESS_MAXMEM", &g.max_mem)
                .output()
                .context("running analyzeHeadless")?;
            let log = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            if log.contains("LockException") {
                continue;
            }
            std::fs::write(out.join("ghidra.log"), &log).ok();
            if !out.join("summary.txt").exists() {
                bail!("the Ghidra export wrote no summary; see {}", out.join("ghidra.log").display());
            }
            return Ok(log);
        }
        std::thread::sleep(std::time::Duration::from_secs(15));
    }
    bail!("every Ghidra project stayed locked")
}
