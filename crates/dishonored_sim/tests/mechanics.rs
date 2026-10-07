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
/// physWalking keeps the pawn `FLOOR_DIST` (2.7305) above the floor (MINFLOORDIST..MAXFLOORDIST).
const HOVER: f32 = dishonored_sim::movement::FLOOR_DIST;

#[test]
fn spawn_settles_on_floor() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., 300.), 0.)]);
    r.settle();
    let p = r.p(1);
    assert_eq!(p.mode, MoveMode::Walking);
    assert!((p.position.z - STAND_Z - HOVER).abs() < 0.4, "z {}", p.position.z);
}

#[test]
fn reference_jump_apex_matches_jumpz_and_gravity() {
    // physFalling moves by v0 + g dt and keeps 2 x average - v0, so the velocity changes by
    // 2 g dt per tick: apex = JumpZ^2 / (4 |GravityZ|) = 800^2 / 6000 = 106.7 uu.
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    let z0 = r.p(1).position.z;
    r.step(&[(1, input(0., 0, buttons::JUMP))]);
    let mut apex = z0;
    for _ in 0..120 {
        r.step(&[(1, input(0., 0, 0))]);
        apex = apex.max(r.p(1).position.z);
    }
    let expected = 800.0f32.powi(2) / (4.0 * 1500.0);
    assert!((apex - z0 - expected).abs() < 2.0, "apex {} expected {}", apex - z0, expected);
    // Airtime: JumpZ / g = 0.533 s (twice the acceleration).
    let landed = r.events.iter().rev().find_map(|e| match e {
        SimEvent::Landed { fall_time, .. } => Some(*fall_time),
        _ => None,
    });
    let t = landed.expect("landed");
    assert!((t - 800.0 / 1500.0).abs() < 0.05, "airtime {t}");
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
    // MaxSpeedModifier (0x00B56C40) applies the sword factor in every stance, sprint too:
    // 600 x 0.9.
    assert!((sprint - 540.0).abs() < 1.0, "sprint {sprint}");
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
    // Diagonal forward (45 deg from the side axis, beyond the 10 deg strafe angle): full
    // run speed, no blend of multipliers.
    r.steps(90, &[(1, PlayerInput { right: 127, ..input(0., 127, 0) })]);
    let diag = r.p(1).velocity.truncate().length();
    assert!((diag - 360.0).abs() < 1.0, "diagonal {diag}");
    // Diagonal backward (45 deg < m_fStrafeAngle_Back 50): the strafe multiplier.
    r.steps(90, &[(1, PlayerInput { right: 127, ..input(0., -127, 0) })]);
    let back_diag = r.p(1).velocity.truncate().length();
    assert!((back_diag - 400.0 * 0.7 * 0.9).abs() < 1.0, "back diagonal {back_diag}");
}

#[test]
fn sprint_in_any_direction_and_walk_speeds() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    // Sprinting sideways: GroundSpeedSprint x StrafeMultiplierSprint x sword factor.
    r.steps(90, &[(1, PlayerInput { right: 127, ..input(0., 0, buttons::SPRINT) })]);
    let v = r.p(1).velocity.truncate().length();
    assert!((v - 600.0 * 0.6 * 0.9).abs() < 1.0, "strafe sprint {v}");
    assert!(r.p(1).sprinting);
    // Half-deflected stick: walk speed (GroundSpeedWalk); barely deflected: slow walk.
    r.steps(120, &[(1, input(0., 80, 0))]);
    let v = r.p(1).velocity.truncate().length();
    assert!((v - 200.0 * 0.9).abs() < 1.0, "walk {v}");
    r.steps(120, &[(1, input(0., 40, 0))]);
    let v = r.p(1).velocity.truncate().length();
    assert!((v - 100.0 * 0.9).abs() < 1.0, "slow walk {v}");
    // Below m_fStopSprint_Threshold the sprint button does nothing.
    r.steps(5, &[(1, input(0., 80, buttons::SPRINT))]);
    assert!(!r.p(1).sprinting);
}

