//! `shader_maps <RefShaderCache.upk> <out_dir> [material filter] [switch=value ...]`: list the
//! material shader maps of a UE3 shader cache: the material's friendly name, its static parameter
//! set (switches, component masks), the shaders it uses (shader type, vertex factory, and the
//! shader number in `shader_dump`'s numbering) and its uniform expression set (the expressions
//! behind `UniformPixelVector_n` / `UniformPixelScalars_n` / `Texture2D_n`).
//!
//! Maps whose friendly name contains the filter and whose switches match every `switch=value`
//! (`1`/`0`) are printed, and their shader blobs written to `<out_dir>/<n>_<ps|vs>.bin` for
//! `fxc /dumpbin`. The output is retail data: write it outside the repo and never commit it.
//!
//! Layout (Dishonored's UE3, found by inspection): a shader record is `type FName, GUID, SHA hash
//! (20 bytes), skip offset, ..., code`; a material shader map is `0x321, ?, skip offset`, the
//! shader entries `(type FName, GUID)` (mesh shader maps introduced by their vertex factory
//! FName), the friendly name `FString`, the `FStaticParameterSet` and the uniform expression
//! arrays (pixel vectors, pixel scalars, 2D textures).
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fmt::Write as _;
use ue3_package::Package;

fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    d.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn f32_at(d: &[u8], p: usize) -> f32 {
    f32::from_bits(u32_at(d, p).unwrap_or(0))
}

struct Shader {
    code_at: usize,
    len: usize,
}

/// Every shader blob in `shader_dump`'s order, keyed by its record GUID.
fn scan_shaders(d: &[u8], names: &[String]) -> (Vec<Shader>, HashMap<[u8; 16], (usize, String)>) {
    let mut out = Vec::new();
    let mut by_guid = HashMap::new();
    let mut p = 4;
    let mut n = 0;
    while p + 8 < d.len() {
        let ver = &d[p..p + 4];
        let is_shader = (ver[3] == 0xFF && (ver[2] == 0xFF || ver[2] == 0xFE)) && (ver[1] == 3 || ver[1] == 2) && ver[0] <= 1;
        if !is_shader {
            p += 1;
            continue;
        }
        let len = u32_at(d, p - 4).unwrap_or(0) as usize;
        if len < 16 || len > 1 << 20 || p + len > d.len() || u32_at(d, p + len - 4) != Some(0x0000_FFFF) || d.get(p + 8..p + 12) != Some(b"CTAB") {
            p += 1;
            continue;
        }
        for back in 40..4096usize {
            let Some(q) = p.checked_sub(back) else { break };
            let (Some(i), Some(num), Some(skip)) = (u32_at(d, q), u32_at(d, q + 4), u32_at(d, q + 44)) else { continue };
            if num != 0 || i as usize >= names.len() || !names[i as usize].contains("Shader") {
                continue;
            }
            if (skip as usize) > p && (skip as usize) < p + 200_000 {
                let mut g = [0u8; 16];
                g.copy_from_slice(&d[q + 8..q + 24]);
                by_guid.insert(g, (n, names[i as usize].clone()));
                break;
            }
        }
        out.push(Shader { code_at: p, len });
        n += 1;
        p += len;
    }
    (out, by_guid)
}

struct Expr<'a> {
    d: &'a [u8],
    names: &'a [String],
    pos: usize,
}

