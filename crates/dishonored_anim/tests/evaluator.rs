//! The evaluator on hand-built trees (no install): a three-bone chain, clips of constant (or
//! linear) transforms behind a stub `Clips`, and the node rules the retail replay relies on:
//! per-bone masks, additive deltas, blend weights, slot channels, sync groups, root options and
//! root motion, stack channels, the tick order.

use dishonored_anim::pose::{self, Pose};
use dishonored_anim::tree::{SlotAction, SlotRequest, TreeInputs};
use dishonored_anim::{AnimTreeInstance, BakedClip, ClipRef, Clips};
use dishonored_ir::BoneAsset;
use dishonored_ir::anim_tree::{AnimChild, AnimNode, AnimTreeAsset, NodeProps, RootBoneOption, RootRotationOption};
use glam::{Quat, Vec3};
use std::collections::BTreeMap;

const EPS: f32 = 1e-4;

/// root (0) -> mid (1) -> tip (2), 10 uu apart along X, identity bind rotations.
fn skeleton() -> Vec<BoneAsset> {
    vec![
        BoneAsset { name: "root".into(), parent: -1, rotation: Quat::IDENTITY, translation: Vec3::ZERO },
        BoneAsset { name: "mid".into(), parent: 0, rotation: Quat::IDENTITY, translation: Vec3::new(10.0, 0.0, 0.0) },
        BoneAsset { name: "tip".into(), parent: 1, rotation: Quat::IDENTITY, translation: Vec3::new(10.0, 0.0, 0.0) },
    ]
}

/// A stub `Clips`: named clips over the three bones (Edge joint = bone index).
struct Stub {
    clips: BTreeMap<String, BakedClip>,
    bones: Vec<Option<usize>>,
    translate: Vec<bool>,
}

impl Stub {
    fn new() -> Self {
        Stub { clips: BTreeMap::new(), bones: vec![Some(0), Some(1), Some(2)], translate: vec![true; 3] }
    }

    /// A clip of `length` seconds holding `rot` on the joints listed (None: not driven).
    fn constant(mut self, name: &str, length: f32, rot: [Option<Quat>; 3]) -> Self {
        self.clips.insert(name.to_string(), BakedClip { num_frames: 2, sample_rate: 1.0 / length, duration: length, rotations: vec![rot.to_vec(); 2], translations: vec![vec![None; 3]; 2] });
        self
    }

    /// A clip whose root travels `speed` uu/s along X and turns `yaw` degrees/s about Z over
    /// one second (11 frames).
    fn moving(mut self, name: &str, speed: f32, yaw: f32) -> Self {
        let frames = 11;
        let rotations = (0..frames).map(|f| vec![Some(Quat::from_rotation_z((yaw * f as f32 * 0.1).to_radians())), Some(Quat::IDENTITY), None]).collect();
        let translations = (0..frames).map(|f| vec![Some(Vec3::new(speed * f as f32 * 0.1, 0.0, 0.0)), None, None]).collect();
        self.clips.insert(name.to_string(), BakedClip { num_frames: frames, sample_rate: 10.0, duration: 1.0, rotations, translations });
        self
    }
}

impl Clips for Stub {
    fn find(&self, name: &str) -> Option<ClipRef<'_>> {
        self.clips.get(name).map(|clip| ClipRef { clip, bones: &self.bones, translate: &self.translate })
    }
}

fn seq(name: &str, sequence: &str) -> AnimNode {
    AnimNode {
        name: name.into(),
        class: "AnimNodeSequence".into(),
        props: NodeProps::Sequence { seq: sequence.into(), rate: 1.0, looping: true, playing: true, no_notifies: false, sync_group: String::new(), synchronize: true, reverse_sync: false, root_bone: [RootBoneOption::Default; 3], root_rotation: [RootRotationOption::Default; 3], keep_undefined: false },
        ..Default::default()
    }
}

fn with_seq(mut n: AnimNode, f: impl FnOnce(&mut NodeProps)) -> AnimNode {
    f(&mut n.props);
    n
}

