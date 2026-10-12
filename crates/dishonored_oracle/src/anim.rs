//! Skeletal animation as a residual (research/dishonored/ORACLE.md, "Skeletal animation: bones
//! and AnimTree state per tick" and "Animation residuals"): one pawn's per-tick AnimTree node
//! states and component-space bones, the same shape for the original (the `Bones` / `AnimState`
//! capture sidecar, `anim-<spec hash>.json`) and the port (the scenario runner samples the
//! simulation's NPC tree), and the comparisons the scenario metrics and the scorecard read.
//!
//! - Nodes are numbered as the capture numbers them: the cooked `TickArrayIndex` (0 is the tree
//!   object); a sequence node carries its sequence, time and rate, every node its
//!   `NodeTotalWeight`; blend nodes their children weights.
//! - Bones are compared by name, as rotations (the angle between the two component-space
//!   quaternions, in degrees), and only where the capture's value is fresh: UE3 recomposes
//!   `SpaceBases` for the bones the mesh's current LOD requires and not at all for a mesh whose
//!   tree does not tick ([`parse_capture`]).
//! - Bone groups are those of `dishonored_anim`'s replay test (`tests/npc_human_replay.rs`):
//!   `head` (`head_jnt` and below), `legs` (`upper_leg_*` and below), `hands` (below
//!   `hand_L_jnt` / `hand_R_jnt`) and `body` (everything else).
//! - The lag is the tick offset `capture tick = port tick + lag` that minimizes the body mean
//!   over -30..+30 ticks, as `oracle anim compare` searches it: a negative lag is a port that
//!   shows at tick `t` what the original showed at `t + lag`, behind it.

use crate::scenario::Metric;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A sequence node's player (`AnimSeqName`, `CurrentTime`, `Rate`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SeqSample {
    pub name: String,
    pub time: f32,
    pub rate: f32,
}

/// One node's state in a tick.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeSample {
    /// `NodeTotalWeight`.
    pub weight: f32,
    pub relevant: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<SeqSample>,
}

/// A component-space bone: quaternion (x, y, z, w) and translation (uu).
pub type Bone = ([f32; 4], [f32; 3]);

/// One tick of a pawn's animation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimFrame {
    pub tick: u32,
    /// By node number (`TickArrayIndex`).
    #[serde(default)]
    pub nodes: BTreeMap<u32, NodeSample>,
    /// Children weights of the blend nodes, by node number.
    #[serde(default)]
    pub blend: BTreeMap<u32, Vec<f32>>,
    /// The fresh bones of the tick, by index into [`AnimTrack::bones`].
    #[serde(default)]
    pub bones: BTreeMap<u32, Bone>,
}

/// A pawn's animation over a run, frames in tick order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimTrack {
    pub bones: Vec<String>,
    pub frames: Vec<AnimFrame>,
}

impl AnimTrack {
    pub fn frame(&self, tick: u32) -> Option<&AnimFrame> {
        self.frames.binary_search_by_key(&tick, |f| f.tick).ok().map(|i| &self.frames[i])
    }

    pub fn bone(&self, name: &str) -> Option<u32> {
        self.bones.iter().position(|b| b.eq_ignore_ascii_case(name)).map(|i| i as u32)
    }

    /// Bones fresh in each frame, by name (the frames are sparse where the capture is stale).
    pub fn fresh_ticks(&self) -> usize {
        self.frames.iter().filter(|f| !f.bones.is_empty()).count()
    }
}

/// The tree's structure in the capture's numbering, from the port's cooked tree (both sides play
/// `NPC_Human_at`): node names and, per node, the sequence nodes of its subtree (a sequence node
/// lists itself; the look-at node, recorded by the capture as a sequence node, is not one).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TreeLayout {
    /// Lowercase `NodeName` -> node number.
    pub names: BTreeMap<String, u32>,
    pub below: BTreeMap<u32, Vec<u32>>,
}

impl TreeLayout {
    /// A node by number or name.
    pub fn resolve(&self, node: &str) -> Option<u32> {
        node.trim().parse::<u32>().ok().or_else(|| self.names.get(&node.trim().to_ascii_lowercase()).copied())
    }
}