impl Expr<'_> {
    fn name(&self, p: usize) -> String {
        u32_at(self.d, p).and_then(|i| self.names.get(i as usize)).cloned().unwrap_or_else(|| "?".into())
    }
    fn expr(&mut self, depth: usize) -> Option<String> {
        if depth > 32 {
            return None;
        }
        let t = u32_at(self.d, self.pos)?;
        if u32_at(self.d, self.pos + 4)? != 0 || !(512..=528).contains(&t) {
            return None;
        }
        self.pos += 8;
        let d = self.d;
        Some(match t {
            512 => format!("Abs({})", self.expr(depth + 1)?),
            513 => {
                let (a, b) = (self.expr(depth + 1)?, self.expr(depth + 1)?);
                self.pos += 4;
                format!("Append({a}, {b})")
            }
            514 => {
                let (a, b, c) = (self.expr(depth + 1)?, self.expr(depth + 1)?, self.expr(depth + 1)?);
                format!("Clamp({a}, {b}, {c})")
            }
            515 => {
                let v = (f32_at(d, self.pos), f32_at(d, self.pos + 4), f32_at(d, self.pos + 8), f32_at(d, self.pos + 12));
                self.pos += 17;
                format!("C({},{},{},{})", v.0, v.1, v.2, v.3)
            }
            517 => format!("Floor({})", self.expr(depth + 1)?),
            518 => {
                let (a, b) = (self.expr(depth + 1)?, self.expr(depth + 1)?);
                format!("Fmod({a}, {b})")
            }
            519 => {
                let (a, b) = (self.expr(depth + 1)?, self.expr(depth + 1)?);
                let op = d[self.pos];
                self.pos += 1;
                let o = ["+", "-", "*", "/", "dot"].get(op as usize).copied().unwrap_or("?");
                format!("({a} {o} {b})")
            }
            520 => format!("Frac({})", self.expr(depth + 1)?),
            521 => format!("Periodic({})", self.expr(depth + 1)?),
            522 => "RealTime".into(),
            523 => {
                let s = format!("S[{}={}]", self.name(self.pos), f32_at(d, self.pos + 8));
                self.pos += 12;
                s
            }
            524 => {
                let a = self.expr(depth + 1)?;
                let c = u32_at(d, self.pos)?;
                self.pos += 4;
                format!("{}({a})", if c != 0 { "Cos" } else { "Sin" })
            }
            525 => {
                let i = u32_at(d, self.pos)?;
                self.pos += 4;
                format!("Tex#{i}")
            }
            526 => {
                let s = format!("T[{}]", self.name(self.pos));
                self.pos += 12;
                s
            }
            527 => "Time".into(),
            528 => {
                let p = self.pos;
                let s = format!("V[{}=({},{},{},{})]", self.name(p), f32_at(d, p + 8), f32_at(d, p + 12), f32_at(d, p + 16), f32_at(d, p + 20));
                self.pos += 24;
                s
            }
            _ => return None,
        })
    }
    fn array(&mut self) -> Option<Vec<String>> {
        let c = u32_at(self.d, self.pos)? as usize;
        if c > 64 {
            return None;
        }
        self.pos += 4;
        (0..c).map(|_| self.expr(0)).collect()
    }
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let pkg = Package::open(a.first().context("package")?)?;
    let out = std::path::PathBuf::from(a.get(1).context("out_dir")?);
    let filter = a.get(2).cloned().unwrap_or_default().to_ascii_lowercase();
    let want: Vec<(String, bool)> = a.iter().skip(3).filter_map(|s| s.split_once('=')).map(|(k, v)| (k.to_ascii_lowercase(), v == "1" || v.eq_ignore_ascii_case("true"))).collect();
    std::fs::create_dir_all(&out)?;
    let d = &pkg.data;
    let names: Vec<String> = pkg.names.iter().map(|n| n.name.clone()).collect();
    let (shaders, by_guid) = scan_shaders(d, &names);
    eprintln!("{} shaders, {} with records", shaders.len(), by_guid.len());
    let name = |p: usize| -> Option<&String> { names.get(u32_at(d, p)? as usize).filter(|_| u32_at(d, p + 4) == Some(0)) };
    // Parameter names: UE3 stores `Name_2` as ("Name", 3).
    let pname = |p: usize| -> String {
        let (Some(i), Some(n)) = (u32_at(d, p), u32_at(d, p + 4)) else { return String::new() };
        let b = names.get(i as usize).cloned().unwrap_or_default();
        if n == 0 { b } else { format!("{b}_{}", n - 1) }
    };
    let mut report = String::new();
    let mut maps = 0;
    let mut p = 0;
    while p + 12 < d.len() {
        if u32_at(d, p) != Some(0x321) {
            p += 1;
            continue;
        }
        let skip = u32_at(d, p + 8).unwrap_or(0) as usize;
        if skip <= p + 12 || skip > p + (8 << 20) || skip > d.len() {
            p += 1;
            continue;
        }
        // Entries up to the friendly name.
        let mut entries: Vec<(String, String, Option<usize>)> = Vec::new();
        let mut vf = String::from("-");
        let mut q = p + 12;
        let mut fname: Option<(String, usize)> = None;
        while q + 24 < skip {
            if let Some(len) = u32_at(d, q).map(|l| l as usize).filter(|l| (4..=128).contains(l)) {
                if q + 4 + len <= d.len() && d[q + 4 + len - 1] == 0 {
                    let s = &d[q + 4..q + 4 + len - 1];
                    if s.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'.') && s.contains(&b'_') {
                        fname = Some((String::from_utf8_lossy(s).into_owned(), q + 4 + len));
                        break;
                    }
                }
            }
            if let Some(n) = name(q) {
                if n.contains("VertexFactory") {
                    vf = n.clone();
                } else if n.contains("Shader") {
                    let mut g = [0u8; 16];
                    g.copy_from_slice(&d[q + 8..q + 24]);
                    entries.push((vf.clone(), n.clone(), by_guid.get(&g).map(|x| x.0)));
                    // (type, GUID, type again)
                    q += 32;
                    continue;
                }
            }
            q += 1;
        }
        let Some((fname, after)) = fname else {
            p += 1;
            continue;
        };
        maps += 1;
        // Static parameter set after the name: base GUID, switches, component masks.
        let mut s = after + 16;
        let mut switches: Vec<(String, bool, bool)> = Vec::new();
        let ns = u32_at(d, s).unwrap_or(0) as usize;
        s += 4;
        if ns < 128 {
            for _ in 0..ns {
                switches.push((pname(s),u32_at(d, s + 8) == Some(1), u32_at(d, s + 12) == Some(1)));
                s += 32;
            }
        }
        let mut masks: Vec<(String, String)> = Vec::new();
        let nm = u32_at(d, s).unwrap_or(0) as usize;
        s += 4;
        if nm < 128 {
            for _ in 0..nm {
                let ch: String = (0..4).filter(|k| u32_at(d, s + 8 + k * 4) == Some(1)).map(|k| ['R', 'G', 'B', 'A'][k]).collect();
                masks.push((pname(s), ch));
                s += 44;
            }
        }
        let matches = fname.to_ascii_lowercase().contains(&filter)
            && want.iter().all(|(k, v)| switches.iter().any(|(n, b, _)| n.eq_ignore_ascii_case(k) && b == v));
        if matches {
            let _ = writeln!(report, "=== map at {p:#x}: {fname}");
            let _ = writeln!(report, "  switches: {}", switches.iter().filter(|x| x.1).map(|x| x.0.as_str()).collect::<Vec<_>>().join(" "));
            let _ = writeln!(report, "  masks: {}", masks.iter().map(|m| format!("{}={}", m.0, m.1)).collect::<Vec<_>>().join(" "));
            for (vf, ty, n) in &entries {
                let _ = writeln!(report, "  {vf} {ty} -> {}", n.map(|n| n.to_string()).unwrap_or_else(|| "?".into()));
                if let Some(n) = n {
                    let sh = &shaders[*n];
                    std::fs::write(out.join(format!("{n}_{}.bin", if d[sh.code_at + 2] == 0xFF { "ps" } else { "vs" })), &d[sh.code_at..sh.code_at + sh.len])?;
                }
            }
            // Uniform expressions: the first vector array starting with SelectionColor.
            for t in s..(s + 20_000).min(skip) {
                if u32_at(d, t + 4) != Some(528) {
                    continue;
                }
                let mut e = Expr { d, names: &names, pos: t };
                let Some(vec) = e.array() else { continue };
                if !vec.first().is_some_and(|v| v.contains("SelectionColor")) {
                    continue;
                }
                let Some(sc) = e.array() else { continue };
                let tex = e.array().unwrap_or_default();
                for (i, v) in vec.iter().enumerate() {
                    let _ = writeln!(report, "  UniformPixelVector_{i} = {v}");
                }
                for (i, v) in sc.iter().enumerate() {
                    let _ = writeln!(report, "  UniformPixelScalars_{}.{} = {v}", i / 4, ['x', 'y', 'z', 'w'][i % 4]);
                }
                for (i, v) in tex.iter().enumerate() {
                    let _ = writeln!(report, "  Texture2D_{i} = {v}");
                }
                break;
            }
        }
        p = skip;
    }
    std::fs::write(out.join("maps.txt"), &report)?;
    println!("{maps} shader maps; report in {}", out.join("maps.txt").display());
    Ok(())
}
