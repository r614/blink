//! AnimTree decoding against the canonical GOG build (skipped without the install): the
//! structure research/dishonored/ANIMATION.md section 2 reconstructed by hand.

use dishonored_assets::Content;
use dishonored_assets::anim_tree::load_anim_tree;
use dishonored_ir::anim_tree::{AnimNode, AnimTreeAsset, NodeProps, RootBoneOption, RootRotationOption};

fn content() -> Option<Content> {
    Content::locate().ok()
}

fn node<'a>(tree: &'a AnimTreeAsset, name: &str) -> &'a AnimNode {
    tree.nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("no node {name} in {}", tree.name))
}

fn branches(n: &AnimNode) -> (Vec<String>, bool, bool) {
    match &n.props {
        NodeProps::BlendPerBone { branches, local_space, exclude_branch } => (branches.clone(), *local_space, *exclude_branch),
        other => panic!("{} is not a per-bone blend: {other:?}", n.name),
    }
}

#[test]
fn the_npc_tree_decodes_to_its_documented_structure() {
    let Some(c) = content() else { return };
    let tree = load_anim_tree(&c, "Startup", "Npc_Common_at.NPC_Human_at").unwrap();
    let root = &tree.nodes[tree.root.expect("root")];
    assert_eq!((root.class.as_str(), root.name.as_str()), ("AnimNodeBlendByPhysics", "NPCPhysicsBlend"));
    assert!(tree.nodes.len() >= 30, "{} nodes", tree.nodes.len());
    for (name, class) in [
        ("AdditiveReactSlot", "DisAnimNodeSlot"),
        ("PoseBlender", "ArkAnimNodeBlendPose"),
        ("MatineeStackNode", "ArkAnimNodeStack"),
        ("FaceFilter", "AnimNodeBlendPerBone"),
        ("BlinkBlender", "ArkAnimNodeStack"),
        ("UpperBodyFilter", "AnimNodeBlendPerBone"),
        ("FullBodySlot", "DisAnimNodeSlot"),
        ("LocoBlender", "ArkAnimNodeLocomotion"),
        ("UpperBodySlot", "DisAnimNodeSlot"),
        ("FaceSlot", "AnimNodeSlot"),
        ("LookatBlender", "ArkAnimNodeLookAt"),
    ] {
        assert_eq!(node(&tree, name).class, class, "{name}");
    }
    // The face slot drives the bones below `head_jnt`, not `head_jnt` itself; both filters
    // blend in component space.
    assert_eq!(branches(node(&tree, "FaceFilter")), (vec!["head_jnt".to_string()], true, true));
    let (upper, _, _) = branches(node(&tree, "UpperBodyFilter"));
    assert_eq!(upper, vec!["spine_0_jnt".to_string()]);
    // The shared child: one node, reached from every physics mode of the root.
    let additive = tree.nodes.iter().position(|n| n.name == "AdditiveReactSlot").unwrap();
    assert!(root.children.iter().filter(|c| c.node == Some(additive)).count() >= 2, "AdditiveReactSlot shared under the physics modes");
    // The locomotion tunables (class defaults: the tree's node only sets the fast turn blend).
    match node(&tree, "LocoBlender").props {
        NodeProps::Locomotion { rate, range, min_ratio, modifier_blend } => {
            assert_eq!((rate, range, min_ratio, modifier_blend), (2000.0, 100.0, 0.1, 0.5));
        }
        ref other => panic!("LocoBlender: {other:?}"),
    }
    // Its first speed-mode slot is the idle loop in the `LocoGroup` sync group; the blink
    // channel is the `Blink` sequence.
    let loco = node(&tree, "LocoBlender");
    assert_eq!(loco.children.len(), 9, "8 speed mode slots + the specific anim");
    let first = &tree.nodes[loco.children[0].node.expect("SpeedModeSlot0")];
    match &first.props {
        NodeProps::Sequence { seq, looping, sync_group, rate, .. } => {
            assert_eq!((seq.as_str(), *looping, sync_group.as_str(), *rate), ("Empty_Idle", true, "LocoGroup", 1.0));
        }
        other => panic!("SpeedModeSlot0: {other:?}"),
    }
    // The idle slot plays on its own (`bSynchronize` false); slots 1-4 run backwards in the group
    // (`Rate` -1, `bReverseSync`); every loop discards the root's motion, the channels of the
    // full-body slot extract it.
    match &first.props {
        NodeProps::Sequence { synchronize, root_bone, root_rotation, .. } => {
            assert!(!synchronize);
            assert_eq!((*root_bone, *root_rotation), ([RootBoneOption::Discard; 3], [RootRotationOption::Discard; 3]));
        }
        other => panic!("SpeedModeSlot0: {other:?}"),
    }
    for k in 1..=4 {
        let n = &tree.nodes[loco.children[k].node.unwrap()];
        assert!(matches!(n.props, NodeProps::Sequence { rate: -1.0, reverse_sync: true, synchronize: true, .. }), "SpeedModeSlot{k}: {:?}", n.props);
    }
    let full = node(&tree, "FullBodySlot");
    assert!(matches!(full.props, NodeProps::Slot { early_anim_end: true }), "the class default");
    let channel = &tree.nodes[full.children[1].node.unwrap()];
    assert!(matches!(&channel.props, NodeProps::Sequence { root_bone, root_rotation, .. } if root_bone.contains(&RootBoneOption::Translate) && *root_rotation == [RootRotationOption::Extract; 3]), "{:?}", channel.props);
    let blink = node(&tree, "BlinkBlender");
    let blink_seq = &tree.nodes[blink.children[1].node.expect("blink channel")];
    assert!(matches!(&blink_seq.props, NodeProps::Sequence { seq, keep_undefined: true, .. } if seq == "Blink"), "{:?}", blink_seq.props);
    // The look-at grids: nine head, nine torso.
    match &node(&tree, "LookatBlender").props {
        NodeProps::AimOffset { grids } => {
            assert_eq!(grids.len(), 18);
            assert_eq!(grids[0], "ADD_Empty_AimHeadIdle_UpLeft");
            assert_eq!(grids[9], "ADD_Empty_AimTorsoIdle_UpLeft");
        }
        other => panic!("LookatBlender: {other:?}"),
    }
    // Skeletal controls: foot placement on both feet, the spine bender, the eyes.
    let feet: Vec<&str> = tree.skel_controls.iter().filter(|e| e.controls.iter().any(|(c, _)| c == "DisSkelControl_FootPlacement")).map(|e| e.bone.as_str()).collect();
    assert_eq!(feet, ["foot_R_jnt", "foot_L_jnt"]);
    assert!(tree.skel_controls.iter().any(|e| e.bone == "spine_0_jnt" && e.controls[0].0 == "DisSkelControl_SpineBender"));
    // Every node keeps its raw properties.
    assert!(tree.nodes.iter().all(|n| !n.raw.is_empty() || n.children.is_empty()));
}