fn link(node: usize, weight: f32) -> AnimChild {
    AnimChild { name: String::new(), node: Some(node), weight, mirror: false, additive: false }
}

fn node(name: &str, class: &str, props: NodeProps, children: Vec<AnimChild>) -> AnimNode {
    AnimNode { name: name.into(), class: class.into(), props, children, ..Default::default() }
}

fn tree(nodes: Vec<AnimNode>, root: usize) -> AnimTreeAsset {
    AnimTreeAsset { name: "test".into(), root: Some(root), nodes, skel_controls: Vec::new() }
}

fn q(deg: f32) -> Quat {
    Quat::from_rotation_z(deg.to_radians())
}

fn angle(a: Quat, b: Quat) -> f32 {
    2.0 * a.dot(b).abs().min(1.0).acos().to_degrees()
}

/// Sequence `Base` -> per-bone filter (target `Over` from `mid`) -> additive (`Add` on the
/// tip): nodes 0 Base, 1 Over, 2 filter, 3 Add, 4 additive (root).
fn filter_tree(exclude_branch: bool, local_space: bool) -> AnimTreeAsset {
    let mut filter = node("Filter", "AnimNodeBlendPerBone", NodeProps::BlendPerBone { branches: vec!["mid".into()], local_space, exclude_branch }, vec![link(0, 1.0), link(1, 1.0)]);
    filter.raw.push(("Child2Weight".into(), "1".into()));
    let mut additive = node("Additive", "AnimNodeAdditiveBlending", NodeProps::AdditiveBlending, vec![link(2, 1.0), link(3, 1.0)]);
    additive.children[1].additive = true;
    tree(vec![seq("Base", "Base"), seq("Over", "Over"), filter, seq("Add", "Add"), additive], 4)
}

fn filter_clips() -> Stub {
    Stub::new().constant("Base", 1.0, [Some(q(30.0)), Some(q(10.0)), Some(q(10.0))]).constant("Over", 1.0, [Some(q(-20.0)), Some(q(40.0)), Some(q(50.0))]).constant("Add", 1.0, [None, None, Some(q(15.0))])
}

#[test]
fn branch_weights_cover_the_branch_down_and_exclude_the_branch_bone() {
    let bones = skeleton();
    assert_eq!(pose::branch_weights(&bones, &["mid".into()], false), vec![0.0, 1.0, 1.0]);
    assert_eq!(pose::branch_weights(&bones, &["mid".into()], true), vec![0.0, 0.0, 1.0]);
    assert_eq!(pose::branch_weights(&bones, &["MID".into()], false), vec![0.0, 1.0, 1.0], "bone names compare without case");
    assert_eq!(pose::branch_weights(&bones, &["root".into()], true), vec![0.0, 1.0, 1.0]);
}

#[test]
fn per_bone_filter_in_local_space_takes_the_target_below_the_branch() {
    let clips = filter_clips();
    let inst = AnimTreeInstance::new(&filter_tree(false, true), &skeleton());
    let p = inst.pose(&clips);
    assert!(angle(p[0].0, q(30.0)) < EPS, "root keeps the source");
    assert!(angle(p[1].0, q(40.0)) < EPS, "mid takes the target");
    // The additive delta on the tip: 15 degrees over the target's 50.
    assert!(angle(p[2].0, q(65.0)) < EPS, "tip: target plus the additive delta, {:?}", p[2].0);
    // Excluding the branch bone keeps `mid` with the source.
    let p = AnimTreeInstance::new(&filter_tree(true, true), &skeleton()).pose(&clips);
    assert!(angle(p[1].0, q(10.0)) < EPS && angle(p[2].0, q(65.0)) < EPS);
}