#[test]
fn sprint_slide_eases_to_sneak_speed() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    r.steps(90, &[(1, input(0., 127, buttons::SPRINT))]);
    r.step(&[(1, input(0., 127, buttons::SPRINT | buttons::CROUCH))]);
    assert!(r.p(1).slide.active && r.p(1).crouched);
    // Halfway through m_fSlideTime the ease is at half: (540 + 275 x 0.9) / 2.
    let half = (1.1f32 / 2.0 * 60.0).round() as usize - 1;
    r.steps(half, &[(1, input(0., 127, buttons::SPRINT))]);
    let v = r.p(1).velocity.truncate().length();
    assert!((v - (540.0 + 247.5) / 2.0).abs() < 8.0, "mid-slide {v}");
    r.steps(40, &[(1, input(0., 127, buttons::SPRINT))]);
    assert!(!r.p(1).slide.active && r.p(1).crouched, "slide ends sneaking");
}

#[test]
fn lean_swings_the_eye_about_the_pivot() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    let eye0 = dishonored_sim::movement::eye_position(r.p(1), &r.cfg.gameplay.movement);
    r.steps(60, &[(1, input(0., 0, buttons::LEAN_RIGHT))]);
    let eye = dishonored_sim::movement::eye_position(r.p(1), &r.cfg.gameplay.movement);
    // Pivot 2.0 x 87.5 x 2 = 350 uu below the eye, 15 deg: 90.6 uu right, 11.9 down.
    let d = eye - eye0;
    assert!((d.y + 350.0 * 15f32.to_radians().sin()).abs() < 2.0 || (d.y - 350.0 * 15f32.to_radians().sin()).abs() < 2.0, "lean {d}");
    assert!((d.z + 350.0 * (1.0 - 15f32.to_radians().cos())).abs() < 1.0, "lean drop {d}");
}

#[test]
fn speed_drop_is_blended_down() {
    // Sprint, then release: the modifier falls at m_fSpeedBlendDownSpeed (8/s), so the
    // max speed does not drop to run speed in one tick.
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    r.steps(90, &[(1, input(0., 127, buttons::SPRINT))]);
    r.step(&[(1, input(0., 127, 0))]);
    let m = r.p(1).speed_mod;
    let expect = 1.5 * 0.9 + (0.9 - 1.5 * 0.9) * (8.0 / 60.0);
    assert!((m - expect).abs() < 1e-4, "modifier {m} expected {expect}");
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
    assert!((r.p(1).position.z - 65.0 - HOVER).abs() < 1.0, "crouch z {}", r.p(1).position.z);
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
    // With twice the gravity per tick the impact speed after a drop h is ~sqrt(4 g h):
    // 700 uu -> 2049 < 2250 (no damage); 1100 uu -> 2569 (damage); 2000 uu -> 3464 > 2900 (death).
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., 700. + STAND_Z), 0.), (2, Vec3::new(500., 0., 1100. + STAND_Z), 0.), (3, Vec3::new(1000., 0., 2000. + STAND_Z), 0.)]);
    r.steps(180, &[]);
    assert_eq!(r.p(1).vitals.health, 70.0);
    assert!(r.p(2).vitals.health < 70.0 && r.p(2).alive, "p2 health {}", r.p(2).vitals.health);
    assert!(!r.p(3).alive, "p3 should die");
}

