//! `oracle`: the differential oracle (research/dishonored/ORACLE.md).
//!
//! ```text
//! oracle coverage [--dlc] [--no-cache] [--map M]... [--out DIR] [--repo DIR]
//! oracle import collision --map M --queries Q.txt --results R.txt [--exe PATH | --exe-sha HEX]
//!                         [--generator G] [--capture-seconds S]
//! oracle snapshot --map M...                 build the port snapshot (needs the `import` feature)
//! oracle collision [--map M]... [--ids 1,2,3] [--out DIR] [--examples N]
//! oracle gen collision --map M --out Q.txt [--sampling navmesh,prim] [--reach R] [--profiles enumerated]
//!                                            query batch for `probe_launch collide`
//! oracle densify collision --map M --out Q.txt [--per N] [--radius R]   queries around disagreements
//! oracle structure --map M                   port collision structure (vs a cached reflection dump)
//! oracle scenario [NAME...] [--system S]... [--all] [--jobs N] [--json] [--dir DIR] [--out DIR]
//! oracle sweep [--out DIR] [--no-coverage]   every cached map and scenario -> scorecard + work list
//! oracle bench import|queries --map M        measure the port's costs
//! oracle cache list
//! oracle index build|fn|class|uscript|xref|search|natives|traces|field|vcall|stats ...
//!                                            the static index (`oracle index help`)
//! ```
//! Outputs go to `harness_out/oracle/<command>/` (gitignored); cached answers and port
//! snapshots to the oracle cache (`$DISHONORED_CACHE/oracle`).

mod querygen;
mod port;
#[cfg(feature = "scenarios")]
mod present;
#[cfg(feature = "scenarios")]
mod runner;
#[cfg(feature = "scenarios")]
mod scen;

use anyhow::{Context, Result, bail};
use dishonored_oracle::cache::{self, OracleKey, Probe, Provenance};
use dishonored_oracle::coverage::{self, scan, source::PortRegistry};
use dishonored_oracle::diff;
use dishonored_oracle::query::{self, CollisionSet};
use dishonored_oracle::report::{self, Scorecard, Table, WorkItem};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Args(Vec<String>);

