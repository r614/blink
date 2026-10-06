//! `shader_dump <package> <out_dir> [filter]`: carve the D3D9 SM3 shader blobs out of a shader
//! cache package (`RefShaderCache-PC-D3D-SM3.upk`, or a level's `SeekFreeShaderCache`), name each
//! by the shader type FName that precedes it in the cache record, and list the constant-table
//! (CTAB) parameter names. Blobs whose type or constants contain `filter` (case-insensitive) are
//! written to `<out_dir>/<n>_<type>.bin` for `fxc /dumpbin`; `<out_dir>/index.txt` lists every
//! shader. The output is retail data: write it outside the repo and never commit it.
use anyhow::{Context, Result};
use ue3_package::Package;

fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    d.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Constant names of a D3D9 shader's CTAB comment (first comment block after the version).
fn ctab_names(code: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(tok) = u32_at(code, 4) else { return out };
    if tok & 0xFFFF != 0xFFFE || code.get(8..12) != Some(b"CTAB") {
        return out;
    }
    let len = ((tok >> 16) & 0x7FFF) as usize * 4;
    let base = 12;
    let ctab = &code[base..(8 + len).min(code.len())];
    let (Some(n), Some(info)) = (u32_at(ctab, 12), u32_at(ctab, 16)) else { return out };
    for i in 0..n as usize {
        let e = info as usize + i * 20;
        let Some(name_off) = u32_at(ctab, e) else { break };
        let reg = ctab.get(e + 4..e + 10).map(|b| (u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]), u16::from_le_bytes([b[4], b[5]])));
        let s: Vec<u8> = ctab.iter().skip(name_off as usize).take_while(|&&c| c != 0).copied().collect();
        let (set, idx, cnt) = reg.unwrap_or_default();
        let set = ["b", "i", "c", "s"].get(set as usize).copied().unwrap_or("?");
        out.push(format!("{}={set}{idx}x{cnt}", String::from_utf8_lossy(&s)));
    }
    out
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let path = a.first().context("package")?;
    // Raw files (`GlobalShaderCache-*.bin`) are carved without a name table.
    let pkg = Package::open(path).ok();
    let raw;
    let d: &[u8] = match &pkg {
        Some(p) => &p.data,
        None => {
            raw = std::fs::read(path)?;
            &raw
        }
    };
    // `SHADER_DUMP_RAW=<file>`: also write the decompressed cache (research, scratch only).
    if let Some(p) = std::env::var_os("SHADER_DUMP_RAW") {
        std::fs::write(p, d)?;
    }
    let names: Vec<String> = pkg.as_ref().map(|p| p.names.iter().map(|n| n.name.clone()).collect()).unwrap_or_default();
    let out = std::path::PathBuf::from(a.get(1).context("out_dir")?);
    let filter = a.get(2).map(|s| s.to_ascii_lowercase());
    std::fs::create_dir_all(&out)?;
    let mut index = String::new();
    let (mut n, mut written) = (0usize, 0usize);
    let mut p = 4;
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
        let code = &d[p..p + len];
        // The shader type FName sits a few fields before the code array (type, guid, hash,
        // skip offset, target); take the nearest preceding valid name with number 0.
        let mut ty = String::from("?");
        for back in (8..=96).step_by(1) {
            let Some(q) = (p - 4).checked_sub(back) else { break };
            let (Some(i), Some(num)) = (u32_at(d, q), u32_at(d, q + 4)) else { continue };
            if num == 0 && (i as usize) < names.len() {
                let s = &names[i as usize];
                if s.contains("Shader") || s.contains("Policy") || s.starts_with('T') || s.starts_with('F') {
                    ty = s.clone();
                    break;
                }
            }
        }
        let kind = if ver[2] == 0xFF { "ps" } else { "vs" };
        if std::env::var_os("SHADER_CTX").is_some() && n < 4 {
            let from = p.saturating_sub(128);
            println!("before shader {n} at {p:#x}:");
            for (k, c) in d[from..p].chunks(16).enumerate() {
                println!("  -{:3}: {}", p - from - k * 16, c.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
            }
        }
        let names = ctab_names(code);
        index.push_str(&format!("{n}\t{p:#x}\t{kind}_{}_{}\t{ty}\t{}\n", ver[1], ver[0], names.join(" ")));
        let hay = format!("{ty} {}", names.join(" ")).to_ascii_lowercase();
        if filter.as_ref().is_some_and(|f| hay.contains(f.as_str())) {
            std::fs::write(out.join(format!("{n}_{kind}_{}.bin", ty.replace(['<', '>', ',', ' ', ':', '?', '*', '"', '|', '/', '\\'], "_"))), code)?;
            written += 1;
        }
        n += 1;
        p += len;
    }
    std::fs::write(out.join("index.txt"), index)?;
    println!("{n} shaders, {written} written to {}", out.display());
    Ok(())
}
