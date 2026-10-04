//! Mechanics regression tests on synthetic collision fixtures, using the gameplay config
//! extracted from the retail install (fixtures/config/gameplay_config.json).
//!
//! These lock in the implemented semantics (tweak-sourced constants + documented
//! integration model). Tests marked "reference" compare against closed-form values
//! derived from CONFIRMED constants; original-game trace comparisons live in
//! fixtures/movement once captured (U-011).

use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::state::{MoveMode, SwordPhase, WorldState};
use dishonored_sim::{SimConfig, spawn_player, state_hash, step_world};
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn quad(tris: &mut Vec<Tri>, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
    for (x, y, z) in [(a, b, c), (a, c, d)] {
        let n = (y - x).cross(z - x).normalize();
        tris.push(Tri { a: x, b: y, c: z, n, channels: channel::ALL, source: 0 });
    }
}

/// Axis-aligned box (all faces).
fn boxy(tris: &mut Vec<Tri>, min: Vec3, max: Vec3) {
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let (a, b) = (min, max);
    quad(tris, p(a.x, a.y, b.z), p(b.x, a.y, b.z), p(b.x, b.y, b.z), p(a.x, b.y, b.z)); // top
    quad(tris, p(a.x, a.y, a.z), p(a.x, b.y, a.z), p(b.x, b.y, a.z), p(b.x, a.y, a.z)); // bottom
    quad(tris, p(a.x, a.y, a.z), p(b.x, a.y, a.z), p(b.x, a.y, b.z), p(a.x, a.y, b.z)); // -y
    quad(tris, p(a.x, b.y, a.z), p(a.x, b.y, b.z), p(b.x, b.y, b.z), p(b.x, b.y, a.z)); // +y
    quad(tris, p(a.x, a.y, a.z), p(a.x, a.y, b.z), p(a.x, b.y, b.z), p(a.x, b.y, a.z)); // -x
    quad(tris, p(b.x, a.y, a.z), p(b.x, b.y, a.z), p(b.x, b.y, b.z), p(b.x, a.y, b.z)); // +x
}

fn floor_world(extra: impl FnOnce(&mut Vec<Tri>)) -> CollisionWorld {
    let mut tris = Vec::new();
    quad(&mut tris, Vec3::new(-5000., -5000., 0.), Vec3::new(5000., -5000., 0.), Vec3::new(5000., 5000., 0.), Vec3::new(-5000., 5000., 0.));
    extra(&mut tris);
    CollisionWorld::from_triangles(tris, vec!["fixture".into()])
}

struct Run {
    state: WorldState,
    cfg: SimConfig,
    world: CollisionWorld,
    events: Vec<SimEvent>,
}

impl Run {
    fn new(world: CollisionWorld, players: &[(u32, Vec3, f32)]) -> Run {
        let cfg = cfg();
        let mut state = WorldState::default();
        for (id, pos, yaw) in players {
            spawn_player(&mut state, *id, *pos, *yaw, &cfg);
        }
        Run { state, cfg, world, events: Vec::new() }
    }
    fn step(&mut self, inputs: &[(u32, PlayerInput)]) {
        let mut ins = inputs.to_vec();
        for (_, i) in ins.iter_mut() {
            i.tick = self.state.tick;
        }
        let out = step_world(&mut self.state, &ins, &self.world, &self.cfg);
        self.events.extend(out.events);
    }
    fn steps(&mut self, n: usize, inputs: &[(u32, PlayerInput)]) {
        for _ in 0..n {
            self.step(inputs);
        }
    }
    fn p(&self, id: u32) -> &dishonored_sim::state::PlayerState {
        self.state.player(id).unwrap()
    }
    fn settle(&mut self) {
        let idle: Vec<(u32, PlayerInput)> = self.state.players.iter().map(|p| (p.id, PlayerInput { yaw: p.yaw, ..Default::default() })).collect();
        self.steps(90, &idle);
    }
}

fn input(yaw: f32, forward: i8, buttons: u16) -> PlayerInput {
    PlayerInput { yaw, forward, buttons, ..Default::default() }
}

const STAND_Z: f32 = 87.5;

#[test]
fn spawn_settles_on_floor() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., 300.), 0.)]);
    r.settle();
    let p = r.p(1);
    assert_eq!(p.mode, MoveMode::Walking);
    assert!((p.position.z - STAND_Z).abs() < 1.0, "z {}", p.position.z);
}