impl Args {
    fn opt(&self, f: &str) -> Option<String> {
        self.0.iter().position(|a| a == f).and_then(|i| self.0.get(i + 1)).cloned()
    }
    fn all(&self, f: &str) -> Vec<String> {
        self.0.windows(2).filter(|w| w[0] == f).map(|w| w[1].clone()).collect()
    }
    fn flag(&self, f: &str) -> bool {
        self.0.iter().any(|a| a == f)
    }
    /// Positional arguments after the subcommand (not options or their values).
    #[cfg_attr(not(feature = "scenarios"), allow(dead_code))]
    fn free(&self, skip: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = skip;
        while i < self.0.len() {
            let a = &self.0[i];
            if a.starts_with("--") {
                if !matches!(a.as_str(), "--dlc" | "--no-cache" | "--no-coverage" | "--rebuild" | "--all" | "--json") {
                    i += 1;
                }
            } else {
                out.push(a.clone());
            }
            i += 1;
        }
        out
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Classify the residuals with the cause book (`fixtures/oracle/causes.ron`), add the
/// residual table (definition of done) and write the scorecard, work list and dossiers.
fn write_report(dir: &Path, card: &mut Scorecard, items: &mut [WorkItem], limit: usize) -> Result<()> {
    let book = dishonored_oracle::residual::CauseBook::load(&repo_root().join("fixtures").join("oracle").join("causes.ron"))?;
    book.classify(items);
    dishonored_oracle::index::annotate(items);
    let summary = dishonored_oracle::residual::ResidualSummary::of(items);
    card.tables.insert(0, summary.table());
    report::write_all(dir, card, items, limit)?;
    Ok(())
}

fn out_dir(a: &Args, cmd: &str) -> PathBuf {
    a.opt("--out").map(PathBuf::from).unwrap_or_else(|| repo_root().join("harness_out").join("oracle").join(cmd))
}

fn exe_sha(a: &Args, root: &Path) -> Result<[u8; 32]> {
    if let Some(h) = a.opt("--exe-sha") {
        return cache::unhex32(&h).context("--exe-sha: 64 hex digits");
    }
    let exe = a.opt("--exe").map(PathBuf::from).unwrap_or_else(|| cache::exe_path(&dishonored_oracle::install_dir()));
    cache::file_sha256(root, &exe)
}

fn main() -> Result<()> {
    let a = Args(std::env::args().skip(1).collect());
    let cmd = a.0.first().map(String::as_str).unwrap_or("");
    match cmd {
        "coverage" => cmd_coverage(&a),
        "import" => cmd_import(&a),
        "snapshot" => cmd_snapshot(&a),
        "collision" => cmd_collision(&a).map(|_| ()),
        "gen" => cmd_gen(&a),
        "densify" => cmd_densify(&a),
        "structure" => cmd_structure(&a),
        "scenario" => cmd_scenario(&a).map(|_| ()),
        "sweep" => cmd_sweep(&a),
        "bench" => cmd_bench(&a),
        "cache" => cmd_cache(&a),
        "index" => dishonored_oracle::index::cli::run(&a.0[1..]),
        _ => {
            eprintln!("{}", include_str!("main.rs").lines().skip(2).take(18).map(|l| l.trim_start_matches("//! ")).collect::<Vec<_>>().join("\n"));
            bail!("unknown command `{cmd}`")
        }
    }
}

// ----- coverage -----

#[derive(Serialize, Deserialize)]
struct CoverageOut {
    run: coverage::CoverageRun,
}

fn run_coverage(a: &Args) -> Result<(coverage::CoverageRun, Vec<WorkItem>)> {
    let install = dishonored_oracle::install_dir();
    let t0 = Instant::now();
    let files = scan::list_packages(&install, a.flag("--dlc"))?;
    let cache_dir = cache::root().join("coverage");
    let (scans, hits, errors) = coverage::scan_all(&files, if a.flag("--no-cache") { None } else { Some(&cache_dir) });
    for e in &errors {
        eprintln!("scan error: {e}");
    }
    let scan_s = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let repo = a.opt("--repo").map(PathBuf::from).unwrap_or_else(repo_root);
    let reg = PortRegistry::scan(&repo).with_context(|| format!("scanning {}", repo.display()))?;
    let reg_s = t1.elapsed().as_secs_f64();
    let t2 = Instant::now();
    let res = coverage::coverage(&scans, &reg, 100_000);
    let (maps, all, script_all, nclasses) = (res.maps, res.all, res.script_all, res.classes_known);
    let agg_s = t2.elapsed().as_secs_f64();
    eprintln!("coverage: {} packages ({hits} from cache) scanned in {scan_s:.2} s, port source {} files in {reg_s:.2} s, aggregated in {agg_s:.2} s", scans.len(), reg.files.len());
    let mut examples: HashMap<String, Vec<String>> = HashMap::new();
    for s in &scans {
        for (c, ex) in &s.examples {
            let e = examples.entry(c.clone()).or_default();
            for x in ex {
                if e.len() < 3 {
                    e.push(format!("{}:{x}", s.name));
                }
            }
        }
    }
    let wanted = a.all("--map");
    let maps: Vec<coverage::MapCoverage> = if wanted.is_empty() { maps } else { maps.into_iter().filter(|m| wanted.iter().any(|w| w.eq_ignore_ascii_case(&m.map))).collect() };
    let mut notes: HashMap<String, String> = HashMap::new();
    for c in &res.classes {
        for (what, h) in [("port", Some(&c.handling)), ("Kismet", c.kismet.as_ref())] {
            if let Some(coverage::source::Handling::Acknowledged { list, at }) = h {
                notes.insert(c.class.clone(), format!("the port lists it as a no-op ({what}: `{list}` at {at}): implementing it means replacing the no-op with the original's behaviour"));
            }
        }
    }
    let items = coverage::work_items(&maps, &examples, &notes);
    let run = coverage::CoverageRun {
        packages: scans.len(),
        packages_from_cache: hits,
        parse_failures: scans.iter().map(|s| s.parse_failures).sum(),
        scan_seconds: scan_s,
        registry_files: reg.files.len(),
        registry_lines: reg.lines,
        classes_known: nclasses,
        maps,
        all,
        script_all,
    };
    // The evidence behind every rate: how each class used by the install is handled.
    let dir = out_dir(a, "coverage");
    std::fs::create_dir_all(&dir).ok();
    let mut s = String::from("class\tobjects\thandling\tat\tkismet\n");
    let fmt = |h: &coverage::source::Handling| match h {
        coverage::source::Handling::Direct(at) => ("direct".to_string(), at.clone()),
        coverage::source::Handling::Inherited { ancestor, at } => (format!("inherited({ancestor})"), at.clone()),
        coverage::source::Handling::Pattern { pattern, at } => (format!("pattern {pattern}"), at.clone()),
        coverage::source::Handling::Acknowledged { list, at } => (format!("acknowledged({list})"), at.clone()),
        coverage::source::Handling::None => ("none".into(), String::new()),
    };
    for c in &res.classes {
        let (h, at) = fmt(&c.handling);
        let k = c.kismet.as_ref().map(|k| { let (t, at) = fmt(k); format!("{t} {at}") }).unwrap_or_default();
        s.push_str(&format!("{}\t{}\t{h}\t{at}\t{k}\n", c.class, c.objects));
    }
    std::fs::write(dir.join("classes.tsv"), s).ok();
    Ok((run, items))
}

fn coverage_card(run: &coverage::CoverageRun) -> Scorecard {
    let mut card = Scorecard { title: "Coverage: what the original's packages use vs what the port handles".into(), ..Default::default() };
    card.summary.push(("packages".into(), format!("{} ({} from the scan cache), {} parse failures", run.packages, run.packages_from_cache, run.parse_failures)));
    card.summary.push(("scan time".into(), format!("{:.2} s", run.scan_seconds)));
    card.summary.push(("port source scanned".into(), format!("{} files, {} lines", run.registry_files, run.registry_lines)));
    card.summary.push(("classes defined by the install".into(), run.classes_known.to_string()));
    let s = &run.script_all;
    card.summary.push((
        "UnrealScript (all classes)".into(),
        format!("{} functions: {} native, {} script ({} with bytecode, {} KiB)", s.native + s.script, s.native, s.script, s.script_with_code, s.script_bytes / 1024),
    ));
    let mut t = Table::new("Per map", &["map", "pkgs", "placed actors", "actor classes", "Kismet ops", "op classes", "props read", "mat. expr.", "functions native/script(code)", "overall"]);
    let row = |m: &coverage::MapCoverage| {
        vec![
            m.map.clone(),
            m.packages.len().to_string(),
            m.actors.cell(),
            m.actor_classes.cell(),
            m.kismet_ops.cell(),
            m.kismet_op_classes.cell(),
            m.props.cell(),
            m.mat_exprs.cell(),
            format!("{}/{}({})", m.functions.native, m.functions.script, m.functions.script_with_code),
            format!("{:.1} %", 100.0 * m.overall),
        ]
    };
    for m in &run.maps {
        t.row(row(m));
    }
    t.row(row(&run.all));
    card.tables.push(t);
    let mut tt = Table::new("Top unhandled placed-actor classes (all packages)", &["class", "instances"]);
    for (c, n) in run.all.unhandled_actor_classes.iter().take(40) {
        tt.row(vec![c.clone(), n.to_string()]);
    }
    card.tables.push(tt);
    let mut tk = Table::new("Top unimplemented Kismet ops (all packages)", &["class", "instances"]);
    for (c, n) in run.all.unimplemented_ops.iter().take(40) {
        tk.row(vec![c.clone(), n.to_string()]);
    }
    card.tables.push(tk);
    let mut tp = Table::new("Top ignored properties on handled classes (all packages)", &["class", "property", "objects"]);
    for (c, p, n) in run.all.ignored_props.iter().take(40) {
        tp.row(vec![c.clone(), p.clone(), n.to_string()]);
    }
    card.tables.push(tp);
    let mut by_name: HashMap<&str, (u64, u32)> = HashMap::new();
    for (_, p, n) in &run.all.ignored_props {
        let e = by_name.entry(p.as_str()).or_default();
        e.0 += *n as u64;
        e.1 += 1;
    }
    let mut v: Vec<(&str, (u64, u32))> = by_name.into_iter().collect();
    v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
    let mut tn = Table::new("Top ignored property names (all packages, over every handled class)", &["property", "objects", "classes"]);
    for (p, (n, c)) in v.iter().take(150) {
        tn.row(vec![p.to_string(), n.to_string(), c.to_string()]);
    }
    card.tables.push(tn);
    let mut tm = Table::new("Material features (all packages)", &["feature", "materials", "named by the port"]);
    for (f, n, ok) in &run.all.material_features {
        tm.row(vec![f.clone(), n.to_string(), if *ok { "yes".into() } else { "no".into() }]);
    }
    card.tables.push(tm);
    card.notes.push("Rules: research/dishonored/ORACLE.md \"Coverage scan\". Handled = named by the port's source (direct literal, `is_a` of an ancestor, or a name pattern); Kismet ops count only in the interpreter and the Kismet host. Overall = mean of the placed-actor, Kismet-op and property rates (instance weighted). Static coverage says what the port looks at, not that it behaves like the original: the query and behaviour oracles measure that.".into());
    card
}

fn cmd_coverage(a: &Args) -> Result<()> {
    let t0 = Instant::now();
    let (run, mut items) = run_coverage(a)?;
    let mut card = coverage_card(&run);
    let dir = out_dir(a, "coverage");
    write_report(&dir, &mut card, &mut items, 200)?;
    std::fs::write(dir.join("coverage.json"), serde_json::to_string_pretty(&CoverageOut { run: run.clone() })?)?;
    println!("{:<34} {:>18} {:>18} {:>20} {:>8}", "map", "actors", "Kismet ops", "props read", "overall");
    for m in run.maps.iter().chain(std::iter::once(&run.all)) {
        println!("{:<34} {:>18} {:>18} {:>20} {:>7.1}%", m.map, m.actors.cell(), m.kismet_ops.cell(), m.props.cell(), 100.0 * m.overall);
    }
    println!("{} work items; report: {}", items.len(), dir.join("SCORECARD.md").display());
    println!("total {:.2} s", t0.elapsed().as_secs_f64());
    Ok(())
}

// ----- import / snapshot / gen -----

fn cmd_import(a: &Args) -> Result<()> {
    if a.0.get(1).map(String::as_str) != Some("collision") {
        bail!("oracle import collision --map M --queries Q --results R");
    }
    let root = cache::root();
    let map = a.opt("--map").context("--map")?;
    let qf = a.opt("--queries").context("--queries")?;
    let rf = a.opt("--results").context("--results")?;
    let generator = a.opt("--generator").unwrap_or_else(|| querygen::GENERATOR.into());
    let t = Instant::now();
    let set = query::parse_probe_text(&generator, &std::fs::read_to_string(&qf)?, &std::fs::read_to_string(&rf)?)?;
    let key = OracleKey { exe_sha256: exe_sha(a, &root)?, subject: map.clone(), probe: Probe::Collision, query_hash: set.query_hash() };
    let prov = Provenance {
        captured_unix: std::fs::metadata(&rf).and_then(|m| m.modified()).ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0),
        source: format!("probe_launch collide; imported from {qf} + {rf}"),
        capture_seconds: a.opt("--capture-seconds").and_then(|s| s.parse().ok()).unwrap_or(0.0),
    };
    let p = cache::store(&root, &key, &(prov, &set))?;
    println!("{map}: {} queries, {} answered, {} names, {} doors -> {} ({:.2} s)", set.queries.len(), set.answered(), set.names.len(), set.doors.len(), p.display(), t.elapsed().as_secs_f64());
    Ok(())
}

fn maps_arg(a: &Args) -> Vec<String> {
    a.all("--map")
}

fn cmd_snapshot(a: &Args) -> Result<()> {
    let root = cache::root();
    for map in maps_arg(a) {
        let t = Instant::now();
        let s = port::build_snapshot(&map)?;
        let built = t.elapsed().as_secs_f64();
        let p = port::save_snapshot(&root, &s)?;
        println!("{map}: {} triangles, {} movers, nav {} polys; built in {built:.2} s -> {}", s.collision.tris.len(), s.collision.movers.len(), s.nav.as_ref().map_or(0, |n| n.polys.len()), p.display());
    }
    Ok(())
}

/// The player's collision cylinder (radius, half height) from the gameplay config JSON.
fn pawn_extent(json: &str) -> Result<(f32, f32)> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let mv = &v["movement"];
    let f = |k: &str| mv[k].as_f64().map(|x| x as f32).with_context(|| format!("gameplay config: movement.{k}"));
    Ok((f("collision_radius")?, f("collision_half_height")?))
}