/// The bone groups of `dishonored_anim`'s replay test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BoneGroup {
    Body,
    Legs,
    Head,
    Hands,
}

impl BoneGroup {
    pub const ALL: [BoneGroup; 4] = [BoneGroup::Body, BoneGroup::Legs, BoneGroup::Hands, BoneGroup::Head];

    pub fn name(self) -> &'static str {
        match self {
            BoneGroup::Body => "body",
            BoneGroup::Legs => "legs",
            BoneGroup::Head => "head",
            BoneGroup::Hands => "hands",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.name().eq_ignore_ascii_case(s.trim()))
    }
}

/// The group of every bone of a skeleton (`parents[i]`: `None` for a root): `head_jnt` starts
/// the head, `upper_leg_*` the legs, a child of `hand_L_jnt` / `hand_R_jnt` the hands; every
/// other bone takes its parent's group.
pub fn bone_groups(names: &[String], parents: &[Option<usize>]) -> Vec<BoneGroup> {
    let mut g = vec![BoneGroup::Body; names.len()];
    for i in 0..names.len() {
        let n = names[i].to_ascii_lowercase();
        let parent = parents.get(i).copied().flatten().filter(|p| *p < i);
        g[i] = if n == "head_jnt" {
            BoneGroup::Head
        } else if n.starts_with("upper_leg_") {
            BoneGroup::Legs
        } else {
            match parent {
                Some(p) if names[p].eq_ignore_ascii_case("hand_l_jnt") || names[p].eq_ignore_ascii_case("hand_r_jnt") => BoneGroup::Hands,
                Some(p) => g[p],
                None => BoneGroup::Body,
            }
        };
    }
    g
}

/// Which bones a metric reads: a group, every bone, or a list of names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoneSel {
    Group(BoneGroup),
    All,
    Names(Vec<String>),
}

impl BoneSel {
    /// `body` / `legs` / `hands` / `head`, `all`, or bone names separated by commas.
    pub fn parse(s: &str) -> Self {
        if let Some(g) = BoneGroup::parse(s) {
            return BoneSel::Group(g);
        }
        if s.trim().eq_ignore_ascii_case("all") {
            return BoneSel::All;
        }
        BoneSel::Names(s.split(',').map(|b| b.trim().to_ascii_lowercase()).filter(|b| !b.is_empty()).collect())
    }

    fn wants(&self, name: &str, groups: &HashMap<String, BoneGroup>) -> bool {
        match self {
            BoneSel::Group(g) => groups.get(&name.to_ascii_lowercase()) == Some(g),
            BoneSel::All => true,
            BoneSel::Names(n) => n.iter().any(|b| b.eq_ignore_ascii_case(name)),
        }
    }
}

/// One subject's animation on both sides.
#[derive(Debug, Clone, Default)]
pub struct AnimSubject {
    pub capture: Option<AnimTrack>,
    pub port: Option<AnimTrack>,
    pub layout: Option<TreeLayout>,
    /// Lowercase bone name -> group (the port's skeleton).
    pub groups: HashMap<String, BoneGroup>,
}

/// Every recorded subject of a run (`Bones` / `AnimState` records), by subject.
pub type AnimData = BTreeMap<String, AnimSubject>;

/// Angle between two quaternions (degrees, either sign of either), in double precision (`acos`
/// of an f32 dot product near 1 is only good to about 0.02 deg).
pub fn angle_deg(a: [f32; 4], b: [f32; 4]) -> f32 {
    let n = |q: [f32; 4]| {
        let q = q.map(|x| x as f64);
        let l = q.iter().map(|x| x * x).sum::<f64>().sqrt();
        if l > 0.0 { q.map(|x| x / l) } else { [0.0, 0.0, 0.0, 1.0] }
    };
    let (a, b) = (n(a), n(b));
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.0);
    (2.0 * d.acos().to_degrees()) as f32
}

/// Angle statistics over (tick, bone) samples.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AngleStats {
    pub sum: f64,
    pub max: f32,
    pub samples: usize,
    pub ticks: usize,
}

