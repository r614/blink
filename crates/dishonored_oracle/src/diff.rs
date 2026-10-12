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
//
// Alignment (both sides, ORACLE.md "Trace alignment"): frame k is the state at the end of world
// tick k (input processed, physics done, before the next tick); frame 0 is the state after the
// setup and the `settle` ticks, before any scripted input; the input of timeline tick t (a key
// change, a view write, a console command) acts in the world tick that produces frame t + 1,
// and an event raised during that world tick carries tick t + 1. Frames are matched by tick.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DivergenceKind {
    /// A channel differs by more than its tolerance. In a relative comparison `original` and
    /// `port` are the displacements from the baseline frame.
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

/// How one channel was judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelMode {
    /// Values compared as they are.
    Absolute,
    /// Displacements from the baseline frame compared (the spec's `relative`): the offset is
    /// the difference at the baseline frame.
    Relative,
    /// A continuous channel whose sides differ by a steady amount most of the run (the median
    /// of original minus port, beyond the tolerance): compared after removing it, so the offset
    /// is reported once and the motion on top of it is still judged.
    SteadyOffset,
}

/// One channel's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelVerdict {
    /// Within the tolerance on every compared tick.
    Agree,
    /// Only a constant offset: once it is removed the channel agrees on every tick.
    Offset,
    /// The comparison (after removing the offset, if any) fails on some ticks (`spans`).
    Diverge,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelDiff {
    /// The port's channel name (the original's when it differs: `alias`).
    pub channel: String,
    /// The original's channel name when it is recorded under another name.
    #[serde(default)]
    pub alias: Option<String>,
    pub group: String,
    pub mode: ChannelMode,
    pub verdict: ChannelVerdict,
    pub tolerance: f64,
    /// Original minus port at the baseline frame (the first one both sampled).
    pub start_offset: f64,
    pub baseline_tick: u32,
    /// The offset removed before judging (original minus port; 0 for `Absolute`).
    pub offset: f64,
    /// The first tick beyond the tolerance after removing `offset`: (tick, original, port),
    /// raw values.
    pub first: Option<(u32, f64, f64)>,
    /// The runs of ticks beyond the tolerance (first, last), at most 8.
    #[serde(default)]
    pub spans: Vec<(u32, u32)>,
    /// Worst difference after removing `offset`, and its tick.
    pub worst: (f64, u32),
    pub compared: u32,
    /// Compared ticks within the tolerance after removing `offset`.
    #[serde(default)]
    pub agree_ticks: u32,
}

/// A channel group's (or an event kind's) first divergence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupDiff {
    /// `pawn position`, `pawn velocity`, `pawn state`, `vitals`, `camera`, `left hand`, `npc`,
    /// `behaviour`, `world`, `events:<kind>`.
    pub group: String,
    pub channels: u32,
    pub agree: u32,
    pub offset: u32,
    pub diverge: u32,
    /// The earliest divergence in the group (offset-only channels excluded).
    pub first: Option<Divergence>,
    /// Offsets removed in the group: (channel, original minus port).
    pub offsets: Vec<(String, f64)>,
    /// Mapped events: counts per side, pairs and their mean tick offset (original minus port).
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TraceDiff {
    /// The earliest divergence over every group (offset-only channels excluded).
    pub first: Option<Divergence>,
    /// Channels recorded on one side only (not compared).
    pub only_original: Vec<String>,
    pub only_port: Vec<String>,
    pub compared_ticks: u32,
    /// Ticks of the original without a port frame.
    #[serde(default)]
    pub missing_ticks: Vec<u32>,
    /// Per channel: worst absolute difference and its tick.
    pub worst: BTreeMap<String, (f64, u32)>,
    #[serde(default)]
    pub channels: Vec<ChannelDiff>,
    #[serde(default)]
    pub groups: Vec<GroupDiff>,
}

