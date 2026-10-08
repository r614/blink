//! `oracle scenario`: the scenario library run on the port in parallel, judged against the
//! original's measurements (checkpoints) and, where captured, its traces (first divergence).
//!
//! Outputs (`harness_out/oracle/scenario/`, or `--out`):
//! - `results.json`: per scenario status (`pass` / `fail` / `unmeasured` / `error`), every
//!   check with its port value, reference and status, the first divergence, setup notes;
//! - `capture_requests.json` / `CAPTURE_REQUESTS.md`: every unmeasured expectation, for the hook;
//! - `traces/<system>__<name>.json`: the port traces; `SCORECARD.md`, `WORKLIST.md`, dossiers.

use crate::present::FxLibrary;
use crate::runner;
use anyhow::{Result, bail};
use dishonored_oracle::cache::{self, OracleKey, Probe, Provenance};
use dishonored_oracle::diff;
use dishonored_oracle::report::{Scorecard, Table, WorkItem};
use dishonored_oracle::scenario::{self, Check, CheckStatus, Cmp, Reference, Scenario};
use dishonored_oracle::trace::Trace;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Serialize)]
pub struct Divergence {
    /// `original` (trace against the captured original trace) or `checkpoint` (the first
    /// failing measured expectation).
    pub against: String,
    pub tick: Option<u32>,
    pub text: String,
}

#[derive(Serialize)]
pub struct ScenarioResult {
    pub name: String,
    pub system: String,
    pub map: String,
    /// `pass`, `fail`, `unmeasured` (no measured or static expectation), `error`, `sweep`.
    pub status: String,
    pub passed: usize,
    pub failed: usize,
    pub unmeasured: usize,
    pub seconds: f64,
    pub error: Option<String>,
    pub notes: Vec<String>,
    pub first_divergence: Option<Divergence>,
    pub original_trace: bool,
    pub spec_hash: String,
    pub trace_file: Option<String>,
    pub checks: Vec<Check>,
}

#[derive(Serialize)]
pub struct CaptureRequest {
    pub scenario: String,
    pub map: String,
    pub checkpoint: String,
    pub metric: String,
    pub port_value: Option<f64>,
    pub note: String,
    pub capture: Option<String>,
    pub spec_hash: String,
}

#[derive(Serialize)]
pub struct Totals {
    pub scenarios: usize,
    pub pass: usize,
    pub fail: usize,
    pub unmeasured: usize,
    pub error: usize,
    pub checks_pass: usize,
    pub checks_fail: usize,
    pub checks_unmeasured: usize,
    pub seconds: f64,
}

#[derive(Serialize)]
pub struct Results {
    pub totals: Totals,
    pub by_system: BTreeMap<String, (usize, usize, usize, usize)>,
    pub scenarios: Vec<ScenarioResult>,
    pub capture_requests: Vec<CaptureRequest>,
}

fn status_of(checks: &[Check]) -> &'static str {
    let judged: Vec<&Check> = checks.iter().filter(|c| c.status != CheckStatus::Unmeasured).collect();
    if judged.iter().any(|c| c.status == CheckStatus::Fail) {
        "fail"
    } else if judged.is_empty() {
        "unmeasured"
    } else {
        "pass"
    }
}

fn cmp_text(c: &Check) -> String {
    match c.cmp {
        Cmp::Eq => format!("{} +- {}", c.expected, c.tol),
        Cmp::AtLeast => format!(">= {}", c.expected - c.tol),
        Cmp::AtMost => format!("<= {}", c.expected + c.tol),
    }
}

fn val(v: Option<f64>) -> String {
    v.map(|v| if v.fract() == 0.0 && v.abs() < 1e9 { format!("{v:.0}") } else { format!("{v:.3}") }).unwrap_or_else(|| "missing".into())
}

/// Select scenarios: names (full or the part after the system), `--system S` (repeatable),
/// `--all` / nothing: every one.
pub fn select<'a>(all: &'a [(PathBuf, Scenario)], names: &[String], systems: &[String]) -> Vec<&'a Scenario> {
    all.iter()
        .map(|x| &x.1)
        .filter(|s| names.is_empty() || names.iter().any(|n| s.name == *n || s.name.ends_with(&format!("/{n}")) || (n.ends_with('*') && s.name.starts_with(n.trim_end_matches('*')))))
        .filter(|s| systems.is_empty() || systems.iter().any(|y| format!("{:?}", s.system).eq_ignore_ascii_case(y) || s.name.split('/').next().is_some_and(|p| p.eq_ignore_ascii_case(y))))
        .collect()
}