fn cmd_gen(a: &Args) -> Result<()> {
    if a.0.get(1).map(String::as_str) != Some("collision") {
        bail!("oracle gen collision --map M --out FILE");
    }
    let map = a.opt("--map").context("--map")?;
    let out = a.opt("--out").context("--out")?;
    let (w, json) = port::snapshot(&cache::root(), &map, false)?.world();
    let pawn = pawn_extent(&json)?;
    let sampling = a.opt("--sampling").unwrap_or_else(|| "navmesh".into());
    let mut q = Vec::new();
    let mut generator = Vec::new();
    for s in sampling.split(',') {
        match s {
            "navmesh" => {
                let nav = w.nav.as_ref().context("map without navigation mesh")?;
                q.extend(querygen::generate(nav, pawn));
                generator.push(querygen::GENERATOR.to_string());
            }
            "prim" => {
                // Aimed at the port's primitives until the hook's structural dump lists the
                // original's (phase 2): every collision source, every measured profile.
                let reach: f32 = a.opt("--reach").and_then(|s| s.parse().ok()).unwrap_or(64.0);
                let prims = querygen::port_primitives(&w);
                // `--profiles enumerated`: every trace profile of the exe's call sites (the static index).
                let enumerated = a.opt("--profiles").as_deref() == Some("enumerated");
                let profiles = if enumerated { dishonored_oracle::index::enumerated_profiles(Some([pawn.0, pawn.0, pawn.1]))? } else { querygen::measured_profiles(pawn) };
                q.extend(dishonored_oracle::sampling::primitive_directed(&prims, &profiles, reach));
                generator.push(format!("{}-v1(reach {reach}{})", querygen::PRIM_GENERATOR, if enumerated { ", enumerated profiles" } else { "" }));
            }
            other => bail!("unknown sampling `{other}` (navmesh, prim)"),
        }
    }
    let generator = generator.join("+");
    std::fs::write(&out, query::queries_text(&map, &q))?;
    println!("{} queries ({generator}) -> {out}; hash {:016x}; import the capture with --generator \"{generator}\"", q.len(), query::query_set_hash(&generator, &q));
    Ok(())
}