impl TraceDiff {
    /// (agree, offset, diverge) over the compared channels.
    pub fn channel_counts(&self) -> (usize, usize, usize) {
        let n = |v: ChannelVerdict| self.channels.iter().filter(|c| c.verdict == v).count();
        (n(ChannelVerdict::Agree), n(ChannelVerdict::Offset), n(ChannelVerdict::Diverge))
    }
}

/// The comparison group of a channel.
pub fn channel_group(name: &str) -> &'static str {
    if let Some(rest) = name.strip_prefix("npc.") {
        let field = rest.split_once('.').map(|x| x.1).unwrap_or("");
        return match field {
            "x" | "y" | "z" | "yaw" | "speed2d" | "health" | "alive" | "condition" | "physics" | "time_scale" => "npc",
            _ => "behaviour",
        };
    }
    match name {
        "pawn.x" | "pawn.y" | "pawn.z" => "pawn position",
        "pawn.vx" | "pawn.vy" | "pawn.vz" | "pawn.speed2d" => "pawn velocity",
        "pawn.health" | "pawn.mana" | "pawn.health_max" | "pawn.air_used" => "vitals",
        _ if name.starts_with("pawn.") => "pawn state",
        _ if name.starts_with("cam.rel_") => "camera offset",
        _ if name.starts_with("cam.") || name.starts_with("camera.") => "camera",
        _ if name.starts_with("left.") => "left hand",
        _ => "world",
    }
}

/// The port's name of an original channel recorded under another name (the same quantity).
/// `npc.<n>.player_attn.value` is `DisAttentionInfo_Complex.m_AttentionAccumulator.m_fCurrentValue`
/// of the player's entry, the port's `npc.<n>.attn` (AI.md "Attention").
///
/// `camera.x/y/z/pitch/yaw/roll` (`Camera.CameraCache.POV`, the view the frame is rendered
/// from) are the port's `cam.*` (the view point and rotation it renders from): the same
/// quantity in the same world frame (the hook subtracts the layout offset from `camera.*` as
/// from `pawn.*`; rotations are degrees, pitch up positive on both, compared wrapped to
/// +-180). Measured on Boyle (2026-10-08): the original's view sits 77.9 uu above the pawn's
/// Location and 5.2 uu ahead at level pitch (the port: 85.0 and 0), its pitch reads 0.15
/// degrees under the commanded one; yaw, roll and the lean's sideways offset agree.
pub fn port_name(original: &str) -> String {
    if let Some(n) = original.strip_prefix("npc.").and_then(|r| r.strip_suffix(".player_attn.value")) {
        return format!("npc.{n}.attn");
    }
    if let Some(c) = original.strip_prefix("camera.")
        && matches!(c, "x" | "y" | "z" | "pitch" | "yaw" | "roll")
    {
        return format!("cam.{c}");
    }
    original.to_string()
}

/// A trace with the shared names (the original's channels renamed by [`port_name`]) and the
/// derived channels both sides can give: `cam.rel_x/y/z`, the view point minus the pawn's
/// Location (the camera rule apart from where the pawn is). Returns the renames (new -> old).
fn normalised(t: &Trace, original: bool) -> (Trace, BTreeMap<String, String>) {
    let mut t = t.clone();
    let mut renamed = BTreeMap::new();
    if original {
        for c in t.channels.iter_mut() {
            let n = port_name(&c.name);
            if n != c.name {
                renamed.insert(n.clone(), std::mem::replace(&mut c.name, n));
            }
        }
    }
    let idx: Vec<Option<(usize, usize)>> = ["x", "y", "z"].iter().map(|a| t.channel(&format!("cam.{a}")).zip(t.channel(&format!("pawn.{a}")))).collect();
    if idx.iter().all(Option::is_some) {
        for (a, ix) in ["x", "y", "z"].iter().zip(&idx) {
            let (c, p) = ix.unwrap();
            t.channels.push(crate::trace::Channel { name: format!("cam.rel_{a}"), unit: "uu".into() });
            for f in t.frames.iter_mut() {
                let v = f.values[c] - f.values[p];
                f.values.push(v);
            }
        }
    }
    (t, renamed)
}

