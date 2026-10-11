//! `NPC_Human_at` evaluated against the original (skipped without the install or the capture):
//! the oracle capture of `stealth/detect_1300` (research/dishonored/ORACLE.md, "Skeletal
//! animation: bones and AnimTree state per tick") records, for the Boyle party elite guard
//! standing still for 201 ticks, every AnimTree node's state and the component-space bones.
//!
//! 1. Replay: every node's captured state (weights, sequences, times, rates) driven in through
//!    the replay API, the pose evaluated and compared with the captured bones.
//! 2. Inputs: the same window produced by the evaluator's own tick from the requests the
//!    gameplay layer makes (the ambient idle on `FullBodySlot`, its face sequence on `FaceSlot`
//!    with the face filter, the blinks, locomotion at rest), compared the same way, node states
//!    included.
//!
//! The capture reads `SpaceBases`, which UE3 recomposes only for the bones the mesh's current
//! LOD requires (`ComposeSkeleton` over `RequiredBones`): on most ticks only 37 bones (spine,
//! limbs, `head_jnt` / `head_end_jnt` / `camera_jnt`, no fingers but `middle_0`, no face) are
//! rewritten and the others keep their value from the last full-LOD tick. A bone is compared on
//! a tick only when its captured value is fresh: it belongs to that reduced set, or the tick is a
//! full-LOD tick (its delta rewrites a bone outside the set; 14 of the 201 ticks).
//!
//! Bones are reported in four groups: `head_jnt` and below (the face slot, the head grid of
//! the look-at, the blink, the eye controls), the legs (`upper_leg_*` and below, coat flaps
//! included: the bones `DisSkelControl_FootPlacement` moves after the tree, not modelled, U-280),
//! the bones below `hand_L_jnt` / `hand_R_jnt`, and the rest of the body. The hands are apart
//! because the coordinator's `oracle anim clip` comparison of this capture (2026-10-11) found
//! every finger joint 60-65 deg off (180 max) on every frame and asked for them to be reported
//! as a known deviation; compared only where their captured value is fresh, they match like
//! the rest of the arm, and against every frame's carried-forward value (the diagnostic below)
//! they show that comparison's size: the finger deviation is the capture's stale LOD values, not
//! the decoder or the binding.
//!
//! The look-at node is recorded as a sequence node (`ArkAnimNodeLookAt` derives from
//! `AnimNodeSequence`: its `AnimSeqName` is the first grid entry, time 0), not its cell weights,
//! so both runs give it the centred aim (cell CC of both grids at full weight).

use dishonored_anim::pose::{self, Pose};
use dishonored_anim::tree::{FilterTarget, LocoInput, LookInput, SlotAction, SlotRequest, TreeInputs};
use dishonored_anim::{AnimSets, AnimTreeInstance, SetFlags};
use dishonored_assets::Content;
use dishonored_assets::anim_tree::load_anim_tree;
use dishonored_assets::characters::{GUARD_LOCO, GUARD_SETS, bind_anim_sets, load_anim_set, load_skeleton};
use dishonored_ir::BoneAsset;
use dishonored_ir::anim_tree::{AnimTreeAsset, NodeProps};
use glam::{Quat, Vec3};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const PACKAGE: &str = "L_Boyle_Ext_Script";
const SKELETON: &str = "Npc_EliteGuard.Mesh.Skm_EliteGuard_Body";
const CAPTURE: &str = "70da96138fe2171c/stealth/detect_1300/anim-100145dd15bef764.json";
const DT: f32 = 1.0 / 60.0;