/// `oracle densify collision --map M --out FILE [--per N] [--radius R] [--seed S]`: queries
/// around every disagreement of the cached set (adaptive densification, next capture).
fn cmd_densify(a: &Args) -> Result<()> {
    let root = cache::root();
    let map = a.opt("--map").context("--map")?;
    let out = a.opt("--out").context("--out")?;
    let per = a.opt("--per").and_then(|s| s.parse().ok()).unwrap_or(8);
    let radius = a.opt("--radius").and_then(|s| s.parse().ok()).unwrap_or(16.0);
    let seed = a.opt("--seed").and_then(|s| s.parse().ok()).unwrap_or(1);
    let exe = exe_sha(a, &root)?;
    let (_, path) = cached_sets(&exe).into_iter().find(|(k, _)| k.subject.eq_ignore_ascii_case(&map)).context("no cached collision set for the map")?;
    let (_, set): Stored = cache::decode(&std::fs::read(path)?)?.1;
    let (w, _) = port::snapshot(&root, &map, false)?.world();
    let port = port::collision_answers(&w, &port::movers(&w, &set.doors), &set, None);
    let d = diff::compare_collision(&set, &port);
    let ids: Vec<u32> = d.disagreements.iter().map(|x| x.id).collect();
    let q = dishonored_oracle::sampling::densify(&set.queries, &ids, per, radius, seed);
    let generator = format!("densify-v1({}, per {per}, radius {radius}, seed {seed})", set.generator);
    std::fs::write(&out, query::queries_text(&map, &q))?;
    println!("{} queries around {} disagreements -> {out}; import with --generator \"{generator}\"", q.len(), ids.len());
    Ok(())
}

