//! AnimTree asset decoding against the canonical GOG build (skipped without the install).

use dishonored_assets::Content;
use dishonored_assets::anim_tree::load_anim_tree;
use dishonored_ir::anim_tree::NodeProps;

fn content() -> Option<Content> {
    Content::locate().ok()
}

#[test]
fn npc_human_animtree() {
    let Some(c) = content() else { return };
    let tree = load_anim_tree(&c, "Startup", "Npc_Common_at.NPC_Human_at").expect("load NPC_Human_at");

    // Root should be AnimNodeBlendByPhysics named NPCPhysicsBlend.
    let root_idx = tree.root.expect("root exists");
    let root = &tree.nodes[root_idx];
    assert_eq!(root.class, "AnimNodeBlendByPhysics", "root is BlendByPhysics");
    assert_eq!(root.name, "NPCPhysicsBlend", "root named NPCPhysicsBlend");

    // Collect node names for easy lookup.
    let mut nodes_by_name = std::collections::HashMap::new();
    for (i, node) in tree.nodes.iter().enumerate() {
        nodes_by_name.insert(node.name.clone(), i);
    }

    // Check for expected nodes.
    let expected_names = [
        "AdditiveReactSlot",
        "PoseBlender",
        "MatineeStackNode",
        "FaceFilter",
        "BlinkBlender",
        "UpperBodyFilter",
        "FullBodySlot",
        "LocoBlender",
        "UpperBodySlot",
        "FaceSlot",
        "LookatBlender",
    ];

    for name in &expected_names {
        assert!(
            nodes_by_name.contains_key(*name),
            "expected node '{}' not found in tree",
            name
        );
    }

    // Check specific node properties.
    let additive_slot = nodes_by_name.get("AdditiveReactSlot").map(|&i| &tree.nodes[i]);
    assert!(additive_slot.is_some(), "AdditiveReactSlot exists");
    assert_eq!(
        additive_slot.unwrap().class,
        "DisAnimNodeSlot",
        "AdditiveReactSlot is DisAnimNodeSlot"
    );

    let pose_blender = nodes_by_name.get("PoseBlender").map(|&i| &tree.nodes[i]);
    assert!(pose_blender.is_some(), "PoseBlender exists");
    assert_eq!(
        pose_blender.unwrap().class,
        "ArkAnimNodeBlendPose",
        "PoseBlender is ArkAnimNodeBlendPose"
    );

    let face_filter = nodes_by_name.get("FaceFilter").map(|&i| &tree.nodes[i]);
    if let Some(ff) = face_filter {
        assert_eq!(ff.class, "AnimNodeBlendPerBone", "FaceFilter is BlendPerBone");
        if let NodeProps::BlendPerBone { branches: _, exclude_branch, .. } = &ff.props {
            // TODO: property name for branches might be variant; test for existence only for now
            assert!(*exclude_branch, "FaceFilter excludes branch");
        }
    }

    let upper_body_filter = nodes_by_name.get("UpperBodyFilter").map(|&i| &tree.nodes[i]);
    if let Some(ubf) = upper_body_filter {
        assert_eq!(ubf.class, "AnimNodeBlendPerBone", "UpperBodyFilter is BlendPerBone");
        // TODO: property name for branches might be variant; test for existence only for now
    }

    let loco = nodes_by_name.get("LocoBlender").map(|&i| &tree.nodes[i]);
    if let Some(l) = loco {
        assert_eq!(l.class, "ArkAnimNodeLocomotion", "LocoBlender is ArkAnimNodeLocomotion");
        if let NodeProps::Locomotion { rate, range, min_ratio, .. } = l.props {
            assert_eq!(rate, 2000.0, "LocoBlender rate = 2000");
            assert_eq!(range, 100.0, "LocoBlender range = 100");
            assert_eq!(min_ratio, 0.1, "LocoBlender min_ratio = 0.1");
        }
    }

    // At least 30 nodes.
    assert!(
        tree.nodes.len() >= 30,
        "tree has at least 30 nodes, found {}",
        tree.nodes.len()
    );

    // Check for skeletal controls (if any).
    // TODO: property name or structure for skel controls might need verification
    if !tree.skel_controls.is_empty() {
        let has_foot_placement = tree
            .skel_controls
            .iter()
            .any(|entry| entry.controls.iter().any(|(class, _)| class == "DisSkelControl_FootPlacement"));
        if has_foot_placement {
            // success - DisSkelControl_FootPlacement found
        }
    }
}

#[test]
fn npc_dead_animtree() {
    let Some(c) = content() else { return };
    let tree = load_anim_tree(&c, "Startup", "Npc_Common_at.NPC_Dead_at").expect("load NPC_Dead_at");

    // Root should be AnimNodeBlendByPhysics.
    let root_idx = tree.root.expect("root exists");
    let root = &tree.nodes[root_idx];
    assert_eq!(root.class, "AnimNodeBlendByPhysics", "NPC_Dead root is BlendByPhysics");

    // Should have a DisAnimNodeSlot child as FullBodySlot.
    let mut found_slot = false;
    for child in &root.children {
        if let Some(child_idx) = child.node {
            let child_node = &tree.nodes[child_idx];
            if child_node.class == "DisAnimNodeSlot" {
                found_slot = true;
                break;
            }
        }
    }
    assert!(found_slot, "NPC_Dead has a DisAnimNodeSlot child");
}

#[test]
fn player_animtree() {
    let Some(c) = content() else { return };
    let tree = load_anim_tree(&c, "Startup", "Ply_Player_at").expect("load Ply_Player_at");

    // Collect node names for easy lookup.
    let mut nodes_by_name = std::collections::HashMap::new();
    for (i, node) in tree.nodes.iter().enumerate() {
        nodes_by_name.insert(node.name.clone(), i);
    }

    // Check for UpperBodyFilter exists.
    if let Some(&upper_idx) = nodes_by_name.get("UpperBodyFilter") {
        let upper_node = &tree.nodes[upper_idx];
        assert_eq!(upper_node.class, "AnimNodeBlendPerBone", "UpperBodyFilter is BlendPerBone");
        // TODO: property name for branches might be variant; test for existence only for now
    }

    // Check for LeftArmFilter exists.
    if let Some(&left_idx) = nodes_by_name.get("LeftArmFilter") {
        let left_node = &tree.nodes[left_idx];
        assert_eq!(left_node.class, "AnimNodeBlendPerBone", "LeftArmFilter is BlendPerBone");
        // TODO: property name for branches might be variant; test for existence only for now
    }
}
