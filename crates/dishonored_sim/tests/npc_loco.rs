//! The NPC locomotion component's start, stop and turn sequences (`dishonored_sim::npc_loco`) on
//! the fixture config, without the install: a guard's locomotion request with a navmesh path
//! starts it with its idle mode's start, stops it near a non-accurate destination at a foot of
//! its walk loop, and turns it toward a path behind it (research/dishonored/ANIMATION.md
//! section 3, UNKNOWNS U-280).

use dishonored_sim::input::{PlayerInput, buttons};
use dishonored_sim::npc_loco::{kind, state as st};
use dishonored_sim::pawn_states::{LocoPath, Request};
use dishonored_sim::state::{PawnKind, WorldState};
use dishonored_sim::{SimConfig, spawn_npc, spawn_player, step_world};
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn floor() -> CollisionWorld {
    let s = 20000.0;
    let (a, b, c, d) = (Vec3::new(-s, -s, 0.), Vec3::new(s, -s, 0.), Vec3::new(s, s, 0.), Vec3::new(-s, s, 0.));
    let tri = |x: Vec3, y: Vec3, z: Vec3| Tri { a: x, b: y, c: z, n: (y - x).cross(z - x).normalize(), channels: channel::ALL, source: 0 };
    CollisionWorld::from_triangles(vec![tri(a, b, c), tri(a, c, d)], vec!["fixture".into()])
}

const Z: f32 = 88.5;
const GUARD: u32 = 100;

struct Run {
    state: WorldState,
    cfg: SimConfig,
    world: CollisionWorld,
    /// The guard's locomotion config and its `EmptyPatrol` stance, the request's speed.
    loco: (u16, u8),
    speed: f32,
}

impl Run {
    /// A player far away and a guard at the origin facing `yaw`, settled.
    fn new(yaw: f32) -> Run {
        let cfg = cfg();
        let mut state = WorldState::default();
        spawn_player(&mut state, 1, Vec3::new(-9000., 0., Z), 0.0, &cfg);
        spawn_npc(&mut state, GUARD, PawnKind::Guard, 1, Vec3::new(0., 0., Z), yaw, &cfg);
        let ci = cfg.gameplay.ai.locomotion.iter().position(|c| c.name.ends_with("EliteGuardLocomotionConfig")).unwrap();
        let lc = &cfg.gameplay.ai.locomotion[ci];
        let mi = lc.modifier("EmptyPatrol").unwrap();
        let speed = lc.mode(mi, 1).unwrap().move_anims[2].1 * lc.modifiers[mi].speed_multiplier;
        let mut r = Run { state, cfg, world: floor(), loco: (ci as u16, mi as u8), speed };
        for _ in 0..30 {
            r.step(None, yaw);
        }
        r
    }

    /// A tick of the guard walking to `goal` along a one-segment path from `from` (None: standing).
    fn step(&mut self, walk: Option<(Vec3, Vec3, bool)>, yaw: f32) {
        let t = self.state.tick;
        let g = self.guard().position;
        let (req, input) = match walk {
            Some((from, goal, accurate)) => {
                let d = goal - g;
                let y = d.y.atan2(d.x).to_degrees();
                let req = Request { loco: Some(self.loco), loco_speed: self.speed, path: Some(LocoPath { from, to: goal, next: None, dest: goal, accurate }), ..Default::default() };
                (req, PlayerInput { tick: t, yaw: y, forward: 127, buttons: buttons::WALK, ..Default::default() })
            }
            None => (Request { loco: Some(self.loco), ..Default::default() }, PlayerInput { tick: t, yaw, ..Default::default() }),
        };
        self.state.requests = vec![(GUARD, req)];
        let ins = [(1, PlayerInput { tick: t, ..Default::default() }), (GUARD, input)];
        step_world(&mut self.state, &ins, &self.world, &self.cfg);
    }

    fn guard(&self) -> dishonored_sim::state::PlayerState {
        *self.state.player(GUARD).unwrap()
    }

    fn loco(&self) -> dishonored_sim::npc_loco::NpcLoco {
        *dishonored_sim::npc_loco::component(&self.state, GUARD).unwrap()
    }

    fn specific_name(&self) -> Option<String> {
        self.loco().specific.and_then(|s| self.cfg.anim.name(s.seq).map(str::to_string))
    }
}