/// `oracle structure --map M`: the port's collision structure (per collision source: trace
/// channels, triangle count, bounds) against the original's reflection dump when one is
/// cached (`Probe::State`, phase 2); without it the port side is written and the diff is
/// reported as pending.
fn cmd_structure(a: &Args) -> Result<()> {
    use dishonored_oracle::structure::{FieldValue, ObjectRecord, StructureDump};
    let root = cache::root();
    let map = a.opt("--map").context("--map")?;
    let (w, _) = port::snapshot(&root, &map, false)?.world();
    let mut port = StructureDump { system: "collision".into(), side: "port".into(), ..Default::default() };
    let mut acc: Vec<(u8, u64, [f32; 3], [f32; 3])> = vec![(0, 0, [f32::MAX; 3], [f32::MIN; 3]); w.sources.len()];
    for t in &w.tris {
        let Some(e) = acc.get_mut(t.source as usize) else { continue };
        e.0 |= t.channels;
        e.1 += 1;
        for p in [t.a, t.b, t.c] {
            for k in 0..3 {
                e.2[k] = e.2[k].min(p[k]);
                e.3[k] = e.3[k].max(p[k]);
            }
        }
    }
    for (name, (ch, n, lo, hi)) in w.sources.iter().zip(&acc) {
        if *n == 0 {
            continue;
        }
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("PortChannels".into(), FieldValue::Int(*ch as i64));
        fields.insert("PortTriangles".into(), FieldValue::Int(*n as i64));
        fields.insert("BoundsMin".into(), FieldValue::Floats(lo.iter().map(|x| *x as f64).collect()));
        fields.insert("BoundsMax".into(), FieldValue::Floats(hi.iter().map(|x| *x as f64).collect()));
        port.objects.insert(name.clone(), ObjectRecord { class: "CollisionSource".into(), fields });
    }
    let dir = out_dir(a, "structure");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{map}.port.json")), serde_json::to_string(&port)?)?;
    let exe = exe_sha(a, &root)?;
    let orig = cache::list(&root, Some(&exe)).into_iter().find(|(k, _)| k.probe == Probe::State && k.subject.eq_ignore_ascii_case(&map));
    match orig {
        None => println!("{map}: port structure of {} collision sources -> {}; no original reflection dump cached yet (phase 2): structural diff pending", port.objects.len(), dir.display()),
        Some((_, p)) => {
            let (_, (_, o)): (OracleKey, (Provenance, StructureDump)) = cache::decode(&std::fs::read(p)?)?;
            let d = dishonored_oracle::structure::diff(&o, &port, &|_| 0.01);
            let mut items = dishonored_oracle::structure::work_items(&map, &o, &d, 10);
            let mut card = Scorecard { title: format!("Structure: {map}"), tables: vec![dishonored_oracle::structure::table(&map, &d)], ..Default::default() };
            write_report(&dir, &mut card, &mut items, 100)?;
            println!("{map}: {} of {} fields equal; report {}", d.fields_equal, d.fields_compared, dir.display());
        }
    }
    Ok(())
}

