//! What the port handles, read mechanically from its source code.
//!
//! The port names the original's classes, Kismet ops, properties and enum values as string
//! literals where it dispatches on them (`match class.as_str() { "StaticMeshActor" => ... }`,
//! `is_a(op, "SeqVar_Float")`, `find(&props, "DrawScale")`, `c.ends_with("Light")`). This module
//! lexes every `.rs` file of the port's crates, collects those literals with the call they sit
//! in, and answers "does the port handle class X / read property Y" from them. Nothing is
//! hand-maintained: a new importer arm or Kismet handler shows up in the next scan.
//!
//! Rules (documented in research/dishonored/ORACLE.md):
//! - Scanned: `crates/*/src/**/*.rs` minus the crates that are not the port (`ue3_package`, the
//!   format reader; `dishonored_probe*`, the engine hook; `dishonored_oracle`, this tool) and minus
//!   test code (`tests/` and everything after a file's `#[cfg(test)]`). Comments are skipped.
//! - A class is **direct** when its name is a literal, **inherited** when an ancestor's name is the
//!   literal argument of an `is_a(...)` call, **pattern** when only an `ends_with` / `starts_with`
//!   / `contains` literal (at least four characters, starting upper-case) matches it.
//! - A property is **read** when its name is a literal or a dot-separated segment of one.
//! - Kismet ops count as implemented only by literals in the Kismet interpreter
//!   (`dishonored_kismet/src/runtime.rs`) and the game's Kismet host
//!   (`dishonored_multiplayer/src/kismet_host*`).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Crates under `crates/` that are not the port.
pub const NOT_PORT: &[&str] = &["ue3_package", "dishonored_probe", "dishonored_probe_dll", "dishonored_oracle"];

/// Files whose literals implement Kismet ops (prefixes of repo-relative paths).
pub const KISMET_FILES: &[&str] = &["crates/dishonored_kismet/src/runtime.rs", "crates/dishonored_multiplayer/src/kismet_host"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LitCtx {
    Plain,
    IsA,
    Suffix,
    Prefix,
    Contains,
    /// The argument of a string test on something that is not a class name (an animation,
    /// an asset path...): not a class or property mention.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Site {
    pub file: u32,
    pub line: u32,
    pub ctx: LitCtx,
    /// The enclosing function.
    pub func: String,
}

/// A part of the port's source: files (path prefixes) minus some functions.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub files: &'a [&'a str],
    pub exclude_fns: &'a [&'a str],
}

/// Where Kismet ops are implemented: the interpreter and the Kismet host, minus the
/// functions that only describe ops (`deactivation`: which outputs a class fires when it ends;
/// `renders_moved`: presentation of moved actors) and the host's lists of ops it runs as no-ops
/// (`PRESENTATION`: HUD, tutorials, statistics...; `NOT_MODELLED`: systems the port does not
/// simulate). Classes found only there are reported as acknowledged, not implemented.
pub const NO_OP_LISTS: &[&str] = &["deactivation", "renders_moved", "const:PRESENTATION", "const:NOT_MODELLED"];

/// The whole port minus its no-op lists (the scope of every non-Kismet class).
pub const PORT_SCOPE: Scope<'static> = Scope { files: &[""], exclude_fns: NO_OP_LISTS };

pub const KISMET_SCOPE: Scope<'static> = Scope { files: KISMET_FILES, exclude_fns: NO_OP_LISTS };

/// How the port covers a class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handling {
    /// Named directly (file:line of the first mention).
    Direct(String),
    /// Through an `is_a` check of an ancestor.
    Inherited { ancestor: String, at: String },
    /// Only through a name pattern.
    Pattern { pattern: String, at: String },
    /// Named only where the port lists what it deliberately does not do (a no-op list): not
    /// handled, but known.
    Acknowledged { list: String, at: String },
    None,
}