impl AngleStats {
    pub fn mean(&self) -> Option<f64> {
        (self.samples > 0).then(|| self.sum / self.samples as f64)
    }
}

/// The bone residual over the port's ticks `from..=to`: each against the capture's frame at
/// `tick + lag`, over the selected bones fresh in that capture frame and present in the port's.
pub fn bone_residual(capture: &AnimTrack, port: &AnimTrack, groups: &HashMap<String, BoneGroup>, sel: &BoneSel, from: u32, to: u32, lag: i32) -> AngleStats {
    // Capture bone index -> port bone index, for the selected bones.
    let map: HashMap<u32, u32> = capture.bones.iter().enumerate().filter(|(_, n)| sel.wants(n, groups)).filter_map(|(i, n)| port.bone(n).map(|p| (i as u32, p))).collect();
    let mut s = AngleStats::default();
    for pf in port.frames.iter().filter(|f| f.tick >= from && f.tick <= to) {
        let ct = pf.tick as i64 + lag as i64;
        let Some(cf) = u32::try_from(ct).ok().and_then(|t| capture.frame(t)) else { continue };
        let mut any = false;
        for (ci, cb) in &cf.bones {
            let Some(pb) = map.get(ci).and_then(|p| pf.bones.get(p)) else { continue };
            let a = angle_deg(cb.0, pb.0);
            s.sum += a as f64;
            s.max = s.max.max(a);
            s.samples += 1;
            any = true;
        }
        s.ticks += any as usize;
    }
    s
}

/// The search range of [`best_lag`] (ticks each way), as `oracle anim compare`'s.
pub const LAG_RANGE: i32 = 30;

/// The lag that minimizes the body mean over the whole run, tried from 0 outwards (a later lag
/// replaces an earlier one only when strictly better); a lag compares at least half the ticks
/// lag 0 compares.
pub fn best_lag(capture: &AnimTrack, port: &AnimTrack, groups: &HashMap<String, BoneGroup>) -> Option<(i32, AngleStats)> {
    let sel = BoneSel::Group(BoneGroup::Body);
    let at = |lag: i32| bone_residual(capture, port, groups, &sel, 0, u32::MAX, lag);
    let zero = at(0);
    let min_ticks = zero.ticks.div_ceil(2).max(1);
    let mut best: Option<(i32, AngleStats)> = None;
    for k in 0..=LAG_RANGE {
        for lag in if k == 0 { vec![0] } else { vec![-k, k] } {
            let s = if lag == 0 { zero } else { at(lag) };
            let (Some(m), true) = (s.mean(), s.ticks >= min_ticks) else { continue };
            if best.as_ref().is_none_or(|b| m < b.1.mean().unwrap_or(f64::MAX) - 1e-9) {
                best = Some((lag, s));
            }
        }
    }
    best
}

/// The sequence playing on a node in a frame: on a sequence node its own (while it weighs
/// anything), on another node the heaviest sequence of its subtree ([`TreeLayout::below`]).
pub fn playing<'a>(frame: &'a AnimFrame, layout: Option<&TreeLayout>, node: u32) -> Option<&'a SeqSample> {
    let live = |k: &u32| frame.nodes.get(k).filter(|n| n.weight > 0.0).and_then(|n| n.seq.as_ref().map(|s| (n.weight, s))).filter(|(_, s)| !s.name.is_empty() && !s.name.eq_ignore_ascii_case("None"));
    let own = frame.nodes.get(&node).is_some_and(|n| n.seq.is_some()) && layout.is_none_or(|l| l.below.get(&node).is_some_and(|b| b == &[node]));
    if own {
        return live(&node).map(|x| x.1);
    }
    let below = layout?.below.get(&node)?;
    let mut best: Option<(f32, &SeqSample)> = None;
    for k in below {
        if let Some((w, s)) = live(k) {
            if best.is_none_or(|b| w > b.0) {
                best = Some((w, s));
            }
        }
    }
    best.map(|b| b.1)
}

/// A metric's value: a number, or a name (`AnimSeq`).
#[derive(Debug, Clone, PartialEq)]
pub enum AnimValue {
    Num(f64),
    Name(String),
}