// ----- collision -----

type Stored = (Provenance, CollisionSet);

/// Cached collision sets for this exe: map -> (key, path), the newest file per map.
fn cached_sets(exe: &[u8; 32]) -> Vec<(OracleKey, PathBuf)> {
    let mut best: HashMap<String, (OracleKey, PathBuf, std::time::SystemTime)> = HashMap::new();
    for (k, p) in cache::list(&cache::root(), Some(exe)) {
        if k.probe != Probe::Collision {
            continue;
        }
        let m = std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        if best.get(&k.subject).is_none_or(|b| m > b.2) {
            best.insert(k.subject.clone(), (k, p, m));
        }
    }
    let mut v: Vec<(OracleKey, PathBuf)> = best.into_values().map(|x| (x.0, x.1)).collect();
    v.sort_by(|a, b| a.0.subject.cmp(&b.0.subject));
    v
}

struct CollisionRun {
    card: Scorecard,
    items: Vec<WorkItem>,
}

fn cmd_collision(a: &Args) -> Result<CollisionRun> {
    let t_all = Instant::now();
    let root = cache::root();
    let exe = exe_sha(a, &root)?;
    let want = maps_arg(a);
    let sets: Vec<(OracleKey, PathBuf)> = cached_sets(&exe).into_iter().filter(|(k, _)| want.is_empty() || want.iter().any(|w| w.eq_ignore_ascii_case(&k.subject))).collect();
    if sets.is_empty() {
        bail!("no cached collision answers for exe {} (import a capture with `oracle import collision`)", &cache::hex(&exe)[..16]);
    }
    let ids: Option<Vec<u32>> = a.opt("--ids").map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect());
    let examples = a.opt("--examples").and_then(|s| s.parse().ok()).unwrap_or(8);
    let mut card = Scorecard { title: "Collision: port vs the original's UWorld::SingleLineCheck".into(), ..Default::default() };
    card.summary.push(("exe".into(), cache::hex(&exe)));
    let mut items = Vec::new();
    let mut summary = Table::new("Per map", &["map", "queries", "compared", "agree", "rate", "excluded (pawn hits)", "clusters", "load s", "query s"]);
    for (key, path) in &sets {
        let map = &key.subject;
        let t = Instant::now();
        let (prov, set): Stored = {
            let bytes = std::fs::read(path)?;
            cache::decode(&bytes)?.1
        };
        let t_oracle = t.elapsed().as_secs_f64();
        let t = Instant::now();
        // [streaming] `--stream L1,L2`: these Kismet-streamed sublevels visible (default none, as on a map opened from the console).
        let stream: Vec<String> = a.opt("--stream").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
        let (w, _) = port::snapshot(&root, map, a.flag("--rebuild"))?.world_with(&stream);
        let mv = port::movers(&w, &set.doors);
        let t_load = t.elapsed().as_secs_f64() + t_oracle;
        let t = Instant::now();
        let mut set = set;
        if let Some(ids) = &ids {
            for (i, ans) in set.answers.iter_mut().enumerate() {
                if !ids.contains(&(i as u32)) {
                    *ans = None;
                }
            }
        }
        let port = port::collision_answers(&w, &mv, &set, ids.as_deref());
        let t_query = t.elapsed().as_secs_f64();
        let d = diff::compare_collision(&set, &port);
        println!("{map}: {} / {} agree ({:.2} %), {} clusters; load {t_load:.2} s, {} queries in {t_query:.2} s", d.agree, d.total, 100.0 * d.rate(), d.clusters.len(), set.queries.len());
        summary.row(vec![
            map.clone(),
            set.queries.len().to_string(),
            d.total.to_string(),
            d.agree.to_string(),
            format!("{:.2} %", 100.0 * d.rate()),
            d.excluded.to_string(),
            d.clusters.len().to_string(),
            format!("{t_load:.2}"),
            format!("{t_query:.2}"),
        ]);
        card.summary.push((format!("{map} oracle"), format!("{} ({}), query hash {:016x}", prov.source, set.generator, key.query_hash)));
        card.tables.extend(diff::collision_tables(map, &d));
        if let Some(ids) = &ids {
            for x in d.disagreements.iter().filter(|x| ids.contains(&x.id)) {
                println!("  {} {} {:?}: original {} {:.4} `{}`, port {:?} `{}`", x.id, set.queries[x.id as usize].tag, x.direction, x.original, x.orig_time, x.component, x.port_time, x.port);
            }
        }
        items.extend(diff::collision_work_items(map, &set, &d, examples));
    }
    card.tables.insert(0, summary);
    card.summary.push(("total time".into(), format!("{:.2} s", t_all.elapsed().as_secs_f64())));
    items.sort_by(|x, y| y.impact.total_cmp(&x.impact).then(x.id.cmp(&y.id)));
    if a.0.first().map(String::as_str) == Some("collision") {
        let dir = out_dir(a, "collision");
        write_report(&dir, &mut card, &mut items, 100)?;
        println!("{} work items; report: {} ({:.2} s)", items.len(), dir.join("SCORECARD.md").display(), t_all.elapsed().as_secs_f64());
    }
    Ok(CollisionRun { card, items })
}