#[test]
fn fall_damage_matches_the_original_at_60_hz() {
    // Reference (engine hook, 2026-10-06, the original at a fixed 1/60 s step, the landing speed
    // read at `TakeFallingDamage` 0x00B4DCC0): drops of 1000 / 1200 / 1300 uu land at
    // 2401 / 2675 / 2775 uu/s and take 16 / 45 / 56 of 70.
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., 1000. + STAND_Z), 0.), (2, Vec3::new(500., 0., 1200. + STAND_Z), 0.), (3, Vec3::new(1000., 0., 1300. + STAND_Z), 0.)]);
    r.steps(120, &[]);
    // The original's drops started from a written height plus one step forward, so the landing
    // tick can differ by one: within 2 points (the port takes 18 at 1000 uu).
    let got: Vec<f32> = (1..=3).map(|i| 70.0 - r.p(i).vitals.health).collect();
    for (g, want) in got.iter().zip([16.0, 45.0, 56.0]) {
        assert!((g - want).abs() <= 2.0, "fall damage {got:?}, original [16, 45, 56]");
    }
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
    // A player (2) straight ahead and an NPC (3) slightly to the side.
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.), 0.), (2, Vec3::new(400., 0., STAND_Z + 1.), 180.), (3, Vec3::new(400., 200., STAND_Z + 1.), 180.)]);
    r.state.player_mut(3).unwrap().kind = dishonored_sim::state::PawnKind::Guard;
    r.settle();
    let wb = |b| PlayerInput { yaw: 0.0, buttons: b, left_hand: LeftHand::Windblast, ..Default::default() };
    r.step(&[(1, wb(buttons::SECONDARY)), (2, input(180., 0, 0))]);
    // Nothing yet: 0.1 s warmup, then the front travels at 5000 uu/s.
    assert_eq!(r.p(2).mode, MoveMode::Walking);
    assert_eq!(r.p(1).vitals.mana, 50.0);
    r.steps(12, &[(1, wb(0)), (2, input(180., 0, 0))]);
    // The NPC is thrown; the player's damage path clears the momentum (0x00B5CCD0, U-051).
    let n = r.p(3);
    assert_eq!(n.mode, MoveMode::Falling);
    assert!(n.velocity.x > 500.0, "npc push vx {}", n.velocity.x);
    let v = r.p(2);
    assert_eq!(v.mode, MoveMode::Walking);
    assert!(v.velocity.x.abs() < 1.0, "player not pushed: vx {}", v.velocity.x);
    // Damage = round(30 x factor), factor = (1 - d/3000)^2 straight ahead (d ~ 400).
    let d = 400.0f32 - 0.0;
    let k = (1.0 - d / 3000.0).powi(2);
    let expect = 70.0 - (30.0 * k + 0.5).floor();
    assert!((v.vitals.health - expect).abs() <= 1.0, "health {} expected ~{expect}", v.vitals.health);
}

#[test]
fn windblast_factor_and_momentum_rules() {
    use dishonored_sim::left_hand::{windblast_factor, windblast_momentum};
    let cos_half = (135.0f32 * 0.5).to_radians().cos();
    // Straight ahead within 100 uu: range factor only.
    let k = windblast_factor(Vec3::X, Vec3::new(50., 0., 0.), Vec3::ZERO, 3000.0, cos_half);
    assert!((k - (1.0 - 50.0 / 3000.0f32).powi(2)).abs() < 1e-5);
    // Outside the 67.5 deg half-angle (in the horizontal plane): nothing.
    assert_eq!(windblast_factor(Vec3::X, Vec3::new(100., 300., 0.), Vec3::ZERO, 3000.0, cos_half), 0.0);
    // Level aim at 300 uu: the momentum is the impulse times k, z = (1 - 0.1) x 1600.
    let m = windblast_momentum(Vec3::X, 2700.0, 1600.0, 300.0, 3000.0, 1.0);
    assert!((m.x - 2700.0).abs() < 1e-2);
    assert!((m.z - 0.9 * 1600.0).abs() < 1e-2, "{m}");
    // The original (engine hook, 2026-10-06): a guard 182.8 uu from the blast origin, level
    // aim (0.995, -0.096, -0.002): 26 damage and momentum (2362, -227, 1317).
    let dir = Vec3::new(0.995, -0.096, -0.002).normalize();
    let k = (1.0 - 182.8 / 3000.0f32).powi(2);
    let m = windblast_momentum(dir, 2700.0, 1600.0, 182.8, 3000.0, k);
    assert!((m.x - 2362.0).abs() < 15.0 && (m.y + 227.0).abs() < 5.0 && (m.z - 1317.0).abs() < 15.0, "{m}");
    assert_eq!((30.0 * k + 0.5).floor(), 26.0);
}

#[test]
fn parry_tap_window_and_forced_block() {
    use dishonored_sim::combat::parries;
    use dishonored_sim::state::SwordState;
    let t = &cfg().tuning;
    assert!((t.parry_tap_window - 0.2).abs() < 1e-6 && (t.force_block_after_tap - 0.3).abs() < 1e-6);
    assert!((t.block_half_angle - 50.0).abs() < 1e-6);
    let s = |block_time: f32, force_block: f32| SwordState { phase: SwordPhase::Blocking, block_time, force_block, ..Default::default() };
    assert!(parries(&s(0.05, 0.0), t));
    assert!(parries(&s(0.4, 0.0), t), "inside the BlockParryZone notify");
    assert!(!parries(&s(0.6, 0.0), t));
    assert!(!parries(&s(0.05, 0.2), t), "a forced block after a tap parry only blocks");
}

