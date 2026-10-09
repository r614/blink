//! Movable props (`DishonoredMovable`): pick up, carry, drop and throw, on a flat floor with a
//! wall, with the extracted gameplay config (fixtures/config/gameplay_config.json; the movable
//! tuning is the retail defaults the loader reads from `Twk_EmptyHands`). The rules and their
//! addresses are in research/dishonored/systems/movables.md.

use dishonored_ir::world_actors::{BreakStepDef, BreakableDef, MovableDef, WorldActors};
use dishonored_sim::events::{DamageCause, SimEvent, SoundCategory};
use dishonored_sim::input::{PlayerInput, buttons};
use dishonored_sim::state::{MovablePhase, PawnKind, WorldState};
use dishonored_sim::world_actors::{WorldActorsState, WorldEvent};
use dishonored_sim::{SimConfig, movables, movement, spawn_npc, spawn_player, state_hash, step_world};
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A floor at z = 0 and a wall at x = 600 (facing -X).
fn room() -> CollisionWorld {
    let mut tris = Vec::new();
    let quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3, n: Vec3, tris: &mut Vec<Tri>| {
        for (x, y, z) in [(a, b, c), (a, c, d)] {
            tris.push(Tri { a: x, b: y, c: z, n, channels: channel::ALL, source: 0 });
        }
    };
    quad(Vec3::new(-5000., -5000., 0.), Vec3::new(5000., -5000., 0.), Vec3::new(5000., 5000., 0.), Vec3::new(-5000., 5000., 0.), Vec3::Z, &mut tris);
    quad(Vec3::new(600., -5000., 0.), Vec3::new(600., 5000., 0.), Vec3::new(600., 5000., 2000.), Vec3::new(600., -5000., 2000.), -Vec3::X, &mut tris);
    CollisionWorld::from_triangles(tris, vec!["fixture".into()])
}

/// `movable_twk.bottle_01_twk` of Boyle Int (DishonoredMovable_5): weight Tiny, `m_Damage` 5,
/// interactable, break speeds 1000 / 4000 on drop, glass; the mesh's box.
fn bottle(at: Vec3) -> MovableDef {
    MovableDef {
        name: "DishonoredMovable_5".into(),
        package: "L_Boyle_Int_Script".into(),
        class: "DishonoredMovable".into(),
        position: at,
        weight: 0,
        damage: 5,
        interactable: true,
        max_distance: 200.0,
        prompt: "`GBA_Use` Carry".into(),
        min_speed_to_cause_damage: 250.0,
        min_speed_to_be_damaged: 1000.0,
        min_speed_to_be_damaged_on_drop: 4000.0,
        contact_type: "DisContactType_Env_Glass".into(),
        bounds_min: Vec3::new(-6.863295, -6.216063, 0.023569107),
        bounds_max: Vec3::new(7.1058545, 7.069383, 45.639267),
        breakable: Some(0),
        ..Default::default()
    }
}

struct Run {
    state: WorldState,
    cfg: SimConfig,
    world: CollisionWorld,
    events: Vec<SimEvent>,
}

const Z: f32 = 92.0;

impl Run {
    /// Player 1 at the origin facing +X, a bottle `ahead` in front of him at a table's height.
    fn new(npcs: &[(Vec3, f32)]) -> Run {
        let cfg = cfg();
        let mut state = WorldState::default();
        spawn_player(&mut state, 1, Vec3::new(0.0, 0.0, Z), 0.0, &cfg);
        for (i, (p, yaw)) in npcs.iter().enumerate() {
            spawn_npc(&mut state, 100 + i as u32, PawnKind::Guard, 1, *p, *yaw, &cfg);
        }
        let mut r = Run { state, cfg, world: room(), events: Vec::new() };
        r.steps(30, &[]);
        // On a shelf just below the crosshair (the placed prop floats: it rests where it is).
        let at = Vec3::new(120.0, 0.0, r.eye().z - 30.0);
        movables::spawn(&mut r.state, &bottle(at));
        r.events.clear();
        r
    }
    fn step(&mut self, inputs: &[(u32, PlayerInput)]) {
        let tick = self.state.tick;
        let mut ins: Vec<(u32, PlayerInput)> = inputs.to_vec();
        for (_, i) in ins.iter_mut() {
            i.tick = tick;
        }
        for p in &self.state.players {
            if p.kind != PawnKind::Human && !ins.iter().any(|(id, _)| *id == p.id) {
                ins.push((p.id, PlayerInput { tick, yaw: p.yaw, ..Default::default() }));
            }
        }
        let out = step_world(&mut self.state, &ins, &self.world, &self.cfg);
        self.events.extend(out.events);
    }
    fn steps(&mut self, n: usize, inputs: &[(u32, PlayerInput)]) {
        for _ in 0..n {
            self.step(inputs);
        }
    }
    fn p(&self) -> &dishonored_sim::state::PlayerState {
        self.state.player(1).unwrap()
    }
    fn bottle(&self) -> &dishonored_sim::state::MovableState {
        &self.state.movables[0]
    }
    fn eye(&self) -> Vec3 {
        movement::eye_position(self.p(), &self.cfg.gameplay.movement)
    }
    fn grab(&mut self) {
        self.step(&[(1, input(buttons::USE))]);
        self.step(&[(1, input(0))]);
    }
}