/// Whether a metric reads the animation records.
pub fn is_anim(m: &Metric) -> bool {
    matches!(m, Metric::AnimBoneMean(..) | Metric::AnimBoneMax(..) | Metric::AnimSeq(..) | Metric::AnimSeqTime(..) | Metric::AnimLag(..))
}

/// The value of an animation metric on the port side (the bone metrics and the lag against the
/// capture); `None` when its records are missing.
pub fn metric_value(m: &Metric, data: &AnimData) -> Option<AnimValue> {
    side_value(m, data, false)
}

/// The value of `AnimSeq` / `AnimSeqTime` read from the capture instead (the reference the spec
/// states; the other metrics compare both sides and have no capture-only value).
pub fn capture_value(m: &Metric, data: &AnimData) -> Option<AnimValue> {
    match m {
        Metric::AnimSeq(..) | Metric::AnimSeqTime(..) => side_value(m, data, true),
        _ => None,
    }
}

fn side_value(m: &Metric, data: &AnimData, capture_side: bool) -> Option<AnimValue> {
    let pick = |s: &str| data.get(s).or_else(|| data.iter().find(|(k, _)| k.eq_ignore_ascii_case(s)).map(|x| x.1));
    match m {
        Metric::AnimBoneMean(s, bones, from, to) | Metric::AnimBoneMax(s, bones, from, to) => {
            let sub = pick(s)?;
            let st = bone_residual(sub.capture.as_ref()?, sub.port.as_ref()?, &sub.groups, &BoneSel::parse(bones), *from, *to, 0);
            if matches!(m, Metric::AnimBoneMean(..)) { st.mean() } else { (st.samples > 0).then_some(st.max as f64) }.map(AnimValue::Num)
        }
        Metric::AnimLag(s) => {
            let sub = pick(s)?;
            best_lag(sub.capture.as_ref()?, sub.port.as_ref()?, &sub.groups).map(|b| AnimValue::Num(b.0 as f64))
        }
        Metric::AnimSeq(s, node, tick) | Metric::AnimSeqTime(s, node, tick) => {
            let sub = pick(s)?;
            let track = if capture_side { sub.capture.as_ref()? } else { sub.port.as_ref()? };
            let layout = sub.layout.as_ref();
            let k = match layout {
                Some(l) => l.resolve(node)?,
                None => node.trim().parse().ok()?,
            };
            let seq = playing(track.frame(*tick)?, layout, k);
            match m {
                Metric::AnimSeq(..) => Some(AnimValue::Name(seq.map_or_else(|| "None".to_string(), |q| q.name.clone()))),
                _ => seq.map(|q| AnimValue::Num(q.time as f64)),
            }
        }
        _ => None,
    }
}

/// The node every summary names the full-body animation by.
pub const FULL_BODY_NODE: &str = "FullBodySlot";

/// One group's residual at lag 0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GroupResidual {
    pub mean: f64,
    pub max: f32,
    pub samples: usize,
}

/// A subject's animation residual over the run (the scorecard's `anim` column).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimSummary {
    pub subject: String,
    /// Ticks recorded on each side, ticks with fresh captured bones.
    pub capture_ticks: usize,
    pub port_ticks: usize,
    pub fresh_ticks: usize,
    /// Per group at lag 0 (`body`, `legs`, `hands`, `head`).
    pub groups: BTreeMap<String, GroupResidual>,
    /// The best lag and the body mean there.
    pub lag: Option<i32>,
    pub lag_body_mean: Option<f64>,
    /// What plays on `FullBodySlot` (its heaviest sequence) on each side: sequence and ticks,
    /// most first.
    pub capture_plays: Vec<(String, usize)>,
    pub port_plays: Vec<(String, usize)>,
    /// Ticks where both sides play the same full-body sequence, of the ticks both record.
    pub same_sequence: (usize, usize),
    pub notes: Vec<String>,
}

impl AnimSummary {
    pub fn body(&self) -> Option<&GroupResidual> {
        self.groups.get("body")
    }