/// The oracle cache (`dishonored_oracle::cache::root`'s variables, else the local cache).
fn capture_path() -> PathBuf {
    let root = std::env::var_os("DISHONORED_ORACLE_CACHE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("DISHONORED_CACHE").map(|p| PathBuf::from(p).join("oracle")))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("dishonored-rs").join("cache").join("oracle")))
        .unwrap_or_else(|| PathBuf::from("cache").join("oracle"));
    root.join(CAPTURE)
}

/// One node's captured state.
#[derive(Clone, Debug)]
enum NodeRec {
    Blend { weight: f32 },
    Seq { weight: f32, seq: String, time: f32, rate: f32 },
}

/// A capture frame with everything carried forward from the previous frames.
struct Frame {
    tick: u64,
    /// Component-space rotation and translation per bone index.
    cs: Vec<(Quat, Vec3)>,
    /// Bones whose value this frame's delta rewrote.
    delta: BTreeSet<usize>,
    nodes: BTreeMap<usize, NodeRec>,
    blend: BTreeMap<usize, Vec<f32>>,
}

struct Capture {
    bones: Vec<String>,
    classes: Vec<String>,
    frames: Vec<Frame>,
}

fn load_capture(path: &PathBuf) -> Option<Capture> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).expect("capture JSON");
    let mesh = &v["meshes"][0];
    let bones: Vec<String> = mesh["bones"].as_array().unwrap().iter().map(|b| b[1].as_str().unwrap().to_string()).collect();
    let classes: Vec<String> = mesh["nodes"].as_array().unwrap().iter().map(|n| n["class"].as_str().unwrap().to_string()).collect();
    let mut cs = vec![(Quat::IDENTITY, Vec3::ZERO); bones.len()];
    let (mut nodes, mut blend) = (BTreeMap::new(), BTreeMap::new());
    let mut frames = Vec::new();
    let num = |x: &Value| x.as_f64().unwrap() as f32;
    for f in mesh["frames"].as_array().unwrap() {
        let mut delta = BTreeSet::new();
        if let Some(m) = f["cs"].as_object() {
            for (k, val) in m {
                let b: usize = k.parse().unwrap();
                let a: Vec<f32> = val.as_array().unwrap().iter().map(num).collect();
                cs[b] = (Quat::from_xyzw(a[0], a[1], a[2], a[3]).normalize(), Vec3::new(a[4], a[5], a[6]) / 100.0);
                delta.insert(b);
            }
        }
        if let Some(m) = f["nodes"].as_object() {
            for (k, val) in m {
                let a = val.as_array().unwrap();
                let weight = num(&a[0]) / 1000.0;
                let rec = if a.len() >= 5 { NodeRec::Seq { weight, seq: a[2].as_str().unwrap_or("None").to_string(), time: num(&a[3]) / 1000.0, rate: num(&a[4]) / 1000.0 } } else { NodeRec::Blend { weight } };
                nodes.insert(k.parse::<usize>().unwrap(), rec);
            }
        }
        if let Some(m) = f["blend"].as_object() {
            for (k, val) in m {
                blend.insert(k.parse::<usize>().unwrap(), val.as_array().unwrap().iter().map(|x| num(x) / 1000.0).collect::<Vec<f32>>());
            }
        }
        frames.push(Frame { tick: f["t"].as_u64().unwrap_or(0), cs: cs.clone(), delta, nodes: nodes.clone(), blend: blend.clone() });
    }
    Some(Capture { bones, classes, frames })
}

/// The bones each frame's captured value is fresh for (see the module docs).
fn fresh_sets(cap: &Capture) -> Vec<Vec<bool>> {
    let n = cap.bones.len();
    // The reduced LOD's bones: those rewritten on the ticks that rewrite few; bones never
    // rewritten after the first frame are constant and fresh on every tick.
    let mut reduced = vec![false; n];
    let mut ever = vec![false; n];
    for f in cap.frames.iter().skip(1) {
        for b in &f.delta {
            ever[*b] = true;
            if f.delta.len() < n / 2 {
                reduced[*b] = true;
            }
        }
    }
    let base: Vec<bool> = (0..n).map(|b| reduced[b] || !ever[b]).collect();
    cap.frames
        .iter()
        .enumerate()
        .map(|(k, f)| {
            let full = k > 0 && f.delta.iter().any(|b| !base[*b]);
            (0..n).map(|b| base[b] || full).collect()
        })
        .collect()
}