fn input(buttons: u16) -> PlayerInput {
    PlayerInput { yaw: 0.0, pitch: 0.0, buttons, ..Default::default() }
}

fn picked_up(ev: &[SimEvent]) -> usize {
    ev.iter().filter(|e| matches!(e, SimEvent::MovablePickedUp { .. })).count()
}

#[test]
fn use_on_the_focused_prop_picks_it_up_and_holds_it_at_the_decoded_place() {
    let mut r = Run::new(&[]);
    r.step(&[(1, input(buttons::USE))]);
    assert_eq!(picked_up(&r.events), 1);
    assert!(matches!(r.events[0], SimEvent::MovablePickedUp { id: 1, .. }));
    assert_eq!(r.bottle().phase, MovablePhase::Held);
    assert_eq!(r.bottle().holder, Some(1));
    assert_eq!(r.p().holding.map(|h| h.movable), Some(r.bottle().id));
    // After `m_fInitialGrabTime` (0.066 s = 4 ticks) the prop is at eye + forward x D - up x H:
    // D = box.min.x + box.max.x - m_fMovableHoldDist(-100) = 100.2, H = box.min.z + box.max.z + 70 -
    // m_fShowMovableHeight_Tiny(75) = 40.66 (0x00C4C190).
    r.steps(8, &[(1, input(0))]);
    let eye = r.eye();
    let b = r.bottle();
    let d = (b.bounds_min.x + b.bounds_max.x) + 100.0;
    let h = (b.bounds_max.z + b.bounds_min.z + 70.0) - 75.0;
    let want = eye + Vec3::X * d - Vec3::Z * h;
    assert!((b.position - want).length() < 1e-3, "{:?} vs {:?}", b.position, want);
    assert!(!b.grabbing);
    assert_eq!(b.rotation, Vec3::new(0.0, 0.0, 0.0), "the camera's rotation");
    // It follows the view: looking up 45 degrees swings it along the forward and up axes.
    r.step(&[(1, PlayerInput { pitch: 45.0, ..input(0) })]);
    let eye = r.eye();
    let (sp, cp) = 45f32.to_radians().sin_cos();
    let want = eye + Vec3::new(cp, 0.0, sp) * d - Vec3::new(-sp, 0.0, cp) * h;
    assert!((r.bottle().position - want).length() < 1e-2);
    assert!((r.bottle().rotation.x - 45.0).abs() < 1e-3);
}

#[test]
fn the_grab_slides_the_prop_over_the_initial_grab_time() {
    let mut r = Run::new(&[]);
    let start = r.bottle().position;
    r.step(&[(1, input(buttons::USE))]);
    r.step(&[(1, input(0))]);
    let target = movables::hold_target(r.bottle(), r.eye(), 0.0, 0.0, &r.cfg.gameplay).0;
    // Two ticks in (0.033 s of 0.066): about half way.
    let f = (r.bottle().position - start).length() / (target - start).length();
    assert!((0.4..0.7).contains(&f), "slid {f} of the way after 2 ticks");
    r.steps(3, &[(1, input(0))]);
    assert!((r.bottle().position - movables::hold_target(r.bottle(), r.eye(), 0.0, 0.0, &r.cfg.gameplay).0).length() < 1e-3);
}

#[test]
fn no_pickup_out_of_reach_or_out_of_view() {
    let mut r = Run::new(&[]);
    // Looking away.
    r.step(&[(1, PlayerInput { yaw: 120.0, ..input(buttons::USE) })]);
    assert_eq!(picked_up(&r.events), 0);
    // Too far (m_fMaxDistance 200 + the pawn's radius).
    let mut r = Run::new(&[]);
    r.state.movables[0].position.x = 420.0;
    r.step(&[(1, input(buttons::USE))]);
    assert_eq!(picked_up(&r.events), 0);
    // A prop that is not interactable (the candlesticks, the rope) stays.
    let mut r = Run::new(&[]);
    r.state.movables[0].interactable = false;
    r.step(&[(1, input(buttons::USE))]);
    assert_eq!(picked_up(&r.events), 0);
}