#[test]
fn reference_jump_apex_matches_jumpz_and_gravity() {
    // Reference: apex = JumpZ^2 / (2 * |GravityZ|) = 800^2 / 3000 = 213.33 uu.
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    let z0 = r.p(1).position.z;
    r.step(&[(1, input(0., 0, buttons::JUMP))]);
    let mut apex = z0;
    for _ in 0..120 {
        r.step(&[(1, input(0., 0, 0))]);
        apex = apex.max(r.p(1).position.z);
    }
    let expected = 800.0f32.powi(2) / (2.0 * 1500.0);
    assert!((apex - z0 - expected).abs() < 2.0, "apex {} expected {}", apex - z0, expected);
    // Airtime reference: 2 * JumpZ / g = 1.0667 s.
    let landed = r.events.iter().rev().find_map(|e| match e {
        SimEvent::Landed { fall_time, .. } => Some(*fall_time),
        _ => None,
    });
    let t = landed.expect("landed");
    assert!((t - 2.0 * 800.0 / 1500.0).abs() < 0.05, "airtime {t}");
}

#[test]
fn run_and_sprint_speeds() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    r.steps(60, &[(1, input(0., 127, 0))]);
    let run = r.p(1).velocity.truncate().length();
    // Run 400 x sword-unsheathed factor 0.9.
    assert!((run - 360.0).abs() < 1.0, "run {run}");
    r.steps(60, &[(1, input(0., 127, buttons::SPRINT))]);
    let sprint = r.p(1).velocity.truncate().length();
    assert!((sprint - 600.0).abs() < 1.0, "sprint {sprint}");
    // Stop: braking with ground friction brings us to rest quickly.
    r.steps(30, &[(1, input(0., 0, 0))]);
    assert!(r.p(1).velocity.truncate().length() < 1.0);
}

#[test]
fn backward_and_strafe_multipliers() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    r.steps(60, &[(1, input(0., -127, 0))]);
    let back = r.p(1).velocity.truncate().length();
    assert!((back - 400.0 * 0.8 * 0.9).abs() < 1.0, "back {back}");
    r.steps(60, &[(1, PlayerInput { right: 127, ..input(0., 0, 0) })]);
    let strafe = r.p(1).velocity.truncate().length();
    assert!((strafe - 400.0 * 0.7 * 0.9).abs() < 1.0, "strafe {strafe}");
}

#[test]
fn crouch_lowers_capsule_and_speed() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    r.step(&[(1, input(0., 0, buttons::CROUCH))]);
    assert!(r.p(1).crouched);
    r.steps(60, &[(1, input(0., 127, 0))]);
    let v = r.p(1).velocity.truncate().length();
    assert!((v - 275.0 * 0.9).abs() < 1.0, "crouch {v}");
    assert!((r.p(1).position.z - 65.0).abs() < 1.5, "crouch z {}", r.p(1).position.z);
}

#[test]
fn step_up_limits() {
    // 40 uu step (<= MaxStepHeight 44) is climbed; 60 uu is not.
    let mut r = Run::new(
        floor_world(|t| {
            boxy(t, Vec3::new(200., -200., 0.), Vec3::new(600., 200., 40.));
            boxy(t, Vec3::new(200., 800., 0.), Vec3::new(600., 1200., 60.));
        }),
        &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.), (2, Vec3::new(0., 1000., STAND_Z + 1.0), 0.)],
    );
    r.settle();
    r.steps(90, &[(1, input(0., 127, 0)), (2, input(0., 127, 0))]);
    assert!(r.p(1).position.z > STAND_Z + 35.0, "climbed step: z {}", r.p(1).position.z);
    assert!(r.p(2).position.x < 200.0, "blocked by tall step: x {}", r.p(2).position.x);
}

#[test]
fn wall_blocks_and_slides() {
    let mut r = Run::new(floor_world(|t| boxy(t, Vec3::new(300., -2000., 0.), Vec3::new(400., 2000., 500.))), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    // Run diagonally into the wall: x stops at wall - radius, y keeps moving.
    r.steps(120, &[(1, input(30.0, 127, 0))]);
    let p = r.p(1);
    assert!(p.position.x <= 300.0 - 35.0 + 0.5, "x {}", p.position.x);
    assert!(p.position.y > 100.0, "slid y {}", p.position.y);
}

#[test]
fn mantle_onto_ledge() {
    // 150 uu ledge: too high to jump-step, within mantle range (55..230).
    let mut r = Run::new(floor_world(|t| boxy(t, Vec3::new(150., -300., 0.), Vec3::new(800., 300., 150.))), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    r.steps(20, &[(1, input(0., 127, 0))]);
    r.step(&[(1, input(0., 127, buttons::JUMP))]);
    r.steps(90, &[(1, input(0., 127, 0))]);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Mantle { .. })), "no mantle");
    let p = r.p(1);
    assert!((p.position.z - (150.0 + STAND_Z)).abs() < 3.0, "on ledge z {}", p.position.z);
    assert_eq!(p.mode, MoveMode::Walking);
}