/// Bone groups: 0 body, 1 legs (`upper_leg_*` and below), 2 head (`head_jnt` and below), 3
/// hands (below `hand_L_jnt` / `hand_R_jnt`: fingers, offsets, attachments).
fn groups(bones: &[BoneAsset]) -> Vec<usize> {
    let par = pose::parents(bones);
    let mut g = vec![0; bones.len()];
    for i in 0..bones.len() {
        let n = bones[i].name.to_ascii_lowercase();
        let parent = par[i].map(|p| (g[p], bones[p].name.to_ascii_lowercase()));
        g[i] = match parent {
            _ if n == "head_jnt" => 2,
            _ if n.starts_with("upper_leg_") => 1,
            Some((_, p)) if p == "hand_l_jnt" || p == "hand_r_jnt" => 3,
            Some((pg, _)) => pg,
            None => 0,
        };
    }
    g
}

const GROUPS: [&str; 4] = ["body", "legs", "head", "hands"];

fn angle_deg(a: Quat, b: Quat) -> f32 {
    2.0 * a.dot(b).abs().min(1.0).acos().to_degrees()
}

#[derive(Default, Clone, Copy)]
struct Stats {
    sum: f64,
    max: f32,
    n: usize,
    tsum: f64,
    tmax: f32,
}

impl Stats {
    fn add(&mut self, deg: f32, dist: f32) {
        self.sum += deg as f64;
        self.max = self.max.max(deg);
        self.tsum += dist as f64;
        self.tmax = self.tmax.max(dist);
        self.n += 1;
    }
    fn merge(&mut self, o: &Stats) {
        self.sum += o.sum;
        self.max = self.max.max(o.max);
        self.tsum += o.tsum;
        self.tmax = self.tmax.max(o.tmax);
        self.n += o.n;
    }
    fn mean(&self) -> f32 {
        if self.n == 0 { 0.0 } else { (self.sum / self.n as f64) as f32 }
    }
    fn tmean(&self) -> f32 {
        if self.n == 0 { 0.0 } else { (self.tsum / self.n as f64) as f32 }
    }
}

struct Setup {
    cap: Capture,
    skeleton: Vec<BoneAsset>,
    tree: AnimTreeAsset,
    sets: AnimSets,
    content: Content,
    wanted: Vec<String>,
}

fn setup() -> Option<Setup> {
    let content = Content::locate().ok()?;
    let path = capture_path();
    let Some(cap) = load_capture(&path) else {
        eprintln!("no capture at {}", path.display());
        return None;
    };
    let tree = load_anim_tree(&content, "Startup", "Npc_Common_at.NPC_Human_at").unwrap();
    let skeleton = load_skeleton(&content, PACKAGE, SKELETON).unwrap();
    // The capture's bones are the skeleton's, in order.
    assert_eq!(cap.bones.len(), skeleton.len());
    for (a, b) in cap.bones.iter().zip(&skeleton) {
        assert!(a.eq_ignore_ascii_case(&b.name), "capture bone {a} vs skeleton {}", b.name);
    }
    // Every sequence the capture names, and the look-at grids.
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    for f in &cap.frames {
        for r in f.nodes.values() {
            if let NodeRec::Seq { seq, .. } = r {
                wanted.insert(seq.clone());
            }
        }
    }
    for n in &tree.nodes {
        if let NodeProps::AimOffset { grids } = &n.props {
            wanted.extend(grids.iter().cloned());
        }
    }
    let wanted: Vec<String> = wanted.into_iter().collect();
    let names: Vec<&str> = wanted.iter().map(String::as_str).collect();
    // The guard's sets were compressed against its own body (`EliteGuardLocomotionConfig`'s
    // `m_SkeletalRefMeshName`).
    let sets = bind_anim_sets(&content, PACKAGE, GUARD_SETS, &names, &skeleton, &skeleton).unwrap();
    Some(Setup { cap, skeleton, tree, sets, content, wanted })
}