#[test]
fn use_while_holding_drops_it_and_it_falls_without_breaking() {
    let mut r = Run::new(&[]);
    r.grab();
    r.steps(10, &[(1, input(0))]);
    r.events.clear();
    r.step(&[(1, input(buttons::USE))]);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::MovableDropped { id: 1, thrown: false, .. })));
    assert!(r.p().holding.is_none());
    assert_eq!(r.bottle().phase, MovablePhase::Fly);
    assert!(r.bottle().dropped, "m_bDropped selects the on-drop break speed");
    // Pressing Use to drop is not also a new pick-up.
    assert_eq!(picked_up(&r.events), 0);
    r.steps(60, &[(1, input(0))]);
    assert_eq!(r.bottle().phase, MovablePhase::Rest, "settled on the floor");
    assert!(r.bottle().position.z.abs() < 3.0, "pivot on the floor: {}", r.bottle().position.z);
    // The contact is a glass-on-environment AI noise (500 uu, Anomaly) made by the dropper, and the
    // bottle does not break from a drop (about 500 uu/s against 1000 / 4000 on drop).
    let sounds: Vec<_> = r.events.iter().filter_map(|e| if let SimEvent::Sound { radius, category: SoundCategory::Thrown, source, .. } = e { Some((*radius, *source)) } else { None }).collect();
    assert_eq!(sounds, vec![(500.0, 1)], "{:?}", r.events);
    assert!(!r.events.iter().any(|e| matches!(e, SimEvent::MovableImpact { .. })));
}

#[test]
fn primary_while_holding_throws_it_along_the_view_at_throw_strength() {
    let mut r = Run::new(&[]);
    r.grab();
    r.steps(10, &[(1, input(0))]);
    r.events.clear();
    r.step(&[(1, input(buttons::PRIMARY))]);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::MovableDropped { id: 1, thrown: true, .. })));
    assert!(!r.events.iter().any(|e| matches!(e, SimEvent::SwordSwing { .. })), "the press throws, it does not swing");
    let b = r.bottle();
    assert!(!b.dropped, "a throw is not a drop");
    let strength = r.cfg.gameplay.items.inventory.throw_strength;
    assert_eq!(strength, 2000.0);
    assert!((b.velocity.x - strength).abs() < 1e-3 && b.velocity.y.abs() < 1e-3, "{:?}", b.velocity);
    // The thrower is the instigator: his noise, his blame.
    assert_eq!(b.instigator, Some(1));
}

#[test]
fn a_thrown_bottle_breaks_on_the_wall_and_makes_noise() {
    let mut r = Run::new(&[]);
    r.grab();
    r.steps(10, &[(1, input(0))]);
    r.events.clear();
    r.step(&[(1, input(buttons::PRIMARY))]);
    r.steps(40, &[(1, input(0))]);
    let hit = r.events.iter().find_map(|e| if let SimEvent::MovableImpact { speed, breakable, damage, victim, .. } = e { Some((*speed, *breakable, *damage, *victim)) } else { None });
    let (speed, breakable, damage, victim) = hit.unwrap_or_else(|| panic!("no impact: {:?}", r.events));
    assert!((speed - 2000.0).abs() < 1.0, "the wall hit is along the throw: {speed}");
    assert_eq!(breakable, Some(0), "above m_fMinSpeedToBeDamaged (1000): the bottle takes m_Damage");
    assert_eq!(damage, 5);
    assert_eq!(victim, None);
    assert_eq!(r.bottle().damage, 0, "m_Damage wears by what was dealt");
    // Noise at the wall: Env_Glass -> Environment, 500 uu, made by the thrower.
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Sound { radius, category: SoundCategory::Thrown, source: 1, position, .. } if *radius == 500.0 && (position.x - 600.0).abs() < 12.0)));
    // The mission layer feeds the impact to the actor's breakable: health 5, threshold 1, one step.
    let defs = WorldActors {
        breakables: vec![BreakableDef { name: "DishonoredMovable_5".into(), health: 5, damage_threshold: 1, steps: vec![BreakStepDef { health_threshold: 0, ..Default::default() }], ..Default::default() }],
        ..Default::default()
    };
    let mut wa = WorldActorsState::new(&defs);
    let evs = wa.step(&defs, &r.events, &r.cfg.gameplay, 1.0 / 60.0);
    assert!(evs.contains(&WorldEvent::Broken { breakable: 0, output: 0 }), "{evs:?}");
    movables::on_broken(&mut r.state, 0);
    assert!(r.bottle().gone);
}

