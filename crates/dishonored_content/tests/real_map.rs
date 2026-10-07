//! Simulation on the real Boyle exterior collision (skips without the install).
use dishonored_content::{DEFAULT_MAP, load_match};
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{MoveMode, WorldState};
use dishonored_sim::{spawn_player, step_world};

/// The AI light probe against the original (U-121): the raw light value of the player's
/// `DishonoredVisibilityComponent` (`m_fCurrentLightValue`) read with the engine hook at
/// these pawn locations on Boyle Ext, standing and crouched (2026-10-06,
/// research/dishonored/HOOK_TRIAGE.md). Printed for comparison; the range check is loose
/// because the port's probe sums light actors instead of the pawn's light environment.
#[test]
fn light_probe_against_the_original() {
    if dishonored_assets::Content::locate().is_err() {
        return;
    }
    let mc = load_match(DEFAULT_MAP).unwrap();
    let lvl = &mc.ai_level;
    let pts: [([f32; 3], f32); 14] = [
        ([-3017.3, -2406.6, -15.8], 0.3126),
        ([-3162.5, -2539.2, -15.8], 0.2091),
        ([-3231.7, -2722.4, -15.8], 0.1897),
        ([-3296.5, -2881.8, -28.2], 0.1933),
        ([-3373.4, -3094.3, -215.8], 0.1714),
        ([-3480.6, -3335.6, -216.0], 0.1972),
        ([-3623.1, -3688.8, -216.3], 0.1918),
        ([-3087.7, -2366.2, -38.3], 0.3100),
        ([-3161.3, -2548.0, -38.3], 0.2592),
        ([-3234.2, -2727.9, -38.3], 0.1880),
        ([-3304.4, -2901.4, -64.2], 0.1881),
        ([-3375.0, -3077.5, -238.3], 0.1678),
        ([-3487.7, -3354.2, -238.5], 0.1981),
        ([-3614.3, -3667.0, -238.8], 0.1944),
    ];
    let mut worst: f32 = 0.0;
    for (p, orig) in pts {
        let v = dishonored_ai::light_at_with(&lvl.lights, lvl.sky_light, glam::Vec3::from(p), &mc.collision);
        println!("({:.0}, {:.0}, {:.0}): port {v:.4} original {orig:.4} ratio {:.2}", p[0], p[1], p[2], v / orig);
        worst = worst.max((v - orig).abs());
    }
    println!("largest difference {worst:.4}");
    assert!(worst < 1.0);
}

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
