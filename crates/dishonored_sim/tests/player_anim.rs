//! The player's states' requests on its layers (`dishonored_sim::player_anim`) on the fixture
//! config, without the install: the navigation trees' speed blends (`Ply_Player_Nav_Default_at`:
//! idle at 0, walk up to 250 uu/s, run above), the left hand's navigation in the full body's sync
//! group, the block's in / loop / out, the transitions' one-shots, the mantle by edge height.

use dishonored_sim::anim::{layer, sync, token};
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::state::{MoveMode, WorldState};
use dishonored_sim::{SimConfig, spawn_player, step_world};
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn floor() -> CollisionWorld {
    let (a, b, c, d) = (Vec3::new(-50000., -50000., 0.), Vec3::new(50000., -50000., 0.), Vec3::new(50000., 50000., 0.), Vec3::new(-50000., 50000., 0.));
    let tri = |x: Vec3, y: Vec3, z: Vec3| Tri { a: x, b: y, c: z, n: (y - x).cross(z - x).normalize(), channels: channel::ALL, source: 0 };
    CollisionWorld::from_triangles(vec![tri(a, b, c), tri(a, c, d)], vec!["fixture".into()])
}

struct Run {
    state: WorldState,
    cfg: SimConfig,
    world: CollisionWorld,
}

impl Run {
    fn new() -> Run {
        let cfg = cfg();
        let mut state = WorldState::default();
        spawn_player(&mut state, 1, Vec3::new(0., 0., 88.5), 0.0, &cfg);
        let mut r = Run { state, cfg, world: floor() };
        for _ in 0..90 {
            r.step(PlayerInput::default());
        }
        r
    }
    fn step(&mut self, i: PlayerInput) {
        let ins = [(1, PlayerInput { tick: self.state.tick, ..i })];
        step_world(&mut self.state, &ins, &self.world, &self.cfg);
    }
    fn steps(&mut self, n: usize, i: PlayerInput) {
        for _ in 0..n {
            self.step(i);
        }
    }
    /// The sequence key and token on a layer of the player.
    fn layer(&self, l: usize) -> Option<(String, u32)> {
        let s = self.state.anim.get(&1)?.layers[l].as_ref().filter(|s| s.out.is_none())?;
        Some((self.cfg.anim.key(s.seq)?.to_string(), s.token))
    }
    fn name(&self, l: usize) -> String {
        self.layer(l).map(|(k, _)| k).unwrap_or_default()
    }
    fn speed(&self) -> f32 {
        self.state.player(1).unwrap().velocity.truncate().length()
    }
}

/// Standing: `Nav_Default_Ready`'s idle on the full body, a continuing state's token in the idle
/// sync group; moving it plays the walk at or below 250 uu/s and the run above, at rate 1.
#[test]
fn navigation_follows_the_speed_blend() {
    let mut r = Run::new();
    let (k, tok) = r.layer(layer::FULL_BODY).expect("the navigation plays");
    assert_eq!(k, "Ply_Sword_Idle_as.Sword_Ready_Idle");
    assert_eq!(tok & token::KIND, token::NAV);
    assert_eq!((tok >> 16) & 0xFF, sync::IDLE);
    // Walk (the WALK button) then run: the pick follows the speed against the constraint.
    let mut seen = Vec::new();
    for k in 0..90 {
        r.step(PlayerInput { forward: 127, buttons: if k < 45 { buttons::WALK } else { 0 }, ..Default::default() });
        let n = r.name(layer::FULL_BODY);
        let want = if r.speed() <= 0.0 { "Sword_Ready_Idle" } else if r.speed() <= 250.0 { "Sword_Ready_Walk" } else { "Sword_Ready_Run" };
        assert!(n.ends_with(want), "speed {} plays {n}", r.speed());
        if !seen.contains(&n) {
            seen.push(n);
        }
        let s = r.state.anim[&1].layers[layer::FULL_BODY].as_ref().unwrap();
        assert_eq!(s.rate, 1.0);
    }
    assert!(seen.iter().any(|n| n.ends_with("Sword_Ready_Run")), "{seen:?}");
}

/// The pistol in the left hand: its navigation loop (`Guns_Idle`) on the left arm, without
/// notifies (`bNoNotifies`), in the full body's idle group: it takes the full body's relative
/// position after every advance.
#[test]
fn the_left_hand_navigation_follows_the_full_body() {
    let mut r = Run::new();
    r.steps(60, PlayerInput { left_hand: LeftHand::Pistol, ..Default::default() });
    let a = &r.state.anim[&1];
    let (fb, la) = (a.layers[layer::FULL_BODY].as_ref().unwrap(), a.layers[layer::LEFT_ARM].as_ref().unwrap());
    assert_eq!(r.cfg.anim.name(la.seq), Some("Guns_Idle"));
    assert!(la.quiet);
    let (lf, ll) = (r.cfg.anim.info(fb.seq).unwrap().duration, r.cfg.anim.info(la.seq).unwrap().duration);
    assert!(((fb.time / lf).rem_euclid(1.0) - la.time / ll).abs() < 1e-4, "{} / {} against {} / {}", fb.time, lf, la.time, ll);
}

/// The block: its in (whose window is the parry zone), then the loop once the in ended; released,
/// the block's out plays as a one-shot.
#[test]
fn the_block_plays_in_loop_and_out() {
    let mut r = Run::new();
    let block = PlayerInput { buttons: buttons::BLOCK, ..Default::default() };
    r.step(block);
    let (k, tok) = r.layer(layer::UPPER_BODY).unwrap();
    assert_eq!(k, "Ply_Sword_Versus_as.Sword_Ready_ParryIdle_In");
    assert_eq!(tok, token::BLOCK | dishonored_sim::player_anim::block::IN);
    r.steps(90, block);
    assert_eq!(r.name(layer::UPPER_BODY), "Ply_Sword_Versus_as.Sword_Ready_ParryIdle");
    r.step(PlayerInput::default());
    r.step(PlayerInput::default());
    let (k, tok) = r.layer(layer::UPPER_BODY).unwrap();
    assert_eq!(k, "Ply_Sword_Versus_as.Sword_Ready_ParryIdle_Out");
    assert_eq!(tok & token::KIND, token::ACTION);
}

/// A jump: `Upper_Jump_In` (`Sword_Ready_JumpIn`) on the upper body and the left item's jump-in on
/// the left arm; the air child once in the jump state, the small land's one-shot on landing.
#[test]
fn a_jump_plays_its_reactions_and_the_air_child() {
    let mut r = Run::new();
    r.steps(30, PlayerInput { left_hand: LeftHand::Blink, ..Default::default() });
    r.step(PlayerInput { left_hand: LeftHand::Blink, buttons: buttons::JUMP, ..Default::default() });
    let mut up = false;
    let mut air = false;
    let mut land = false;
    for _ in 0..120 {
        r.step(PlayerInput { left_hand: LeftHand::Blink, ..Default::default() });
        let p = *r.state.player(1).unwrap();
        up |= r.name(layer::UPPER_BODY).ends_with("Sword_Ready_JumpIn") && r.name(layer::LEFT_ARM).ends_with("Powers_JumpIn");
        air |= p.mode == MoveMode::Falling && r.name(layer::FULL_BODY).ends_with("Sword_Ready_Jump");
        land |= p.mode == MoveMode::Walking && r.name(layer::UPPER_BODY).ends_with("Sword_Ready_JumpLandSmall");
    }
    assert!(up && air && land, "jump-in {up}, air {air}, land {land}");
}