impl Handling {
    pub fn handled(&self) -> bool {
        !matches!(self, Handling::None | Handling::Acknowledged { .. })
    }
    pub fn tag(&self) -> &'static str {
        match self {
            Handling::Direct(_) => "direct",
            Handling::Inherited { .. } => "inherited",
            Handling::Pattern { .. } => "pattern",
            Handling::Acknowledged { .. } => "acknowledged",
            Handling::None => "none",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PortRegistry {
    pub files: Vec<String>,
    pub lines: usize,
    /// Lower-case literal -> where it appears.
    pub literals: HashMap<String, Vec<Site>>,
    /// Lower-case dot-separated segments of literals (property paths) -> count.
    pub segments: HashMap<String, u32>,
    /// Pattern literals (suffix / prefix / contains), original case.
    pub patterns: Vec<(String, Site)>,
}

impl PortRegistry {
    /// Scan the port's crates under `repo/crates`.
    pub fn scan(repo: &Path) -> std::io::Result<Self> {
        let mut files = Vec::new();
        let crates = repo.join("crates");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&crates)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            let name = d.file_name().unwrap().to_string_lossy().into_owned();
            if NOT_PORT.contains(&name.as_str()) {
                continue;
            }
            collect_rs(&d.join("src"), &mut files);
        }
        files.sort();
        let mut reg = PortRegistry::default();
        for f in &files {
            let rel = f.strip_prefix(repo).unwrap_or(f).to_string_lossy().replace('\\', "/");
            let Ok(text) = std::fs::read_to_string(f) else { continue };
            let idx = reg.files.len() as u32;
            reg.files.push(rel);
            reg.add_source(idx, &text);
        }
        Ok(reg)
    }

    /// Add one file's literals (used by [`Self::scan`] and the tests).
    pub fn add_source(&mut self, file: u32, text: &str) {
        let text = match text.find("#[cfg(test)]") {
            Some(i) => &text[..i],
            None => text,
        };
        self.lines += text.lines().count();
        for (lit, line, ctx, func) in lex_literals(text) {
            if lit.is_empty() || lit.len() > 200 || ctx == LitCtx::Other {
                continue;
            }
            let site = Site { file, line, ctx, func };
            if matches!(ctx, LitCtx::Suffix | LitCtx::Prefix | LitCtx::Contains) && lit.len() >= 4 && lit.starts_with(|c: char| c.is_ascii_uppercase()) {
                self.patterns.push((lit.clone(), site.clone()));
            }
            if lit.contains('.') {
                for seg in lit.split('.') {
                    if is_ident(seg) {
                        *self.segments.entry(seg.to_ascii_lowercase()).or_default() += 1;
                    }
                }
            }
            if is_ident(&lit) {
                self.literals.entry(lit.to_ascii_lowercase()).or_default().push(site);
            }
        }
    }

    pub fn at(&self, s: &Site) -> String {
        format!("{}:{}", self.files.get(s.file as usize).map(String::as_str).unwrap_or("?"), s.line)
    }

    fn in_scope(&self, s: &Site, scope: Option<&Scope>) -> bool {
        match scope {
            None => true,
            Some(sc) => {
                let path = &self.files[s.file as usize];
                sc.files.iter().any(|p| path.starts_with(p)) && !sc.exclude_fns.contains(&s.func.as_str())
            }
        }
    }

