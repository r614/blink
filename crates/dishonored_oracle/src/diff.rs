//! Diff engine: agreement rates, clustering of disagreements by the object responsible, and
//! the first divergence of two traces. It reports differences; it never adjusts either side.

use crate::query::{CollisionAnswer, CollisionQuery, CollisionSet};
use crate::report::{Table, WorkItem};
use crate::scenario::{Checkpoint, Metric, Scenario};
use crate::trace::{Trace, TraceEvent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ----- collision -----

/// The port's first hit of one query.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PortHit {
    /// Fraction of the way, as the original reports it (back-off applied).
    pub time: f32,
    /// Index into [`PortAnswers::names`].
    pub source: u32,
    /// The port's hit kind (`dishonored_world::hit_kind`, 0 when the port has none).
    pub kind: u8,
}

/// The port's answers to a query set (parallel to its queries).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PortAnswers {
    pub hits: Vec<Option<PortHit>>,
    pub names: Vec<String>,
}

impl PortAnswers {
    pub fn name(&self, h: &PortHit) -> &str {
        self.names.get(h.source as usize).map(String::as_str).unwrap_or("?")
    }
}

/// Distance tolerance along the check: 2 uu for lines, 4 uu for boxes (the `harness collision`
/// rule).
pub fn tolerance(q: &CollisionQuery) -> f32 {
    if q.is_box() { 4.0 } else { 2.0 }
}

/// Equal hit flags, and both hits within the tolerance along the way.
pub fn agrees(q: &CollisionQuery, o: &CollisionAnswer, p: Option<&PortHit>) -> bool {
    match (o.hit, p) {
        (false, None) => true,
        (true, Some(h)) => ((h.time - o.time) * q.length()).abs() <= tolerance(q),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Direction {
    OrigHitPortMiss,
    OrigMissPortHit,
    PortNearer,
    PortFarther,
}

impl Direction {
    pub fn text(self) -> &'static str {
        match self {
            Direction::OrigHitPortMiss => "original hits, port misses",
            Direction::OrigMissPortHit => "original misses, port hits",
            Direction::PortNearer => "port hits nearer",
            Direction::PortFarther => "port hits farther",
        }
    }
}

/// One disagreement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Disagreement {
    pub id: u32,
    pub direction: Direction,
    /// `<package>:<actor>` the original hit (`-`: none).
    pub original: String,
    /// The original's hit component (short name).
    pub component: String,
    /// The port's hit source (`-`: none).
    pub port: String,
    pub orig_time: f32,
    pub port_time: Option<f32>,
    /// Both hit: the offset key (`<line|box> <port kind> <trace>`) and the original's hit
    /// minus the port's along the check, uu.
    #[serde(default)]
    pub offset: Option<(String, f32)>,
}

/// Disagreements grouped by the object responsible: the original's hit object, or the port's
/// when the original hit nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cluster {
    pub object: String,
    pub by_direction: BTreeMap<String, u32>,
    /// The other side's objects in this cluster, with counts.
    pub counterparts: BTreeMap<String, u32>,
    pub ids: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CollisionDiff {
    pub total: u64,
    pub agree: u64,
    /// Answers that hit a living pawn (they say nothing about the level).
    pub excluded: u64,
    pub unanswered: u64,
    /// Per query tag (`grid:P`): (queries, agreeing).
    pub by_tag: BTreeMap<String, (u64, u64)>,
    /// Both hit within 20 uu: original minus port along the check (uu), by
    /// `<line|box> <port kind> <trace>`.
    pub offsets: BTreeMap<String, Vec<f32>>,
    pub disagreements: Vec<Disagreement>,
    pub clusters: Vec<Cluster>,
    /// Offset keys whose hits differ by a constant (median, uu): a rule-level difference (an
    /// engine back-off, a skin) rather than a per-object one.
    #[serde(default)]
    pub systematic: BTreeMap<String, f32>,
}

/// A key is systematic when it has at least 20 offsets, a median of at least 1 uu, and 60 %
/// of its offsets within 0.5 uu of the median.
pub fn systematic_offsets(offsets: &BTreeMap<String, Vec<f32>>) -> BTreeMap<String, f32> {
    let mut out = BTreeMap::new();
    for (k, v) in offsets {
        if v.len() < 20 {
            continue;
        }
        let (_, med, _) = quantiles(v);
        let near = v.iter().filter(|x| (**x - med).abs() <= 0.5).count();
        if med.abs() >= 1.0 && near as f32 >= 0.6 * v.len() as f32 {
            out.insert(k.clone(), med);
        }
    }
    out
}