#[test]
fn per_bone_filter_in_component_space_keeps_the_target_relative_to_the_root() {
    let clips = filter_clips();
    let bones = skeleton();
    let p = AnimTreeInstance::new(&filter_tree(false, false), &bones).pose(&clips);
    let cs = pose::to_component(&pose::parents(&bones), &p);
    // The target's component-space rotation of `mid` is -20 + 40 = 20 degrees; under the
    // source's root (30) its local rotation becomes -10.
    assert!(angle(cs[1].0, q(20.0)) < EPS, "mid in component space {:?}", cs[1].0);
    assert!(angle(p[1].0, q(-10.0)) < EPS);
    // Half the filter weight blends the component-space rotations halfway.
    let mut inst = AnimTreeInstance::new(&filter_tree(false, false), &bones);
    inst.set_children_weights(2, &[1.0, 0.5]);
    let cs = pose::to_component(&pose::parents(&bones), &inst.pose(&clips));
    assert!(angle(cs[1].0, q(30.0)) < 0.01, "halfway between 40 (source) and 20 (target): {:?}", cs[1].0);
}

#[test]
fn additive_weight_scales_the_delta_and_untracked_bones_stay_put() {
    let clips = filter_clips();
    let mut inst = AnimTreeInstance::new(&filter_tree(false, true), &skeleton());
    inst.set_children_weights(4, &[1.0, 0.5]);
    let p = inst.pose(&clips);
    assert!(angle(p[2].0, q(57.5)) < EPS, "half the 15 degree delta: {:?}", p[2].0);
    assert!(angle(p[1].0, q(40.0)) < EPS, "bones the additive clip does not drive get the identity delta");
    inst.set_children_weights(4, &[1.0, 0.0]);
    assert!(angle(inst.pose(&clips)[2].0, q(50.0)) < EPS);
}

#[test]
fn blend_list_mixes_children_by_weight() {
    let clips = Stub::new().constant("A", 1.0, [Some(q(0.0)), Some(q(0.0)), None]).constant("B", 1.0, [Some(q(80.0)), Some(q(40.0)), None]);
    let t = tree(vec![seq("A", "A"), seq("B", "B"), node("List", "AnimNodeBlendList", NodeProps::BlendList, vec![link(0, 0.75), link(1, 0.25)])], 2);
    let p = AnimTreeInstance::new(&t, &skeleton()).pose(&clips);
    // Normalised lerp of the two (UE3's weighted quaternion sum).
    let expect = Quat::IDENTITY.lerp(q(80.0), 0.25).normalize();
    assert!(angle(p[0].0, expect) < EPS, "{:?} vs {expect:?}", p[0].0);
    assert!(angle(p[2].0, Quat::IDENTITY) < EPS, "undriven bones keep the reference pose");
}

#[test]
fn slot_channel_blends_in_linearly_and_out_before_its_end() {
    let clips = Stub::new().constant("Loop", 1.0, [Some(q(0.0)), None, None]).constant("Act", 1.0, [Some(q(90.0)), None, None]);
    let t = tree(vec![seq("Loop", "Loop"), with_seq(seq("Chan", ""), |p| if let NodeProps::Sequence { looping, .. } = p { *looping = false }), node("Slot", "DisAnimNodeSlot", NodeProps::Slot { early_anim_end: true }, vec![link(0, 1.0), link(1, 0.0)])], 2);
    let mut inst = AnimTreeInstance::new(&t, &skeleton());
    let play = [SlotRequest { slot: "Slot".into(), action: SlotAction::Play { sequence: "Act".into(), rate: 1.0, blend_in: 0.2, blend_out: Some(0.1), looping: false, start: 0.0 } }];
    let dt = 0.05;
    inst.update(&clips, &TreeInputs { slots: &play, ..Default::default() }, dt);
    // The request lands after the tick: the channel reads time 0 and weight 0 on its tick.
    assert_eq!(inst.children_weights(2), vec![1.0, 0.0]);
    assert_eq!(inst.sequence(1).unwrap().time, 0.0);
    let mut seen = Vec::new();
    for _ in 0..4 {
        inst.update(&clips, &TreeInputs::default(), dt);
        seen.push(inst.children_weights(2)[1]);
    }
    for (w, e) in seen.iter().zip([0.25, 0.5, 0.75, 1.0]) {
        assert!((w - e).abs() < 1e-5, "channel weights {seen:?}");
    }
    assert!((inst.sequence(1).unwrap().time - 0.2).abs() < 1e-5);
    // At 0.1 s left the channel blends back over the time left, done by the clip's end.
    for _ in 0..14 {
        inst.update(&clips, &TreeInputs::default(), dt);
    }
    assert!((inst.sequence(1).unwrap().time - 0.9).abs() < 1e-4);
    assert_eq!(inst.children_weights(2)[1], 1.0, "blend-out starts on the next tick");
    inst.update(&clips, &TreeInputs::default(), dt);
    assert!((inst.children_weights(2)[1] - 0.5).abs() < 1e-3, "{:?}", inst.children_weights(2));
    inst.update(&clips, &TreeInputs::default(), dt);
    assert_eq!(inst.children_weights(2), vec![1.0, 0.0]);
    // Out of the blend the channel is no longer relevant and no longer advances.
    assert_eq!(inst.total_weight(1), 0.0);
    assert!((inst.sequence(1).unwrap().time - 0.95).abs() < 1e-4);
    inst.update(&clips, &TreeInputs::default(), dt);
    assert!((inst.sequence(1).unwrap().time - 0.95).abs() < 1e-4);
}