#[test]
fn blink_keeps_momentum_and_travels_in_steps() {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    let blink = |b, fwd| PlayerInput { yaw: 0.0, pitch: 0.0, forward: fwd, buttons: b, left_hand: LeftHand::Blink, ..Default::default() };
    // Run, aim, release: 100 uu per tick (m_fDefaultBlinkStepDistance, one step per tick).
    r.steps(60, &[(1, blink(buttons::SECONDARY, 127))]);
    let v0 = r.p(1).velocity;
    let x0 = r.p(1).position.x;
    r.step(&[(1, blink(0, 127))]);
    assert_eq!(r.p(1).mode, MoveMode::Blinking);
    r.step(&[(1, blink(0, 127))]);
    assert!((r.p(1).position.x - x0 - 100.0).abs() < 8.0, "step {}", r.p(1).position.x - x0);
    // Until the end, then the pre-Blink velocity is back.
    for _ in 0..40 {
        if r.p(1).mode != MoveMode::Blinking {
            break;
        }
        r.step(&[(1, blink(0, 0))]);
    }
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::BlinkEnd { .. })));
    assert!(r.p(1).position.x - x0 > 1400.0, "x {}", r.p(1).position.x - x0);
    assert!(v0.x > 300.0 && (r.p(1).velocity.x - v0.x).abs() < 1.0, "velocity {} before {}", r.p(1).velocity, v0);
}

#[test]
fn blink_cancels_when_target_is_too_close() {
    // A wall right in front: the target backs off to the pawn, closer than the bounding
    // sphere radius, so the release fizzles and no mana is spent.
    let mut r = Run::new(floor_world(|t| boxy(t, Vec3::new(60., -500., 0.), Vec3::new(100., 500., 600.))), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    let blink = |b| PlayerInput { yaw: 0.0, pitch: 0.0, buttons: b, left_hand: LeftHand::Blink, ..Default::default() };
    r.steps(3, &[(1, blink(buttons::SECONDARY))]);
    r.step(&[(1, blink(0))]);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::BlinkFizzle { .. })));
    assert_eq!(r.p(1).vitals.mana, 100.0);
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

/// Sprint, then slide (crouch) toward pawn 2 standing `ahead` uu in front of the slide start.
fn slide_into(npc: Option<dishonored_sim::state::PawnKind>) -> (Run, f32, f32) {
    let mut r = Run::new(floor_world(|_| {}), &[(1, Vec3::new(0., 0., STAND_Z + 1.0), 0.)]);
    r.settle();
    r.steps(90, &[(1, input(0., 127, buttons::SPRINT))]);
    let at = r.p(1).position;
    match npc {
        Some(k) => dishonored_sim::spawn_npc(&mut r.state, 2, k, 0, at + Vec3::new(250., 0., 0.), 180.0, &r.cfg.clone()),
        None => spawn_player(&mut r.state, 2, at + Vec3::new(250., 0., 0.), 180.0, &r.cfg.clone()),
    }
    let h0 = r.p(2).vitals.health;
    r.step(&[(1, input(0., 127, buttons::SPRINT | buttons::CROUCH)), (2, input(180., 0, 0))]);
    assert!(r.p(1).slide.active);
    r.steps(80, &[(1, input(0., 127, buttons::SPRINT)), (2, input(180., 0, 0))]);
    let h1 = r.p(2).vitals.health;
    (r, h0, h1)
}

#[test]
fn slide_impact_hurts_another_player_once() {
    let (r, h0, h1) = slide_into(None);
    let hits = r.events.iter().filter(|e| matches!(e, SimEvent::Damage { victim: 2, cause: dishonored_sim::events::DamageCause::Slide, .. })).count();
    assert_eq!(hits, 1, "one impact per slide");
    assert_eq!(h0 - h1, 5.0, "m_SlideDamageInfo: other actors take 5");
}

#[test]
fn slide_impact_pushes_npcs_without_damage() {
    let (r, h0, h1) = slide_into(Some(dishonored_sim::state::PawnKind::Guard));
    assert_eq!(h0, h1, "DishonoredNPCPawn takes 0");
    assert!(r.p(1).slide.hit, "the slide met the guard");
}