impl CollisionDiff {
    pub fn rate(&self) -> f64 {
        if self.total == 0 { 1.0 } else { self.agree as f64 / self.total as f64 }
    }
}

fn short(component: &str) -> String {
    component.rsplit('.').next().unwrap_or(component).to_string()
}

/// Compare the port's answers with the original's.
pub fn compare_collision(set: &CollisionSet, port: &PortAnswers) -> CollisionDiff {
    let mut d = CollisionDiff::default();
    for (i, q) in set.queries.iter().enumerate() {
        let Some(o) = set.answers[i] else {
            d.unanswered += 1;
            continue;
        };
        if o.hit && set.actor_class(&o).contains("Pawn") {
            d.excluded += 1;
            continue;
        }
        let p = port.hits.get(i).copied().flatten();
        let mut offset = None;
        if let (true, Some(h)) = (o.hit, p) {
            let off = (o.time - h.time) * q.length();
            let key = format!("{} {} {}", if q.is_box() { "box" } else { "line" }, h.kind, q.trace());
            if off.abs() < 20.0 && o.time > 0.0 && h.time > 0.0 {
                d.offsets.entry(key.clone()).or_default().push(off);
            }
            offset = Some((key, off));
        }
        let ok = agrees(q, &o, p.as_ref());
        d.total += 1;
        let e = d.by_tag.entry(q.tag.clone()).or_default();
        e.0 += 1;
        if ok {
            d.agree += 1;
            e.1 += 1;
            continue;
        }
        let direction = match (o.hit, p) {
            (true, None) => Direction::OrigHitPortMiss,
            (false, Some(_)) => Direction::OrigMissPortHit,
            (true, Some(h)) if h.time < o.time => Direction::PortNearer,
            _ => Direction::PortFarther,
        };
        d.disagreements.push(Disagreement {
            id: i as u32,
            direction,
            original: if o.hit { set.actor_key(&o) } else { "-".into() },
            component: if o.hit { short(set.name(o.component)) } else { "-".into() },
            port: p.map(|h| port.name(&h).to_string()).unwrap_or_else(|| "-".into()),
            orig_time: o.time,
            port_time: p.map(|h| h.time),
            offset,
        });
    }
    d.systematic = systematic_offsets(&d.offsets);
    d.clusters = cluster(&d.disagreements, &d.systematic);
    d
}

/// Group disagreements by the responsible object (the original's hit component, or the
/// port's source when the original hit nothing), largest first. Disagreements explained by a
/// systematic offset go to one cluster per offset key.
pub fn cluster(ds: &[Disagreement], systematic: &BTreeMap<String, f32>) -> Vec<Cluster> {
    let mut m: BTreeMap<String, Cluster> = BTreeMap::new();
    for x in ds {
        let sys = x.offset.as_ref().and_then(|(k, off)| systematic.get(k).filter(|med| (off - **med).abs() <= 0.5).map(|med| format!("systematic: {k} checks hit {:.2} uu {} in the original", med.abs(), if *med < 0.0 { "earlier" } else { "later" })));
        let (object, other) = if let Some(s) = sys {
            (s, x.original.clone())
        } else if x.original != "-" {
            (format!("{}.{}", x.original, x.component), x.port.clone())
        } else {
            (format!("port:{}", x.port), "-".to_string())
        };
        let c = m.entry(object.clone()).or_insert_with(|| Cluster { object, by_direction: BTreeMap::new(), counterparts: BTreeMap::new(), ids: Vec::new() });
        *c.by_direction.entry(x.direction.text().to_string()).or_default() += 1;
        *c.counterparts.entry(other).or_default() += 1;
        c.ids.push(x.id);
    }
    let mut v: Vec<Cluster> = m.into_values().collect();
    v.sort_by(|a, b| b.ids.len().cmp(&a.ids.len()).then(a.object.cmp(&b.object)));
    v
}

fn quantiles(v: &[f32]) -> (f32, f32, f32) {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let q = |f: f32| v[((v.len() - 1) as f32 * f) as usize];
    (q(0.1), q(0.5), q(0.9))
}