/// Drive every captured node state of a frame into the instance (the replay API).
fn drive(inst: &mut AnimTreeInstance, f: &Frame, lookat: usize, time_shift: f32) {
    for (k, r) in &f.nodes {
        let Some(node) = inst.node_at_tick_index(*k) else { continue };
        match r {
            NodeRec::Seq { weight, seq, time, rate } if node != lookat => inst.set_node_state(node, *weight, seq, (*time + rate * time_shift).max(0.0), *rate),
            NodeRec::Seq { weight, .. } | NodeRec::Blend { weight } => inst.set_node_state(node, *weight, "", 0.0, 0.0),
        }
    }
    for (k, w) in &f.blend {
        if let Some(node) = inst.node_at_tick_index(*k) {
            inst.set_children_weights(node, w);
        }
    }
    let mut centre = [0.0; 18];
    centre[4] = 1.0;
    centre[13] = 1.0;
    inst.set_children_weights(lookat, &centre);
}

/// A pose against a frame's fresh captured bones, per group and per bone.
fn compare(skeleton: &[BoneAsset], local: &Pose, f: &Frame, fresh: &[bool], group: &[usize], per_bone: &mut [Stats]) -> [Stats; 4] {
    let cs = pose::to_component(&pose::parents(skeleton), local);
    let mut out = [Stats::default(); 4];
    for b in 0..skeleton.len() {
        if !fresh[b] {
            continue;
        }
        let deg = angle_deg(cs[b].0, f.cs[b].0);
        let dist = cs[b].1.distance(f.cs[b].1);
        out[group[b]].add(deg, dist);
        per_bone[b].add(deg, dist);
    }
    out
}

/// Totals of a run: per group, and the task's "body" (every bone outside the head group).
struct Run {
    groups: [Stats; 4],
    per_bone: Vec<Stats>,
}

impl Run {
    fn new(n: usize) -> Self {
        Run { groups: [Stats::default(); 4], per_bone: vec![Stats::default(); n] }
    }
    fn add(&mut self, s: &[Stats; 4]) {
        for (a, b) in self.groups.iter_mut().zip(s) {
            a.merge(b);
        }
    }
    /// Every bone outside the head group (the task's "body bones").
    fn not_head(&self) -> Stats {
        let mut s = self.groups[0];
        s.merge(&self.groups[1]);
        s.merge(&self.groups[3]);
        s
    }
    fn print(&self, label: &str, skeleton: &[BoneAsset]) {
        let nh = self.not_head();
        println!("{label} outside the head: mean {:.3} deg, max {:.3} deg, translation mean {:.3} max {:.3} uu, {} samples", nh.mean(), nh.max, nh.tmean(), nh.tmax, nh.n);
        for (g, s) in GROUPS.iter().zip(&self.groups) {
            println!("{label} {g}: mean {:.3} deg, max {:.3} deg, translation mean {:.3} max {:.3} uu, {} samples", s.mean(), s.max, s.tmean(), s.tmax, s.n);
        }
        let mut ranked: Vec<usize> = (0..skeleton.len()).filter(|i| self.per_bone[*i].n > 0).collect();
        ranked.sort_by(|a, b| self.per_bone[*b].mean().total_cmp(&self.per_bone[*a].mean()));
        for i in ranked.iter().take(8) {
            let st = &self.per_bone[*i];
            println!("  {:22} mean {:.3} max {:.3} deg, translation mean {:.3} max {:.3} uu ({} samples)", skeleton[*i].name, st.mean(), st.max, st.tmean(), st.tmax, st.n);
        }
    }
}