    /// `body 0.012 / 0.30 deg, lag 0` (the scorecard cell).
    pub fn cell(&self) -> String {
        let body = self.body().map(|b| format!("body {:.3} / {:.2} deg", b.mean, b.max)).unwrap_or_else(|| "body -".into());
        let lag = match (self.lag, self.lag_body_mean) {
            (Some(l), Some(m)) => format!("lag {l:+} ({m:.3})"),
            _ => "lag -".into(),
        };
        format!("{}: {body}, {lag}", self.subject)
    }
}

fn plays(track: &AnimTrack, layout: Option<&TreeLayout>) -> (Vec<(String, usize)>, BTreeMap<u32, String>) {
    let Some(node) = layout.and_then(|l| l.resolve(FULL_BODY_NODE)) else { return (Vec::new(), BTreeMap::new()) };
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_tick = BTreeMap::new();
    for f in &track.frames {
        let name = playing(f, layout, node).map_or_else(|| "None".to_string(), |s| s.name.clone());
        *count.entry(name.clone()).or_default() += 1;
        by_tick.insert(f.tick, name);
    }
    let mut v: Vec<(String, usize)> = count.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    (v, by_tick)
}

/// The residual summary of one subject.
pub fn summarize(subject: &str, s: &AnimSubject) -> AnimSummary {
    let mut out = AnimSummary { subject: subject.to_string(), ..Default::default() };
    let layout = s.layout.as_ref();
    let mut cap_ticks = BTreeMap::new();
    let mut port_ticks = BTreeMap::new();
    if let Some(c) = &s.capture {
        out.capture_ticks = c.frames.len();
        out.fresh_ticks = c.fresh_ticks();
        (out.capture_plays, cap_ticks) = plays(c, layout);
    } else {
        out.notes.push("no capture sidecar for this spec hash".into());
    }
    if let Some(p) = &s.port {
        out.port_ticks = p.frames.len();
        (out.port_plays, port_ticks) = plays(p, layout);
    } else {
        out.notes.push("no port tree sampled".into());
    }
    let both = cap_ticks.iter().filter_map(|(t, n)| port_ticks.get(t).map(|p| p == n));
    out.same_sequence = both.fold((0, 0), |(a, b), same| (a + same as usize, b + 1));
    if let (Some(c), Some(p)) = (&s.capture, &s.port) {
        for g in BoneGroup::ALL {
            let st = bone_residual(c, p, &s.groups, &BoneSel::Group(g), 0, u32::MAX, 0);
            if let Some(mean) = st.mean() {
                out.groups.insert(g.name().to_string(), GroupResidual { mean, max: st.max, samples: st.samples });
            }
        }
        if let Some((lag, st)) = best_lag(c, p, &s.groups) {
            out.lag = Some(lag);
            out.lag_body_mean = st.mean();
            if lag.abs() == LAG_RANGE {
                out.notes.push(format!("the best lag is at the edge of the search (+-{LAG_RANGE} ticks): the two sides do not align in range"));
            }
        }
    }
    out
}

// ---- the capture sidecar ----

#[derive(Deserialize)]
struct CaptureFile {
    meshes: Vec<CaptureMesh>,
}

#[derive(Deserialize)]
struct CaptureMesh {
    #[serde(default)]
    bones: Vec<(u32, String)>,
    #[serde(default)]
    frames: Vec<CaptureFrame>,
}

#[derive(Deserialize, Default)]
struct CaptureFrame {
    #[serde(default)]
    t: Option<u32>,
    #[serde(default)]
    cs: Option<BTreeMap<String, Vec<f64>>>,
    #[serde(default)]
    nodes: Option<BTreeMap<String, Vec<serde_json::Value>>>,
    #[serde(default)]
    blend: Option<BTreeMap<String, Vec<f64>>>,
}

