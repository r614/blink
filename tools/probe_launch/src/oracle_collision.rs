//! `probe_launch collide <queries.csv> <out.csv> [--source pawn|none]`: run a batch of
//! collision checks in the original with the engine's own `UWorld::SingleLineCheck`
//! (research/dishonored/COLLISION.md).
//!
//! Query lines (`#` comments): `id sx sy sz ex ey ez hx hy hz flags` (flags hex, `0x...`),
//! the start, the end, the box half size (0 0 0: a line). Output lines:
//! `id hit time lx ly lz nx ny nz actor component` (names `-` when nothing was hit). The
//! source actor (default the player's pawn) is ignored by the checks and selects the movement
//! trace type when the flags carry no gameplay type. The output is retail-derived: keep it out
//! of the repository (the harness stores only coordinates and hit flags).
use anyhow::{Context, Result};
use dishonored_probe::LineQuery;
use std::collections::HashMap;
use std::io::Write;

pub fn run(args: &[String]) -> Result<()> {
    let input = args.first().context("queries file")?;
    let output = args.get(1).context("output file")?;
    let source = args.iter().position(|a| a == "--source").and_then(|i| args.get(i + 1)).map(String::as_str).unwrap_or("pawn");
    let mut c = crate::connect()?;
    let src = match source {
        "none" => 0,
        _ => c.status_value("pawn")?.and_then(|v| crate::parse_u32(&v).ok()).unwrap_or(0),
    };
    let text = std::fs::read_to_string(input)?;
    let mut ids = Vec::new();
    let mut q = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 11 {
            continue;
        }
        let v: Vec<f32> = f[1..10].iter().map(|s| s.parse().unwrap_or(0.0)).collect();
        ids.push(f[0].to_string());
        q.push(LineQuery {
            start: [v[0], v[1], v[2]],
            end: [v[3], v[4], v[5]],
            extent: [v[6], v[7], v[8]],
            flags: crate::parse_u32(f[10])?,
            source: src,
        });
    }
    let t0 = std::time::Instant::now();
    let r = c.line_checks(&q)?;
    eprintln!("{} checks in {:.1} s", r.len(), t0.elapsed().as_secs_f32());
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut name = |c: &mut dishonored_probe::Client, a: u32| -> String {
        if a == 0 {
            return "-".into();
        }
        names
            .entry(a)
            .or_insert_with(|| c.name_of(a).unwrap_or_else(|_| format!("{a:#x}")).replace(' ', "/"))
            .clone()
    };
    let mut out = std::fs::File::create(output)?;
    writeln!(out, "# id hit time lx ly lz nx ny nz actor component")?;
    for (id, x) in ids.iter().zip(&r) {
        let (a, k) = if x.hit { (name(&mut c, x.actor), name(&mut c, x.component)) } else { ("-".into(), "-".into()) };
        writeln!(
            out,
            "{id} {} {:.5} {:.2} {:.2} {:.2} {:.4} {:.4} {:.4} {a} {k}",
            x.hit as u8, x.time, x.location[0], x.location[1], x.location[2], x.normal[0], x.normal[1], x.normal[2]
        )?;
    }
    eprintln!("wrote {output}");
    Ok(())
}