#[test]
fn slot_channel_holds_its_last_frame_without_a_blend_out() {
    let clips = Stub::new().constant("Loop", 1.0, [Some(q(0.0)), None, None]).constant("Act", 0.5, [Some(q(90.0)), None, None]);
    let t = tree(vec![seq("Loop", "Loop"), seq("Chan", ""), node("Slot", "DisAnimNodeSlot", NodeProps::Slot { early_anim_end: true }, vec![link(0, 1.0), link(1, 0.0)])], 2);
    let mut inst = AnimTreeInstance::new(&t, &skeleton());
    let play = [SlotRequest { slot: "Slot".into(), action: SlotAction::Play { sequence: "Act".into(), rate: 1.0, blend_in: 0.0, blend_out: None, looping: false, start: 0.0 } }];
    inst.update(&clips, &TreeInputs { slots: &play, ..Default::default() }, 0.1);
    assert_eq!(inst.children_weights(2), vec![0.0, 1.0], "no blend-in time: at once");
    for _ in 0..8 {
        inst.update(&clips, &TreeInputs::default(), 0.1);
    }
    let s = inst.sequence(1).unwrap();
    assert!(!s.playing && (s.time - 0.5).abs() < 1e-5, "stopped at its end");
    assert_eq!(inst.children_weights(2), vec![0.0, 1.0], "held");
    assert!(angle(inst.pose(&clips)[0].0, q(90.0)) < EPS);
}

#[test]
fn sync_group_shares_one_phase_with_the_heaviest_as_master() {
    let clips = Stub::new().constant("Short", 1.0, [Some(q(0.0)), None, None]).constant("Long", 2.0, [Some(q(0.0)), None, None]);
    let grouped = |name: &str, s: &str, reverse: bool| {
        with_seq(seq(name, s), |p| {
            if let NodeProps::Sequence { sync_group, reverse_sync, .. } = p {
                *sync_group = "G".into();
                *reverse_sync = reverse;
            }
        })
    };
    for (reverse, expect) in [(false, 0.5), (true, 1.5)] {
        let t = tree(vec![grouped("A", "Short", false), grouped("B", "Long", reverse), node("List", "AnimNodeBlendList", NodeProps::BlendList, vec![link(0, 0.6), link(1, 0.4)])], 2);
        let mut inst = AnimTreeInstance::new(&t, &skeleton());
        inst.update(&clips, &TreeInputs::default(), 0.25);
        // The master (`Short`, the heavier) advances 0.25 of its 1 s; `Long` takes the same
        // relative position (reversed: 1 - 0.25).
        assert!((inst.sequence(0).unwrap().time - 0.25).abs() < 1e-5);
        assert!((inst.sequence(1).unwrap().time - expect).abs() < 1e-4, "reverse {reverse}: {}", inst.sequence(1).unwrap().time);
        // Now `Long` is the heavier: it leads, `Short` follows.
        inst.set_children_weights(2, &[0.3, 0.7]);
        inst.update(&clips, &TreeInputs::default(), 0.25);
        let long = inst.sequence(1).unwrap().time;
        let pos = if reverse { 1.0 - long / 2.0 } else { long / 2.0 };
        assert!((inst.sequence(0).unwrap().time - pos).abs() < 1e-4, "short follows the long one's phase");
    }
}