#[test]
fn the_dead_tree_is_a_physics_blend_over_one_slot() {
    let Some(c) = content() else { return };
    let tree = load_anim_tree(&c, "Startup", "Npc_Common_at.NPC_Dead_at").unwrap();
    let root = &tree.nodes[tree.root.expect("root")];
    assert_eq!(root.class, "AnimNodeBlendByPhysics");
    assert!(root.children.iter().filter_map(|c| c.node).any(|i| tree.nodes[i].class == "DisAnimNodeSlot"));
}

#[test]
fn the_player_tree_filters_match_the_viewmodel() {
    let Some(c) = content() else { return };
    // The player trees are package-named objects; a bare package name is refused.
    assert!(load_anim_tree(&c, "Startup", "Ply_Player_at").is_err());
    let tree = load_anim_tree(&c, "Startup", "Ply_Player_at.Ply_Player_at").unwrap();
    // `viewmodel.rs`: `UpperBodyFilter` branches spine_3_jnt, Collarbone_R_Jnt, Collarbone_L_Jnt;
    // `LeftArmFilter` Collarbone_L_Jnt; both blend in component space.
    let (upper, upper_local, _) = branches(node(&tree, "UpperBodyFilter"));
    for b in ["spine_3_jnt", "Collarbone_R_Jnt", "Collarbone_L_Jnt"] {
        assert!(upper.iter().any(|x| x.eq_ignore_ascii_case(b)), "UpperBodyFilter {upper:?} lacks {b}");
    }
    let (left, left_local, _) = branches(node(&tree, "LeftArmFilter"));
    assert_eq!(left.len(), 1);
    assert!(left[0].eq_ignore_ascii_case("Collarbone_L_Jnt"), "{left:?}");
    assert!(!upper_local && !left_local, "the player's filters blend in component space (not bForceLocalSpaceBlend; viewmodel.rs)");
    // The state pickers instantiate other tree assets (`Ply_Player_MeleeAttack_at`...).
    let refs: Vec<&AnimNode> = tree.nodes.iter().filter(|n| matches!(n.props, NodeProps::TreeRef { .. })).collect();
    assert!(!refs.is_empty(), "no state picker / tree ref in the player tree");
    assert!(refs.iter().any(|n| matches!(&n.props, NodeProps::TreeRef { templates } if templates.iter().any(|(name, path)| name == "MeleeAttack" && path.ends_with("Ply_Player_MeleeAttack_at")))), "{:?}", refs.iter().map(|n| &n.props).collect::<Vec<_>>());
}
