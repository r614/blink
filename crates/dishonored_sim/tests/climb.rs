//! Climbing `DisClimbable` chains (`StatePlayerMasterClimb`, `physLadder` 0x00AA82C0): grabbing
//! by Use and by touch, climbing up and down between the limits, the top exit over a ledge,
//! jumping off, letting go, determinism.

use dishonored_sim::climb;
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::{PlayerInput, buttons};
use dishonored_sim::state::{MoveMode, WorldState};
use dishonored_sim::{SimConfig, spawn_player, state_hash, step_world};
use dishonored_world::{Climbable, CollisionWorld, Tri, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Boyle's basement chain (`Climbable.chain_400`, extracted values) hung from (0, 0, 600): bones
/// every 100 uu below the anchor, the physics bodies' bounds sphere 198.04.
fn chain() -> Climbable {
    let anchor = Vec3::new(0.0, 0.0, 600.0);
    let mut bones = vec![anchor + Vec3::Z * 4.58];
    bones.extend((0..5).map(|k| anchor - Vec3::Z * (100.0 * k as f32)));
    Climbable { name: "DisClimbable_1".into(), anchor, bones, sphere_radius: 198.04077, body_half_width: 5.05, top: anchor - Vec3::Z * 6.176, bottom: anchor - Vec3::Z * 391.25 }
}

/// A floor at z = 0 and, with `ledge`, a platform from x = 40 on whose top is at z = 560.
fn world(ledge: bool) -> CollisionWorld {
    let mut tris = Vec::new();
    let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
        for (x, y, z) in [(a, b, c), (a, c, d)] {
            let n = (y - x).cross(z - x).normalize();
            tris.push(Tri { a: x, b: y, c: z, n, channels: channel::ALL, source: 0 });
        }
    };
    quad(Vec3::new(-5000., -5000., 0.), Vec3::new(5000., -5000., 0.), Vec3::new(5000., 5000., 0.), Vec3::new(-5000., 5000., 0.));
    if ledge {
        quad(Vec3::new(40., -500., 560.), Vec3::new(1000., -500., 560.), Vec3::new(1000., 500., 560.), Vec3::new(40., 500., 560.));
        quad(Vec3::new(40., -500., 0.), Vec3::new(40., -500., 560.), Vec3::new(40., 500., 560.), Vec3::new(40., 500., 0.));
    }
    let mut w = CollisionWorld::from_triangles(tris, vec!["fixture".into()]);
    w.climbables = Some(std::sync::Arc::new(vec![chain()]));
    w
}

struct Run {
    state: WorldState,
    cfg: SimConfig,
    world: CollisionWorld,
    events: Vec<SimEvent>,
}

impl Run {
    fn new(at: Vec3, yaw: f32, ledge: bool) -> Run {
        let cfg = cfg();
        let mut state = WorldState::default();
        spawn_player(&mut state, 1, at, yaw, &cfg);
        let mut r = Run { state, cfg, world: world(ledge), events: Vec::new() };
        r.steps(30, input(yaw, 0.0, 0, 0));
        r
    }
    fn step(&mut self, i: PlayerInput) {
        let ins = vec![(1, PlayerInput { tick: self.state.tick, ..i })];
        let out = step_world(&mut self.state, &ins, &self.world, &self.cfg);
        self.events.extend(out.events);
    }
    fn steps(&mut self, n: usize, i: PlayerInput) {
        for _ in 0..n {
            self.step(i);
        }
    }
    fn p(&self) -> &dishonored_sim::state::PlayerState {
        self.state.player(1).unwrap()
    }
    fn tap(&mut self, i: PlayerInput, b: u16) {
        self.step(PlayerInput { buttons: b, ..i });
        self.step(i);
    }
    /// Stand 80 uu behind the chain facing it, look up at it and press Use.
    fn grab() -> Run {
        let mut r = Run::new(Vec3::new(-80.0, 0.0, 92.0), 0.0, false);
        r.tap(input(0.0, 35.0, 0, 0), buttons::USE);
        assert_eq!(r.p().mode, MoveMode::Climbing, "pos {}", r.p().position);
        r
    }
}

fn input(yaw: f32, pitch: f32, forward: i8, b: u16) -> PlayerInput {
    PlayerInput { yaw, pitch, forward, buttons: b, ..Default::default() }
}

const STEP: f32 = 0.0005 * climb::SPEED_SCALE;
const LOW: f32 = 600.0 - 2.0 * 198.04077;
const HIGH: f32 = 600.0 - 87.5;

#[test]
fn use_grabs_the_chain_and_hangs_behind_its_closest_bone() {
    let mut r = Run::grab();
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Climb { id: 1, climbable: 0, started: true })));
    assert_eq!(r.p().climb.on, Some(0));
    // Idle: no height change; x/y converge 30 uu behind the bone and 10 uu to its right
    // (yaw + 90 deg: +y when facing +x).
    let z = r.p().position.z;
    r.steps(120, input(0.0, 35.0, 0, 0));
    let p = r.p().position;
    assert!((p.z - z).abs() < 1e-3, "{} -> {}", z, p.z);
    assert!((p.x - -30.0).abs() < 0.5 && (p.y - 10.0).abs() < 0.5, "{p}");
    assert_eq!(r.p().velocity, Vec3::ZERO);
}