#[test]
fn replay_matches_the_captured_bones() {
    let Some(s) = setup() else { return };
    let mut inst = AnimTreeInstance::new(&s.tree, &s.skeleton);
    // The tick order reproduces the cooked `TickArrayIndex` (slot 0 is the tree object), and the
    // capture's node classes are the tree's.
    for (k, node) in inst.tick_order().iter().enumerate() {
        assert_eq!(inst.tick_index(*node), Some(k + 1), "{} ticks at {k}", s.tree.nodes[*node].name);
        assert_eq!(s.cap.classes[k + 1], s.tree.nodes[*node].class);
    }
    assert_eq!(inst.tick_order().len() + 1, s.cap.classes.len());
    let lookat = inst.node_index("LookatBlender").unwrap();
    let fresh = fresh_sets(&s.cap);
    let group = groups(&s.skeleton);
    let mut run = Run::new(s.skeleton.len());
    println!("tick: outside the head, body, legs, hands, head: mean / max deg (compared bones)");
    for (k, f) in s.cap.frames.iter().enumerate() {
        drive(&mut inst, f, lookat, 0.0);
        let g = compare(&s.skeleton, &inst.pose(&s.sets), f, &fresh[k], &group, &mut run.per_bone);
        let mut nh = g[0];
        nh.merge(&g[1]);
        nh.merge(&g[3]);
        println!("{:3}: {:.3} / {:.3}, {:.3} / {:.3}, {:.3} / {:.3}, {:.3} / {:.3}, {:.3} / {:.3} ({} bones)", f.tick, nh.mean(), nh.max, g[0].mean(), g[0].max, g[1].mean(), g[1].max, g[3].mean(), g[3].max, g[2].mean(), g[2].max, nh.n + g[2].n);
        run.add(&g);
    }
    run.print("replay", &s.skeleton);
    // Diagnostic: the captured bones against the poses a few ticks before / after the captured
    // node times (the bones are of the recorded times: the error grows either way).
    for shift in [-2i32, -1, 1, 2] {
        let mut r = Run::new(s.skeleton.len());
        for (k, f) in s.cap.frames.iter().enumerate() {
            drive(&mut inst, f, lookat, shift as f32 * DT);
            let g = compare(&s.skeleton, &inst.pose(&s.sets), f, &fresh[k], &group, &mut r.per_bone);
            r.add(&g);
        }
        println!("  node times shifted {shift:+} ticks: body mean {:.3} max {:.3} deg", r.groups[0].mean(), r.groups[0].max);
    }
    // Diagnostic: the runtime's retargeting rule (clip translations for the root and its children
    // only) instead of the sets' `bAnimRotationOnly` false (every clip translation): the face
    // bones' positions tell them apart.
    let root_children: Vec<String> = s.skeleton.iter().filter(|b| b.parent == 0).map(|b| b.name.clone()).collect();
    let flags = SetFlags { rotation_only: true, use_translation: root_children, force_mesh_translation: Vec::new() };
    let names: Vec<&str> = s.wanted.iter().map(String::as_str).collect();
    let mut shallow = AnimSets::new();
    for path in GUARD_SETS {
        let mut lib = load_anim_set(&s.content, PACKAGE, path, &names).unwrap();
        lib.edge_order(&s.skeleton);
        shallow.push(&lib.tracks, lib.clips, &flags, &s.skeleton);
    }
    let mut r = Run::new(s.skeleton.len());
    for (k, f) in s.cap.frames.iter().enumerate() {
        drive(&mut inst, f, lookat, 0.0);
        let g = compare(&s.skeleton, &inst.pose(&shallow), f, &fresh[k], &group, &mut r.per_bone);
        r.add(&g);
    }
    println!("  root-and-children translations: head translation mean {:.3} max {:.3} uu (every clip translation: mean {:.3} max {:.3})", r.groups[2].tmean(), r.groups[2].tmax, run.groups[2].tmean(), run.groups[2].tmax);
    // Diagnostic: every bone against every frame's carried-forward value, stale or not (the
    // comparison behind the coordinator's finger finding).
    let mut r = Run::new(s.skeleton.len());
    let all = vec![true; s.skeleton.len()];
    for f in &s.cap.frames {
        drive(&mut inst, f, lookat, 0.0);
        let g = compare(&s.skeleton, &inst.pose(&s.sets), f, &all, &group, &mut r.per_bone);
        r.add(&g);
    }
    println!("  stale values included: hands mean {:.3} max {:.3} deg, head mean {:.3} max {:.3} deg, body mean {:.3} max {:.3} deg", r.groups[3].mean(), r.groups[3].max, r.groups[2].mean(), r.groups[2].max, r.groups[0].mean(), r.groups[0].max);

    // Bounds, measured 2026-10-11 (mean / max deg): body 0.008 / 0.125; hands 0.025 / 0.143
    // (stale values included: 45.8 / 180.0, the finger finding); head 0.014 / 0.112; the legs
    // carry the foot placement the evaluator does not apply, 2.265 / 6.900 (at a coat flap;
    // `upper_leg_R_jnt` 2.84 on every tick); everything outside the head 0.709 / 6.900. The
    // capture's quaternions (x 1e4) and node times (x 1e3 s) are rounded.
    let [body, legs, head, hands] = run.groups;
    assert!(body.mean() < 0.02 && body.max < 0.2, "body {:.3} / {:.3}", body.mean(), body.max);
    assert!(hands.mean() < 0.05 && hands.max < 0.2, "hands {:.3} / {:.3}", hands.mean(), hands.max);
    assert!(head.mean() < 0.02 && head.max < 0.15, "head {:.3} / {:.3}", head.mean(), head.max);
    assert!(legs.mean() < 2.3 && legs.max < 7.0, "legs {:.3} / {:.3}", legs.mean(), legs.max);
}