/// Whether a channel's values are a continuum (a start offset can be separated from the
/// motion): not booleans, enumerations or counts.
fn continuous(unit: &str) -> bool {
    !(unit == "bool" || unit.starts_with("enum") || unit == "n" || unit == "ptr")
}

/// The difference of two values of a channel (angles wrapped to +-180).
fn delta(unit: &str, a: f64, b: f64) -> f64 {
    let d = a - b;
    if unit == "deg" { (d + 180.0).rem_euclid(360.0) - 180.0 } else { d }
}

/// Compare two traces channel by channel and event kind by event kind.
///
/// Each shared channel (matched by name, or through [`port_name`]) is compared on the ticks both
/// sides sampled. A channel the spec marks `relative` is compared as displacements from the
/// baseline frame; a continuous channel whose two sides differ by a steady amount (the median of
/// original minus port, beyond the tolerance) is compared after removing it. Either way the
/// offset is reported once (`Offset`, with its value) and does not hide what the channel does
/// on top of it; the ticks still beyond the tolerance are listed as spans. Each group reports its own first divergence, so a
/// position offset does not mask velocity, vitals, NPC or behaviour channels. Event kinds both
/// sides record are compared in order (tick, object, name), each kind on its own.
pub fn compare_traces(orig: &Trace, port: &Trace, tol: &dyn Fn(&str) -> f64, relative: &dyn Fn(&str) -> bool, event_tol: u32) -> TraceDiff {
    let mut d = TraceDiff::default();
    let (orig_n, renamed) = normalised(orig, true);
    let (port_n, _) = normalised(port, false);
    let (orig, port) = (&orig_n, &port_n);
    let pairs: Vec<(String, Option<String>, usize, usize, String)> = orig
        .channels
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            port.channel(&c.name).map(|j| {
                let unit = if port.channels[j].unit.is_empty() { c.unit.clone() } else { port.channels[j].unit.clone() };
                (c.name.clone(), renamed.get(&c.name).cloned(), i, j, unit)
            })
        })
        .collect();
    let matched_port: std::collections::BTreeSet<&str> = pairs.iter().map(|p| p.0.as_str()).collect();
    d.only_original = orig.channels.iter().filter(|c| !matched_port.contains(c.name.as_str())).map(|c| renamed.get(&c.name).cloned().unwrap_or_else(|| c.name.clone())).collect();
    d.only_port = port.channels.iter().filter(|c| !matched_port.contains(c.name.as_str())).map(|c| c.name.clone()).collect();
    let pf: BTreeMap<u32, &Vec<f64>> = port.frames.iter().map(|f| (f.tick, &f.values)).collect();
    let both: Vec<(u32, &Vec<f64>, &Vec<f64>)> = orig
        .frames
        .iter()
        .filter_map(|f| match pf.get(&f.tick) {
            Some(pv) => Some((f.tick, &f.values, *pv)),
            None => {
                d.missing_ticks.push(f.tick);
                None
            }
        })
        .collect();
    d.compared_ticks = both.len() as u32;
    for (name, alias, i, j, unit) in &pairs {
        let t = tol(name);
        let samples: Vec<(u32, f64, f64)> = both.iter().map(|(k, a, b)| (*k, a[*i], b[*j])).filter(|x| !x.1.is_nan() && !x.2.is_nan()).collect();
        let Some(&(t0, a0, b0)) = samples.first() else { continue };
        let start_offset = delta(unit, a0, b0);
        let raw: Vec<f64> = samples.iter().map(|&(_, a, b)| delta(unit, a, b)).collect();
        d.worst.insert(name.clone(), raw.iter().enumerate().map(|(n, x)| (x.abs(), samples[n].0)).fold((0.0, t0), |m, x| if x.0 > m.0 { x } else { m }));
        let (mode, offset) = if relative(name) {
            (ChannelMode::Relative, start_offset)
        } else if continuous(unit) {
            let mut v = raw.clone();
            v.sort_by(|a, b| a.total_cmp(b));
            let med = v[v.len() / 2];
            if med.abs() > t { (ChannelMode::SteadyOffset, med) } else { (ChannelMode::Absolute, 0.0) }
        } else {
            (ChannelMode::Absolute, 0.0)
        };
        let mut first = None;
        let mut spans: Vec<(u32, u32)> = Vec::new();
        let mut worst = (0.0f64, t0);
        let mut open: Option<(u32, u32)> = None;
        let mut agree_ticks = 0u32;
        for (n, &(k, a, b)) in samples.iter().enumerate() {
            let r = (raw[n] - offset).abs();
            if r > worst.0 {
                worst = (r, k);
            }
            if r <= t {
                agree_ticks += 1;
            }
            if r > t {
                first.get_or_insert((k, a, b));
                open = Some(match open {
                    Some((s, _)) => (s, k),
                    None => (k, k),
                });
            } else if let Some(sp) = open.take() {
                spans.push(sp);
            }
        }
        spans.extend(open);
        spans.truncate(8);
        let verdict = match (first.is_some(), offset.abs() > t) {
            (true, _) => ChannelVerdict::Diverge,
            (false, true) => ChannelVerdict::Offset,
            (false, false) => ChannelVerdict::Agree,
        };
        d.channels.push(ChannelDiff { channel: name.clone(), alias: alias.clone(), group: channel_group(name).to_string(), mode, verdict, tolerance: t, start_offset, baseline_tick: t0, offset, first, spans, worst, compared: samples.len() as u32, agree_ticks });
    }    // Groups.
    let mut groups: BTreeMap<String, GroupDiff> = BTreeMap::new();
    for c in &d.channels {
        let g = groups.entry(c.group.clone()).or_insert_with(|| GroupDiff { group: c.group.clone(), channels: 0, agree: 0, offset: 0, diverge: 0, first: None, offsets: Vec::new(), note: String::new() });
        g.channels += 1;
        match c.verdict {
            ChannelVerdict::Agree => g.agree += 1,
            ChannelVerdict::Offset => g.offset += 1,
            ChannelVerdict::Diverge => g.diverge += 1,
        }
        if c.offset.abs() > c.tolerance {
            g.offsets.push((c.channel.clone(), c.offset));
        }
        if let Some((k, a, b)) = c.first
            && g.first.as_ref().is_none_or(|f| k < f.tick)
        {
            let channel = if c.offset != 0.0 { format!("{} (after removing the offset {:+.3})", c.channel, c.offset) } else { c.channel.clone() };
            g.first = Some(Divergence { tick: k, kind: DivergenceKind::Channel { channel, original: a, port: b, tolerance: c.tolerance } });
        }
    }
    // Events, per kind both sides record.
    let kinds = |t: &Trace| t.events.iter().map(|e| e.kind.clone()).collect::<std::collections::BTreeSet<String>>();
    let (ko, kp) = (kinds(orig), kinds(port));
    d.only_original.extend(ko.difference(&kp).map(|k| format!("events:{k}")));
    d.only_port.extend(kp.difference(&ko).map(|k| format!("events:{k}")));
    let key = |e: &TraceEvent| (e.tick, e.object.clone(), e.name.clone());
    for kind in ko.intersection(&kp) {
        let oe: Vec<&TraceEvent> = orig.events.iter().filter(|e| &e.kind == kind).collect();
        let pe: Vec<&TraceEvent> = port.events.iter().filter(|e| &e.kind == kind).collect();
        let mut first = None;
        let mut same_n = 0u32;
        for k in 0..oe.len().max(pe.len()) {
            let (a, b) = (oe.get(k).copied(), pe.get(k).copied());
            if let (Some(x), Some(y)) = (a, b)
                && key(x) == key(y)
            {
                same_n += 1;
                continue;
            }
            let tick = a.map(|e| e.tick).into_iter().chain(b.map(|e| e.tick)).min().unwrap_or(0);
            first = Some(Divergence { tick, kind: DivergenceKind::Event { index: k, original: a.cloned(), port: b.cloned() } });
            break;
        }
        let n = oe.len().max(pe.len()) as u32;
        groups.insert(format!("events:{kind}"), GroupDiff { group: format!("events:{kind}"), channels: n, agree: same_n, offset: 0, diverge: n - same_n, first, offsets: Vec::new(), note: String::new() });
    }
    // Mapped events (`event_map`): per label, paired in order per key, timing within `event_tol`.
    for g in mapped_event_groups(orig, port, event_tol) {
        groups.insert(g.group.clone(), g);
    }
    if let Some(&k) = d.missing_ticks.first() {
        groups.insert("frames".into(), GroupDiff { group: "frames".into(), channels: 1, agree: 0, offset: 0, diverge: 1, first: Some(Divergence { tick: k, kind: DivergenceKind::MissingFrame { side: "port".into() } }), offsets: Vec::new(), note: String::new() });
    }
    d.groups = groups.into_values().collect();
    // Earliest; on the same tick a channel before an event.
    d.first = d.groups.iter().filter_map(|g| g.first.clone()).min_by_key(|x| (x.tick, matches!(x.kind, DivergenceKind::Event { .. })));
    d
}