// ----- scenarios -----

#[cfg_attr(not(feature = "scenarios"), allow(dead_code))]
fn scenario_dir(a: &Args) -> PathBuf {
    a.opt("--dir").map(PathBuf::from).unwrap_or_else(|| repo_root().join("fixtures").join("oracle").join("scenarios"))
}

#[cfg(not(feature = "scenarios"))]
fn cmd_scenario(_a: &Args) -> Result<(Scorecard, Vec<WorkItem>)> {
    bail!("this build has no scenario runner: build with the `scenarios` feature")
}

#[cfg(feature = "scenarios")]
fn cmd_scenario(a: &Args) -> Result<(Scorecard, Vec<WorkItem>)> {
    let root = cache::root();
    let names = if a.0.first().map(String::as_str) == Some("scenario") { a.free(1) } else { Vec::new() };
    let all = scen::load(&scenario_dir(a))?;
    let systems = a.all("--system");
    let names = if a.flag("--all") { Vec::new() } else { names };
    let chosen = scen::select(&all, &names, &systems);
    if chosen.is_empty() {
        bail!("no scenario matches {names:?} / --system {systems:?} in {}", scenario_dir(a).display());
    }
    let exe = exe_sha(a, &root).ok();
    let dir = out_dir(a, "scenario");
    std::fs::create_dir_all(&dir)?;
    let jobs = a.opt("--jobs").and_then(|s| s.parse().ok());
    let (res, mut card, mut items) = scen::run(chosen, &dir, exe, jobs)?;
    if a.0.first().map(String::as_str) == Some("scenario") {
        write_report(&dir, &mut card, &mut items, 200)?;
        if a.flag("--json") {
            println!("{}", serde_json::to_string_pretty(&res)?);
        } else {
            // The per-system and per-scenario tables (after the residual table write_report adds).
            for t in card.tables.iter().filter(|t| t.title == "Per system" || t.title == "Scenarios" || t.title.starts_with("Agreement with the original")) {
                print!("{}", t.markdown());
            }
            let t = &res.totals;
            println!(
                "{} scenarios: {} pass, {} fail, {} unmeasured, {} error; checks {} pass / {} fail / {} unmeasured / {} unverified; {:.2} s",
                t.scenarios, t.pass, t.fail, t.unmeasured, t.error, t.checks_pass, t.checks_fail, t.checks_unmeasured, t.checks_unverified, t.seconds
            );
            println!("results: {} (capture requests: {})", dir.join("results.json").display(), dir.join("CAPTURE_REQUESTS.md").display());
        }
    }
    Ok((card, items))
}
// ----- sweep -----

fn cmd_sweep(a: &Args) -> Result<()> {
    let t0 = Instant::now();
    let mut card = Scorecard { title: "Oracle sweep: every cached map and scenario".into(), ..Default::default() };
    let mut items = Vec::new();
    let mut timings = Vec::new();
    if !a.flag("--no-coverage") {
        let t = Instant::now();
        let (run, ci) = run_coverage(a)?;
        let cc = coverage_card(&run);
        card.tables.extend(cc.tables.into_iter().take(1));
        items.extend(ci);
        timings.push(format!("coverage {:.2} s", t.elapsed().as_secs_f64()));
    }
    let t = Instant::now();
    match cmd_collision(a) {
        Ok(r) => {
            card.summary.extend(r.card.summary);
            card.tables.extend(r.card.tables.into_iter().filter(|t| !t.title.contains("minus port")));
            items.extend(r.items);
        }
        Err(e) => card.notes.push(format!("collision: {e:#}")),
    }
    timings.push(format!("collision {:.2} s", t.elapsed().as_secs_f64()));
    let t = Instant::now();
    match cmd_scenario(a) {
        Ok((c, i)) => {
            card.tables.extend(c.tables);
            items.extend(i);
        }
        Err(e) => card.notes.push(format!("scenarios: {e:#}")),
    }
    timings.push(format!("scenarios {:.2} s", t.elapsed().as_secs_f64()));
    items.sort_by(|x, y| y.impact.total_cmp(&x.impact).then(x.id.cmp(&y.id)));
    card.summary.push(("timings".into(), format!("{} (total {:.2} s)", timings.join(", "), t0.elapsed().as_secs_f64())));
    let dir = out_dir(a, "sweep");
    write_report(&dir, &mut card, &mut items, 300)?;
    println!("{} work items; {}; report: {}", items.len(), card.summary.last().map(|x| x.1.clone()).unwrap_or_default(), dir.join("SCORECARD.md").display());
    Ok(())
}