#[test]
fn unsynchronized_group_members_run_on_their_own() {
    let clips = Stub::new().constant("Short", 1.0, [Some(q(0.0)), None, None]).constant("Long", 2.0, [Some(q(0.0)), None, None]);
    let a = with_seq(seq("A", "Short"), |p| if let NodeProps::Sequence { sync_group, .. } = p { *sync_group = "G".into() });
    let b = with_seq(seq("B", "Long"), |p| {
        if let NodeProps::Sequence { sync_group, synchronize, .. } = p {
            *sync_group = "G".into();
            *synchronize = false;
        }
    });
    let t = tree(vec![a, b, node("List", "AnimNodeBlendList", NodeProps::BlendList, vec![link(0, 0.6), link(1, 0.4)])], 2);
    let mut inst = AnimTreeInstance::new(&t, &skeleton());
    inst.update(&clips, &TreeInputs::default(), 0.25);
    assert!((inst.sequence(1).unwrap().time - 0.25).abs() < 1e-5, "its own 0.25 s, not the master's phase");
}

#[test]
fn root_options_keep_the_reference_root_and_extract_root_motion() {
    let clips = Stub::new().moving("Walk", 100.0, 90.0);
    let options = |bone: RootBoneOption, rot: RootRotationOption| {
        with_seq(seq("Walk", "Walk"), |p| {
            if let NodeProps::Sequence { root_bone, root_rotation, looping, .. } = p {
                *root_bone = [bone; 3];
                *root_rotation = [rot; 3];
                *looping = false;
            }
        })
    };
    // Default: the clip moves and turns the root.
    let mut inst = AnimTreeInstance::new(&tree(vec![options(RootBoneOption::Default, RootRotationOption::Default)], 0), &skeleton());
    inst.update(&clips, &TreeInputs::default(), 0.5);
    let p: Pose = inst.pose(&clips);
    assert!((p[0].1.x - 50.0).abs() < 1e-3 && angle(p[0].0, q(45.0)) < 0.01);
    assert_eq!(inst.root_delta(), (Vec3::ZERO, 0.0), "nothing extracted");
    // Discard: the reference root, nothing extracted.
    let mut inst = AnimTreeInstance::new(&tree(vec![options(RootBoneOption::Discard, RootRotationOption::Discard)], 0), &skeleton());
    inst.update(&clips, &TreeInputs::default(), 0.5);
    let p = inst.pose(&clips);
    assert!(p[0].1.length() < 1e-5 && angle(p[0].0, Quat::IDENTITY) < EPS);
    assert_eq!(inst.root_delta(), (Vec3::ZERO, 0.0));
    // Translate / extract: the reference root in the pose, the travel and turn of the tick as
    // root motion.
    let mut inst = AnimTreeInstance::new(&tree(vec![options(RootBoneOption::Translate, RootRotationOption::Extract)], 0), &skeleton());
    inst.update(&clips, &TreeInputs::default(), 0.2);
    let p = inst.pose(&clips);
    assert!(p[0].1.length() < 1e-5 && angle(p[0].0, Quat::IDENTITY) < EPS);
    let (travel, yaw) = inst.root_delta();
    assert!((travel - Vec3::new(20.0, 0.0, 0.0)).length() < 1e-3, "{travel:?}");
    assert!((yaw - 18.0).abs() < 0.01, "{yaw}");
    // Through the mesh's rotation into the pawn's frame.
    let mut inst = AnimTreeInstance::new(&tree(vec![options(RootBoneOption::Translate, RootRotationOption::Extract)], 0), &skeleton()).with_mesh_rotation(q(90.0));
    inst.update(&clips, &TreeInputs::default(), 0.2);
    assert!((inst.root_delta().0 - Vec3::new(0.0, 20.0, 0.0)).length() < 1e-3);
}