#[test]
fn climbs_up_at_163_8_per_second_and_stops_at_the_top() {
    let mut r = Run::grab();
    // Grabbed from the floor, below the lower limit: the first push lifts to it.
    r.steps(60, input(0.0, 35.0, 127, 0));
    assert!(r.p().position.z > LOW, "{}", r.p().position.z);
    // Mid-chain: each tick moves 20 dt of the 8.19 uu target offset.
    let z0 = r.p().position.z;
    r.steps(30, input(0.0, 35.0, 127, 0));
    let dz = r.p().position.z - z0;
    let expect = 30.0 * STEP * 20.0 / 60.0;
    assert!((dz - expect).abs() < 0.05, "dz {dz} expect {expect}");
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::ClimbSound { id: 1, .. })));
    r.steps(600, input(0.0, 35.0, 127, 0));
    assert!((r.p().position.z - HIGH).abs() < 0.01, "top {}", r.p().position.z);
    assert_eq!(r.p().mode, MoveMode::Climbing);
}

#[test]
fn climbs_down_with_the_stick_back_or_sideways_and_stops_at_the_bottom() {
    let mut r = Run::grab();
    r.steps(120, input(0.0, 35.0, 127, 0));
    let z0 = r.p().position.z;
    r.steps(10, input(0.0, 35.0, -127, 0));
    assert!(r.p().position.z < z0 - 5.0);
    // Sideways input is more than acos(0.9) from the facing: down as well.
    let z1 = r.p().position.z;
    r.steps(10, PlayerInput { right: 127, ..input(0.0, 35.0, 0, 0) });
    assert!(r.p().position.z < z1 - 5.0);
    r.steps(600, input(0.0, 35.0, -127, 0));
    assert!((r.p().position.z - LOW).abs() < 0.01, "bottom {}", r.p().position.z);
    assert_eq!(r.p().mode, MoveMode::Climbing);
}

#[test]
fn camera_relative_climbing_reverses_when_looking_down() {
    let mut r = Run::grab();
    r.steps(120, input(0.0, 35.0, 127, 0));
    r.cfg.gameplay.movement.climb.camera_relative = true;
    // Looking down past -m_ClimbingAimThreshold (1000 units, 5.5 deg): forward climbs down.
    let z0 = r.p().position.z;
    r.steps(20, input(0.0, -30.0, 127, 0));
    assert!(r.p().climb.down);
    assert!(r.p().position.z < z0 - 10.0);
    // Hysteresis: at 3 deg up it stays "down" until the pitch passes +1000 units.
    r.steps(5, input(0.0, 3.0, 127, 0));
    assert!(r.p().climb.down);
    r.steps(5, input(0.0, 6.0, 127, 0));
    assert!(!r.p().climb.down);
    // Without camera-relative climbing the pitch only sets the flag.
    r.cfg.gameplay.movement.climb.camera_relative = false;
    let z1 = r.p().position.z;
    r.steps(20, input(0.0, -30.0, 127, 0));
    assert!(r.p().position.z > z1 + 10.0);
}

#[test]
fn jump_off_after_the_reuse_delay_pushes_along_the_facing() {
    let mut r = Run::new(Vec3::new(-80.0, 0.0, 92.0), 0.0, false);
    r.tap(input(0.0, 35.0, 0, 0), buttons::USE);
    // Within m_fClimbReuseDelay (0.5 s) the jump is refused.
    r.tap(input(0.0, 35.0, 0, 0), buttons::JUMP);
    assert_eq!(r.p().mode, MoveMode::Climbing);
    r.steps(40, input(0.0, 35.0, 0, 0));
    r.step(input(0.0, 35.0, 0, buttons::JUMP));
    let p = *r.p();
    assert_eq!(p.mode, MoveMode::Falling);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Climb { id: 1, started: false, .. })));
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Jumped { id: 1 })));
    // m_fJumpImpulse along the facing, then JumpZ (one falling tick already applied).
    assert!((p.velocity.x - 525.0).abs() < 1.0 && p.velocity.y.abs() < 1e-3, "{}", p.velocity);
    assert!(p.velocity.z > 700.0 && p.velocity.z <= 800.0, "{}", p.velocity);
    assert!(p.climb.jumped && p.climb.on == Some(0));
    // Still held through the jump: no re-grab though the pawn passes the chain; lands later.
    r.steps(300, input(0.0, 0.0, 0, 0));
    assert_eq!(r.p().mode, MoveMode::Walking);
    assert_eq!(r.p().climb.on, None);
    assert_eq!(r.events.iter().filter(|e| matches!(e, SimEvent::Climb { started: true, .. })).count(), 1);
}

