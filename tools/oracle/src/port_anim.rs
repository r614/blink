//! The port side of the animation residuals (research/dishonored/ORACLE.md, "Animation
//! residuals"): for every NPC a spec records with `Bones` / `AnimState`, the scenario runner
//! samples the simulation's own tree state (`WorldState::anim[id].tree`, the
//! `dishonored_anim::TreeState` the simulation evaluates with the install's `AnimSource`) once
//! per recorded tick, in the capture's shape (`dishonored_oracle::anim::AnimFrame`):
//!
//! - every node's `NodeTotalWeight`, a sequence node's sequence, time and rate, a blend node's
//!   children weights, numbered by the cooked `TickArrayIndex` (`TreeShape::tick_index`, the
//!   capture's numbering);
//! - the component-space bones of the tick's pose (`TreeState::pose` with the simulation's clips,
//!   `dishonored_anim::pose::to_component` over the profile's skeleton), for the bones the
//!   capture refreshes on that tick only.
//!
//! An NPC whose spec pins its ambient idle (`NpcSetup::ambient`, `ambient_age`) gets the same
//! pin in its tree ([`pin_ambient`]) unless the simulation already plays it: the brain's pinned
//! idle reaches the simulation through its request (`pawn_states::Request::ambient`, played
//! through `StateNPCMasterPlayAnim` from its age), so the pin only stands for an NPC whose pawn
//! states have not taken it yet.

use anyhow::Result;
use dishonored_anim::tree::{FilterTarget, SlotAction, SlotRequest, TreeInputs, ZERO_WEIGHT};
use dishonored_anim::{AnimTreeInstance, TreeShape};
use dishonored_ir::anim_tree::{AnimTreeAsset, NodeProps};
use dishonored_ir::npc_anim::NpcAnim;
use dishonored_oracle::anim::{AnimFrame, AnimTrack, BoneGroup, NodeSample, SeqSample, TreeLayout, bone_groups};
use dishonored_sim::SimConfig;
use dishonored_sim::state::{PlayerId, WorldState};
use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

/// The simulation's NPC profile (`dishonored_assets::sim_anim::NPC_TWEAK`): its skeleton and the
/// structure of `NPC_Human_at` in the capture's numbering.
pub struct Profile {
    pub bones: Vec<String>,
    pub parents: Vec<Option<usize>>,
    /// Lowercase bone name -> group.
    pub groups: HashMap<String, BoneGroup>,
    pub layout: TreeLayout,
}

/// The profile, loaded once from the install (the simulation's own `InstallAnim` loads the same
/// data for its trees).
pub fn profile() -> std::result::Result<&'static Profile, String> {
    static P: OnceLock<std::result::Result<Profile, String>> = OnceLock::new();
    P.get_or_init(|| load().map_err(|e| format!("{e:#}"))).as_ref().map_err(|e| e.clone())
}

fn load() -> Result<Profile> {
    let content = dishonored_assets::Content::locate()?;
    let data = dishonored_assets::sim_anim::load_npc_tree_data(&content)?;
    let bones: Vec<String> = data.skeleton.iter().map(|b| b.name.clone()).collect();
    let parents = dishonored_anim::pose::parents(&data.skeleton);
    let groups = bone_groups(&bones, &parents).into_iter().zip(&bones).map(|(g, n)| (n.to_ascii_lowercase(), g)).collect();
    let shape = TreeShape::new(&data.human, &data.skeleton, data.mesh_rotation);
    Ok(Profile { layout: layout(&data.human, &shape), bones, parents, groups })
}

/// Node names and the sequence nodes below every node, by `TickArrayIndex`.
fn layout(tree: &AnimTreeAsset, shape: &TreeShape) -> TreeLayout {
    let mut out = TreeLayout::default();
    for (i, n) in tree.nodes.iter().enumerate() {
        let Some(k) = shape.tick_index(i) else { continue };
        if !n.name.is_empty() {
            out.names.entry(n.name.to_ascii_lowercase()).or_insert(k as u32);
        }
        let (mut seqs, mut seen, mut stack) = (BTreeSet::new(), BTreeSet::new(), vec![i]);
        while let Some(j) = stack.pop() {
            if !seen.insert(j) {
                continue;
            }
            if matches!(tree.nodes[j].props, NodeProps::Sequence { .. }) {
                if let Some(t) = shape.tick_index(j) {
                    seqs.insert(t as u32);
                }
            }
            stack.extend(tree.nodes[j].children.iter().filter_map(|c| c.node));
        }
        out.below.insert(k as u32, seqs.into_iter().collect());
    }
    out
}