    fn sites<'a>(&'a self, name: &str, scope: Option<&'a Scope<'a>>) -> impl Iterator<Item = &'a Site> + 'a {
        self.literals.get(&name.to_ascii_lowercase()).into_iter().flatten().filter(move |s| self.in_scope(s, scope))
    }

    /// How the port covers `class` (`ancestors`: its super chain, nearest first). `scope`
    /// restricts the literals to part of the source, e.g. [`KISMET_SCOPE`].
    pub fn class_handling(&self, class: &str, ancestors: &[String], scope: Option<&Scope>) -> Handling {
        if let Some(s) = self.sites(class, scope).next() {
            return Handling::Direct(self.at(s));
        }
        for a in ancestors {
            if let Some(s) = self.sites(a, scope).find(|s| s.ctx == LitCtx::IsA) {
                return Handling::Inherited { ancestor: a.clone(), at: self.at(s) };
            }
        }
        let lc = class.to_ascii_lowercase();
        for (p, s) in &self.patterns {
            if !self.in_scope(s, scope) {
                continue;
            }
            let pl = p.to_ascii_lowercase();
            let hit = match s.ctx {
                LitCtx::Suffix => lc.ends_with(&pl),
                LitCtx::Prefix => lc.starts_with(&pl),
                LitCtx::Contains => lc.contains(&pl),
                _ => false,
            };
            if hit {
                return Handling::Pattern { pattern: format!("{:?}({p})", s.ctx), at: self.at(s) };
            }
        }
        if let Some(sc) = scope {
            let files_only = Scope { files: sc.files, exclude_fns: &[] };
            if let Some(s) = self.sites(class, Some(&files_only)).find(|s| sc.exclude_fns.contains(&s.func.as_str())) {
                return Handling::Acknowledged { list: s.func.clone(), at: self.at(s) };
            }
        }
        Handling::None
    }

    /// Whether the port names a property (or enum value) anywhere.
    pub fn names(&self, name: &str) -> bool {
        let l = name.to_ascii_lowercase();
        self.literals.contains_key(&l) || self.segments.contains_key(&l)
    }

    /// First place the port names `name`.
    pub fn first_site(&self, name: &str) -> Option<String> {
        self.sites(name, None).next().map(|s| self.at(s))
    }

    /// Literal counts per file (for the report header).
    pub fn per_crate(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for f in &self.files {
            let c = f.split('/').nth(1).unwrap_or("").to_string();
            *m.entry(c).or_default() += 1;
        }
        m
    }
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !s.starts_with(|c: char| c.is_ascii_digit())
}

/// The call a literal is an argument of: the identifier before the innermost unclosed `(` on
/// the literal's line, before the literal.
fn call_ctx(before: &str) -> LitCtx {
    let b = before.as_bytes();
    let mut depth = 0i32;
    let mut i = b.len();
    while i > 0 {
        i -= 1;
        match b[i] {
            b')' => depth += 1,
            b'(' => {
                if depth == 0 {
                    let mut j = i;
                    while j > 0 && (b[j - 1].is_ascii_alphanumeric() || b[j - 1] == b'_') {
                        j -= 1;
                    }
                    let pattern = match &before[j..i] {
                        "is_a" => return LitCtx::IsA,
                        "ends_with" => LitCtx::Suffix,
                        "starts_with" => LitCtx::Prefix,
                        "contains" => LitCtx::Contains,
                        _ => return LitCtx::Plain,
                    };
                    // A string test is a class pattern only on a class name: the receiver
                    // mentions `class` (`class`, `a.class`, `export_class(i)`) or is the `c` of
                    // a `match class { c if c.ends_with(..) }` arm.
                    let mut k = j;
                    if k == 0 || b[k - 1] != b'.' {
                        return LitCtx::Other;
                    }
                    k -= 1;
                    let end = k;
                    while k > 0 && (b[k - 1].is_ascii_alphanumeric() || matches!(b[k - 1], b'_' | b'.' | b'(' | b')' | b'[' | b']')) {
                        k -= 1;
                    }
                    let recv = before[k..end].to_ascii_lowercase();
                    return if recv.contains("class") || recv == "c" { pattern } else { LitCtx::Other };
                }
                depth -= 1;
            }
            b'\n' => break,
            _ => {}
        }
    }
    LitCtx::Plain
}