#[test]
fn input_driven_tick_matches_the_capture() {
    let Some(s) = setup() else { return };
    let mut inst = AnimTreeInstance::new(&s.tree, &s.skeleton);
    let loco_cfg = dishonored_assets::npc_anim::load_loco_config(&s.content, &(PACKAGE.to_string(), GUARD_LOCO.to_string())).unwrap();
    let lookat_cfg = dishonored_assets::npc_anim::load_lookat_config(&s.content, &("DishonoredGame".to_string(), "DisLookatConfig.DefaultLookatConfig".to_string())).unwrap();
    let mode = lookat_cfg.modes.iter().find(|m| m.name.eq_ignore_ascii_case("EmptyIdle")).unwrap();
    // The modifier whose idle loop the capture's `SpeedModeSlot0` plays (`Empty_Idle`).
    let f0 = &s.cap.frames[0];
    let idle_node = inst.node_index("LocoBlender").map(|l| s.tree.nodes[l].children[0].node.unwrap()).unwrap();
    let (idle_seq, idle_t0) = match &f0.nodes[&inst.tick_index(idle_node).unwrap()] {
        NodeRec::Seq { seq, time, .. } => (seq.clone(), *time),
        _ => panic!("idle slot"),
    };
    let modifier = (0..loco_cfg.modifiers.len()).find(|&m| loco_cfg.mode(m, 0).is_some_and(|sm| sm.move_anims.first().is_some_and(|a| a.0 == idle_seq))).map(|m| loco_cfg.modifiers[m].name.clone()).unwrap();
    println!("locomotion: modifier {modifier} (idle loop {idle_seq})");
    // Requests, from the capture's first tick: the ambient idle and its face sequence asked 4
    // ticks before (`ambient_age` 0.067 s: the `FullBodySlot` channel at 0.333 of its 0.2 s
    // blend), the face filter to 1 over 0.1 s with them (0.667 on the first tick); the blinks at
    // the rates the capture shows, 12 ticks before (time 0.254 at rate 1.272) and on tick 157
    // (time 0, rate 1.188).
    let first = -12i64;
    let request_at = -4i64;
    let blinks = [(-12i64, 1.272f32), (157, 1.188)];
    // The idle loop's time before the first tick (it advances at rate 1 on every tick until the
    // capture's first).
    inst.set_node_state(idle_node, 1.0, &idle_seq, idle_t0 - (1 - first) as f32 * DT, 1.0);
    let lookat = inst.node_index("LookatBlender").unwrap();
    let fresh = fresh_sets(&s.cap);
    let group = groups(&s.skeleton);
    let mut run = Run::new(s.skeleton.len());
    let (mut wmax, mut tmax) = ((0.0f32, String::new()), (0.0f32, String::new()));
    let slot_requests = [
        SlotRequest { slot: "FullBodySlot".into(), action: SlotAction::Play { sequence: "Empty_Idle_04".into(), rate: 1.0, blend_in: 0.2, blend_out: Some(0.1), looping: false, start: 0.0 } },
        SlotRequest { slot: "FaceSlot".into(), action: SlotAction::Play { sequence: "Empty_Idle_04_Face".into(), rate: 1.0, blend_in: 0.2, blend_out: Some(0.1), looping: false, start: 0.0 } },
    ];
    let filters = [FilterTarget { node: "FaceFilter".into(), target: 1.0, time: 0.1 }];
    for tick in first..=s.cap.frames.last().unwrap().tick as i64 {
        let at_request = tick == request_at;
        let inputs = TreeInputs {
            physics: Some(1),
            slots: if at_request { &slot_requests } else { &[] },
            filters: if at_request { &filters } else { &[] },
            locomotion: Some(LocoInput { config: &loco_cfg, modifier: &modifier, velocity: (0.0, 0.0), specific: None }),
            look: Some(LookInput { mode, head: [0.0, 0.0], torso: [0.0, 0.0], weight: 1.0 }),
            blink: blinks.iter().find(|b| b.0 == tick).map(|b| b.1),
            ..Default::default()
        };
        inst.update(&s.sets, &inputs, DT);
        if tick < 0 {
            continue;
        }
        let k = tick as usize;
        let f = &s.cap.frames[k];
        // The tick's node states against the captured ones: every node's total weight, the
        // times of the sequences that play, the children weights the capture records.
        for (idx, rec) in &f.nodes {
            let Some(node) = inst.node_at_tick_index(*idx) else { continue };
            let (w, name) = (inst.total_weight(node), &s.tree.nodes[node].name);
            let weight = match rec {
                NodeRec::Seq { weight, time, seq, .. } => {
                    if *weight > 0.0 && seq != "None" && node != lookat {
                        let d = (inst.sequence(node).map_or(0.0, |q| q.time) - time).abs();
                        if d > tmax.0 {
                            tmax = (d, format!("{name} ({seq}) tick {tick}"));
                        }
                    }
                    *weight
                }
                NodeRec::Blend { weight } => *weight,
            };
            if (w - weight).abs() > wmax.0 {
                wmax = ((w - weight).abs(), format!("{name} weight tick {tick}"));
            }
            if let Some(cw) = f.blend.get(idx).filter(|_| node != lookat) {
                for (a, b) in inst.children_weights(node).iter().zip(cw) {
                    if (a - b).abs() > wmax.0 {
                        wmax = ((a - b).abs(), format!("{name} children tick {tick}"));
                    }
                }
            }
        }
        let g = compare(&s.skeleton, &inst.pose(&s.sets), f, &fresh[k], &group, &mut run.per_bone);
        if k % 25 == 0 || k < 4 {
            println!("{:3}: body {:.3} / {:.3}, legs {:.3} / {:.3}, hands {:.3} / {:.3}, head {:.3} / {:.3}", f.tick, g[0].mean(), g[0].max, g[1].mean(), g[1].max, g[3].mean(), g[3].max, g[2].mean(), g[2].max);
        }
        run.add(&g);
    }
    run.print("inputs", &s.skeleton);
    println!("inputs node states: max weight difference {:.4} ({}), max time difference {:.4} s ({})", wmax.0, wmax.1, tmax.0, tmax.1);
    // Bounds, measured 2026-10-11 (mean / max deg): body 0.003 / 0.056, hands 0.004 / 0.056,
    // head 0.003 / 0.056, legs 2.265 / 6.911 (the foot placement), everything outside the head
    // 0.702 / 6.911: closer than the replay, whose node times carry the capture's 1e-3 s
    // rounding. Node states: weights within 0.0003 (the capture's rounding), times within
    // 0.0007 s.
    let [body, legs, head, hands] = run.groups;
    assert!(body.mean() < 0.02 && body.max < 0.2, "body {:.3} / {:.3}", body.mean(), body.max);
    assert!(hands.mean() < 0.05 && hands.max < 0.2, "hands {:.3} / {:.3}", hands.mean(), hands.max);
    assert!(head.mean() < 0.02 && head.max < 0.15, "head {:.3} / {:.3}", head.mean(), head.max);
    assert!(legs.mean() < 2.3 && legs.max < 7.0, "legs {:.3} / {:.3}", legs.mean(), legs.max);
    assert!(wmax.0 < 0.002 && tmax.0 < 0.002, "node states: weight {:.4} ({}), time {:.4} ({})", wmax.0, wmax.1, tmax.0, tmax.1);
}
