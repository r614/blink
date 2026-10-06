//! Simulation on the real Boyle exterior collision (skips without the install).
use dishonored_content::{DEFAULT_MAP, load_match};
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{MoveMode, WorldState};
use dishonored_sim::{spawn_player, step_world};

#[test]
fn spawn_and_walk_on_boyle() {
    if dishonored_assets::Content::locate().is_err() {
        return;
    }
    let mc = load_match(DEFAULT_MAP).unwrap();
    println!("collision tris {} spawns {}", mc.collision.tris.len(), mc.spawns.len());
    assert!(mc.collision.tris.len() > 50_000);
    assert!(!mc.spawns.is_empty());
    let mut grounded = 0;
    for (i, s) in mc.spawns.iter().enumerate() {
        let mut st = WorldState::default();
        spawn_player(&mut st, 1, s.position, s.yaw, &mc.sim);
        for _ in 0..120 {
            step_world(&mut st, &[], &mc.collision, &mc.sim);
        }
        let p0 = *st.player(1).unwrap();
        // Walk forward for 3 seconds.
        for _ in 0..180 {
            let inp = PlayerInput { yaw: s.yaw, forward: 127, ..Default::default() };
            step_world(&mut st, &[(1, inp)], &mc.collision, &mc.sim);
        }
        let p1 = *st.player(1).unwrap();
        println!("spawn {i}: settle {:?} z {:.1}->{:.1}, walked {:.0}uu, end mode {:?} z {:.1}", p0.mode, s.position.z, p0.position.z, (p1.position - p0.position).truncate().length(), p1.mode, p1.position.z);
        assert!(p1.position.z > s.position.z - 1500.0, "fell through the world at spawn {i}");
        if p0.mode == MoveMode::Walking {
            grounded += 1;
        }
    }
    assert!(grounded >= mc.spawns.len() / 2, "most spawns should settle on ground");
}

/// Boyle's two chains: a pawn dropped beside each grabs it by touch and climbs it within the
/// limits (`dishonored_sim::climb`, research/dishonored/PLAYER.md "Climbing").
#[test]
fn climb_boyle_chains() {
    if dishonored_assets::Content::locate().is_err() {
        return;
    }
    for map in ["L_Boyle_Int_P", "L_Boyle_Ext_P"] {
        let mc = load_match(map).unwrap();
        let chains = dishonored_world::WorldQuery::climbables(&mc.collision).to_vec();
        assert_eq!(chains.len(), 1, "{map}: {chains:?}");
        let c = &chains[0];
        let (low, high) = dishonored_sim::climb::limits(c, 87.5);
        println!("{map}: {} anchor {} limits {low:.1}..{high:.1}", c.name, c.anchor);
        // Dropped 30 uu behind the chain's middle, facing it.
        let mut st = WorldState::default();
        let start = glam::Vec3::new(c.anchor.x - 30.0, c.anchor.y, (low + high) * 0.5);
        spawn_player(&mut st, 1, start, 0.0, &mc.sim);
        let idle = PlayerInput { yaw: 0.0, pitch: 30.0, ..Default::default() };
        for _ in 0..5 {
            step_world(&mut st, &[(1, idle)], &mc.collision, &mc.sim);
        }
        assert_eq!(st.player(1).unwrap().mode, MoveMode::Climbing, "{map}: {}", st.player(1).unwrap().position);
        let up = PlayerInput { forward: 127, ..idle };
        for _ in 0..900 {
            step_world(&mut st, &[(1, up)], &mc.collision, &mc.sim);
        }
        let p = *st.player(1).unwrap();
        println!("{map}: top {}", p.position);
        assert_eq!(p.mode, MoveMode::Climbing);
        assert!(p.position.z <= high + 0.01 && p.position.z > start.z + 50.0, "{map}: {}", p.position);
        let down = PlayerInput { forward: -127, ..idle };
        for _ in 0..900 {
            step_world(&mut st, &[(1, down)], &mc.collision, &mc.sim);
        }
        let p = *st.player(1).unwrap();
        println!("{map}: bottom {}", p.position);
        assert!(p.position.z >= low - 0.01 && p.position.z < start.z - 50.0, "{map}: {}", p.position);
    }
}