/// String literals of Rust source with their line and call context. Skips comments, char
/// literals and lifetimes; handles escapes, raw strings and byte strings.
pub fn lex_literals(src: &str) -> Vec<(String, u32, LitCtx, String)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut line, mut line_start) = (0usize, 1u32, 0usize);
    let n = b.len();
    // The enclosing function of every literal: one stack entry per open brace (`Some` for a
    // function body).
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut pending: Option<String> = None;
    // A `const` / `static` item being defined (until its `;`): `const:<NAME>`.
    let mut item: Option<String> = None;
    let func = |stack: &Vec<Option<String>>, item: &Option<String>| item.clone().or_else(|| stack.iter().rev().find_map(|x| x.clone())).unwrap_or_default();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < n {
        let c = b[i];
        match c {
            b'{' => stack.push(pending.take()),
            b'}' => {
                stack.pop();
            }
            b';' => {
                pending = None;
                item = None;
            }
            b'c' | b's' if (i == 0 || !(ident(b[i - 1]) || b[i - 1] == b'\'')) && (src[i..].starts_with("const ") || src[i..].starts_with("static ")) => {
                let mut j = i + if c == b'c' { 6 } else { 7 };
                while j < n && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let s = j;
                while j < n && ident(b[j]) {
                    j += 1;
                }
                let name = &src[s..j];
                if !name.is_empty() && name != "fn" && name != "mut" {
                    item = Some(format!("const:{name}"));
                }
            }
            b'f' if i + 2 < n && b[i + 1] == b'n' && b[i + 2].is_ascii_whitespace() && (i == 0 || !ident(b[i - 1])) => {
                let mut j = i + 2;
                while j < n && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let s = j;
                while j < n && ident(b[j]) {
                    j += 1;
                }
                if j > s {
                    pending = Some(src[s..j].to_string());
                }
            }
            _ => {}
        }
        if c == b'\n' {
            line += 1;
            line_start = i + 1;
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let mut depth = 1;
            i += 2;
            while i < n && depth > 0 {
                if b[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                if b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && i + 1 < n && b[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if c == b'\'' {
            // Char literal or lifetime.
            if i + 2 < n && b[i + 1] == b'\\' {
                // Skip the quote, the backslash and the escaped character.
                i += 3;
                while i < n && b[i] != b'\'' && b[i] != b'\n' {
                    i += 1;
                }
                i += 1;
            } else if i + 2 < n && b[i + 2] == b'\'' {
                i += 3;
            } else {
                // Multi-byte char literal ('é') or a lifetime.
                let rest = &src[i + 1..];
                let mut it = rest.char_indices();
                if let (Some((_, _)), Some((k, '\''))) = (it.next(), it.next()) {
                    i += 1 + k + 1;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // Raw strings: r"..", r#".."#, br"..".
        let raw_start = if c == b'r' && (i + 1 < n && (b[i + 1] == b'"' || b[i + 1] == b'#')) && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            Some(i + 1)
        } else if c == b'b' && i + 2 < n && b[i + 1] == b'r' && (b[i + 2] == b'"' || b[i + 2] == b'#') && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            Some(i + 2)
        } else {
            None
        };
        if let Some(mut j) = raw_start {
            let mut hashes = 0;
            while j < n && b[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < n && b[j] == b'"' {
                let ctx = call_ctx(&src[line_start..i]);
                let start_line = line;
                j += 1;
                let body_start = j;
                loop {
                    if j >= n {
                        break;
                    }
                    if b[j] == b'"' && (0..hashes).all(|k| j + 1 + k < n && b[j + 1 + k] == b'#') {
                        break;
                    }
                    if b[j] == b'\n' {
                        line += 1;
                        line_start = j + 1;
                    }
                    j += 1;
                }
                out.push((src[body_start..j.min(n)].to_string(), start_line, ctx, func(&stack, &item)));
                i = (j + 1 + hashes).min(n);
                continue;
            }
        }
        if c == b'"' {
            let ctx = call_ctx(&src[line_start..i]);
            let start_line = line;
            let mut s = String::new();
            i += 1;
            let mut seg = i;
            while i < n && b[i] != b'"' {
                if b[i] == b'\\' {
                    s.push_str(&src[seg..i]);
                    i += 1;
                    if i < n {
                        match b[i] {
                            b'n' => s.push('\n'),
                            b't' => s.push('\t'),
                            b'"' => s.push('"'),
                            b'\\' => s.push('\\'),
                            b'\n' => {
                                line += 1;
                                line_start = i + 1;
                                // Line continuation: skip the next line's leading whitespace.
                                while i + 1 < n && (b[i + 1] == b' ' || b[i + 1] == b'\t') {
                                    i += 1;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    seg = i;
                    continue;
                }
                if b[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
            }
            s.push_str(&src[seg..i.min(n)]);
            out.push((s, start_line, ctx, func(&stack, &item)));
            i += 1;
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_with_context_skip_comments_and_chars() {
        let src = r#####"
// "InComment"
fn f(c: &str) -> bool {
    /* "Block" /* "Nested" */ */
    let x = 'a'; let y = '\''; let z: &'static str = "Plain";
    if db.is_a(&class, "SeqVar_Float") { return true; }
    c.ends_with("Light") || matches!(c, "StaticMeshActor" | "DisFog") && find(&props, "DrawScale").is_some()
        || r#"Raw"# == "a.b_c.Path" || anim.contains("Walk")
}
"#####;
        let v = lex_literals(src);
        let names: Vec<(&str, LitCtx)> = v.iter().map(|(s, _, c, _)| (s.as_str(), *c)).collect();
        assert_eq!(
            names,
            vec![
                ("Plain", LitCtx::Plain),
                ("SeqVar_Float", LitCtx::IsA),
                ("Light", LitCtx::Suffix),
                ("StaticMeshActor", LitCtx::Plain),
                ("DisFog", LitCtx::Plain),
                ("DrawScale", LitCtx::Plain),
                ("Raw", LitCtx::Plain),
                ("a.b_c.Path", LitCtx::Plain),
                ("Walk", LitCtx::Other),
            ]
        );
        assert_eq!(v[1].1, 6);
    }

    #[test]
    fn class_handling_rules() {
        let mut r = PortRegistry::default();
        r.files.push("crates/dishonored_assets/src/level.rs".into());
        r.files.push("crates/dishonored_kismet/src/runtime.rs".into());
        r.add_source(0, r#"match c { "StaticMeshActor" => 1, c if c.ends_with("Light") => 2, _ => find(&p, "m_Tint.R") }"#);
        r.add_source(1, r#"if self.is_a(op, "SeqVar_Float") {} match class { "SeqAct_Delay" => {} }"#);
        assert!(matches!(r.class_handling("StaticMeshActor", &[], None), Handling::Direct(_)));
        assert!(matches!(r.class_handling("PointLight", &["Light".into()], None), Handling::Pattern { .. }));
        assert!(matches!(r.class_handling("SeqVar_RandomFloat", &["SeqVar_Float".into()], Some(&KISMET_SCOPE)), Handling::Inherited { .. }));
        assert_eq!(r.class_handling("StaticMeshActor", &[], Some(&KISMET_SCOPE)), Handling::None);
        assert!(matches!(r.class_handling("SeqAct_Delay", &[], Some(&KISMET_SCOPE)), Handling::Direct(_)));
        // A class named only in a function that describes ops does not count as implemented.
        r.add_source(1, "pub fn deactivation(class: &str) -> u8 { match class { \"DisSeqAct_AIDoFollow\" => 1, _ => 0 } }\nfn activated() { let c = '{'; let q = '\\''; x(\"DisSeqAct_Real\"); }");
        assert!(matches!(r.class_handling("DisSeqAct_AIDoFollow", &[], Some(&KISMET_SCOPE)), Handling::Acknowledged { .. }));
        assert!(matches!(r.class_handling("DisSeqAct_AIDoFollow", &[], None), Handling::Direct(_)));
        assert!(matches!(r.class_handling("DisSeqAct_Real", &[], Some(&KISMET_SCOPE)), Handling::Direct(_)));
        let v = lex_literals("impl X { fn a(&self) { f(\"in_a\") } fn b() -> u8 { g(\"in_b\"); 0 } }\nfn c();\nconst Z: &str = \"top\";");
        let funcs: Vec<(&str, &str)> = v.iter().map(|x| (x.0.as_str(), x.3.as_str())).collect();
        assert_eq!(funcs, vec![("in_a", "a"), ("in_b", "b"), ("top", "const:Z")]);
        assert!(r.names("m_tint") && r.names("R") && r.names("drawscale") == false);
    }
}
