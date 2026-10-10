//! Skeletal animation comparison: oracle capture vs port dump.

use anyhow::{anyhow, bail, Context, Result};
use glam::Quat;
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
                    (cf.bones.get(bone), pf.bones.iter().find(|b| &b.name == bone))
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
                (cf.bones.get(bone), pf.bones.iter().find(|b| &b.name == bone))
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
        _ => bail!("oracle anim info|compare"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .map(|i| FullFrame { tick: i as u64, bones: vec![("bone_0".to_string(), BoneTransform { q: turn(i), t: [0.0; 3] })].into_iter().collect(), nodes: HashMap::new() })
            .collect();
        let port_frames: Vec<PortFrame> = (0..5)
            .map(|i| PortFrame { tick: (i + 3) as u64, id: 1, pos: [0.0; 3], yaw: 0.0, modifier: None, vel: None, phase: None, full: None, upper: None, ragdoll: false, bones: vec![PortBone { name: "bone_0".into(), q: turn(i), t: [0.0; 3] }] })
            .collect();

        let bones = vec!["bone_0".to_string()];
        let offset = find_best_offset(&capture_frames, &port_frames, &bones).unwrap();

        assert_eq!(offset, -3);
    }
}