/// Run the selected scenarios in parallel; write the outputs; return the scorecard pieces.
pub fn run(chosen: Vec<&Scenario>, out: &Path, exe: Option<[u8; 32]>, jobs: Option<usize>) -> Result<(Results, Scorecard, Vec<WorkItem>)> {
    let t_all = Instant::now();
    if chosen.is_empty() {
        bail!("no scenario selected");
    }
    let root = cache::root();
    // Layout maps (the Playground) are found next to the repository.
    if std::env::var_os("DISHONORED_MAPS").is_none() {
        // SAFETY: set before any worker thread starts.
        unsafe { std::env::set_var("DISHONORED_MAPS", crate::repo_root().join("maps")) };
    }
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs.unwrap_or(0)).build()?;
    let mut maps: Vec<String> = chosen.iter().filter(|s| s.sweep.is_none()).map(|s| s.map.clone()).collect();
    maps.sort();
    maps.dedup();
    let t_load = Instant::now();
    let (loaded, fx) = pool.install(|| {
        rayon::join(
            || maps.par_iter().map(|m| (m.clone(), dishonored_content::MapContents::load(std::slice::from_ref(m)).map(Arc::new).map_err(|e| format!("{e:#}")))).collect::<HashMap<_, _>>(),
            || FxLibrary::load(&root).map(Arc::new),
        )
    });
    let fx = fx?;
    // Warm the contact system once (shared by every run).
    let _ = crate::present::contact_system();
    let load_s = t_load.elapsed().as_secs_f64();
    let trace_dir = out.join("traces");
    std::fs::create_dir_all(&trace_dir)?;
    let results: Vec<(ScenarioResult, Option<Trace>)> = pool.install(|| {
        chosen
            .par_iter()
            .map(|s| {
                let mut r = ScenarioResult {
                    name: s.name.clone(),
                    system: format!("{:?}", s.system).to_lowercase(),
                    map: s.map.clone(),
                    status: String::new(),
                    passed: 0,
                    failed: 0,
                    unmeasured: 0,
                    seconds: 0.0,
                    error: None,
                    notes: Vec::new(),
                    first_divergence: None,
                    original_trace: false,
                    spec_hash: format!("{:016x}", s.spec_hash()),
                    trace_file: None,
                    checks: Vec::new(),
                };
                if s.sweep.is_some() {
                    r.status = "sweep".into();
                    r.notes.push("map sweep: run by `oracle collision`".into());
                    return (r, None);
                }
                let maps = match loaded.get(&s.map) {
                    Some(Ok(m)) => m.clone(),
                    Some(Err(e)) => {
                        r.status = "error".into();
                        r.error = Some(format!("map {}: {e}", s.map));
                        return (r, None);
                    }
                    None => unreachable!(),
                };
                let run = match runner::run(s, &maps, fx.clone()) {
                    Ok(x) => x,
                    Err(e) => {
                        r.status = "error".into();
                        r.error = Some(format!("{e:#}"));
                        return (r, None);
                    }
                };
                r.seconds = run.seconds;
                r.notes = run.notes;
                r.checks = s.evaluate(&run.trace);
                r.passed = r.checks.iter().filter(|c| c.status == CheckStatus::Pass).count();
                r.failed = r.checks.iter().filter(|c| c.status == CheckStatus::Fail).count();
                r.unmeasured = r.checks.iter().filter(|c| c.status == CheckStatus::Unmeasured).count();
                r.status = status_of(&r.checks).into();
                // The original's trace for this run (cached by the hook), compared on the
                // channels and event kinds it recorded.
                let orig: Option<Trace> = exe.and_then(|e| {
                    let key = OracleKey { exe_sha256: e, subject: s.name.clone(), probe: Probe::Trace, query_hash: s.spec_hash() };
                    cache::load::<(Provenance, Trace)>(&root, &key).ok().map(|x| x.1)
                });
                r.original_trace = orig.is_some();
                r.first_divergence = match &orig {
                    Some(o) => {
                        let kinds: Vec<&str> = o.events.iter().map(|e| e.kind.as_str()).collect();
                        let mut port = run.trace.clone();
                        port.events.retain(|e| kinds.contains(&e.kind.as_str()));
                        let d = diff::first_divergence(o, &port, &|c| s.tolerance(c));
                        d.first.map(|x| Divergence { against: "original".into(), tick: Some(x.tick), text: format!("{:?}", x.kind) })
                    }
                    None => r.checks.iter().find(|c| c.status == CheckStatus::Fail).map(|c| Divergence { against: "checkpoint".into(), tick: c.tick, text: format!("{}: {} = {} (original {}; {})", c.checkpoint, c.metric, val(c.actual), cmp_text(c), c.source) }),
                };
                let file = trace_dir.join(format!("{}.json", s.name.replace('/', "__")));
                if std::fs::write(&file, serde_json::to_string(&run.trace).unwrap_or_default()).is_ok() {
                    r.trace_file = Some(file.display().to_string());
                }
                (r, Some(run.trace))
            })
            .collect()
    });
    let mut card = Scorecard { title: "Scenarios: the port against the original's measurements and traces".into(), ..Default::default() };
    let mut table = Table::new("Scenarios", &["scenario", "status", "pass/fail/unmeasured", "first divergence", "s"]);
    let mut checks_table = Table::new("Checks", &["scenario", "checkpoint", "metric", "port", "expected", "status", "reference"]);
    let mut items = Vec::new();
    let mut captures = Vec::new();
    let mut by_system: BTreeMap<String, (usize, usize, usize, usize)> = BTreeMap::new();
    let mut tot = Totals { scenarios: 0, pass: 0, fail: 0, unmeasured: 0, error: 0, checks_pass: 0, checks_fail: 0, checks_unmeasured: 0, seconds: 0.0 };
    let mut out_results = Vec::new();
    for ((r, _), s) in results.into_iter().zip(chosen.iter()) {
        if r.status == "sweep" {
            continue;
        }
        tot.scenarios += 1;
        let e = by_system.entry(r.system.clone()).or_default();
        match r.status.as_str() {
            "pass" => {
                tot.pass += 1;
                e.0 += 1;
            }
            "fail" => {
                tot.fail += 1;
                e.1 += 1;
            }
            "unmeasured" => {
                tot.unmeasured += 1;
                e.2 += 1;
            }
            _ => {
                tot.error += 1;
                e.3 += 1;
            }
        }
        tot.checks_pass += r.passed;
        tot.checks_fail += r.failed;
        tot.checks_unmeasured += r.unmeasured;
        let div = match (&r.error, &r.first_divergence) {
            (Some(e), _) => format!("error: {e}"),
            (None, Some(d)) => format!("{}{}: {}", d.against, d.tick.map(|t| format!(" tick {t}")).unwrap_or_default(), d.text),
            (None, None) if r.original_trace => "none (original trace matches)".into(),
            (None, None) => "-".into(),
        };
        table.row(vec![r.name.clone(), r.status.clone(), format!("{}/{}/{}", r.passed, r.failed, r.unmeasured), div, format!("{:.2}", r.seconds)]);
        for c in &r.checks {
            let st = match c.status {
                CheckStatus::Pass => "pass",
                CheckStatus::Fail => "**FAIL**",
                CheckStatus::Unmeasured => "unmeasured",
            };
            checks_table.row(vec![r.name.clone(), c.checkpoint.clone(), c.metric.clone(), val(c.actual), if c.reference == Reference::Unmeasured { "?".into() } else { cmp_text(c) }, st.into(), format!("{:?}: {}", c.reference, c.source)]);
            if c.status == CheckStatus::Unmeasured {
                captures.push(CaptureRequest { scenario: r.name.clone(), map: r.map.clone(), checkpoint: c.checkpoint.clone(), metric: c.metric.clone(), port_value: c.actual, note: c.source.clone(), capture: s.capture.clone(), spec_hash: r.spec_hash.clone() });
            }
            if c.status == CheckStatus::Fail {
                items.push(WorkItem {
                    id: format!("scenario:{}:{}:{}", r.name, c.checkpoint, c.metric),
                    title: format!("{}: {} {} = {} (original {})", r.name, c.checkpoint, c.metric, val(c.actual), cmp_text(c)),
                    probe: "scenario".into(),
                    system: r.system.clone(),
                    count: 1,
                    impact: if c.reference == Reference::Measured { 10.0 } else { 6.0 },
                    maps: vec![r.map.clone()],
                    evidence: vec![format!("reference ({:?}): {}", c.reference, c.source), format!("scenario: {}", s.description), format!("trace: {}", r.trace_file.clone().unwrap_or_default())],
                    repro: vec![format!("oracle scenario {}", r.name)],
                    code: vec![format!("the {} code of dishonored_sim / dishonored_ai / dishonored_runtime (presentation)", r.system)],
                    cause: None,
                });
            }
        }
        if let Some(e) = &r.error {
            items.push(WorkItem { id: format!("scenario:{}:error", r.name), title: format!("{}: the runner could not play it: {e}", r.name), probe: "scenario".into(), system: r.system.clone(), count: 1, impact: 8.0, maps: vec![r.map.clone()], evidence: r.notes.clone(), repro: vec![format!("oracle scenario {}", r.name)], code: vec!["tools/oracle/src/runner.rs".into()], cause: None });
        }
        if let Some(d) = r.first_divergence.as_ref().filter(|d| d.against == "original") {
            items.push(WorkItem { id: format!("trace:{}", r.name), title: format!("{}: first divergence from the original at tick {}", r.name, d.tick.unwrap_or(0)), probe: "trace".into(), system: r.system.clone(), count: 1, impact: 20.0, maps: vec![r.map.clone()], evidence: vec![d.text.clone()], repro: vec![format!("oracle scenario {}", r.name)], code: Vec::new(), cause: None });
        }
        out_results.push(r);
    }
    tot.seconds = t_all.elapsed().as_secs_f64();
    card.summary.push(("scenarios".into(), format!("{} run: {} pass, {} fail, {} unmeasured, {} error", tot.scenarios, tot.pass, tot.fail, tot.unmeasured, tot.error)));
    card.summary.push(("checks".into(), format!("{} pass, {} fail, {} unmeasured (capture requests)", tot.checks_pass, tot.checks_fail, tot.checks_unmeasured)));
    card.summary.push(("time".into(), format!("{:.2} s ({load_s:.2} s loading maps and effects)", tot.seconds)));
    let mut sys = Table::new("Per system", &["system", "pass", "fail", "unmeasured", "error"]);
    for (k, v) in &by_system {
        sys.row(vec![k.clone(), v.0.to_string(), v.1.to_string(), v.2.to_string(), v.3.to_string()]);
    }
    card.tables.push(sys);
    card.tables.push(table);
    card.tables.push(checks_table);
    let res = Results { totals: tot, by_system, scenarios: out_results, capture_requests: captures };
    std::fs::write(out.join("results.json"), serde_json::to_string_pretty(&res)?)?;
    std::fs::write(out.join("capture_requests.json"), serde_json::to_string_pretty(&res.capture_requests)?)?;
    let mut md = String::from("# Capture requests for the hook (unmeasured expectations)\n\nEach row: run the scenario's setup and input on the original (`capture`), then cache the trace\nunder the scenario's spec hash, or measure the metric and add the value to the spec\n(`status: Measured`, with the source).\n\n| scenario | map | checkpoint | metric | port value | capture |\n|---|---|---|---|---|---|\n");
    for c in &res.capture_requests {
        md.push_str(&format!("| {} | {} | {} | `{}` | {} | {} |\n", c.scenario, c.map, c.checkpoint, c.metric, val(c.port_value), c.capture.clone().unwrap_or_default().replace('|', "/")));
    }
    std::fs::write(out.join("CAPTURE_REQUESTS.md"), md)?;
    Ok((res, card, items))
}

/// Every scenario of a directory (sorted) plus the selection flags.
pub fn load(dir: &Path) -> Result<Vec<(PathBuf, Scenario)>> {
    scenario::load_dir(dir)
}