#[test]
fn jump_off_under_a_ceiling_doubles_the_impulse() {
    let mut r = Run::grab();
    let mut w = world(false);
    let mut tris = w.tris.clone();
    for (a, b, c) in [
        (Vec3::new(-500., -500., 650.), Vec3::new(500., 500., 650.), Vec3::new(500., -500., 650.)),
        (Vec3::new(-500., -500., 650.), Vec3::new(-500., 500., 650.), Vec3::new(500., 500., 650.)),
    ] {
        tris.push(Tri { a, b, c, n: (b - a).cross(c - a).normalize(), channels: channel::ALL, source: 0 });
    }
    w = CollisionWorld { climbables: w.climbables.clone(), ..CollisionWorld::from_triangles(tris, vec!["fixture".into()]) };
    r.world = w;
    r.steps(600, input(0.0, 35.0, 127, 0));
    r.step(input(0.0, 35.0, 0, buttons::JUMP));
    assert_eq!(r.p().mode, MoveMode::Falling);
    assert!((r.p().velocity.x - 1050.0).abs() < 2.0, "{}", r.p().velocity);
}

#[test]
fn use_lets_go_and_touching_does_not_grab_again_until_landed() {
    let mut r = Run::grab();
    r.steps(60, input(0.0, 35.0, 127, 0));
    r.tap(input(0.0, 35.0, 0, 0), buttons::USE);
    let p = *r.p();
    assert_eq!(p.mode, MoveMode::Falling);
    assert!(p.climb.letting_go && p.climb.on.is_none());
    assert!((p.velocity.x - 525.0).abs() < 1.0, "{}", p.velocity);
    // Fall back through the chain with the stick pulling back: no grab.
    r.steps(200, input(0.0, 0.0, -127, 0));
    assert_eq!(r.p().mode, MoveMode::Walking);
    assert!(!r.p().climb.letting_go);
    assert_eq!(r.events.iter().filter(|e| matches!(e, SimEvent::Climb { started: true, .. })).count(), 1);
}

#[test]
fn jumping_into_the_chain_grabs_it() {
    let mut r = Run::new(Vec3::new(-500.0, 0.0, 92.0), 0.0, false);
    // Run at the chain and jump 160 uu before it (walking under it never grabs).
    while r.p().position.x < -160.0 {
        r.step(input(0.0, 0.0, 127, 0));
        assert_eq!(r.p().mode, MoveMode::Walking);
    }
    r.step(input(0.0, 0.0, 127, buttons::JUMP));
    for _ in 0..60 {
        r.step(input(0.0, 0.0, 127, 0));
        if r.p().mode == MoveMode::Climbing {
            break;
        }
    }
    assert_eq!(r.p().mode, MoveMode::Climbing, "pos {}", r.p().position);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Climb { id: 1, climbable: 0, started: true })));
}

#[test]
fn top_exit_jumps_onto_the_ledge() {
    let mut r = Run::new(Vec3::new(-80.0, 0.0, 92.0), 0.0, true);
    r.tap(input(0.0, 35.0, 0, 0), buttons::USE);
    assert_eq!(r.p().mode, MoveMode::Climbing);
    r.steps(400, input(0.0, 35.0, 127, 0));
    assert!((r.p().position.z - HIGH).abs() < 0.01);
    // Jump with the stick forward: off the chain, onto the platform (mantle while airborne). The
    // airborne mantle is requested only while the Jump button is down (0x00ADBD00, movement.md
    // MV-2), so Jump stays held.
    r.step(input(0.0, 0.0, 127, buttons::JUMP));
    for _ in 0..240 {
        r.step(input(0.0, 0.0, 127, buttons::JUMP));
        if r.p().mode == MoveMode::Walking {
            break;
        }
    }
    let p = r.p();
    assert_eq!(p.mode, MoveMode::Walking, "pos {}", p.position);
    assert!(p.position.z > 560.0 + 80.0 && p.position.x > 40.0, "pos {}", p.position);
}

#[test]
fn climbing_is_deterministic() {
    let run = || {
        let mut r = Run::grab();
        r.steps(50, input(0.0, 35.0, 127, 0));
        r.steps(20, input(0.0, -20.0, -127, 0));
        r.step(input(0.0, 0.0, 0, buttons::JUMP));
        r.steps(100, input(0.0, 0.0, 0, 0));
        (state_hash(&r.state), r.p().position)
    };
    let a = run();
    let b = run();
    assert_eq!(a.0, b.0);
    assert_eq!(a.1.to_array().map(f32::to_bits), b.1.to_array().map(f32::to_bits));
}

#[test]
fn limits_and_bone_target() {
    let c = chain();
    let (lo, hi) = climb::limits(&c, 87.5);
    assert!((lo - LOW).abs() < 1e-3 && (hi - HIGH).abs() < 1e-3);
    // The bone nearest 2/3 of the height above the centre.
    let p = dishonored_sim::state::PlayerState::new(1, Vec3::new(-30.0, 10.0, 400.0), 0.0);
    let t = climb::bone_target(&c, &p, 87.5).unwrap();
    assert!((t - Vec3::new(-30.0, 10.0, 500.0)).length() < 1e-3, "{t}");
    assert_eq!(climb::pitch_units(5.4931640625), 1000);
}