/// One recorded NPC's track.
pub struct Recorder {
    pub subject: String,
    pub id: PlayerId,
    pub track: AnimTrack,
    /// Capture bone index -> profile bone index (by name).
    map: Vec<Option<u32>>,
}

impl Recorder {
    pub fn new(subject: &str, id: PlayerId, profile: &Profile, capture: Option<&AnimTrack>) -> Self {
        let map = capture.map(|c| c.bones.iter().map(|n| profile.bones.iter().position(|b| b.eq_ignore_ascii_case(n)).map(|i| i as u32)).collect()).unwrap_or_default();
        Recorder { subject: subject.to_string(), id, track: AnimTrack { bones: profile.bones.clone(), frames: Vec::new() }, map }
    }

    /// Sample the NPC's tree at `tick` (nothing while it has none: no install, or before its
    /// first update).
    pub fn sample(&mut self, tick: u32, world: &WorldState, cfg: &SimConfig, profile: &Profile, capture: Option<&AnimTrack>) {
        let Some(t) = world.anim.get(&self.id).and_then(|a| a.tree.as_ref()) else { return };
        let Some(assets) = cfg.anim_source.npc_tree(t.tree == 1) else { return };
        let inst = AnimTreeInstance::from_parts(assets.shape.clone(), t.state.clone());
        let mut f = AnimFrame { tick, ..Default::default() };
        for &node in inst.tick_order() {
            let Some(k) = inst.tick_index(node) else { continue };
            let weight = inst.total_weight(node);
            let seq = inst.sequence(node).map(|s| SeqSample { name: s.name.clone(), time: s.time, rate: s.rate });
            if seq.is_none() {
                let cw = inst.children_weights(node);
                if !cw.is_empty() {
                    f.blend.insert(k as u32, cw);
                }
            }
            f.nodes.insert(k as u32, NodeSample { weight, relevant: weight > ZERO_WEIGHT, seq });
        }
        if let Some(cf) = capture.and_then(|c| c.frame(tick)).filter(|cf| !cf.bones.is_empty()) {
            let local = t.state.pose(&assets.shape, assets.clips.as_ref());
            let cs = dishonored_anim::pose::to_component(&profile.parents, &local);
            for ci in cf.bones.keys() {
                let Some(p) = self.map.get(*ci as usize).copied().flatten() else { continue };
                if let Some((q, tr)) = cs.get(p as usize) {
                    f.bones.insert(p, (q.to_array(), tr.to_array()));
                }
            }
        }
        self.track.frames.push(f);
    }
}

