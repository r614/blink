//! Skeletal animation comparison: oracle capture vs port dump.

use anyhow::{anyhow, bail, Context, Result};
use glam::Quat;
use serde::{Deserialize, Serialize};
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
    children: Option<Vec<usize>>,
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
    nodes: Option<BTreeMap<String, Vec<i32>>>,
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

/// Port dump frame.
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
            let last_tick = frames.iter().find_map(|f| f.t).unwrap_or(0);
            println!("    Tick range: {} to {}", first_tick, last_tick);

            let has_cs = frames.iter().filter(|f| f.cs.is_some()).count();
            let has_nodes = frames.iter().filter(|f| f.nodes.is_some()).count();
            let has_blend = frames.iter().filter(|f| f.blend.is_some()).count();
            println!("    With cs: {}, nodes: {}, blend: {}", has_cs, has_nodes, has_blend);
        }

        // Find sequences that play (weight > 0)
        if let Some(nodes_list) = body.nodes.as_ref() {
            let mut playing_seqs = std::collections::HashSet::new();
            for frame in frames {
                if let Some(frame_nodes) = &frame.nodes {
                    for (node_idx, values) in frame_nodes {
                        if let Ok(idx) = node_idx.parse::<usize>() {
                            if let Some(node) = nodes_list.get(idx) {
                                if node.sequence == Some(true) && values.len() >= 1 {
                                    let weight = values[0] as f32 / 1000.0;
                                    if weight > 0.0 && !node.name.is_empty() && node.name != "None" {
                                        playing_seqs.insert(node.name.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !playing_seqs.is_empty() {
                println!("  Playing sequences: {}", playing_seqs.iter().cloned().collect::<Vec<_>>().join(", "));
            }
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
                let weight = values[0] as f32 / 1000.0;
                let relevant = values.len() > 1 && values[1] != 0;

                if values.len() >= 5 {
                    // Sequence node
                    let seq = if values[2] < 0 {
                        "".to_string()
                    } else {
                        // Try to decode the sequence name from the value if needed
                        values.get(2).map(|_| "".to_string()).unwrap_or_default()
                    };
                    let time = values.get(3).map(|v| *v as f32 / 1000.0).unwrap_or(0.0);
                    let rate = values.get(4).map(|v| *v as f32 / 1000.0).unwrap_or(1.0);
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
    worst_ticks: Vec<(u64, Vec<(String, f32, f32)>)>,
    sequences: SequenceStats,
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
            let capture_idx = (pf.tick as i32 + offset) as usize;
            if capture_idx >= capture_frames.len() {
                continue;
            }
            let cf = &capture_frames[capture_idx];

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
    let mut seq_offset_diffs = Vec::new();

    for pf in port_frames {
        let capture_idx = (pf.tick as i32 + offset) as usize;
        if capture_idx >= capture_frames.len() {
            continue;
        }
        let cf = &capture_frames[capture_idx];

        // Compare bones
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
            }
        }

        // Compare sequences
        let capture_seqs = get_playing_sequences(cf);
        if let Some(port_seq) = &pf.full {
            if !capture_seqs.contains(&port_seq.seq) {
                if seq_mismatches.len() < 10 {
                    seq_mismatches.push(SequenceMismatch {
                        tick: pf.tick,
                        capture_seqs: capture_seqs.clone(),
                        port_seq: Some(port_seq.seq.clone()),
                    });
                }
            } else {
                // Same sequence, check time offset
                seq_offset_diffs.push(pf.full.as_ref().unwrap().t);
            }
        }
    }

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
        worst_ticks: Vec::new(),
        sequences: SequenceStats {
            mismatches: seq_mismatches.len(),
            examples: seq_mismatches,
            same_seq_offset: if seq_offset_diffs.is_empty() {
                None
            } else {
                Some(seq_offset_diffs.iter().sum::<f32>() / seq_offset_diffs.len() as f32)
            },
        },
    })
}

fn get_playing_sequences(frame: &FullFrame) -> Vec<String> {
    let mut seqs = Vec::new();
    for (_, node) in &frame.nodes {
        if let NodeState::Sequence { weight, seq, .. } = node {
            if *weight > 0.0 && !seq.is_empty() {
                seqs.push(seq.clone());
            }
        }
    }
    seqs
}

fn print_comparison_report(results: &ComparisonResults, offset: i32, _bones: &[String]) {
    println!("Offset: {} ticks", offset);
    println!();
    println!("Overall: mean angle {:.2}°, max angle {:.2}°, mean translation {:.2} uu",
        results.overall.mean_angle, results.overall.max_angle, results.overall.mean_translation);
    println!();

    println!("Worst 10 bones:");
    for (bone, stats) in &results.worst_bones {
        println!("  {}: mean {:.2}°, max {:.2}°, trans {:.2} uu ({} samples)",
            bone, stats.mean_angle, stats.max_angle, stats.mean_translation, stats.samples);
    }
    println!();

    println!("Sequences: {} mismatches", results.sequences.mismatches);
    for m in &results.sequences.examples {
        println!("  Tick {}: capture {:?}, port {:?}",
            m.tick, m.capture_seqs, m.port_seq);
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
        let capture_frame = FullFrame {
            tick: 0,
            bones: vec![("bone_0".into(), BoneTransform { q: [0.0, 0.0, 0.0, 1.0], t: [0.0, 0.0, 0.0] })]
                .into_iter()
                .collect(),
            nodes: HashMap::new(),
        };

        let capture_frames = vec![
            capture_frame.clone(),
            capture_frame.clone(),
            capture_frame.clone(),
            capture_frame.clone(),
            capture_frame.clone(),
        ];

        let mut port_frames = Vec::new();
        for i in 0..5 {
            port_frames.push(PortFrame {
                tick: (i + 3) as u64,
                id: 1,
                pos: [0.0, 0.0, 0.0],
                yaw: 0.0,
                modifier: None,
                vel: None,
                phase: None,
                full: None,
                upper: None,
                ragdoll: false,
                bones: vec![PortBone {
                    name: "bone_0".into(),
                    q: [0.0, 0.0, 0.0, 1.0],
                    t: [0.0, 0.0, 0.0],
                }],
            });
        }

        let bones = vec!["bone_0".to_string()];
        let offset = find_best_offset(&capture_frames, &port_frames, &bones).unwrap();

        assert_eq!(offset, -3);
    }
}