/// A standing guard facing its path starts with `Empty_Walk_Start` (the idle mode's only start
/// prop): it stands while the start blends in, is carried by the start's root motion while it is
/// fully in, and walks the path at its request's speed after it.
#[test]
fn a_standing_guard_starts_with_its_start_sequence() {
    let mut r = Run::new(0.0);
    let goal = Vec3::new(1500., 0., 0.);
    r.step(Some((Vec3::ZERO, goal, false)), 0.0);
    let l = r.loco();
    assert_eq!(l.state, st::START_REQUESTED);
    assert_eq!(l.specific.unwrap().kind, kind::START);
    assert!(r.specific_name().unwrap().ends_with("Empty_Walk_Start"), "{:?}", r.specific_name());
    let mut states = Vec::new();
    for _ in 0..120 {
        r.step(Some((Vec3::ZERO, goal, false)), 0.0);
        let s = r.loco().state;
        if states.last() != Some(&s) {
            states.push(s);
        }
    }
    assert_eq!(states, vec![st::START_BLEND_IN, st::START_IN, st::START_BLEND_OUT, st::NONE], "the start's states");
    let g = r.guard();
    assert!(g.position.x > 100.0, "the start and the walk moved the guard {} uu", g.position.x);
    let v = g.velocity.truncate().length();
    assert!((v - r.speed).abs() < 0.1 * r.speed, "walking at {v} uu/s, the request's {}", r.speed);
}

/// Walking toward a non-accurate destination, the guard opens the stop window within
/// `m_fUnaccuracyRadius` (120 uu) and asks a stop when its walk loop crosses a stop prop's
/// `m_fMoveAnimIn`; the stop's root motion carries it and it ends standing, short of the point.
#[test]
fn a_walking_guard_stops_with_its_stop_sequence() {
    let mut r = Run::new(0.0);
    let goal = Vec3::new(600., 0., 0.);
    let mut stop = None;
    let mut window = None;
    for k in 0..600 {
        r.step(Some((Vec3::ZERO, goal, false)), 0.0);
        let l = r.loco();
        let d = goal.x - r.guard().position.x;
        if window.is_none() && l.state == st::STOP_WINDOW {
            window = Some(d);
        }
        if stop.is_none() && l.state == st::STOP_REQUESTED {
            stop = Some((k, d, r.specific_name().unwrap()));
        }
        if stop.is_some() && l.state == st::NONE {
            break;
        }
    }
    let w = window.expect("the stop window opened");
    assert!(w < 120.0 && w > 100.0, "the window opened {w} uu from the point");
    let (_, d, name) = stop.expect("a stop was asked");
    assert!(name.contains("Empty_Walk_Stop"), "{name}");
    // |r - s| > |r| with r = d - 70, s = 1.167 x 157.7 / 2: the stop is asked within about 116 uu.
    assert!(d < 117.0, "the stop was asked {d} uu from the point");
    // The arrival comes after the stop (the brain's walk is then over).
    assert!(r.guard().loco_arrived, "arrived after the stop");
    for _ in 0..30 {
        r.step(None, 0.0);
    }
    let g = r.guard();
    assert!(g.velocity.truncate().length() < 1.0, "the guard stands after its stop");
    // The rule picks the foot whose stop lands nearer the point (now: `r` from it; a foot later:
    // `r - s`), so a stop ends within about half a foot's travel (s / 2 = 38 uu) plus the walk
    // while it blends in, on either side.
    let off = (goal.x - g.position.x).abs();
    assert!(off < 60.0, "it stopped {off} uu from the point (asked at {d} uu)");
}

/// An accurate request (a scripted go-to that must end on its point) never stop-animates.
#[test]
fn an_accurate_request_never_stops_with_a_sequence() {
    let mut r = Run::new(0.0);
    let goal = Vec3::new(500., 0., 0.);
    for _ in 0..400 {
        r.step(Some((Vec3::ZERO, goal, true)), 0.0);
        assert!(r.loco().state < st::STOP_WINDOW, "state {}", r.loco().state);
    }
}

/// A standing guard with its path behind it turns first: the static turn asks the idle mode's
/// 180 degree turn into a walk (`Empty_TurnToWalk*180`), the pawn holds its yaw until the turn is
/// fully in, the turn's root yaw turns it, and it leaves toward the path.
#[test]
fn a_guard_turns_toward_a_path_behind_it() {
    let mut r = Run::new(0.0);
    let goal = Vec3::new(-1500., 30., 0.);
    r.step(Some((Vec3::ZERO, goal, false)), 0.0);
    let l = r.loco();
    assert_eq!(l.state, st::TURN_REQUESTED, "an immediate static turn");
    let name = r.specific_name().unwrap();
    assert!(name.contains("Empty_TurnToWalk") && name.ends_with("180"), "{name}");
    assert!((r.guard().yaw - 0.0).abs() < 1e-3, "the yaw is held while the turn is asked");
    let mut ended = false;
    for _ in 0..200 {
        r.step(Some((Vec3::ZERO, goal, false)), 0.0);
        if r.loco().state == st::NONE {
            ended = true;
            break;
        }
    }
    assert!(ended, "the turn ended");
    let g = r.guard();
    let to = (goal - g.position).truncate();
    let want = to.y.atan2(to.x).to_degrees();
    let off = ((g.yaw - want + 540.0) % 360.0 - 180.0).abs();
    assert!(off < 10.0, "facing {} for {want}", g.yaw);
    assert!(g.velocity.truncate().length() > 20.0, "it leaves walking");
}
