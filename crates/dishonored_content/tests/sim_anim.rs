//! The simulation's NPC trees with the install's animation (`dishonored_content::InstallAnim`,
//! skips without the install): a guard's `NPC_Human_at` follows its layers' requests and its
//! physics, swaps to `NPC_Dead_at` when it dies, and its state round-trips through
//! serialization and resumes tick for tick.

use dishonored_sim::anim::layer;
use dishonored_sim::input::{PlayerInput, buttons};
use dishonored_sim::state::{PawnKind, WorldState, awareness};
use dishonored_sim::{spawn_npc, spawn_player, step_world};
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;

fn floor() -> CollisionWorld {
    let (a, b, c, d) = (Vec3::new(-5000., -5000., 0.), Vec3::new(5000., -5000., 0.), Vec3::new(5000., 5000., 0.), Vec3::new(-5000., 5000., 0.));
    let tri = |x: Vec3, y: Vec3, z: Vec3| Tri { a: x, b: y, c: z, n: (y - x).cross(z - x).normalize(), channels: channel::ALL, source: 0 };
    CollisionWorld::from_triangles(vec![tri(a, b, c), tri(a, c, d)], vec!["fixture".into()])
}

#[test]
fn guard_tree_follows_its_layers_and_round_trips() {
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    let cfg = dishonored_content::load_sim_config(&content).unwrap();
    let Some(assets) = cfg.anim_source.npc_tree(false) else { panic!("the install's NPC tree") };
    assert!(assets.shape.node_index("FullBodySlot").is_some());
    let world = floor();
    let mut st = WorldState::default();
    spawn_player(&mut st, 1, Vec3::new(0., 0., 88.5), 0.0, &cfg);
    spawn_npc(&mut st, 100, PawnKind::Guard, 1, Vec3::new(120., 0., 88.5), 180.0, &cfg);
    st.player_mut(100).unwrap().ai_awareness = awareness::IN_COMBAT;
    let idle = |yaw| PlayerInput { yaw, ..Default::default() };
    let step = |st: &mut WorldState, guard: PlayerInput| {
        let t = st.tick;
        step_world(st, &[(1, PlayerInput { tick: t, ..idle(0.0) }), (100, PlayerInput { tick: t, ..guard })], &world, &cfg);
    };
    for _ in 0..30 {
        step(&mut st, idle(180.0));
    }
    let tree = st.anim[&100].tree.clone().expect("the guard's tree");
    assert_eq!(tree.tree, 0, "NPC_Human_at while alive");
    // A swing: the full-body layer plays the attack and the tree's slot takes it.
    step(&mut st, PlayerInput { buttons: buttons::PRIMARY, ..idle(180.0) });
    let swing = st.anim[&100].layers[layer::FULL_BODY].as_ref().expect("the swing").token;
    for _ in 0..3 {
        step(&mut st, idle(180.0));
    }
    let tree = st.anim[&100].tree.clone().unwrap();
    assert_eq!(tree.requested[layer::FULL_BODY], swing, "the tree has the swing's request");
    // The state round-trips and resumes identically.
    let json = serde_json::to_string(&st).unwrap();
    let mut back: WorldState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.anim, st.anim);
    for _ in 0..20 {
        step(&mut st, idle(180.0));
        step(&mut back, idle(180.0));
        assert_eq!(back.anim, st.anim);
    }
    // Dead: NPC_Dead_at.
    st.player_mut(100).unwrap().vitals.health = 1.0;
    let mut killed = false;
    for _ in 0..200 {
        step(&mut st, idle(180.0));
        if !st.player(100).unwrap().alive {
            killed = true;
            break;
        }
        // The player swings until the guard falls.
        let t = st.tick;
        step_world(&mut st, &[(1, PlayerInput { tick: t, buttons: buttons::PRIMARY, ..idle(0.0) }), (100, PlayerInput { tick: t, ..idle(180.0) })], &world, &cfg);
    }
    assert!(killed, "the guard dies");
    step(&mut st, idle(180.0));
    assert_eq!(st.anim[&100].tree.as_ref().map(|t| t.tree), Some(1), "NPC_Dead_at when dead");
}