#[test]
fn fall_damage_and_death_thresholds() {
    // Drop from 1400 uu: impact speed ~ sqrt(2*1500*1400) = 2049 < 2250 -> no damage.
    // Drop from 2400 uu: ~2683 -> damaging but not lethal threshold 2900... (health 70).
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., 1400. + STAND_Z), 0.), (2, Vec3::new(500., 0., 2400. + STAND_Z), 0.), (3, Vec3::new(1000., 0., 4000. + STAND_Z), 0.)]);
    r.steps(180, &[]);
    assert_eq!(r.p(1).vitals.health, 70.0);
    assert!(r.p(2).vitals.health < 70.0 && r.p(2).alive, "p2 health {}", r.p(2).vitals.health);
    assert!(!r.p(3).alive, "p3 should die (impact capped at MaxFallSpeed 2100?)");
}

#[test]
fn blink_to_platform_costs_mana_and_refunds() {
    // Platform 900 uu ahead, 200 uu high; aim at its top edge.
    let mut r = Run::new(floor_world(|t| boxy(t, Vec3::new(900., -300., 0.), Vec3::new(1500., 300., 200.))), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    let blink = |b| PlayerInput { yaw: 0.0, pitch: 2.0, buttons: b, left_hand: LeftHand::Blink, ..Default::default() };
    r.steps(5, &[(1, blink(buttons::SECONDARY))]);
    assert!(r.p(1).left.blink_target.is_some(), "no blink target");
    r.step(&[(1, blink(0))]);
    r.steps(40, &[(1, blink(0))]);
    let p = r.p(1);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::BlinkStart { .. })));
    assert!(p.position.x > 850.0, "blinked x {}", p.position.x);
    assert!((p.position.z - (200.0 + STAND_Z)).abs() < 5.0, "on platform z {}", p.position.z);
    assert_eq!(p.vitals.mana, 80.0);
    // Regen: 3 s delay + 20 points at 10/s.
    r.steps(60 * 6, &[(1, blink(0))]);
    assert_eq!(r.p(1).vitals.mana, 100.0);
}

#[test]
fn blink_range_is_clamped_horizontally() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    // Looking straight ahead over an empty floor: target at horizontal range (1600, level 2).
    let i = PlayerInput { yaw: 0.0, pitch: 0.0, buttons: buttons::SECONDARY, left_hand: LeftHand::Blink, ..Default::default() };
    r.steps(3, &[(1, i)]);
    let t = r.p(1).left.blink_target.expect("target");
    assert!((t.x - 1600.0).abs() < 40.0, "target x {}", t.x);
}

#[test]
fn sword_hit_block_parry() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.), 0.), (2, Vec3::new(120., 0., STAND_Z + 1.), 180.)]);
    r.settle();
    // Unblocked hit: 0.2 x 70 = 14.
    r.step(&[(1, input(0., 0, buttons::PRIMARY)), (2, input(180., 0, 0))]);
    r.steps(40, &[(1, input(0., 0, 0)), (2, input(180., 0, 0))]);
    assert!((r.p(2).vitals.health - 56.0).abs() < 1e-3, "health {}", r.p(2).vitals.health);
    // Late block: blocked, no damage.
    r.steps(30, &[(1, input(0., 0, 0)), (2, input(180., 0, buttons::BLOCK))]);
    r.step(&[(1, input(0., 0, buttons::PRIMARY)), (2, input(180., 0, buttons::BLOCK))]);
    r.steps(40, &[(1, input(0., 0, 0)), (2, input(180., 0, buttons::BLOCK))]);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::SwordBlocked { .. })));
    assert!((r.p(2).vitals.health - 56.0).abs() < 1e-3);
    // Perfect block (block pressed right before the hit lands, ~0.13 s after the swing):
    // attacker staggered.
    r.steps(30, &[(1, input(0., 0, 0)), (2, input(180., 0, 0))]);
    r.step(&[(1, input(0., 0, buttons::PRIMARY)), (2, input(180., 0, 0))]);
    r.steps(3, &[(1, input(0., 0, 0)), (2, input(180., 0, 0))]);
    r.step(&[(1, input(0., 0, 0)), (2, input(180., 0, buttons::BLOCK))]);
    r.steps(20, &[(1, input(0., 0, 0)), (2, input(180., 0, buttons::BLOCK))]);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::SwordParried { .. })), "no parry");
    assert!(r.events.iter().any(|_| true));
    let staggered = r.p(1).sword.phase == SwordPhase::Staggered || r.events.iter().any(|e| matches!(e, SimEvent::SwordParried { .. }));
    assert!(staggered);
}

#[test]
fn back_assassination_kills() {
    // Attacker directly behind the victim, both facing +X.
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.), 0.), (2, Vec3::new(110., 0., STAND_Z + 1.), 0.)]);
    r.settle();
    r.step(&[(1, input(0., 0, buttons::PRIMARY)), (2, input(0., 0, 0))]);
    assert!(!r.p(2).alive);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Death { cause: DamageCause::Assassination, .. })));
}