/// The ambient idle of a spec's NPC (`NpcSetup::ambient`) requested in its tree as the AI's
/// ambient chain plays it: `FullBodySlot`, the chain step's blend-in and its group's blend-out
/// (`AmbientIdle`, `GameplayConfig::npc_anim`), from `start` seconds in; with the `FaceSlot`
/// playing the step's face sequence, else `<sequence>_Face` when the sets have one, and the face
/// filter raised over 0.1 s (what the capture of `stealth/detect_1300` shows with
/// `Empty_Idle_04`; the chain's `m_FaceAnimName` is empty and who drives the face is U-974).
/// Nothing when the NPC has no tree yet or its tree already plays the sequence (a simulation
/// that requests it itself). Returns what was requested.
pub fn pin_ambient(world: &mut WorldState, cfg: &SimConfig, id: PlayerId, sequence: &str, start: f32) -> Option<String> {
    let assets = cfg.anim_source.npc_tree(false)?;
    let tree = world.anim.get_mut(&id)?.tree.as_mut().filter(|t| t.tree == 0)?;
    let inst = AnimTreeInstance::from_parts(assets.shape.clone(), tree.state.clone());
    if inst.tick_order().iter().any(|&n| inst.sequence(n).is_some_and(|s| s.name.eq_ignore_ascii_case(sequence) && s.playing)) {
        return None;
    }
    let chain = cfg.gameplay.npc_anim.defs.chain(NpcAnim::AmbientIdle);
    let found = chain.and_then(|c| c.groups.iter().find_map(|g| g.steps.iter().find(|s| s.names.iter().any(|n| n.eq_ignore_ascii_case(sequence))).map(|s| (g, s))));
    let (blend_in, blend_out, face) = match found {
        Some((g, s)) => (s.blend_in, g.blend_out, s.face.clone()),
        None => (0.2, 0.3, String::new()),
    };
    let face = if face.is_empty() { format!("{sequence}_Face") } else { face };
    let mut slots = vec![SlotRequest { slot: "FullBodySlot".into(), action: SlotAction::Play { sequence: sequence.to_string(), rate: 1.0, blend_in, blend_out: Some(blend_out), looping: false, start } }];
    let mut filters = Vec::new();
    let with_face = assets.clips.find(&face).is_some();
    if with_face {
        slots.push(SlotRequest { slot: "FaceSlot".into(), action: SlotAction::Play { sequence: face.clone(), rate: 1.0, blend_in, blend_out: Some(blend_out), looping: false, start } });
        filters.push(FilterTarget { node: "FaceFilter".into(), target: 1.0, time: 0.1 });
    }
    tree.state.request(&assets.shape, &TreeInputs { slots: &slots, filters: &filters, ..Default::default() });
    Some(format!("{sequence} on FullBodySlot from {start:.3} s (blend in {blend_in}, out {blend_out}){}", if with_face { format!(", {face} on FaceSlot") } else { String::new() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::BoneAsset;
    use dishonored_ir::anim_tree::{AnimChild, AnimNode, RootBoneOption, RootRotationOption};
    use glam::{Quat, Vec3};

    fn node(name: &str, class: &str, props: NodeProps, tick: usize, children: &[usize]) -> AnimNode {
        AnimNode {
            name: name.into(),
            class: class.into(),
            props,
            children: children.iter().map(|c| AnimChild { name: String::new(), node: Some(*c), weight: 1.0, mirror: false, additive: false }).collect(),
            raw: vec![("TickArrayIndex".into(), tick.to_string())],
            ..Default::default()
        }
    }

    fn seq(name: &str, tick: usize) -> AnimNode {
        let props = NodeProps::Sequence { seq: name.into(), rate: 1.0, looping: true, playing: true, no_notifies: false, sync_group: String::new(), synchronize: true, reverse_sync: false, root_bone: [RootBoneOption::Default; 3], root_rotation: [RootRotationOption::Default; 3], keep_undefined: false };
        node(name, "AnimNodeSequence", props, tick, &[])
    }

    /// The layout numbers nodes by `TickArrayIndex`, lists the sequences of each subtree and
    /// leaves the look-at node out of them.
    #[test]
    fn layout_lists_the_sequences_below_each_node() {
        let tree = AnimTreeAsset {
            name: "t".into(),
            root: Some(4),
            nodes: vec![
                seq("Loop", 3),
                seq("Chan", 4),
                node("FullBodySlot", "DisAnimNodeSlot", NodeProps::Slot { early_anim_end: true }, 2, &[0, 1]),
                node("LookatBlender", "ArkAnimNodeLookAt", NodeProps::AimOffset { grids: vec!["G".into()] }, 5, &[]),
                node("Root", "AnimNodeBlendList", NodeProps::BlendList, 1, &[2, 3]),
            ],
            skel_controls: Vec::new(),
        };
        let bones = vec![BoneAsset { name: "root".into(), parent: -1, rotation: Quat::IDENTITY, translation: Vec3::ZERO }];
        let l = layout(&tree, &TreeShape::new(&tree, &bones, Quat::IDENTITY));
        assert_eq!(l.resolve("FullBodySlot"), Some(2));
        assert_eq!(l.resolve("4"), Some(4));
        assert_eq!(l.below[&2], vec![3, 4]);
        assert_eq!(l.below[&1], vec![3, 4]);
        assert_eq!(l.below[&3], vec![3]);
        assert!(l.below[&5].is_empty());
    }
}