/// Agreement and offset tables of a map.
pub fn collision_tables(map: &str, d: &CollisionDiff) -> Vec<Table> {
    let mut t = Table::new(&format!("{map}: collision agreement per query kind"), &["kind", "queries", "agree", "rate"]);
    for (tag, (n, a)) in &d.by_tag {
        t.row(vec![tag.clone(), n.to_string(), a.to_string(), format!("{:.2} %", 100.0 * *a as f64 / (*n).max(1) as f64)]);
    }
    t.row(vec!["**all**".into(), d.total.to_string(), d.agree.to_string(), format!("{:.2} %", 100.0 * d.rate())]);
    let mut o = Table::new(&format!("{map}: original hit minus port hit along the check, uu (both hit within 20 uu)"), &["check port-kind trace", "n", "p10", "median", "p90"]);
    for (k, v) in &d.offsets {
        let (a, b, c) = quantiles(v);
        o.row(vec![k.clone(), v.len().to_string(), format!("{a:.2}"), format!("{b:.2}"), format!("{c:.2}")]);
    }
    vec![t, o]
}

/// One work item per cluster.
pub fn collision_work_items(map: &str, set: &CollisionSet, d: &CollisionDiff, examples: usize) -> Vec<WorkItem> {
    let mut out = Vec::new();
    for c in &d.clusters {
        let n = c.ids.len() as u64;
        let mut evidence = vec![
            format!("{n} of {} compared queries on {map} disagree because of this object ({:.3} % of the map's checks).", d.total, 100.0 * n as f64 / d.total.max(1) as f64),
            format!("by direction: {}", c.by_direction.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")),
            {
                let mut cp: Vec<(&String, &u32)> = c.counterparts.iter().collect();
                cp.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                let more = cp.len().saturating_sub(10);
                format!(
                    "other side (top 10): {}{}",
                    cp.iter().take(10).map(|(k, v)| format!("`{k}` {v}")).collect::<Vec<_>>().join(", "),
                    if more > 0 { format!(" and {more} more") } else { String::new() }
                )
            },
        ];
        let mut ex = String::from("| query | kind | start | end | extent | original | port |\n|---|---|---|---|---|---|---|\n");
        for &id in c.ids.iter().take(examples) {
            let q = &set.queries[id as usize];
            let x = d.disagreements.iter().find(|x| x.id == id).unwrap();
            let f = |v: [f32; 3]| format!("{:.1} {:.1} {:.1}", v[0], v[1], v[2]);
            ex.push_str(&format!(
                "| {id} | {} | {} | {} | {} | {} | {} |\n",
                q.tag,
                f(q.start),
                f(q.end),
                f(q.extent),
                if x.original == "-" { "miss".to_string() } else { format!("{:.4} `{}`", x.orig_time, x.component) },
                x.port_time.map(|t| format!("{t:.4} `{}`", x.port)).unwrap_or_else(|| "miss".into())
            ));
        }
        evidence.push(ex);
        if !set.doors.is_empty() {
            evidence.push(format!("door states when captured: {} doors (the port poses its doors the same way)", set.doors.len()));
        }
        let first = &set.queries[c.ids[0] as usize];
        let args = |q: &CollisionQuery| {
            let mut s = format!("{} {} {} {} {} {}", q.start[0], q.start[1], q.start[2], q.end[0], q.end[1], q.end[2]);
            if q.is_box() {
                s.push_str(&format!(" {} {} {}", q.extent[0], q.extent[1], q.extent[2]));
            }
            s
        };
        let title = if let Some(s) = c.object.strip_prefix("systematic: ") {
            format!("Collision rule: {s}")
        } else if c.object.starts_with("port:") {
            format!("Collision: port blocks where the original does not ({})", c.object.trim_start_matches("port:"))
        } else {
            format!("Collision: `{}` answers differently", c.object)
        };
        out.push(WorkItem {
            id: format!("collision:{map}:{}", c.object),
            title,
            probe: "collision".into(),
            system: "collision".into(),
            count: n,
            impact: n as f64,
            maps: vec![map.to_string()],
            evidence,
            repro: vec![
                format!("oracle collision --map {map} --ids {}", c.ids.iter().take(20).map(|i| i.to_string()).collect::<Vec<_>>().join(",")),
                format!("harness collision probe --map {map} {}", args(first)),
            ],
            code: vec![
                "crates/dishonored_world/src/lib.rs (raycast, sweep_box)".into(),
                "crates/dishonored_assets/src/level.rs, prop_collision.rs (which objects collide, on which channels)".into(),
            ],
            cause: None,
        });
    }
    out
}

// ----- traces -----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DivergenceKind {
    /// A channel differs by more than its tolerance.
    Channel { channel: String, original: f64, port: f64, tolerance: f64 },
    /// The event sequences differ at this index.
    Event { index: usize, original: Option<TraceEvent>, port: Option<TraceEvent> },
    /// One side has no frame for this tick.
    MissingFrame { side: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Divergence {
    pub tick: u32,
    pub kind: DivergenceKind,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TraceDiff {
    pub first: Option<Divergence>,
    /// Channels recorded on one side only (not compared).
    pub only_original: Vec<String>,
    pub only_port: Vec<String>,
    pub compared_ticks: u32,
    /// Per channel: worst absolute difference and its tick.
    pub worst: BTreeMap<String, (f64, u32)>,
}

/// First divergence of two traces: the earliest tick where a shared channel differs beyond
/// `tol(channel)` or the event sequences stop matching (kind, object, name).
pub fn first_divergence(orig: &Trace, port: &Trace, tol: &dyn Fn(&str) -> f64) -> TraceDiff {
    let mut d = TraceDiff::default();
    let shared: Vec<(String, usize, usize)> = orig.channels.iter().enumerate().filter_map(|(i, c)| port.channel(&c.name).map(|j| (c.name.clone(), i, j))).collect();
    d.only_original = orig.channels.iter().filter(|c| port.channel(&c.name).is_none()).map(|c| c.name.clone()).collect();
    d.only_port = port.channels.iter().filter(|c| orig.channel(&c.name).is_none()).map(|c| c.name.clone()).collect();
    let pf: BTreeMap<u32, &Vec<f64>> = port.frames.iter().map(|f| (f.tick, &f.values)).collect();
    let mut chan_div: Option<Divergence> = None;
    for f in &orig.frames {
        let Some(pv) = pf.get(&f.tick) else {
            if chan_div.is_none() {
                chan_div = Some(Divergence { tick: f.tick, kind: DivergenceKind::MissingFrame { side: "port".into() } });
            }
            continue;
        };
        d.compared_ticks += 1;
        for (name, i, j) in &shared {
            let (a, b) = (f.values[*i], pv[*j]);
            if a.is_nan() || b.is_nan() {
                continue;
            }
            let diff = (a - b).abs();
            let w = d.worst.entry(name.clone()).or_insert((0.0, f.tick));
            if diff > w.0 {
                *w = (diff, f.tick);
            }
            let t = tol(name);
            if diff > t && chan_div.is_none() {
                chan_div = Some(Divergence { tick: f.tick, kind: DivergenceKind::Channel { channel: name.clone(), original: a, port: b, tolerance: t } });
            }
        }
    }
    // Events must match in order and tick (both sides run a fixed step). Only the kinds both
    // sides record are compared (the original's `ProcessEvent` / `Spawn` records have no port
    // counterpart until the port emits them); the others are listed like one-sided channels.
    let key = |e: &TraceEvent| (e.tick, e.kind.clone(), e.object.clone(), e.name.clone());
    let kinds = |t: &Trace| t.events.iter().map(|e| e.kind.clone()).collect::<std::collections::BTreeSet<String>>();
    let (ko, kp) = (kinds(orig), kinds(port));
    d.only_original.extend(ko.difference(&kp).map(|k| format!("events:{k}")));
    d.only_port.extend(kp.difference(&ko).map(|k| format!("events:{k}")));
    let oe: Vec<&TraceEvent> = orig.events.iter().filter(|e| kp.contains(&e.kind)).collect();
    let pe: Vec<&TraceEvent> = port.events.iter().filter(|e| ko.contains(&e.kind)).collect();
    let mut ev_div = None;
    let n = oe.len().max(pe.len());
    for k in 0..n {
        let (a, b) = (oe.get(k).copied(), pe.get(k).copied());
        let same = match (a, b) {
            (Some(x), Some(y)) => key(x) == key(y),
            _ => false,
        };
        if !same {
            let tick = a.map(|e| e.tick).into_iter().chain(b.map(|e| e.tick)).min().unwrap_or(0);
            ev_div = Some(Divergence { tick, kind: DivergenceKind::Event { index: k, original: a.cloned(), port: b.cloned() } });
            break;
        }
    }
    d.first = match (chan_div, ev_div) {
        (Some(a), Some(b)) => Some(if b.tick < a.tick { b } else { a }),
        (a, b) => a.or(b),
    };
    d
}

/// A checkpoint expectation's outcome on one trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub checkpoint: String,
    pub metric: String,
    pub expected: f64,
    pub tol: f64,
    pub actual: Option<f64>,
    pub pass: bool,
    pub source: String,
}

pub fn metric_value(t: &Trace, m: &Metric) -> Option<f64> {
    match m {
        Metric::At(c, tick) => t.series(c).into_iter().find(|(k, _)| k == tick).map(|x| x.1),
        Metric::Max(c) => t.series(c).into_iter().map(|x| x.1).reduce(f64::max),
        Metric::Min(c) => t.series(c).into_iter().map(|x| x.1).reduce(f64::min),
        Metric::Rise(c) => {
            let s = t.series(c);
            let first = s.first()?.1;
            s.iter().map(|x| x.1).reduce(f64::max).map(|m| m - first)
        }
        Metric::RiseFrom(c, tick) => {
            let s: Vec<(u32, f64)> = t.series(c).into_iter().filter(|x| x.0 >= *tick).collect();
            let first = s.first().filter(|x| x.0 == *tick)?.1;
            s.iter().map(|x| x.1).reduce(f64::max).map(|m| m - first)
        }
        Metric::TimeToReach(c, thr) => {
            let s = t.series(c);
            let t0 = s.first()?.0;
            s.iter().find(|x| x.1 >= *thr).map(|x| (x.0 - t0) as f64 / t.tick_hz.max(1) as f64)
        }
    }
}

pub fn check_checkpoints(t: &Trace, cps: &[Checkpoint]) -> Vec<CheckResult> {
    let mut out = Vec::new();
    for cp in cps {
        for e in &cp.expect {
            let actual = metric_value(t, &e.metric);
            out.push(CheckResult {
                checkpoint: cp.name.clone(),
                metric: format!("{:?}", e.metric),
                expected: e.value,
                tol: e.tol,
                actual,
                pass: actual.is_some_and(|a| (a - e.value).abs() <= e.tol),
                source: e.source.clone(),
            });
        }
    }
    out
}

/// Work items of a scenario: a first divergence against the original's trace, and failed
/// checkpoints (expectations measured on the original).
pub fn scenario_work_items(s: &Scenario, checks: &[CheckResult], diff: Option<&TraceDiff>) -> Vec<WorkItem> {
    let mut out = Vec::new();
    let system = format!("{:?}", s.system).to_lowercase();
    for c in checks.iter().filter(|c| !c.pass) {
        out.push(WorkItem {
            id: format!("scenario:{}:{}:{}", s.name, c.checkpoint, c.metric),
            title: format!("{}: {} {} = {} (original {} +- {})", s.name, c.checkpoint, c.metric, c.actual.map(|a| format!("{a:.3}")).unwrap_or_else(|| "missing".into()), c.expected, c.tol),
            probe: "scenario".into(),
            system: system.clone(),
            count: 1,
            impact: 10.0,
            maps: vec![s.map.clone()],
            evidence: vec![format!("reference: {}", c.source), format!("scenario: {}", s.description)],
            repro: vec![format!("oracle scenario {}", s.name)],
            code: vec![format!("the {system} code of dishonored_sim / dishonored_ai")],
            cause: None,
        });
    }
    if let Some(Divergence { tick, kind }) = diff.and_then(|d| d.first.as_ref()) {
        out.push(WorkItem {
            id: format!("trace:{}", s.name),
            title: format!("{}: first divergence at tick {tick}", s.name),
            probe: "trace".into(),
            system,
            count: 1,
            impact: 20.0,
            maps: vec![s.map.clone()],
            evidence: vec![format!("{kind:?}")],
            repro: vec![format!("oracle scenario {} --trace", s.name)],
            code: Vec::new(),
            cause: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parse_probe_text;
    use crate::scenario::Expect;

    fn set() -> CollisionSet {
        let q = "0:grid:P 0 0 100 0 0 -100 0 0 0 0x20de\n1:grid:P 10 0 100 10 0 -100 0 0 0 0x20de\n2:head:V 0 0 50 100 0 50 0 0 0 0x8023086\n3:head:V 0 5 50 100 5 50 0 0 0 0x8023086\n4:pdrop:P 0 0 200 0 0 0 35 35 88 0x20df\n5:foot:P 0 0 20 100 0 20 0 0 0 0x20df\n";
        let r = "0 1 0.500 0 0 0 0 0 1 SMCA/L_E.TheWorld.PersistentLevel.Wall_1 SMC/L_E.TheWorld.PersistentLevel.Wall_1.C0\n\
                 1 1 0.500 0 0 0 0 0 1 SMCA/L_E.TheWorld.PersistentLevel.Wall_1 SMC/L_E.TheWorld.PersistentLevel.Wall_1.C0\n\
                 2 0 1.0 0 0 0 0 0 0 - -\n\
                 3 1 0.200 0 0 0 0 0 1 SMCA/L_E.TheWorld.PersistentLevel.Wall_1 SMC/L_E.TheWorld.PersistentLevel.Wall_1.C0\n\
                 4 1 0.900 0 0 0 0 0 1 DishonoredPawn/L_E.TheWorld.PersistentLevel.Guard_1 C/x\n\
                 5 1 0.300 0 0 0 0 0 1 SMCA/L_E.TheWorld.PersistentLevel.Crate_2 SMC/L_E.TheWorld.PersistentLevel.Crate_2.C0\n";
        parse_probe_text("t", q, r).unwrap()
    }

    #[test]
    fn collision_agreement_and_clusters() {
        let s = set();
        let ph = |t: f32, src: u32| Some(PortHit { time: t, source: src, kind: 0 });
        // 0: agrees (0.5 vs 0.505 over 200 uu = 1 uu); 1: port misses; 2: port hits extra;
        // 3: port farther; 4: pawn hit, excluded; 5: no port answer -> miss.
        let port = PortAnswers { hits: vec![ph(0.505, 0), None, ph(0.5, 1), ph(0.6, 0), None, None], names: vec!["wall".into(), "lamp".into()] };
        let d = compare_collision(&s, &port);
        assert_eq!((d.total, d.agree, d.excluded), (5, 1, 1));
        assert_eq!(d.by_tag["grid:P"], (2, 1));
        assert_eq!(d.disagreements.len(), 4);
        assert_eq!(d.clusters[0].object, "L_E:Wall_1.C0");
        assert_eq!(d.clusters[0].ids, vec![1, 3]);
        assert!(d.clusters.iter().any(|c| c.object == "port:lamp" && c.by_direction["original misses, port hits"] == 1));
        let items = collision_work_items("L_T", &s, &d, 5);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].count, 2);
        assert!(items[0].repro[0].contains("--ids 1,3"));
        assert_eq!(d.offsets["line 0 P"].len(), 1);
    }

    fn trace(side: &str, zs: &[f64], events: &[(u32, &str)]) -> Trace {
        let mut t = Trace::new("s", side, 60, &[("pawn.z", "uu"), ("pawn.vz", "uu/s")]);
        for (i, z) in zs.iter().enumerate() {
            t.push(i as u32, vec![*z, f64::NAN]);
        }
        t.events = events.iter().map(|(k, n)| TraceEvent { tick: *k, kind: "Sim".into(), object: "p".into(), name: n.to_string(), detail: String::new() }).collect();
        t
    }

    #[test]
    fn first_divergence_channel_and_event() {
        let a = trace("original", &[0.0, 10.0, 20.0, 25.0], &[(1, "Jump"), (3, "Land")]);
        let b = trace("port", &[0.0, 10.0, 20.5, 30.0], &[(1, "Jump"), (2, "Land")]);
        let d = first_divergence(&a, &b, &|_| 0.1);
        match d.first.unwrap() {
            Divergence { tick: 2, kind: DivergenceKind::Channel { channel, .. } } => assert_eq!(channel, "pawn.z"),
            x => panic!("{x:?}"),
        }
        assert_eq!(d.worst["pawn.z"], (5.0, 3));
        // With a looser tolerance the event order decides (tick 2: port lands early).
        let d = first_divergence(&a, &b, &|_| 10.0);
        assert!(matches!(d.first, Some(Divergence { tick: 2, kind: DivergenceKind::Event { index: 1, .. } })));
        // Identical traces: no divergence.
        assert!(first_divergence(&a, &a, &|_| 0.0).first.is_none());
    }

    #[test]
    fn checkpoints() {
        let t = trace("port", &[0.0, 50.0, 106.6, 80.0], &[]);
        let cps = vec![Checkpoint {
            name: "apex".into(),
            expect: vec![
                Expect { metric: Metric::Rise("pawn.z".into()), value: 106.67, tol: 0.5, source: "m".into() },
                Expect { metric: Metric::TimeToReach("pawn.z".into(), 100.0), value: 2.0 / 60.0, tol: 1e-6, source: "m".into() },
                Expect { metric: Metric::At("pawn.vz".into(), 1), value: 0.0, tol: 1.0, source: "m".into() },
            ],
        }];
        let r = check_checkpoints(&t, &cps);
        assert!(r[0].pass && r[1].pass);
        assert!(!r[2].pass && r[2].actual.is_none());
    }
}