/// A capture sidecar (`anim-<spec hash>.json`, ORACLE.md's format: delta-encoded frames, `cs`
/// quaternions x 1e4 and translations x 100, node weights and times x 1e3) as a track with every
/// node state carried forward and only the fresh bones in each frame. The bones are the first
/// mesh's that has any, the node states the first mesh's whose frames carry any (one subject per
/// capture today).
///
/// Fresh bones (`dishonored_anim`'s replay test, extended): the reduced LOD's set (the bones the
/// ticks that rewrite fewer than half the skeleton rewrite) and the bones never rewritten after
/// the first frame are fresh on a tick that rewrites anything; a tick that rewrites a bone outside
/// that set is a full-LOD tick (every bone fresh); a tick that rewrites nothing recomposed nothing
/// (a mesh that is not rendered does not tick its tree: `ai/boyle_patrol_speed` ticks 3-1008) and
/// has no fresh bone.
pub fn parse_capture(text: &str) -> Result<AnimTrack> {
    let file: CaptureFile = serde_json::from_str(text).context("capture sidecar JSON")?;
    let bones_mesh = file.meshes.iter().find(|m| !m.bones.is_empty());
    let nodes_mesh = file.meshes.iter().find(|m| m.frames.iter().any(|f| f.nodes.is_some()));
    let mut names: Vec<String> = Vec::new();
    if let Some(m) = bones_mesh {
        let n = m.bones.iter().map(|b| b.0 as usize + 1).max().unwrap_or(0);
        names = vec![String::new(); n];
        for (i, b) in &m.bones {
            names[*i as usize] = b.clone();
        }
    }
    let nb = names.len();
    // The bone values carried forward and each frame's delta.
    let mut frames: BTreeMap<u32, AnimFrame> = BTreeMap::new();
    if let Some(m) = bones_mesh {
        let mut cur: Vec<Option<Bone>> = vec![None; nb];
        let mut deltas: Vec<(u32, BTreeSet<u32>, Vec<Option<Bone>>)> = Vec::new();
        for (k, f) in m.frames.iter().enumerate() {
            let tick = f.t.unwrap_or(k as u32);
            let mut d = BTreeSet::new();
            for (key, v) in f.cs.iter().flatten() {
                let (Ok(i), true) = (key.parse::<u32>(), v.len() >= 7) else { continue };
                if (i as usize) < nb {
                    cur[i as usize] = Some(([v[0] as f32 / 1e4, v[1] as f32 / 1e4, v[2] as f32 / 1e4, v[3] as f32 / 1e4], [v[4] as f32 / 100.0, v[5] as f32 / 100.0, v[6] as f32 / 100.0]));
                    d.insert(i);
                }
            }
            deltas.push((tick, d, cur.clone()));
        }
        let mut reduced = vec![false; nb];
        let mut ever = vec![false; nb];
        for (_, d, _) in deltas.iter().skip(1) {
            for b in d {
                ever[*b as usize] = true;
                if d.len() < nb / 2 {
                    reduced[*b as usize] = true;
                }
            }
        }
        let base: Vec<bool> = (0..nb).map(|b| reduced[b] || !ever[b]).collect();
        for (k, (tick, d, vals)) in deltas.into_iter().enumerate() {
            let fresh: Vec<bool> = if k > 0 && d.is_empty() {
                vec![false; nb]
            } else {
                let full = k > 0 && d.iter().any(|b| !base[*b as usize]);
                (0..nb).map(|b| base[b] || full).collect()
            };
            let f = frames.entry(tick).or_insert_with(|| AnimFrame { tick, ..Default::default() });
            for (b, v) in vals.into_iter().enumerate() {
                if let (true, Some(v)) = (fresh[b], v) {
                    f.bones.insert(b as u32, v);
                }
            }
        }
    }
    if let Some(m) = nodes_mesh {
        let mut nodes: BTreeMap<u32, NodeSample> = BTreeMap::new();
        let mut blend: BTreeMap<u32, Vec<f32>> = BTreeMap::new();
        let num = |v: &serde_json::Value| v.as_f64().unwrap_or(0.0) as f32;
        for (k, f) in m.frames.iter().enumerate() {
            let tick = f.t.unwrap_or(k as u32);
            for (key, v) in f.nodes.iter().flatten() {
                let (Ok(i), Some(w)) = (key.parse::<u32>(), v.first()) else { continue };
                let seq = (v.len() >= 5).then(|| SeqSample { name: v[2].as_str().unwrap_or("None").to_string(), time: num(&v[3]) / 1e3, rate: num(&v[4]) / 1e3 });
                nodes.insert(i, NodeSample { weight: num(w) / 1e3, relevant: v.get(1).is_some_and(|r| num(r) != 0.0), seq });
            }
            for (key, v) in f.blend.iter().flatten() {
                if let Ok(i) = key.parse::<u32>() {
                    blend.insert(i, v.iter().map(|x| *x as f32 / 1e3).collect());
                }
            }
            let fr = frames.entry(tick).or_insert_with(|| AnimFrame { tick, ..Default::default() });
            fr.nodes = nodes.clone();
            fr.blend = blend.clone();
        }
    }
    Ok(AnimTrack { bones: names, frames: frames.into_values().collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q_z(deg: f32) -> [f32; 4] {
        let h = (deg.to_radians() * 0.5).sin_cos();
        [0.0, 0.0, h.0, h.1]
    }

    fn track(bones: &[&str], ticks: std::ops::Range<u32>, f: impl Fn(u32, usize) -> f32) -> AnimTrack {
        AnimTrack {
            bones: bones.iter().map(|s| s.to_string()).collect(),
            frames: ticks.map(|t| AnimFrame { tick: t, bones: (0..bones.len()).map(|b| (b as u32, (q_z(f(t, b)), [0.0; 3]))).collect(), ..Default::default() }).collect(),
        }
    }

    #[test]
    fn groups_follow_the_replay_test() {
        let names: Vec<String> = ["root0_jnt", "spine_0_jnt", "head_jnt", "jaw_jnt", "upper_leg_L_jnt", "foot_L_jnt", "hand_L_jnt", "index_0_L_jnt", "index_1_L_jnt"].iter().map(|s| s.to_string()).collect();
        let parents = vec![None, Some(0), Some(1), Some(2), Some(0), Some(4), Some(1), Some(6), Some(7)];
        let g = bone_groups(&names, &parents);
        use BoneGroup::*;
        assert_eq!(g, vec![Body, Body, Head, Head, Legs, Legs, Body, Hands, Hands]);
        assert_eq!(BoneSel::parse("legs"), BoneSel::Group(Legs));
        assert_eq!(BoneSel::parse("Head_jnt, jaw_jnt"), BoneSel::Names(vec!["head_jnt".into(), "jaw_jnt".into()]));
    }

    #[test]
    fn residual_and_lag() {
        // The port is the capture three ticks late: the port at tick t shows the capture's t - 3.
        let cap = track(&["a", "b"], 0..60, |t, b| t as f32 * 2.0 + b as f32);
        let port = track(&["b", "a"], 0..60, |t, b| (t as f32 - 3.0) * 2.0 + (1 - b) as f32);
        let groups: HashMap<String, BoneGroup> = [("a".to_string(), BoneGroup::Body), ("b".to_string(), BoneGroup::Body)].into();
        let at0 = bone_residual(&cap, &port, &groups, &BoneSel::Group(BoneGroup::Body), 0, 59, 0);
        assert!((at0.mean().unwrap() - 6.0).abs() < 1e-3, "{at0:?}");
        assert_eq!(at0.ticks, 60);
        let (lag, st) = best_lag(&cap, &port, &groups).unwrap();
        assert_eq!(lag, -3);
        assert!(st.mean().unwrap() < 1e-3);
        let only_a = bone_residual(&cap, &port, &groups, &BoneSel::parse("a"), 10, 19, 0);
        assert_eq!((only_a.samples, only_a.ticks), (10, 10));
    }

    #[test]
    fn playing_resolves_slots_by_their_subtree() {
        let seq = |name: &str, w: f32, t: f32| NodeSample { weight: w, relevant: w > 0.0, seq: Some(SeqSample { name: name.into(), time: t, rate: 1.0 }) };
        let mut f = AnimFrame { tick: 0, ..Default::default() };
        f.nodes.insert(11, NodeSample { weight: 1.0, relevant: true, seq: None });
        f.nodes.insert(13, seq("Sword_Idle", 0.25, 0.1));
        f.nodes.insert(22, seq("Sword_Attack_StepBackA", 0.75, 0.2));
        f.nodes.insert(23, seq("None", 0.0, 0.0));
        let layout = TreeLayout { names: [("fullbodyslot".to_string(), 11)].into(), below: [(11, vec![13, 22, 23]), (13, vec![13]), (22, vec![22]), (23, vec![23])].into() };
        let k = layout.resolve("FullBodySlot").unwrap();
        assert_eq!(playing(&f, Some(&layout), k).map(|s| s.name.as_str()), Some("Sword_Attack_StepBackA"));
        assert_eq!(playing(&f, Some(&layout), 13).map(|s| s.name.as_str()), Some("Sword_Idle"));
        assert!(playing(&f, Some(&layout), 23).is_none());
        let data: AnimData = [("guard".to_string(), AnimSubject { port: Some(AnimTrack { bones: Vec::new(), frames: vec![f] }), layout: Some(layout), ..Default::default() })].into();
        assert_eq!(metric_value(&Metric::AnimSeq("guard".into(), "FullBodySlot".into(), 0), &data), Some(AnimValue::Name("Sword_Attack_StepBackA".into())));
        assert_eq!(metric_value(&Metric::AnimSeqTime("guard".into(), "22".into(), 0), &data).map(|v| matches!(v, AnimValue::Num(t) if (t - 0.2).abs() < 1e-6)), Some(true));
        assert_eq!(metric_value(&Metric::AnimSeq("guard".into(), "FullBodySlot".into(), 5), &data), None);
    }

    /// The sidecar's delta frames: nodes carried forward, bones fresh by the LOD rule, a tick
    /// that rewrites nothing has no fresh bone.
    #[test]
    fn capture_freshness() {
        // Six bones; ticks 1-2 rewrite the reduced set {0, 1}; tick 3 rewrites half the skeleton
        // with bones 4 and 5 (full LOD); tick 4 rewrites nothing.
        let text = r#"{"meshes": [{"bones": [[0, "a"], [1, "b"], [2, "c"], [3, "d"], [4, "e"], [5, "f"]], "frames": [
            {"t": 0, "cs": {"0": [0,0,0,10000,0,0,0], "1": [0,0,0,10000,0,0,0], "2": [0,0,0,10000,0,0,0], "3": [0,0,0,10000,0,0,0], "4": [0,0,0,10000,0,0,0], "5": [0,0,0,10000,0,0,0]},
             "nodes": {"22": [1000, 1, "Empty_Idle_04", 67, 1000], "11": [1000, 1]}, "blend": {"11": [0, 1000]}},
            {"t": 1, "cs": {"0": [0,0,87,9999,0,0,0], "1": [0,0,87,9999,0,0,0]}, "nodes": {"22": [1000, 1, "Empty_Idle_04", 83, 1000]}},
            {"t": 2, "cs": {"0": [0,0,0,10000,0,0,0]}},
            {"t": 3, "cs": {"0": [0,0,0,10000,0,0,0], "4": [0,0,87,9999,0,0,0], "5": [0,0,87,9999,0,0,0]}},
            {"t": 4}
        ]}, {"bones": [], "frames": []}]}"#;
        let tr = parse_capture(text).unwrap();
        assert_eq!(tr.bones.len(), 6);
        let keys = |t: u32| tr.frame(t).unwrap().bones.keys().copied().collect::<Vec<u32>>();
        // Bones 2 and 3 are never rewritten after frame 0 (constant: fresh), 4 and 5 are outside
        // the reduced set.
        assert_eq!(keys(0), vec![0, 1, 2, 3]);
        assert_eq!(keys(1), vec![0, 1, 2, 3]);
        assert_eq!(keys(2), vec![0, 1, 2, 3]);
        assert_eq!(keys(3), vec![0, 1, 2, 3, 4, 5]);
        assert!(keys(4).is_empty());
        let n = &tr.frame(2).unwrap().nodes[&22];
        assert_eq!(n.seq.as_ref().map(|s| (s.name.as_str(), s.time)), Some(("Empty_Idle_04", 0.083)));
        assert_eq!(tr.frame(4).unwrap().blend[&11], vec![0.0, 1.0]);
    }
}
