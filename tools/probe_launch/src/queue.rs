//! `probe_launch queue run [FILE] [--out RESULTS.json] [--dir SCENARIOS] [--only PREFIX]`:
//! the original-side capture queue (`fixtures/oracle/capture_queue.txt`): one scenario name per
//! line (captured with `probe_launch scenario` semantics: fresh map, strict setup, trace into the
//! oracle cache) or `session <op> key=value...` (one session op, its JSON answer kept). Lines are
//! run in file order in the running session; `#` lines are comments. Scenarios on a layout map
//! (the Playground) are run by building the layout in the host level (`--skip-layouts`: listed
//! as skipped instead). The results file maps every line to ok / failed / skipped with the
//! reason; the summary is printed as JSON.
//!
//! `collide map=M [queries=FILE] [results=FILE] [generator=G]`: the map's collision query set
//! (default `<cache>/oracle/queries/M.txt`) run in the session (which must be on M), the
//! answers written next to the results file (`collision/M.results.txt`) and imported into the
//! oracle cache as `oracle import collision` does. The generator is `harness-collision-v1`,
//! or `primitive-directed-v1(reach 64)` when the ids are `N:prim:..`.
use crate::session::{self, Session};
use anyhow::{Context, Result};
use dishonored_oracle::scenario;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Instant;

pub fn run(args: &[String]) -> Result<()> {
    if args.first().map(String::as_str) != Some("run") {
        anyhow::bail!("probe_launch queue run [FILE] [--out RESULTS.json] [--dir SCENARIOS] [--skip-layouts] [--only PREFIX] [--from LINE]");
    }
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let file = args.get(1).filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "fixtures/oracle/capture_queue.txt".into());
    let out = PathBuf::from(opt("--out").unwrap_or_else(|| "harness_out/oracle/capture_queue_results.json".into()));
    let dir = PathBuf::from(opt("--dir").unwrap_or_else(|| "fixtures/oracle/scenarios".into()));
    let skip_layouts = args.iter().any(|a| a == "--skip-layouts");
    let only = opt("--only");
    let from: usize = opt("--from").and_then(|v| v.parse().ok()).unwrap_or(1);
    let text = std::fs::read_to_string(&file).with_context(|| file.clone())?;
    let specs = scenario::load_dir(&dir)?;
    let mut s = Session::connect()?;
    let t0 = Instant::now();
    let mut results = Vec::new();
    let (mut ok, mut failed, mut skipped) = (0, 0, 0);
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || n + 1 < from {
            continue;
        }
        if only.as_ref().is_some_and(|p| !line.starts_with(p.as_str())) {
            continue;
        }
        let t = Instant::now();
        let r: Result<(String, Value)> = if let Some(rest) = line.strip_prefix("session ") {
            let mut it = rest.split_whitespace();
            let op = it.next().unwrap_or("").to_string();
            let a: Vec<String> = it.map(str::to_string).collect();
            session::parse_args(&a).and_then(|a| s.handle(&op, &a)).map(|v| ("ok".to_string(), v))
        } else if let Some(rest) = line.strip_prefix("collide ") {
            let a: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
            session::parse_args(&a).and_then(|a| collide(&mut s, &a, &out)).map(|v| ("ok".to_string(), v))
        } else {
            match specs.iter().find(|(_, sp)| sp.name == line) {
                None => Err(anyhow::anyhow!("no scenario {line} in {}", dir.display())),
                Some((_, sp)) if skip_layouts && crate::playground::layout_file(&sp.map).is_some() => Ok(("skipped".to_string(), json!({"reason": format!("map {} is a layout map (--skip-layouts)", sp.map)}))),
                Some((_, sp)) => crate::capture::capture(&mut s, sp, true, false, &[]).map(|(summary, _)| ("ok".to_string(), summary)),
            }
        };
        let entry = match r {
            Ok((status, v)) => {
                match status.as_str() {
                    "ok" => ok += 1,
                    _ => skipped += 1,
                }
                json!({"line": n + 1, "text": line, "status": status, "result": v, "seconds": t.elapsed().as_secs_f64()})
            }
            Err(e) => {
                failed += 1;
                // Leave the session usable for the next line.
                let _ = s.c.lockstep(false, 1000);
                json!({"line": n + 1, "text": line, "status": "failed", "reason": format!("{e:#}"), "seconds": t.elapsed().as_secs_f64()})
            }
        };
        eprintln!("{} {} ({:.1} s){}", entry["status"].as_str().unwrap_or(""), line, t.elapsed().as_secs_f64(), entry.get("reason").and_then(Value::as_str).map(|r| format!(": {r}")).unwrap_or_default());
        results.push(entry);
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).ok();
        }
        std::fs::write(&out, serde_json::to_string_pretty(&results)?)?;
    }
    println!("{}", serde_json::to_string(&json!({"ok": ok, "failed": failed, "skipped": skipped, "results": out.display().to_string(), "seconds": t0.elapsed().as_secs_f64()}))?);
    Ok(())
}

fn collide(s: &mut Session, a: &Value, out: &std::path::Path) -> Result<Value> {
    use dishonored_oracle::cache::{self, OracleKey, Probe, Provenance};
    use dishonored_oracle::query;
    let map = a.get("map").and_then(Value::as_str).context("map=M")?.to_string();
    let on = s.status()?.get("map").and_then(Value::as_str).unwrap_or("").to_string();
    anyhow::ensure!(on.eq_ignore_ascii_case(&map), "the session is on {on}, not {map} (open it first)");
    let root = cache::root();
    let qf = a.get("queries").and_then(Value::as_str).map(PathBuf::from).unwrap_or_else(|| root.join("queries").join(format!("{map}.txt")));
    let rf = a.get("results").and_then(Value::as_str).map(PathBuf::from).unwrap_or_else(|| out.parent().unwrap_or(std::path::Path::new(".")).join("collision").join(format!("{map}.results.txt")));
    if let Some(p) = rf.parent() {
        std::fs::create_dir_all(p).ok();
    }
    let qtext = std::fs::read_to_string(&qf).with_context(|| qf.display().to_string())?;
    let generator = match a.get("generator").and_then(Value::as_str) {
        Some(g) => g.to_string(),
        None if qtext.lines().find(|l| !l.starts_with('#')).is_some_and(|l| l.split_whitespace().next().is_some_and(|id| id.contains(":prim:"))) => "primitive-directed-v1(reach 64)".into(),
        None => "harness-collision-v1".into(),
    };
    let (n, secs) = crate::oracle_collision::run_on(&mut s.c, &qf.to_string_lossy(), &rf.to_string_lossy(), "pawn")?;
    let set = query::parse_probe_text(&generator, &qtext, &std::fs::read_to_string(&rf)?)?;
    let exe = cache::exe_path(&dishonored_oracle::install_dir());
    let key = OracleKey { exe_sha256: cache::file_sha256(&root, &exe)?, subject: map.clone(), probe: Probe::Collision, query_hash: set.query_hash() };
    let prov = Provenance { captured_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0), source: format!("probe_launch queue collide; {} + {}", qf.display(), rf.display()), capture_seconds: secs };
    let p = cache::store(&root, &key, &(prov, &set))?;
    Ok(json!({"map": map, "generator": generator, "queries": n, "answered": set.answered(), "names": set.names.len(), "doors": set.doors.len(), "capture_s": secs, "results": rf.display().to_string(), "cache": p.display().to_string()}))
}