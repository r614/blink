//! Skeletal animation comparison: oracle capture vs port dump.

use anyhow::{anyhow, bail, Context, Result};
use glam::{Mat4, Quat};
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::{BTreeMap, HashMap};

/// Parse the oracle capture sidecar JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CaptureFile {
    meshes: Vec<CaptureBody>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CaptureBody {
    bones: Vec<(usize, String)>,
    nodes: Option<Vec<CaptureNode>>,
    component: String,
    frames: Vec<CaptureFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CaptureNode {
    name: String,
    class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sequence: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CaptureFrame {
    t: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    l2w: Option<Vec<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cs: Option<BTreeMap<String, Vec<i32>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ws: Option<BTreeMap<String, Vec<i32>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nodes: Option<BTreeMap<String, Vec<serde_json::Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blend: Option<BTreeMap<String, Vec<i32>>>,
}

/// Reconstructed full-state frame from delta-encoded capture.
#[derive(Debug, Clone)]
struct FullFrame {
    tick: u64,
    bones: HashMap<String, BoneTransform>,
    nodes: HashMap<String, NodeState>,
    /// The bones this frame's `cs` rewrote. The original recomposes `SpaceBases` only for the
    /// bones the mesh's current LOD requires (on detect_1300, 37 of 118 on most ticks), and the
    /// delta encoding carries the others' stale values forward: a comparison reads a bone only
    /// in the frames that refresh it (ORACLE.md "Comparing the port").
    fresh: std::collections::HashSet<String>,
}

#[derive(Debug, Clone, Copy)]
struct BoneTransform {
    q: [f32; 4],
    t: [f32; 3],
}

#[derive(Debug, Clone)]
enum NodeState {
    Blend { weight: f32, relevant: bool },
    Sequence { weight: f32, relevant: bool, seq: String, time: f32, rate: f32 },
}

/// Port dump frame (`characters::dump_bones`): the whole record is read, the comparison uses
/// the tick, the bones and the full-body slot.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct PortFrame {
    tick: u64,
    id: u32,
    pos: [f32; 3],
    yaw: f32,
    modifier: Option<String>,
    vel: Option<[f32; 2]>,
    phase: Option<f32>,
    full: Option<PortSeq>,
    upper: Option<PortSeq>,
    ragdoll: bool,
    bones: Vec<PortBone>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct PortSeq {
    key: String,
    seq: String,
    t: f32,
    w: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct PortBone {
    name: String,
    q: [f32; 4],
    t: [f32; 3],
}

/// Command: oracle anim info FILE
pub fn cmd_anim_info(path: &str) -> Result<()> {
    let content = std::fs::read_to_string(path).context("reading capture file")?;
    let file: CaptureFile = serde_json::from_str(&content).context("parsing JSON")?;

    for body in &file.meshes {
        println!("Component: {}", body.component);
        println!(
            "  Bones: {} total",
            body.bones.len()
        );
        if body.bones.len() > 0 {
            println!("    First 20:");
            for (idx, name) in body.bones.iter().take(20) {
                println!("      [{}] {}", idx, name);
            }
        }

        let empty_nodes = Vec::new();
        let nodes = body.nodes.as_ref().unwrap_or(&empty_nodes);
        println!("  Nodes: {} total", nodes.len());
        let mut seqs = std::collections::HashSet::new();
        for n in nodes {
            if n.sequence == Some(true) {
                seqs.insert(n.name.clone());
            }
        }
        let by_class: HashMap<String, usize> = nodes
            .iter()
            .fold(HashMap::new(), |mut m, n| {
                *m.entry(n.class.clone()).or_insert(0) += 1;
                m
            });
        for (class, count) in by_class.iter() {
            let with_seq = nodes.iter().filter(|n| n.class == *class && n.sequence == Some(true)).count();
            if with_seq > 0 {
                println!("    {}: {} ({} with sequences)", class, count, with_seq);
            } else {
                println!("    {}: {}", class, count);
            }
        }

        let frames = &body.frames;
        println!("  Frames: {}", frames.len());
        if frames.len() > 0 {
            let first_tick = frames.iter().find_map(|f| f.t).unwrap_or(0);
            let last_tick = frames.iter().filter_map(|f| f.t).last().unwrap_or(0);
            println!("    Tick range: {} to {}", first_tick, last_tick);

            let has_cs = frames.iter().filter(|f| f.cs.is_some()).count();
            let has_nodes = frames.iter().filter(|f| f.nodes.is_some()).count();
            let has_blend = frames.iter().filter(|f| f.blend.is_some()).count();
            println!("    With cs: {}, nodes: {}, blend: {}", has_cs, has_nodes, has_blend);
        }

        // Sequences that play (a sequence node with weight > 0), per reconstructed frame: the
        // node list names only the nodes with a `NodeName` (the `DishonoredAnimNodeSeq`s have
        // none), the frames carry the sequence each node plays.
        let full = reconstruct_frames(body)?;
        let mut playing: BTreeMap<String, usize> = BTreeMap::new();
        for f in &full {
            let distinct: std::collections::BTreeSet<String> = get_playing_sequences(f).into_iter().collect();
            for s in distinct {
                *playing.entry(s).or_insert(0) += 1;
            }
        }
        if !playing.is_empty() {
            println!("  Playing sequences (frames with weight > 0): {}", playing.iter().map(|(s, n)| format!("{s} ({n})")).collect::<Vec<_>>().join(", "));
        }
        println!();
    }

    Ok(())
}

/// Reconstruct full-state frames from delta-encoded capture.
fn reconstruct_frames(body: &CaptureBody) -> Result<Vec<FullFrame>> {
    let mut frames = Vec::new();
    let mut last_bones: HashMap<String, BoneTransform> = HashMap::new();
    let mut last_nodes: HashMap<String, NodeState> = HashMap::new();

    for cf in &body.frames {
        let tick = cf.t.unwrap_or(0);

        // Update bones (delta encoded)
        let mut fresh = std::collections::HashSet::new();
        if let Some(cs) = &cf.cs {
            for (bone_idx, values) in cs {
                if values.len() >= 7 {
                    let q = [
                        values[0] as f32 / 10000.0,
                        values[1] as f32 / 10000.0,
                        values[2] as f32 / 10000.0,
                        values[3] as f32 / 10000.0,
                    ];
                    let t = [
                        values[4] as f32 / 100.0,
                        values[5] as f32 / 100.0,
                        values[6] as f32 / 100.0,
                    ];
                    // Find bone name
                    let bone_name = body
                        .bones
                        .iter()
                        .find(|(idx, _)| idx.to_string() == *bone_idx)
                        .map(|(_, name)| name.clone())
                        .unwrap_or_else(|| format!("bone_{}", bone_idx));
                    fresh.insert(bone_name.clone());
                    last_bones.insert(bone_name, BoneTransform { q, t });
                }
            }
        }

        // Update nodes (delta encoded)
        if let Some(nodes) = &cf.nodes {
            for (node_idx, values) in nodes {
                if values.is_empty() {
                    continue;
                }
                let weight = match values.get(0) {
                    Some(v) => v.as_i64().unwrap_or(0) as f32 / 1000.0,
                    None => 0.0,
                };
                let relevant = match values.get(1) {
                    Some(v) => v.as_i64().unwrap_or(0) != 0,
                    None => false,
                };

                if values.len() >= 5 {
                    // Sequence node
                    let seq = match values.get(2) {
                        Some(v) => v.as_str().unwrap_or("").to_string(),
                        None => "".to_string(),
                    };
                    let time = match values.get(3) {
                        Some(v) => v.as_i64().unwrap_or(0) as f32 / 1000.0,
                        None => 0.0,
                    };
                    let rate = match values.get(4) {
                        Some(v) => v.as_i64().unwrap_or(1000) as f32 / 1000.0,
                        None => 1.0,
                    };
                    last_nodes.insert(
                        node_idx.clone(),
                        NodeState::Sequence { weight, relevant, seq, time, rate },
                    );
                } else {
                    last_nodes.insert(node_idx.clone(), NodeState::Blend { weight, relevant });
                }
            }
        }

        frames.push(FullFrame {
            tick,
            bones: last_bones.clone(),
            nodes: last_nodes.clone(),
            fresh,
        });
    }

    Ok(frames)
}

/// Command: oracle anim compare
pub fn cmd_anim_compare(
    capture_path: &str,
    port_path: &str,
    bones_opt: Option<Vec<String>>,
    offset_opt: Option<i32>,
    pawn_id: Option<u32>,
    json_out: Option<&str>,
) -> Result<()> {
    let content = std::fs::read_to_string(capture_path).context("reading capture file")?;
    let file: CaptureFile = serde_json::from_str(&content).context("parsing capture JSON")?;

    let body = file.meshes.first().ok_or_else(|| anyhow!("no meshes in capture"))?;
    let capture_frames = reconstruct_frames(body)?;

    // Load port dump
    let port_content = std::fs::read_to_string(port_path).context("reading port dump")?;
    let mut port_frames: Vec<PortFrame> = port_content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    // Filter to pawn if specified
    if let Some(pid) = pawn_id {
        port_frames.retain(|f| f.id == pid);
    } else if !port_frames.is_empty() {
        let first_id = port_frames[0].id;
        port_frames.retain(|f| f.id == first_id);
    }

    if port_frames.is_empty() {
        bail!("no port frames found");
    }

    // Determine bones to compare
    let capture_bone_names: Vec<String> = body.bones.iter().map(|(_, name)| name.clone()).collect();
    let port_bone_names: std::collections::HashSet<String> =
        port_frames.iter().flat_map(|f| f.bones.iter().map(|b| b.name.clone())).collect();

    let compare_bones: Vec<String> = if let Some(bones) = bones_opt {
        bones.into_iter().filter(|b| capture_bone_names.contains(b)).collect()
    } else {
        capture_bone_names.into_iter().filter(|b| port_bone_names.contains(b)).collect()
    };

    if compare_bones.is_empty() {
        bail!("no bones to compare");
    }

    // Find optimal offset
    let offset = if let Some(o) = offset_opt {
        o
    } else {
        find_best_offset(&capture_frames, &port_frames, &compare_bones)?
    };

    // Compare
    let results = compare_animations(&capture_frames, &port_frames, offset, &compare_bones)?;

    // Print results
    print_comparison_report(&results, offset, &compare_bones);

    // Write JSON if requested
    if let Some(out) = json_out {
        let json = serde_json::to_string_pretty(&results)?;
        std::fs::write(out, json)?;
        println!("Results written to {}", out);
    }

    Ok(())
}

#[derive(Serialize, Deserialize)]
struct ComparisonResults {
    bones: HashMap<String, BoneStats>,
    overall: BoneStats,
    worst_bones: Vec<(String, BoneStats)>,
    worst_ticks: Vec<WorstTick>,
    sequences: SequenceStats,
}

/// One aligned tick's error: the mean rotation difference over the compared bones and the bone
/// that differs most.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorstTick {
    port_tick: u64,
    capture_tick: u64,
    mean_angle: f32,
    worst_bone: String,
    worst_angle: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BoneStats {
    mean_angle: f32,
    max_angle: f32,
    mean_translation: f32,
    samples: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SequenceStats {
    mismatches: usize,
    examples: Vec<SequenceMismatch>,
    same_seq_offset: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SequenceMismatch {
    tick: u64,
    capture_seqs: Vec<String>,
    port_seq: Option<String>,
}

fn find_best_offset(
    capture_frames: &[FullFrame],
    port_frames: &[PortFrame],
    bones: &[String],
) -> Result<i32> {
    let mut best_offset = 0;
    let mut best_error = f32::MAX;

    for offset in -30..=30 {
        let mut total_error = 0.0;
        let mut count = 0;

        for pf in port_frames {
            let capture_idx = pf.tick as i64 + offset as i64;
            if capture_idx < 0 || capture_idx as usize >= capture_frames.len() {
                continue;
            }
            let cf = &capture_frames[capture_idx as usize];

            for bone in bones {
                if let (Some(c_bone), Some(p_bone)) =
                    (cf.fresh.contains(bone).then(|| cf.bones.get(bone)).flatten(), pf.bones.iter().find(|b| &b.name == bone))
                {
                    let angle = bone_angle_degrees(c_bone.q, p_bone.q);
                    total_error += angle;
                    count += 1;
                }
            }
        }

        if count > 0 {
            let avg_error = total_error / count as f32;
            if avg_error < best_error {
                best_error = avg_error;
                best_offset = offset;
            }
        }
    }

    Ok(best_offset)
}

fn bone_angle_degrees(q1: [f32; 4], q2: [f32; 4]) -> f32 {
    let q1 = Quat::from_array(q1);
    let q2 = Quat::from_array(q2);
    let dot = (q1.dot(q2)).abs().min(1.0);
    let angle = 2.0 * dot.acos();
    angle.to_degrees()
}

fn compare_animations(
    capture_frames: &[FullFrame],
    port_frames: &[PortFrame],
    offset: i32,
    bones: &[String],
) -> Result<ComparisonResults> {
    let mut bone_stats: HashMap<String, (f32, f32, f32, usize)> = HashMap::new();

    for bone in bones {
        bone_stats.insert(bone.clone(), (0.0, 0.0, 0.0, 0));
    }

    let mut seq_mismatches = Vec::new();
    let mut seq_mismatch_count = 0usize;
    let mut seq_offset_diffs = Vec::new();
    let mut ticks: Vec<WorstTick> = Vec::new();

    for pf in port_frames {
        let capture_idx = pf.tick as i64 + offset as i64;
        if capture_idx < 0 || capture_idx as usize >= capture_frames.len() {
            continue;
        }
        let cf = &capture_frames[capture_idx as usize];

        // Compare bones
        let (mut tick_sum, mut tick_n, mut tick_worst) = (0.0f32, 0usize, (String::new(), 0.0f32));
        for bone in bones {
            if let (Some(c_bone), Some(p_bone)) =
                (cf.fresh.contains(bone).then(|| cf.bones.get(bone)).flatten(), pf.bones.iter().find(|b| &b.name == bone))
            {
                let angle = bone_angle_degrees(c_bone.q, p_bone.q);
                let trans_diff = ((c_bone.t[0] - p_bone.t[0]).powi(2)
                    + (c_bone.t[1] - p_bone.t[1]).powi(2)
                    + (c_bone.t[2] - p_bone.t[2]).powi(2))
                    .sqrt();

                if let Some(entry) = bone_stats.get_mut(bone) {
                    entry.0 += angle;
                    entry.1 = entry.1.max(angle);
                    entry.2 += trans_diff;
                    entry.3 += 1;
                }
                tick_sum += angle;
                tick_n += 1;
                if angle > tick_worst.1 {
                    tick_worst = (bone.clone(), angle);
                }
            }
        }
        if tick_n > 0 {
            ticks.push(WorstTick { port_tick: pf.tick, capture_tick: cf.tick, mean_angle: tick_sum / tick_n as f32, worst_bone: tick_worst.0, worst_angle: tick_worst.1 });
        }

        // Compare sequences
        let capture_seqs = get_playing_sequences(cf);
        if let Some(port_seq) = &pf.full {
            if !capture_seqs.contains(&port_seq.seq) {
                seq_mismatch_count += 1;
                if seq_mismatches.len() < 10 {
                    seq_mismatches.push(SequenceMismatch {
                        tick: pf.tick,
                        capture_seqs: capture_seqs.clone(),
                        port_seq: Some(port_seq.seq.clone()),
                    });
                }
            } else if let Some(ct) = capture_time(cf, &port_seq.seq) {
                // The same sequence on both sides: how far ahead the port's playhead runs.
                seq_offset_diffs.push(port_seq.t - ct);
            }
        }
    }
    ticks.sort_by(|a, b| b.mean_angle.total_cmp(&a.mean_angle));
    ticks.truncate(10);

    // Compute per-bone stats
    let mut bones_map = HashMap::new();
    for (bone, (angle_sum, max_angle, trans_sum, count)) in bone_stats {
        if count > 0 {
            bones_map.insert(
                bone,
                BoneStats {
                    mean_angle: angle_sum / count as f32,
                    max_angle,
                    mean_translation: trans_sum / count as f32,
                    samples: count,
                },
            );
        }
    }

    // Overall stats
    let total_samples: usize = bones_map.values().map(|s| s.samples).sum();
    let overall = if total_samples > 0 {
        BoneStats {
            mean_angle: bones_map.values().map(|s| s.mean_angle * s.samples as f32).sum::<f32>() / total_samples as f32,
            max_angle: bones_map.values().map(|s| s.max_angle).max_by(|a, b| a.total_cmp(b)).unwrap_or(0.0),
            mean_translation: bones_map.values().map(|s| s.mean_translation * s.samples as f32).sum::<f32>()
                / total_samples as f32,
            samples: total_samples,
        }
    } else {
        BoneStats { mean_angle: 0.0, max_angle: 0.0, mean_translation: 0.0, samples: 0 }
    };

    // Worst bones
    let mut worst_bones: Vec<_> = bones_map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    worst_bones.sort_by(|a, b| {
        b.1.mean_angle.total_cmp(&a.1.mean_angle).then(b.1.max_angle.total_cmp(&a.1.max_angle))
    });

    Ok(ComparisonResults {
        bones: bones_map,
        overall,
        worst_bones: worst_bones.into_iter().take(10).collect(),
        worst_ticks: ticks,
        sequences: SequenceStats {
            mismatches: seq_mismatch_count,
            examples: seq_mismatches,
            same_seq_offset: if seq_offset_diffs.is_empty() {
                None
            } else {
                Some(seq_offset_diffs.iter().sum::<f32>() / seq_offset_diffs.len() as f32)
            },
        },
    })
}

/// The sequences playing in a frame (sequence nodes with weight > 0; `None` is an idle node).
fn get_playing_sequences(frame: &FullFrame) -> Vec<String> {
    let mut seqs = Vec::new();
    for (_, node) in &frame.nodes {
        if let NodeState::Sequence { weight, seq, .. } = node {
            if *weight > 0.0 && !seq.is_empty() && seq != "None" {
                seqs.push(seq.clone());
            }
        }
    }
    seqs
}

/// The playhead (seconds) of the heaviest node playing `seq` in a frame.
fn capture_time(frame: &FullFrame, seq_name: &str) -> Option<f32> {
    frame
        .nodes
        .values()
        .filter_map(|n| match n {
            NodeState::Sequence { weight, seq, time, .. } if seq == seq_name && *weight > 0.0 => Some((*weight, *time)),
            _ => None,
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, t)| t)
}

fn print_comparison_report(results: &ComparisonResults, offset: i32, bones: &[String]) {
    println!("Offset: {offset} ticks (capture tick = port tick + offset); {} bones compared", bones.len());
    println!();
    println!("Overall: mean angle {:.2} deg, max angle {:.2} deg, mean translation {:.2} uu over {} samples",
        results.overall.mean_angle, results.overall.max_angle, results.overall.mean_translation, results.overall.samples);
    println!();

    println!("Worst 10 bones:");
    for (bone, stats) in &results.worst_bones {
        println!("  {}: mean {:.2} deg, max {:.2} deg, trans {:.2} uu ({} samples)",
            bone, stats.mean_angle, stats.max_angle, stats.mean_translation, stats.samples);
    }
    println!();

    println!("Worst 10 ticks (port tick -> capture tick: mean angle; worst bone):");
    for t in &results.worst_ticks {
        println!("  {} -> {}: {:.2} deg; {} {:.2} deg", t.port_tick, t.capture_tick, t.mean_angle, t.worst_bone, t.worst_angle);
    }
    println!();

    println!("Sequences: {} ticks where the port's full-body sequence is not playing in the capture", results.sequences.mismatches);
    for m in &results.sequences.examples {
        println!("  Tick {}: capture {:?}, port {:?}",
            m.tick, m.capture_seqs, m.port_seq);
    }
    match results.sequences.same_seq_offset {
        Some(d) => println!("Same sequence on both sides: the port's playhead runs {:+.3} s ({:+.1} ticks) from the capture's", d, d * 60.0),
        None => println!("Same sequence on both sides: never"),
    }
}

/// Command: oracle anim clip --capture FILE --skeleton PKG MESH_PATH --set PKG SET_PATH --sequence NAME [options]
pub fn cmd_anim_clip(
    capture_path: &str,
    skeleton_pkg: &str,
    skeleton_mesh: &str,
    set_pkg: &str,
    set_path: &str,
    sequence: &str,
    node_idx: Option<usize>,
    bones_opt: Option<Vec<String>>,
    exclude_below: Option<String>,
    frames_range: Option<(usize, usize)>,
    json_out: Option<&str>,
) -> Result<()> {
    use dishonored_assets::skeletal_mesh::read_skeletal_mesh;
    use dishonored_assets::characters::load_anim_set;
    use dishonored_assets::Content;
    use ue3_package::ObjRef;

    let content = Content::locate()?;
    let capture_content = std::fs::read_to_string(capture_path).context("reading capture file")?;
    let file: CaptureFile = serde_json::from_str(&capture_content).context("parsing capture JSON")?;
    let body = file.meshes.first().ok_or_else(|| anyhow!("no meshes in capture"))?;

    // Load skeleton and build BoneAssets
    let skel_pkg = content.package(skeleton_pkg)?;
    let mesh_idx = (0..skel_pkg.exports.len())
        .find(|&i| skel_pkg.export_class(i) == "SkeletalMesh" && skel_pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(skeleton_mesh))
        .with_context(|| format!("{skeleton_mesh} not in {skeleton_pkg}"))?;
    let raw_mesh = read_skeletal_mesh(&skel_pkg, mesh_idx)?;

    let bones: Vec<dishonored_ir::BoneAsset> = raw_mesh.bones.iter().map(|b| dishonored_ir::BoneAsset {
        name: b.name.clone(),
        parent: b.parent,
        rotation: b.orientation,
        translation: b.position,
    }).collect();

    // Load AnimSet and get the clip
    let mut lib = load_anim_set(&content, set_pkg, set_path, &[sequence])?;
    lib.edge_order(&bones);
    let clip = lib.clips.get(sequence)
        .with_context(|| format!("sequence {sequence} not found in AnimSet"))?;

    // Reconstruct capture frames
    let capture_frames = reconstruct_frames(body)?;

    // Get bone indices to compare
    let capture_bone_names: Vec<String> = body.bones.iter().map(|(_, name)| name.clone()).collect();
    let compare_bones: Vec<String> = match bones_opt {
        Some(b) => b.into_iter().filter(|n| capture_bone_names.contains(n)).collect(),
        None => capture_bone_names.clone(),
    };

    if compare_bones.is_empty() {
        bail!("no bones to compare");
    }

    // Filter out bones below exclude_below if specified
    let mut exclude_set = std::collections::HashSet::new();
    for name in exclude_below.iter().flat_map(|s| s.split(',')).map(str::trim).filter(|s| !s.is_empty()) {
        exclude_set.extend(build_exclude_set(&bones, name));
    }

    let compare_bones: Vec<String> = compare_bones.into_iter()
        .filter(|b| !exclude_set.contains(b))
        .collect();

    // Perform offset search
    let (best_offset, best_mean) = find_best_clip_offset(&capture_frames, &bones, &clip, &lib.tracks, &compare_bones, sequence, node_idx)?;

    // Compare at offset 0 and best offset
    let offset_0_stats = compare_clip_frames(&capture_frames, &bones, &clip, &lib.tracks, &compare_bones, sequence, node_idx, frames_range, 0)?;
    let offset_best_stats = if best_offset != 0 {
        compare_clip_frames(&capture_frames, &bones, &clip, &lib.tracks, &compare_bones, sequence, node_idx, frames_range, best_offset)?
    } else {
        offset_0_stats.clone()
    };

    // Print report
    print_clip_report(&offset_0_stats, &offset_best_stats, best_offset, best_mean, &compare_bones);

    // Write JSON if requested
    if let Some(out) = json_out {
        let json = serde_json::json!({
            "offset_0": &offset_0_stats,
            "best_offset": best_offset,
            "best_mean_angle": best_mean,
            "offset_best": &offset_best_stats,
            "bones_compared": &compare_bones,
        });
        std::fs::write(out, serde_json::to_string_pretty(&json)?)?;
        println!("Results written to {}", out);
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClipComparisonStats {
    overall: BoneStats,
    worst_bones: Vec<(String, BoneStats)>,
    worst_frames: Vec<(usize, f32, String, f32)>, // (frame_idx, mean_angle, worst_bone, worst_angle)
    frames_compared: usize,
    frames_skipped: usize,
}

/// `exclude_bone` and every bone below it. A bone's parents are walked to the root, which is
/// its own parent in the cooked skeletons (UE3's `ParentIndex` 0 for bone 0; `bind_globals`
/// treats a parent at or above the bone's own index as none).
fn build_exclude_set(bones: &[dishonored_ir::BoneAsset], exclude_bone: &str) -> std::collections::HashSet<String> {
    let mut excluded = std::collections::HashSet::new();
    if let Some(idx) = bones.iter().position(|b| b.name.eq_ignore_ascii_case(exclude_bone)) {
        excluded.insert(bones[idx].name.clone());
        for (i, b) in bones.iter().enumerate() {
            let mut cur = i;
            loop {
                let p = bones[cur].parent;
                if p < 0 || p as usize >= cur {
                    break;
                }
                if p as usize == idx {
                    excluded.insert(b.name.clone());
                    break;
                }
                cur = p as usize;
            }
        }
    }
    excluded
}

fn find_best_clip_offset(
    capture_frames: &[FullFrame],
    bones: &[dishonored_ir::BoneAsset],
    clip: &dishonored_assets::edge_anim::BakedClip,
    tracks: &[String],
    compare_bones: &[String],
    sequence: &str,
    node_idx: Option<usize>,
) -> Result<(i32, f32)> {
    let mut best_offset = 0;
    let mut best_error = f32::MAX;

    for offset in -10..=10 {
        let stats = compare_clip_frames(capture_frames, bones, clip, tracks, compare_bones, sequence, node_idx, None, offset)?;
        if stats.overall.mean_angle < best_error {
            best_error = stats.overall.mean_angle;
            best_offset = offset;
        }
    }

    Ok((best_offset, best_error))
}

fn compare_clip_frames(
    capture_frames: &[FullFrame],
    bones: &[dishonored_ir::BoneAsset],
    clip: &dishonored_assets::edge_anim::BakedClip,
    tracks: &[String],
    compare_bones: &[String],
    sequence: &str,
    node_idx: Option<usize>,
    frames_range: Option<(usize, usize)>,
    time_offset_ticks: i32,
) -> Result<ClipComparisonStats> {
    let mut bone_stats: HashMap<String, (f32, f32, usize)> = HashMap::new();
    for bone in compare_bones {
        bone_stats.insert(bone.clone(), (0.0, 0.0, 0));
    }

    let mut worst_frames = Vec::new();
    let mut frames_compared = 0;
    let mut frames_skipped = 0;

    for (frame_idx, frame) in capture_frames.iter().enumerate() {
        if let Some((start, end)) = frames_range {
            if frame_idx < start || frame_idx >= end {
                continue;
            }
        }

        // Get the time from the playing node
        let clip_time = if let Some(node_n) = node_idx {
            get_node_time_by_index(frame, node_n)
        } else {
            get_playing_sequence_time(frame, sequence)
        };

        let Some(t) = clip_time else {
            frames_skipped += 1;
            continue;
        };

        // Apply time offset
        let t_adjusted = t + (time_offset_ticks as f32) / 60.0;

        // Pose the skeleton
        let posed_globals = pose_clip(bones, clip, tracks, t_adjusted);

        // Compare
        let (mut frame_sum, mut frame_n, mut frame_worst) = (0.0f32, 0usize, (String::new(), 0.0f32));
        for bone_name in compare_bones {
            if !frame.fresh.contains(bone_name) {
                continue;
            }
            if let Some(bone_idx) = bones.iter().position(|b| b.name.eq_ignore_ascii_case(bone_name)) {
                if let Some(capture_bone) = frame.bones.get(bone_name) {
                    let capture_q = Quat::from_array(capture_bone.q);
                    let posed_mat = posed_globals[bone_idx];
                    let (_, posed_q, _) = posed_mat.to_scale_rotation_translation();

                    let angle = bone_angle_degrees(capture_q.to_array(), posed_q.to_array());

                    if let Some(entry) = bone_stats.get_mut(bone_name) {
                        entry.0 += angle;
                        entry.1 = entry.1.max(angle);
                        entry.2 += 1;
                    }
                    frame_sum += angle;
                    frame_n += 1;
                    if angle > frame_worst.1 {
                        frame_worst = (bone_name.clone(), angle);
                    }
                }
            }
        }
        if frame_n > 0 {
            worst_frames.push((frame_idx, frame_sum / frame_n as f32, frame_worst.0, frame_worst.1));
        }
        frames_compared += 1;
    }

    worst_frames.sort_by(|a, b| b.1.total_cmp(&a.1));
    worst_frames.truncate(10);

    // Compute overall stats
    let mut bones_map = HashMap::new();
    let mut total_samples = 0;
    for (bone, (angle_sum, max_angle, count)) in bone_stats {
        if count > 0 {
            bones_map.insert(bone, BoneStats {
                mean_angle: angle_sum / count as f32,
                max_angle,
                mean_translation: 0.0,
                samples: count,
            });
            total_samples += count;
        }
    }

    let overall = if total_samples > 0 {
        BoneStats {
            mean_angle: bones_map.values().map(|s| s.mean_angle * s.samples as f32).sum::<f32>() / total_samples as f32,
            max_angle: bones_map.values().map(|s| s.max_angle).max_by(|a, b| a.total_cmp(b)).unwrap_or(0.0),
            mean_translation: 0.0,
            samples: total_samples,
        }
    } else {
        BoneStats { mean_angle: 0.0, max_angle: 0.0, mean_translation: 0.0, samples: 0 }
    };

    let mut worst_bones: Vec<_> = bones_map.into_iter().map(|(k, v)| (k, v)).collect();
    worst_bones.sort_by(|a, b| b.1.mean_angle.total_cmp(&a.1.mean_angle).then(b.1.max_angle.total_cmp(&a.1.max_angle)));

    Ok(ClipComparisonStats {
        overall,
        worst_bones: worst_bones.into_iter().take(10).collect(),
        worst_frames,
        frames_compared,
        frames_skipped,
    })
}

/// The playhead of the capture's node `node_idx` while it plays (the frames key nodes by their
/// index in the capture's node list).
fn get_node_time_by_index(frame: &FullFrame, node_idx: usize) -> Option<f32> {
    match frame.nodes.get(&node_idx.to_string())? {
        NodeState::Sequence { weight, time, .. } if *weight > 0.0 => Some(*time),
        _ => None,
    }
}

fn get_playing_sequence_time(frame: &FullFrame, seq_name: &str) -> Option<f32> {
    frame.nodes.values()
        .filter_map(|n| match n {
            NodeState::Sequence { weight, seq, time, .. } if seq == seq_name && *weight > 0.0 => Some((*weight, *time)),
            _ => None,
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, t)| t)
}

fn pose_clip(
    bones: &[dishonored_ir::BoneAsset],
    clip: &dishonored_assets::edge_anim::BakedClip,
    tracks: &[String],
    time: f32,
) -> Vec<Mat4> {
    use dishonored_assets::characters::bind_globals;
    // Start from bind pose
    let mut posed_bones = bones.to_vec();

    // Apply clip samples to matching bones
    for (track_idx, track_name) in tracks.iter().enumerate() {
        if let Some(bone_idx) = bones.iter().position(|b| b.name.eq_ignore_ascii_case(track_name)) {
            // A held sequence reports its length as the time; the clip's last key is its end.
            let (rot_opt, trans_opt) = clip.sample(time.clamp(0.0, (clip.duration - 1e-4).max(0.0)), track_idx);

            if let Some(rot) = rot_opt {
                posed_bones[bone_idx].rotation = rot;
            }

            // Apply translation only to root (bone 0) and direct children of root
            if let Some(trans) = trans_opt {
                if bone_idx == 0 || (bones[bone_idx].parent == 0) {
                    posed_bones[bone_idx].translation = trans;
                }
            }
        }
    }

    // Compute global transforms
    bind_globals(&posed_bones)
}

fn print_clip_report(
    stats_0: &ClipComparisonStats,
    stats_best: &ClipComparisonStats,
    best_offset: i32,
    best_mean: f32,
    bones: &[String],
) {
    println!("Frames compared: {}, skipped: {} (a bone counts only in the frames that refresh it: the original recomposes the LOD's bones only)", stats_0.frames_compared, stats_0.frames_skipped);
    println!("Bones: {}", bones.len());
    println!();

    println!("Offset 0 ticks:");
    println!("  Overall: mean angle {:.2} deg, max angle {:.2} deg ({} samples)",
        stats_0.overall.mean_angle, stats_0.overall.max_angle, stats_0.overall.samples);
    println!();

    println!("Best clip-time offset: {} ticks ({:+.3} s; the clip's time ahead of the capture's node time): mean angle {:.2} deg over the search", best_offset, best_offset as f32 / 60.0, best_mean);
    println!("  Overall: mean angle {:.2} deg, max angle {:.2} deg ({} samples)",
        stats_best.overall.mean_angle, stats_best.overall.max_angle, stats_best.overall.samples);
    println!();

    println!("Worst 10 bones at offset 0:");
    for (bone, stats) in &stats_0.worst_bones {
        println!("  {}: mean {:.2} deg, max {:.2} deg ({} samples)",
            bone, stats.mean_angle, stats.max_angle, stats.samples);
    }
    println!();

    println!("Worst 10 frames at offset 0:");
    for (idx, mean, worst_bone, worst_angle) in &stats_0.worst_frames {
        println!("  Frame {}: mean {:.2} deg; {} {:.2} deg", idx, mean, worst_bone, worst_angle);
    }
}

pub fn cmd_anim(args: &crate::Args) -> Result<()> {
    let cmd = args.0.get(1).map(String::as_str).unwrap_or("");
    match cmd {
        "info" => {
            let path = args.0.get(2).ok_or_else(|| anyhow!("oracle anim info FILE"))?;
            cmd_anim_info(path)?;
        }
        "compare" => {
            let capture = args.opt("--capture").ok_or_else(|| anyhow!("--capture required"))?;
            let port = args.opt("--port").ok_or_else(|| anyhow!("--port required"))?;
            let bones = args
                .opt("--bones")
                .map(|s| s.split(',').map(|b| b.trim().to_string()).collect());
            let offset = args.opt("--offset").and_then(|s| s.parse().ok());
            let pawn = args.opt("--pawn").and_then(|s| s.parse().ok());
            let json = args.opt("--json");
            cmd_anim_compare(&capture, &port, bones, offset, pawn, json.as_deref())?;
        }
        "clip" => {
            let capture = args.opt("--capture").ok_or_else(|| anyhow!("--capture required"))?;
            let skeleton_pkg = args.opt("--skeleton").ok_or_else(|| anyhow!("--skeleton PKG MESH_PATH required"))?;
            let skeleton_mesh = args.0.iter().position(|a| a == "--skeleton")
                .and_then(|i| args.0.get(i + 2).cloned())
                .ok_or_else(|| anyhow!("--skeleton PKG MESH_PATH required"))?;
            let set_pkg = args.opt("--set").ok_or_else(|| anyhow!("--set PKG PATH required"))?;
            let set_path = args.0.iter().position(|a| a == "--set")
                .and_then(|i| args.0.get(i + 2).cloned())
                .ok_or_else(|| anyhow!("--set PKG PATH required"))?;
            let sequence = args.opt("--sequence").ok_or_else(|| anyhow!("--sequence required"))?;
            let node = args.opt("--node").and_then(|s| s.parse().ok());
            let bones = args.opt("--bones").map(|s| s.split(',').map(|b| b.trim().to_string()).collect());
            let exclude_below = args.opt("--exclude-below");
            let frames = args.opt("--frames").and_then(|s| {
                let parts: Vec<&str> = s.split("..").collect();
                match parts.as_slice() {
                    [a, b] => Some((a.parse().ok()?, b.parse().ok()?)),
                    _ => None,
                }
            });
            let json = args.opt("--json");
            cmd_anim_clip(&capture, &skeleton_pkg, &skeleton_mesh, &set_pkg, &set_path, &sequence, node, bones, exclude_below, frames, json.as_deref())?;
        }
        _ => bail!("oracle anim info|compare|clip"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn test_delta_reconstruction() {
        let body = CaptureBody {
            bones: vec![(0, "bone_0".into()), (1, "bone_1".into()), (2, "bone_2".into())],
            nodes: None,
            component: "test".into(),
            frames: vec![
                CaptureFrame {
                    t: Some(0),
                    cs: Some(
                        vec![
                            ("0".into(), vec![7071, -7071, 0, 0, 0, 0, 0]),
                            ("1".into(), vec![7071, -7071, 0, 0, 100, 200, 300]),
                            ("2".into(), vec![7071, -7071, 0, 0, 400, 500, 600]),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                    ..Default::default()
                },
                CaptureFrame {
                    t: Some(1),
                    cs: Some(vec![("1".into(), vec![7000, -7000, 0, 0, 110, 210, 310])].into_iter().collect()),
                    ..Default::default()
                },
                CaptureFrame {
                    t: Some(2),
                    cs: Some(vec![("0".into(), vec![6900, -6900, 0, 0, 10, 20, 30])].into_iter().collect()),
                    ..Default::default()
                },
                CaptureFrame { t: Some(3), ..Default::default() },
                CaptureFrame { t: Some(4), ..Default::default() },
            ],
        };

        let frames = reconstruct_frames(&body).unwrap();
        assert_eq!(frames.len(), 5);

        // Frame 0: all bones present
        assert!(frames[0].bones.contains_key("bone_0"));
        assert!(frames[0].bones.contains_key("bone_1"));
        assert!(frames[0].bones.contains_key("bone_2"));

        // Frame 1: bone_1 updated, bone_0 and bone_2 carried forward
        assert!(frames[1].bones.contains_key("bone_0"));
        assert!(frames[1].bones.contains_key("bone_1"));
        assert!(frames[1].bones.contains_key("bone_2"));
        let b1_frame1 = frames[1].bones.get("bone_1").unwrap();
        assert!((b1_frame1.t[0] - 1.1).abs() < 0.01);

        // Frame 2: bone_0 updated, others carried forward
        assert!(frames[2].bones.contains_key("bone_0"));
        assert!(frames[2].bones.contains_key("bone_1"));
        let b0_frame2 = frames[2].bones.get("bone_0").unwrap();
        assert!((b0_frame2.t[0] - 0.1).abs() < 0.01);

        // Frames 3-4: all values carried forward
        assert_eq!(frames[3].bones.len(), 3);
        assert_eq!(frames[4].bones.len(), 3);
    }

    #[test]
    fn test_quaternion_angle() {
        // Identity quaternion
        let q1 = [0.0, 0.0, 0.0, 1.0];
        let q2 = [0.0, 0.0, 0.0, 1.0];
        let angle = bone_angle_degrees(q1, q2);
        assert!(angle < 0.01);

        // Rotated by 10 degrees about Z
        let q_z10 = Quat::from_axis_angle(glam::Vec3::Z, 10.0_f32.to_radians());
        let arr = [q_z10.x, q_z10.y, q_z10.z, q_z10.w];
        let angle = bone_angle_degrees(q1, arr);
        assert!((angle - 10.0).abs() < 0.1);

        // Sign flip: -q and q represent same rotation
        let q_neg = [-q1[0], -q1[1], -q1[2], -q1[3]];
        let angle = bone_angle_degrees(q1, q_neg);
        assert!(angle < 0.01);
    }

    #[test]
    fn test_offset_search() {
        // Frame i turns the bone i x 10 degrees about Z; the port dump is the same motion
        // starting 3 ticks later, so the capture tick is the port tick - 3 (identical frames
        // would score every offset alike).
        let turn = |i: usize| {
            let q = Quat::from_axis_angle(glam::Vec3::Z, (i as f32 * 10.0).to_radians());
            [q.x, q.y, q.z, q.w]
        };
        let capture_frames: Vec<FullFrame> = (0..8)
            .map(|i| FullFrame { tick: i as u64, bones: vec![("bone_0".to_string(), BoneTransform { q: turn(i), t: [0.0; 3] })].into_iter().collect(), nodes: HashMap::new(), fresh: std::iter::once("bone_0".to_string()).collect() })
            .collect();
        let port_frames: Vec<PortFrame> = (0..5)
            .map(|i| PortFrame { tick: (i + 3) as u64, id: 1, pos: [0.0; 3], yaw: 0.0, modifier: None, vel: None, phase: None, full: None, upper: None, ragdoll: false, bones: vec![PortBone { name: "bone_0".into(), q: turn(i), t: [0.0; 3] }] })
            .collect();

        let bones = vec!["bone_0".to_string()];
        let offset = find_best_offset(&capture_frames, &port_frames, &bones).unwrap();

        assert_eq!(offset, -3);
    }

    #[test]
    fn test_bind_globals() {
        use dishonored_assets::characters::bind_globals;
        // 3-bone chain
        let bones = vec![
            dishonored_ir::BoneAsset { name: "root".into(), parent: -1, rotation: Quat::IDENTITY, translation: Vec3::ZERO },
            dishonored_ir::BoneAsset { name: "child1".into(), parent: 0, rotation: Quat::IDENTITY, translation: Vec3::new(1.0, 0.0, 0.0) },
            dishonored_ir::BoneAsset { name: "child2".into(), parent: 1, rotation: Quat::IDENTITY, translation: Vec3::new(1.0, 0.0, 0.0) },
        ];
        let globals = bind_globals(&bones);
        assert_eq!(globals.len(), 3);
        // Root is identity
        let (_, _, t) = globals[0].to_scale_rotation_translation();
        assert!(t.length() < 0.01);
        // Child1 is at (1, 0, 0)
        let (_, _, t) = globals[1].to_scale_rotation_translation();
        assert!((t.x - 1.0).abs() < 0.01);
        // Child2 is at (2, 0, 0)
        let (_, _, t) = globals[2].to_scale_rotation_translation();
        assert!((t.x - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_exclude_set() {
        let bones = vec![
            dishonored_ir::BoneAsset { name: "root".into(), parent: -1, rotation: Quat::IDENTITY, translation: Vec3::ZERO },
            dishonored_ir::BoneAsset { name: "chest".into(), parent: 0, rotation: Quat::IDENTITY, translation: Vec3::ZERO },
            dishonored_ir::BoneAsset { name: "head".into(), parent: 1, rotation: Quat::IDENTITY, translation: Vec3::ZERO },
            dishonored_ir::BoneAsset { name: "neck".into(), parent: 1, rotation: Quat::IDENTITY, translation: Vec3::ZERO },
        ];
        let excluded = build_exclude_set(&bones, "head");
        assert!(excluded.contains("head"));
        assert!(!excluded.contains("chest"));
        assert!(!excluded.contains("neck"));
        // The cooked skeletons' root is its own parent: the walk must still end.
        let mut cooked = bones.clone();
        cooked[0].parent = 0;
        let excluded = build_exclude_set(&cooked, "chest");
        assert_eq!(excluded.len(), 3, "{excluded:?}");
        assert!(!excluded.contains("root"));
    }
}