/// Earliest divergence of two traces with every channel judged absolute unless it starts
/// offset ([`compare_traces`] without `relative` prefixes, events at the same tick).
pub fn first_divergence(orig: &Trace, port: &Trace, tol: &dyn Fn(&str) -> f64) -> TraceDiff {
    compare_traces(orig, port, tol, &|_| false, 0)
}

/// Ticks within which two events of the same key may still be paired (a pair beyond the
/// tolerance is a timing divergence; beyond this window they are two unpaired events).
pub const EVENT_PAIR_WINDOW: u32 = 60;

/// Order-preserving pairing of two event tick lists (an alignment: fewest unpaired events, then
/// fewest pairs beyond `tol`); pairs only within `tol` + [`EVENT_PAIR_WINDOW`] ticks. Returns
/// (original index, port index) per aligned slot, `None` for an unpaired event.
pub fn align_events(a: &[u32], b: &[u32], tol: u32) -> Vec<(Option<usize>, Option<usize>)> {
    let (n, m) = (a.len(), b.len());
    let win = tol + EVENT_PAIR_WINDOW;
    // cost in half units: unpaired 2, paired beyond tol 1, paired within tol 0.
    let mut c = vec![vec![u32::MAX; m + 1]; n + 1];
    c[n][m] = 0;
    for i in (0..=n).rev() {
        for j in (0..=m).rev() {
            if i == n && j == m {
                continue;
            }
            let mut best = u32::MAX;
            if i < n {
                best = best.min(c[i + 1][j].saturating_add(2));
            }
            if j < m {
                best = best.min(c[i][j + 1].saturating_add(2));
            }
            if i < n && j < m {
                let d = a[i].abs_diff(b[j]);
                if d <= win {
                    best = best.min(c[i + 1][j + 1].saturating_add(if d <= tol { 0 } else { 1 }));
                }
            }
            c[i][j] = best;
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && a[i].abs_diff(b[j]) <= win && c[i][j] == c[i + 1][j + 1].saturating_add(if a[i].abs_diff(b[j]) <= tol { 0 } else { 1 }) {
            out.push((Some(i), Some(j)));
            i += 1;
            j += 1;
        } else if i < n && c[i][j] == c[i + 1][j].saturating_add(2) {
            out.push((Some(i), None));
            i += 1;
        } else {
            out.push((None, Some(j)));
            j += 1;
        }
    }
    out
}

/// The mapped events of both sides as `events:<label>` groups. Within a label, events are keyed
/// by [`crate::event_map::pair_key`] (the object where both sides name it, the behaviour state)
/// and paired by [`align_events`]; a pair agrees when the ticks differ by at most `event_tol` and, where
/// both carry a value (a damage amount), the values by at most 0.5. An unpaired event is a
/// divergence at its tick.
fn mapped_event_groups(orig: &Trace, port: &Trace, event_tol: u32) -> Vec<GroupDiff> {
    use crate::event_map::{CanonEvent, Side, canonical_events, pair_key};
    let (a, b) = (canonical_events(orig, Side::Original), canonical_events(port, Side::Port));
    let mut labels: Vec<String> = a.iter().chain(&b).map(|e| e.label.clone()).collect();
    labels.sort();
    labels.dedup();
    let as_event = |e: &CanonEvent| TraceEvent { tick: e.tick, kind: e.label.clone(), object: e.object.clone(), name: e.what.clone(), detail: if e.value.is_nan() { String::new() } else { format!("value={:.2}", e.value) } };
    let mut out = Vec::new();
    for label in labels {
        let (oa, pb): (Vec<&CanonEvent>, Vec<&CanonEvent>) = (a.iter().filter(|e| e.label == label).collect(), b.iter().filter(|e| e.label == label).collect());
        let mut keys: Vec<String> = oa.iter().chain(&pb).map(|e| pair_key(e)).collect();
        keys.sort();
        keys.dedup();
        let (mut n, mut agree, mut paired, mut dt_sum) = (0u32, 0u32, 0u32, 0i64);
        let mut first: Option<Divergence> = None;
        let mut note_first = |dv: Divergence| {
            if first.as_ref().is_none_or(|f| dv.tick < f.tick) {
                first = Some(dv);
            }
        };
        for key in &keys {
            let x: Vec<&&CanonEvent> = oa.iter().filter(|e| &pair_key(e) == key).collect();
            let y: Vec<&&CanonEvent> = pb.iter().filter(|e| &pair_key(e) == key).collect();
            for (i, (pi, qi)) in align_events(&x.iter().map(|e| e.tick).collect::<Vec<_>>(), &y.iter().map(|e| e.tick).collect::<Vec<_>>(), event_tol).into_iter().enumerate() {
                n += 1;
                let (p, q): (Option<&CanonEvent>, Option<&CanonEvent>) = (pi.map(|k| *x[k]), qi.map(|k| *y[k]));
                let ok = match (p, q) {
                    (Some(p), Some(q)) => {
                        paired += 1;
                        dt_sum += p.tick as i64 - q.tick as i64;
                        p.tick.abs_diff(q.tick) <= event_tol && (p.value.is_nan() || q.value.is_nan() || (p.value - q.value).abs() <= 0.5)
                    }
                    _ => false,
                };
                if ok {
                    agree += 1;
                } else {
                    let tick = p.map(|e| e.tick).into_iter().chain(q.map(|e| e.tick)).min().unwrap_or(0);
                    note_first(Divergence { tick, kind: DivergenceKind::Event { index: i, original: p.map(as_event), port: q.map(as_event) } });
                }
            }
        }
        let note = format!("original {}, port {}, paired {paired}{}", oa.len(), pb.len(), if paired > 0 { format!(", mean tick offset (original - port) {:+.2}", dt_sum as f64 / paired as f64) } else { String::new() });
        out.push(GroupDiff { group: format!("events:{label}"), channels: n, agree, offset: 0, diverge: n - agree, first, offsets: Vec::new(), note });
    }
    out
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
        // Event and channel metrics of the scenario library.
        _ => crate::scenario::extra_metric(t, m),
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
    fn start_offset_reported_once_and_groups_independent() {
        // The original hovers 3 uu lower all along; its vertical speed differs at tick 3.
        let mk = |side: &str, z0: f64, vz3: f64| {
            let mut t = Trace::new("s", side, 60, &[("pawn.z", "uu"), ("pawn.vz", "uu/s"), ("npc.g.player_attn.value", "")]);
            for k in 0..5u32 {
                t.push(k, vec![z0 + 10.0 * k as f64, if k == 3 { vz3 } else { 0.0 }, 0.1 * k as f64]);
            }
            t
        };
        let o = mk("original", -217.567, 5.0);
        let mut p = mk("port", -214.567, 0.0);
        // The port names the attention channel `npc.g.attn`.
        p.channels[2].name = "npc.g.attn".into();
        let d = compare_traces(&o, &p, &|_| 0.1, &|_| false, 0);
        let z = d.channels.iter().find(|c| c.channel == "pawn.z").unwrap();
        assert_eq!((z.mode, z.verdict), (ChannelMode::SteadyOffset, ChannelVerdict::Offset));
        assert!((z.start_offset + 3.0).abs() < 1e-9);
        let pos = d.groups.iter().find(|g| g.group == "pawn position").unwrap();
        assert!(pos.first.is_none() && pos.offsets.len() == 1);
        let vel = d.groups.iter().find(|g| g.group == "pawn velocity").unwrap();
        assert!(matches!(vel.first, Some(Divergence { tick: 3, .. })));
        assert_eq!(d.first.as_ref().map(|x| x.tick), Some(3));
        let attn = d.channels.iter().find(|c| c.channel == "npc.g.attn").unwrap();
        assert_eq!((attn.verdict, attn.alias.as_deref(), attn.group.as_str()), (ChannelVerdict::Agree, Some("npc.g.player_attn.value"), "behaviour"));
        assert_eq!(d.channel_counts(), (1, 1, 1));
        // A steady offset reached after a transient (the port settles 1.8 uu higher after two
        // ticks of falling): the offset is removed and only the transient is a divergence.
        let mut a = Trace::new("s", "original", 60, &[("pawn.z", "uu")]);
        let mut b = Trace::new("s", "port", 60, &[("pawn.z", "uu")]);
        for (k, pz) in [0.0, -0.4, -1.6, 1.8, 1.8, 1.8, 1.8, 14.7].iter().enumerate() {
            a.push(k as u32, vec![if k == 7 { 12.9 } else { 0.0 }]);
            b.push(k as u32, vec![*pz]);
        }
        let d = compare_traces(&a, &b, &|_| 0.1, &|_| false, 0);
        let z = &d.channels[0];
        assert_eq!(z.mode, ChannelMode::SteadyOffset);
        assert!((z.offset + 1.8).abs() < 1e-9);
        assert_eq!(z.spans, vec![(0, 2)]);
        // `relative` judges a channel by its displacement even when the start agrees.
        let d = compare_traces(&o, &o, &|_| 0.1, &|c| c == "pawn.z", 0);
        assert_eq!(d.channels[0].mode, ChannelMode::Relative);
        // (The attention channel has no port name on an original-only pair.)
        assert_eq!(d.channel_counts(), (2, 0, 0));
    }

    #[test]
    fn event_alignment() {
        // The port misses the first of three shots; the others are a tick late.
        assert_eq!(align_events(&[161, 311, 461], &[312, 462], 0), vec![(Some(0), None), (Some(1), Some(0)), (Some(2), Some(1))]);
        // Beyond the window: unpaired on both sides.
        assert_eq!(align_events(&[33], &[200], 0), vec![(Some(0), None), (None, Some(0))]);
        assert_eq!(align_events(&[], &[5], 0), vec![(None, Some(0))]);
    }

    #[test]
    fn checkpoints() {
        let t = trace("port", &[0.0, 50.0, 106.6, 80.0], &[]);
        let cps = vec![Checkpoint {
            name: "apex".into(),
            expect: vec![
                Expect { metric: Metric::Rise("pawn.z".into()), value: 106.67, tol: 0.5, source: "m".into(), cmp: Default::default(), status: Default::default(), name: None },
                Expect { metric: Metric::TimeToReach("pawn.z".into(), 100.0), value: 2.0 / 60.0, tol: 1e-6, source: "m".into(), cmp: Default::default(), status: Default::default(), name: None },
                Expect { metric: Metric::At("pawn.vz".into(), 1), value: 0.0, tol: 1.0, source: "m".into(), cmp: Default::default(), status: Default::default(), name: None },
            ],
        }];
        let r = check_checkpoints(&t, &cps);
        assert!(r[0].pass && r[1].pass);
        assert!(!r[2].pass && r[2].actual.is_none());
    }
}