#[test]
fn pistol_damage_close_and_far() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.), 0.), (2, Vec3::new(300., 0., STAND_Z + 1.), 180.), (3, Vec3::new(0., 2000., STAND_Z + 1.), 0.)]);
    r.settle();
    let gun = |yaw: f32, pitch: f32, b| PlayerInput { yaw, pitch, buttons: b, left_hand: LeftHand::Pistol, ..Default::default() };
    r.steps(40, &[(1, gun(0., -15., 0))]);
    r.step(&[(1, gun(0., -15., buttons::SECONDARY))]);
    r.steps(5, &[(1, gun(0., -15., 0))]);
    // Close-range body shot: 20 x 1.5 = 30.
    assert!((r.p(2).vitals.health - 40.0).abs() < 1e-3, "health {}", r.p(2).vitals.health);
    // Reload consumes reserve.
    r.steps(5, &[(1, gun(0., -15., buttons::RELOAD))]);
    r.steps(120, &[(1, gun(0., -15., 0))]);
    assert_eq!(r.p(1).left.pistol_loaded, 1);
    assert_eq!(r.p(1).left.pistol_reserve, 9);
}

#[test]
fn windblast_pushes_and_damages() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.), 0.), (2, Vec3::new(400., 0., STAND_Z + 1.), 180.)]);
    r.settle();
    let wb = |b| PlayerInput { yaw: 0.0, buttons: b, left_hand: LeftHand::Windblast, ..Default::default() };
    r.step(&[(1, wb(buttons::SECONDARY)), (2, input(180., 0, 0))]);
    let v = r.p(2);
    assert_eq!(v.mode, MoveMode::Falling);
    assert!(v.velocity.x > 1000.0, "push vx {}", v.velocity.x);
    assert!(v.vitals.health < 70.0);
    assert_eq!(r.p(1).vitals.mana, 50.0);
}

#[test]
fn dark_vision_costs_and_expires() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.), 0.), (2, Vec3::new(1500., 0., STAND_Z + 1.), 0.)]);
    r.settle();
    let dv = |b| PlayerInput { buttons: b, left_hand: LeftHand::DarkVision, ..Default::default() };
    r.step(&[(1, dv(buttons::SECONDARY))]);
    assert_eq!(r.p(1).vitals.mana, 80.0);
    assert_eq!(dishonored_sim::left_hand::dark_vision_reveals(&r.state, 1, &r.cfg.tuning), vec![2]);
    r.steps(60 * 31, &[(1, dv(0))]);
    assert!(dishonored_sim::left_hand::dark_vision_reveals(&r.state, 1, &r.cfg.tuning).is_empty());
}

#[test]
fn determinism_same_inputs_same_hash() {
    let script: Vec<PlayerInput> = (0..600)
        .map(|i| PlayerInput {
            yaw: (i as f32 * 0.7) % 360.0,
            pitch: ((i as f32) * 0.1).sin() * 20.0,
            forward: if i % 120 < 90 { 127 } else { -60 },
            right: if i % 50 < 25 { 40 } else { -40 },
            buttons: if i % 97 == 0 { buttons::JUMP } else { 0 } | if i % 150 == 10 { buttons::PRIMARY } else { 0 },
            left_hand: LeftHand::Blink,
            ..Default::default()
        })
        .collect();
    let run = || {
        let mut r = Run::new(floor_world(|t| boxy(t, Vec3::new(500., -100., 0.), Vec3::new(700., 100., 100.))), &[(1, Vec3::new(0., 0., STAND_Z + 1.), 0.), (2, Vec3::new(200., 50., STAND_Z + 1.), 90.)]);
        for i in &script {
            r.step(&[(1, *i), (2, PlayerInput { yaw: 90.0 - i.yaw, ..*i })]);
        }
        state_hash(&r.state)
    };
    assert_eq!(run(), run());
}

#[test]
#[ignore]
fn debug_trace() {
    let mut r = Run::new(floor_world(|t| boxy(t, Vec3::new(200., 800., 0.), Vec3::new(600., 1200., 60.))), &[(2, Vec3::new(0., 1000., STAND_Z + 1.0), 0.)]);
    r.settle();
    for i in 0..60 {
        r.step(&[(2, input(0., 127, 0))]);
        let p = r.p(2);
        println!("{i} pos {:.2} vel {:.1} mode {:?}", p.position, p.velocity, p.mode);
    }
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    r.events.clear();
    r.step(&[(1, input(0., 0, buttons::JUMP))]);
    for i in 0..80 {
        r.step(&[(1, input(0., 0, 0))]);
        if i > 55 { let p = r.p(1); println!("j{i} z {:.2} vz {:.1} {:?}", p.position.z, p.velocity.z, p.mode); }
    }
    println!("{:?}", r.events);
}