#[test]
fn looping_root_motion_counts_the_lap() {
    let clips = Stub::new().moving("Walk", 100.0, 0.0);
    let n = with_seq(seq("Walk", "Walk"), |p| {
        if let NodeProps::Sequence { root_bone, .. } = p {
            *root_bone = [RootBoneOption::Translate; 3];
        }
    });
    let mut inst = AnimTreeInstance::new(&tree(vec![n], 0), &skeleton());
    inst.set_node_state(0, 1.0, "Walk", 0.9, 1.0);
    inst.update(&clips, &TreeInputs::default(), 0.2);
    assert!((inst.sequence(0).unwrap().time - 0.1).abs() < 1e-5);
    assert!((inst.root_delta().0.x - 20.0).abs() < 1e-3, "through the loop's end: {:?}", inst.root_delta());
}

#[test]
fn stack_channels_layer_over_the_source_keeping_undefined_bones() {
    let clips = Stub::new().constant("Src", 1.0, [Some(q(10.0)), Some(q(20.0)), Some(q(30.0))]).constant("Lids", 1.0, [None, None, Some(q(90.0))]);
    let chan = with_seq(seq("Blink", "Lids"), |p| if let NodeProps::Sequence { keep_undefined, .. } = p { *keep_undefined = true });
    let plain = seq("Plain", "Lids");
    for (c, keep) in [(chan, true), (plain, false)] {
        let t = tree(vec![seq("Src", "Src"), c, node("Stack", "ArkAnimNodeStack", NodeProps::Other, vec![link(0, 1.0), link(1, 0.5)])], 2);
        let p = AnimTreeInstance::new(&t, &skeleton()).pose(&clips);
        assert!(angle(p[2].0, q(60.0)) < EPS, "the tip halfway to the channel's 90");
        // Without `m_bDontUseRefPoseOnNonDefinedBones` the channel's undefined bones are the
        // reference pose, blended in at the channel's weight.
        let mid = if keep { q(20.0) } else { q(10.0) };
        assert!(angle(p[1].0, mid) < EPS, "keep {keep}: {:?}", p[1].0);
    }
}

#[test]
fn tick_order_adds_a_shared_child_after_its_last_parent() {
    // 0 leaf, 1 shared slot (under 2 and 3), 2 filter target, 3 filter (root).
    let t = tree(
        vec![
            seq("Leaf", "A"),
            node("Shared", "AnimNodeSlot", NodeProps::Slot { early_anim_end: true }, vec![link(0, 1.0)]),
            node("Target", "AnimNodeSlot", NodeProps::Slot { early_anim_end: true }, vec![link(1, 1.0)]),
            node("Filter", "AnimNodeBlendPerBone", NodeProps::BlendPerBone { branches: vec!["mid".into()], local_space: false, exclude_branch: false }, vec![link(1, 1.0), link(2, 1.0)]),
        ],
        3,
    );
    let inst = AnimTreeInstance::new(&t, &skeleton());
    assert_eq!(inst.tick_order(), &[3, 2, 1, 0]);
}

#[test]
fn deterministic_across_instances() {
    let clips = filter_clips();
    let t = filter_tree(false, false);
    let run = || {
        let mut inst = AnimTreeInstance::new(&t, &skeleton());
        for _ in 0..37 {
            inst.update(&clips, &TreeInputs::default(), 1.0 / 60.0);
        }
        inst.pose(&clips)
    };
    assert_eq!(run(), run());
}
