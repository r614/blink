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