#[test]
fn a_thrown_bottle_hurts_the_guard_it_hits_by_its_m_damage() {
    let mut r = Run::new(&[(Vec3::new(360.0, 0.0, Z), 180.0)]);
    r.grab();
    r.steps(10, &[(1, input(0))]);
    r.events.clear();
    r.step(&[(1, input(buttons::PRIMARY))]);
    r.steps(30, &[(1, input(0))]);
    let dmg = r.events.iter().find_map(|e| if let SimEvent::Damage { victim: 100, attacker, amount, cause } = e { Some((*attacker, *amount, *cause)) } else { None });
    assert_eq!(dmg, Some((Some(1), 5.0, DamageCause::ThrownObject)), "{:?}", r.events);
    // The thrower is not hurt by his own prop on the way out.
    assert!(!r.events.iter().any(|e| matches!(e, SimEvent::Damage { victim: 1, .. })));
}

#[test]
fn the_break_speed_follows_the_tweaks_and_m_bdropped() {
    let mut r = Run::new(&[]);
    let m = &mut r.state.movables[0];
    // Glass bottle: be 1000 > cause 250, so the threshold is the cause speed (0x00A40CF0: the lower of
    // the two when both are positive and be is above cause).
    assert_eq!(m.impact_threshold(), 250.0);
    // A prop whose be is at most cause: be, or the on-drop speed while dropped.
    m.min_speed_to_be_damaged = 100.0;
    m.min_speed_to_cause_damage = 250.0;
    assert_eq!(m.impact_threshold(), 100.0);
    m.dropped = true;
    assert_eq!(m.impact_threshold(), 4000.0);
    // No cause speed (<= 1e-8) falls back to be as well.
    m.min_speed_to_cause_damage = 0.0;
    assert_eq!(m.impact_threshold(), 4000.0);
    m.dropped = false;
    assert_eq!(m.impact_threshold(), 100.0);
}

#[test]
fn damage_and_a_dead_holder_drop_the_prop() {
    let mut r = Run::new(&[]);
    r.grab();
    r.steps(10, &[(1, input(0))]);
    // Damage (0x00AB59F0 calls slot 381 with 0).
    let mut ev = vec![SimEvent::Damage { victim: 1, attacker: None, amount: 3.0, cause: DamageCause::World }];
    movables::step(&mut r.state, &r.world, &r.cfg.gameplay, 1.0 / 60.0, &mut ev);
    assert!(r.p().holding.is_none());
    assert!(ev.iter().any(|e| matches!(e, SimEvent::MovableDropped { thrown: false, .. })));
    assert_eq!(r.bottle().phase, MovablePhase::Fly);
}

#[test]
fn a_broken_prop_cannot_be_picked_up_again() {
    let mut r = Run::new(&[]);
    movables::on_broken(&mut r.state, 0);
    r.step(&[(1, input(buttons::USE))]);
    assert_eq!(picked_up(&r.events), 0);
}

/// An unconscious guard whose aim point is `body_off` degrees off the view and the bottle's centre
/// `bottle_off` degrees off it, both in front of the player.
fn crosshair_contest(body_off: f32, bottle_off: f32) -> Run {
    let mut r = Run::new(&[(Vec3::new(-400.0, 0.0, Z), 0.0)]);
    let eye = r.eye();
    let at = |deg: f32, dist: f32| eye + Vec3::new(deg.to_radians().cos(), deg.to_radians().sin(), 0.0) * dist;
    let hh = movement::half_height(r.state.player(100).unwrap(), &r.cfg.gameplay.movement);
    let b = r.state.player_mut(100).unwrap();
    b.condition = dishonored_sim::state::Condition::Unconscious;
    b.position = at(body_off, 100.0) + Vec3::Z * (hh * 0.5);
    let m = &mut r.state.movables[0];
    m.position = at(bottle_off, 120.0) - m.center_offset();
    r.step(&[(1, input(buttons::USE))]);
    r
}

#[test]
fn only_the_prop_or_the_body_nearer_the_crosshair_is_taken() {
    let r = crosshair_contest(0.0, 20.0);
    assert_eq!(r.p().carrying, Some(100), "{:?}", r.events);
    assert_eq!(picked_up(&r.events), 0);
    let r = crosshair_contest(20.0, 0.0);
    assert_eq!(r.p().carrying, None);
    assert_eq!(picked_up(&r.events), 1, "{:?}", r.events);
}

#[test]
fn the_step_is_deterministic() {
    let run = || {
        let mut r = Run::new(&[]);
        r.grab();
        r.steps(10, &[(1, input(0))]);
        r.step(&[(1, input(buttons::PRIMARY))]);
        r.steps(60, &[(1, input(0))]);
        (state_hash(&r.state), r.bottle().position, r.events.len())
    };
    assert_eq!(run(), run());
}