// ----- bench -----

fn cmd_bench(a: &Args) -> Result<()> {
    let what = a.0.get(1).map(String::as_str).unwrap_or("");
    let map = a.opt("--map").unwrap_or_else(|| "L_Boyle_Ext_P".into());
    match what {
        "import" => bench_import(&map),
        "queries" => {
            let root = cache::root();
            let t = Instant::now();
            let (w, _) = port::snapshot(&root, &map, false)?.world();
            let load = t.elapsed().as_secs_f64();
            let exe = exe_sha(a, &root)?;
            let (_, path) = cached_sets(&exe).into_iter().find(|(k, _)| k.subject.eq_ignore_ascii_case(&map)).context("no cached collision set for the map")?;
            let (_, set): Stored = cache::decode(&std::fs::read(path)?)?.1;
            let mv = port::movers(&w, &set.doors);
            let par = Instant::now();
            let _ = port::collision_answers(&w, &mv, &set, None);
            let par = par.elapsed().as_secs_f64();
            let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build()?;
            let one = Instant::now();
            pool.install(|| port::collision_answers(&w, &mv, &set, None));
            let one = one.elapsed().as_secs_f64();
            let n = set.queries.len() as f64;
            let lines = set.queries.iter().filter(|q| !q.is_box()).count();
            println!("{map}: snapshot load {load:.2} s; {} queries ({lines} lines, {} boxes): 1 thread {one:.2} s ({:.0}/s), {} threads {par:.2} s ({:.0}/s)", n, n as usize - lines, n / one, rayon::current_num_threads(), n / par);
            Ok(())
        }
        _ => bail!("oracle bench import|queries --map M"),
    }
}

#[cfg(feature = "import")]
fn bench_import(map: &str) -> Result<()> {
    // Cold: a fresh derived-data cache (the map import, navmesh, gameplay config, NPCs...).
    let tmp = std::env::temp_dir().join(format!("oracle_bench_cache_{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let prev = std::env::var_os("DISHONORED_CACHE");
    // SAFETY: single-threaded at this point; the loader reads the variable.
    unsafe { std::env::set_var("DISHONORED_CACHE", &tmp) };
    let t = Instant::now();
    let mc = dishonored_content::load_match(map)?;
    let cold = t.elapsed().as_secs_f64();
    drop(mc);
    let t = Instant::now();
    let mc = dishonored_content::load_match(map)?;
    let warm = t.elapsed().as_secs_f64();
    let tris = mc.collision.tris.len();
    drop(mc);
    match prev {
        Some(p) => unsafe { std::env::set_var("DISHONORED_CACHE", p) },
        None => unsafe { std::env::remove_var("DISHONORED_CACHE") },
    }
    std::fs::remove_dir_all(&tmp).ok();
    let t = Instant::now();
    let s = port::snapshot(&cache::root(), map, false)?;
    let snap = t.elapsed().as_secs_f64();
    println!("{map}: load_match cold {cold:.2} s, warm {warm:.2} s ({tris} collision triangles); oracle port snapshot load {snap:.2} s ({} tris)", s.collision.tris.len());
    Ok(())
}

#[cfg(not(feature = "import"))]
fn bench_import(_map: &str) -> Result<()> {
    bail!("needs the `import` feature")
}

fn cmd_cache(a: &Args) -> Result<()> {
    if a.0.get(1).map(String::as_str) == Some("show") {
        // `oracle cache show <subject> <out.json>`: a cached original trace as JSON (the same
        // format as the port traces `oracle scenario` writes).
        let (Some(subject), Some(out)) = (a.0.get(2), a.0.get(3)) else { bail!("oracle cache show <subject> <out.json>") };
        let root = cache::root();
        for (k, p) in cache::list(&root, None) {
            if k.probe == Probe::Trace && k.subject == *subject {
                let (_, (_, tr)): (OracleKey, (Provenance, dishonored_oracle::trace::Trace)) = cache::decode(&std::fs::read(&p)?)?;
                std::fs::write(out, serde_json::to_string(&tr)?)?;
                println!("{} ({:016x}) -> {out}", k.subject, k.query_hash);
            }
        }
        return Ok(());
    }
    if a.0.get(1).map(String::as_str) != Some("list") {
        bail!("oracle cache list | oracle cache show <subject> <out.json>");
    }
    let root = cache::root();
    println!("oracle cache: {}", root.display());
    for (k, p) in cache::list(&root, None) {
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        println!("{} {:<28} {:<10} {:016x} {:>10} B", &k.exe_hex()[..16], k.subject, k.probe.tag(), k.query_hash, size);
    }
    Ok(())
}
