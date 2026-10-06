//! The rest of the player: swimming and breath, Agility's power jump, Blood Thirsty, Dark Vision,
//! armour, pistol upgrades, rat pawns and their attacks, rat possession, determinism.

use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::state::{MoveMode, PawnKind, PossessStage, WorldState, power};
use dishonored_sim::{SimConfig, powers, rats, spawn_npc, spawn_player, state_hash, step_world};
use dishonored_world::{CollisionWorld, Tri, WaterVolume, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A floor at z = 0 (x < 2000) and a pool from x = 2000 on: water 400 deep over a floor at -400.
fn world() -> CollisionWorld {
    let mut tris = Vec::new();
    let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
        for (x, y, z) in [(a, b, c), (a, c, d)] {
            let n = (y - x).cross(z - x).normalize();
            tris.push(Tri { a: x, b: y, c: z, n, channels: channel::ALL, source: 0 });
        }
    };
    quad(Vec3::new(-5000., -5000., 0.), Vec3::new(2000., -5000., 0.), Vec3::new(2000., 5000., 0.), Vec3::new(-5000., 5000., 0.));
    quad(Vec3::new(2000., -5000., -400.), Vec3::new(9000., -5000., -400.), Vec3::new(9000., 5000., -400.), Vec3::new(2000., 5000., -400.));
    let mut w = CollisionWorld::from_triangles(tris, vec!["fixture".into()]);
    let pool = WaterVolume {
        name: "pool".into(),
        hulls: vec![vec![[0., 0., 1., -20.], [0., 0., -1., 400.], [1., 0., 0., 9000.], [-1., 0., 0., -2000.], [0., 1., 0., 5000.], [0., -1., 0., 5000.]]],
        min: Vec3::new(2000., -5000., -400.),
        max: Vec3::new(9000., 5000., -20.),
        fluid_friction: 2.4,
        terminal_velocity: 4000.0,
        shallow: false,
    };
    w.water = Some(std::sync::Arc::new(vec![pool]));
    w
}

struct Run {
    state: WorldState,
    cfg: SimConfig,
    world: CollisionWorld,
    events: Vec<SimEvent>,
}

impl Run {
    fn new(players: &[(u32, Vec3, f32)]) -> Run {
        let cfg = cfg();
        let mut state = WorldState::default();
        for (id, p, yaw) in players {
            spawn_player(&mut state, *id, *p, *yaw, &cfg);
        }
        let mut r = Run { state, cfg, world: world(), events: Vec::new() };
        r.steps(40, &[]);
        r
    }
    fn step(&mut self, inputs: &[(u32, PlayerInput)]) {
        let tick = self.state.tick;
        let mut ins = inputs.to_vec();
        for (_, i) in ins.iter_mut() {
            i.tick = tick;
        }
        for p in &self.state.players {
            if !ins.iter().any(|(id, _)| *id == p.id) {
                ins.push((p.id, PlayerInput { tick, yaw: p.yaw, left_hand: p.left.item, ..Default::default() }));
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
    fn p(&self, id: u32) -> &dishonored_sim::state::PlayerState {
        self.state.player(id).unwrap()
    }
    fn pm(&mut self, id: u32) -> &mut dishonored_sim::state::PlayerState {
        self.state.player_mut(id).unwrap()
    }
}

fn input(yaw: f32, forward: i8, b: u16) -> PlayerInput {
    PlayerInput { yaw, forward, buttons: b, ..Default::default() }
}

#[test]
fn walking_into_deep_water_swims_and_leaving_it_falls() {
    let mut r = Run::new(&[(1, Vec3::new(1700., 0., 92.), 0.)]);
    r.steps(240, &[(1, input(0., 127, 0))]);
    assert_eq!(r.p(1).mode, MoveMode::Swimming, "z {}", r.p(1).position.z);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Swim { id: 1, entered: true })));
    // Neutral buoyancy (Buoyancy 1): no sinking to the bottom while idle.
    let z = r.p(1).position.z;
    r.steps(120, &[(1, input(0., 0, 0))]);
    assert!((r.p(1).position.z - z).abs() < 60.0, "drift {}", r.p(1).position.z - z);
    // Swimming speed stays at or below WaterSpeed (600) x the speed modifier.
    r.steps(120, &[(1, input(0., 127, 0))]);
    assert!(r.p(1).velocity.length() <= 600.0 + 1.0, "{}", r.p(1).velocity.length());
}

#[test]
fn breath_runs_out_under_water_and_refills_out_of_it() {
    let mut r = Run::new(&[(1, Vec3::new(4000., 0., -250.), 0.)]);
    let max = r.cfg.gameplay.movement.water.apnea_max;
    // Hold the swimmer under the surface by looking down and swimming down.
    let dive = PlayerInput { pitch: -60.0, forward: 0, buttons: buttons::CROUCH, ..Default::default() };
    r.steps(60, &[(1, dive)]);
    assert!(r.p(1).water.under, "head under water at z {}", r.p(1).position.z);
    // 40 settle ticks plus 60: 100 ticks of the head under water.
    let used = r.p(1).water.air_used;
    assert!((used - 100.0 / 60.0).abs() < 0.05, "air used {used}");
    // Out of air: health drops by the drowning amount every step (directly, not as damage).
    r.pm(1).water.air_used = max;
    let h = r.p(1).vitals.health;
    r.steps(2, &[(1, dive)]);
    assert_eq!(r.p(1).vitals.health, h - r.cfg.gameplay.movement.water.drowning_damage);
    r.steps(30, &[(1, dive)]);
    assert_eq!(r.p(1).vitals.health, h - r.cfg.gameplay.movement.water.drowning_damage, "next point only after the step");
}

#[test]
fn agility_power_jump_at_the_top_of_a_held_jump() {
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 92.), 0.)]);
    let z0 = r.p(1).position.z;
    let apex = |r: &mut Run, hold: bool| {
        r.steps(10, &[(1, input(0., 0, 0))]);
        let z0 = r.p(1).position.z;
        r.step(&[(1, input(0., 0, buttons::JUMP))]);
        let mut top = z0;
        for _ in 0..120 {
            r.step(&[(1, input(0., 0, if hold { buttons::JUMP } else { 0 }))]);
            top = top.max(r.p(1).position.z);
        }
        top - z0
    };
    let normal = apex(&mut r, true);
    assert!((normal - 106.7).abs() < 3.0, "no Agility: {normal}");
    r.pm(1).inv.power_levels[power::AGILITY] = 1;
    let powered = apex(&mut r, true);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::PowerJump { id: 1 })));
    // JumpZ_PowerJump 1300 at the apex: another 1300^2 / 6000 = 282 uu.
    assert!(powered > normal + 250.0, "powered {powered}");
    // Released before the apex: no power jump.
    let released = apex(&mut r, false);
    assert!((released - normal).abs() < 3.0, "released {released}");
    let _ = z0;
}

#[test]
fn blood_thirsty_fills_and_the_finisher_kills_an_npc() {
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 92.), 0.)]);
    spawn_npc(&mut r.state, 100, PawnKind::Guard, 1, Vec3::new(120., 0., 92.), 180.0, &r.cfg);
    r.steps(20, &[]);
    r.pm(1).inv.power_levels[power::BLOOD_THIRSTY] = 1;
    let max = r.cfg.gameplay.adrenaline.max;
    let g = r.cfg.gameplay.clone();
    let mut ev = Vec::new();
    dishonored_sim::adrenaline::gain(r.pm(1), max - 1.0, &g, &mut ev);
    assert!(!r.p(1).adrenaline.full);
    // A sword hit (14 damage x 2.0) fills it.
    r.step(&[(1, input(0., 0, buttons::PRIMARY))]);
    r.steps(30, &[(1, input(0., 0, 0))]);
    assert!(r.p(1).adrenaline.full, "value {}", r.p(1).adrenaline.value);
    assert!(r.p(100).alive);
    // The next hit is a finisher.
    r.steps(30, &[(1, input(0., 0, 0))]);
    r.step(&[(1, input(0., 0, buttons::PRIMARY))]);
    r.steps(30, &[(1, input(0., 0, 0))]);
    assert!(!r.p(100).alive);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Death { victim: 100, cause: DamageCause::Adrenaline, .. })));
    assert_eq!(r.p(1).adrenaline.value, 0.0);
    // Idle for longer than the cooldown: the gauge burns at 1/s.
    dishonored_sim::adrenaline::gain(r.pm(1), 50.0, &g, &mut ev);
    r.steps(60 * 12, &[(1, input(0., 0, 0))]);
    let v = r.p(1).adrenaline.value;
    assert!((v - 48.0).abs() < 0.2, "burned to {v}");
}

#[test]
fn dark_vision_fades_in_runs_and_cannot_be_cancelled_within_its_cooldown() {
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 92.), 0.), (2, Vec3::new(1500., 0., 92.), 0.)]);
    let dv = |b| PlayerInput { buttons: b, left_hand: LeftHand::DarkVision, ..Default::default() };
    r.step(&[(1, dv(buttons::SECONDARY))]);
    let t = r.cfg.tuning.clone();
    assert_eq!(r.p(1).vitals.mana, 100.0 - t.dark_vision_cost);
    assert!(dishonored_sim::left_hand::dark_vision_active(r.p(1)));
    assert_eq!(dishonored_sim::left_hand::dark_vision_reveals(&r.state, 1, &t), vec![2]);
    // Pressing again at once does nothing (cooldown).
    r.step(&[(1, dv(0))]);
    r.step(&[(1, dv(buttons::SECONDARY))]);
    assert!(r.p(1).dark.remaining > 0.0);
    // After the cooldown it ends (fade-out plays).
    r.steps(70, &[(1, dv(0))]);
    r.step(&[(1, dv(buttons::SECONDARY))]);
    assert_eq!(r.p(1).dark.remaining, 0.0);
    r.steps(60, &[(1, dv(0))]);
    assert!(!dishonored_sim::left_hand::dark_vision_active(r.p(1)));
    assert!(dishonored_sim::left_hand::dark_vision_reveals(&r.state, 1, &t).is_empty());
}

#[test]
fn armour_reduces_damage_but_not_below_one_and_not_bites() {
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 92.), 0.)]);
    let g = r.cfg.gameplay.clone();
    assert!(powers::give_upgrade(r.pm(1), "Armor", &g));
    r.step(&[]);
    assert_eq!(r.p(1).damage_reduction, 5.0);
    let v = g.vitals.clone();
    let mut ev = Vec::new();
    let p = r.pm(1);
    dishonored_sim::vitals::apply_damage(p, 14.0, None, DamageCause::Sword, &mut ev, &v);
    assert_eq!(p.vitals.health, 70.0 - 9.0);
    dishonored_sim::vitals::apply_damage(p, 3.0, None, DamageCause::Pistol, &mut ev, &v);
    assert_eq!(p.vitals.health, 70.0 - 10.0);
    dishonored_sim::vitals::apply_damage(p, 4.0, None, DamageCause::RatBite, &mut ev, &v);
    assert_eq!(p.vitals.health, 70.0 - 14.0);
}

#[test]
fn pistol_magazine_upgrade_loads_more_rounds() {
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 92.), 0.)]);
    let g = r.cfg.gameplay.clone();
    assert!(powers::give_upgrade(r.pm(1), "PistolMagazine3", &g));
    let gun = |b| PlayerInput { buttons: b, left_hand: LeftHand::Pistol, ..Default::default() };
    r.steps(40, &[(1, gun(0))]);
    r.steps(3, &[(1, gun(buttons::RELOAD))]);
    r.steps(120, &[(1, gun(0))]);
    assert_eq!(r.p(1).left.pistol_loaded, 4);
}

#[test]
fn pistol_dispersion_narrows_while_aiming_and_blooms_after_a_shot() {
    use dishonored_sim::left_hand::{DISPERSION_REF_ASPECT, DISPERSION_REF_HFOV_DEG};
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 92.), 0.)]);
    let gun = |b| PlayerInput { buttons: b, left_hand: LeftHand::Pistol, ..Default::default() };
    // Equipping starts at Attribute_DispersionMax (45).
    r.steps(1, &[(1, gun(0))]);
    assert!((r.p(1).left.dispersion - 45.0).abs() < 70.0 / 60.0 + 1e-3, "{}", r.p(1).left.dispersion);
    // With a round loaded it falls to the minimum (15) at 70 per second: 30 / 70 = 0.43 s.
    r.steps(20, &[(1, gun(0))]);
    let d = r.p(1).left.dispersion;
    assert!(d > 15.0 && d < 45.0, "{d}");
    r.steps(30, &[(1, gun(0))]);
    assert_eq!(r.p(1).left.dispersion, 15.0);
    // A shot leaves inside the minimum cone, then the dispersion is at the maximum.
    r.steps(1, &[(1, gun(buttons::SECONDARY))]);
    let pr = r.state.projectiles.iter().find(|p| p.owner == 1).expect("a bullet");
    let k = 2.0 * (DISPERSION_REF_HFOV_DEG.to_radians() * 0.5).tan() / (720.0 * DISPERSION_REF_ASPECT);
    let off = pr.velocity.normalize().angle_between(Vec3::X);
    assert!(off <= (15.0 * k).atan() + 1e-4, "{off}");
    assert_eq!(r.p(1).left.dispersion, 45.0);
    // Empty (reloading): no narrowing.
    r.steps(10, &[(1, gun(0))]);
    assert_eq!(r.p(1).left.dispersion, 45.0);
}

#[test]
fn dispersion_is_held_at_the_maximum_in_the_air_and_upgrades_apply_to_their_weapon_only() {
    use dishonored_sim::left_hand::{Dispersion, dispersion_offset, offset_direction, update_dispersion};
    use dishonored_sim::tuning::CombatTuning;
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 92.), 0.)]);
    // Twk_Upgrade_Pistol_Accuracy1 (host DishonoredWepPistol) and Crossbow_Accuracy, as cooked.
    r.cfg.gameplay.items.upgrades.push(("Pistol_Accuracy1".into(), vec![("Attribute_DispersionMax".into(), -10.0), ("Attribute_DispersionMin".into(), -10.0)]));
    r.cfg.gameplay.items.upgrades.push(("Crossbow_Accuracy".into(), vec![("Attribute_DispersionMax".into(), -10.0), ("Attribute_DispersionInterpolationSpeed".into(), 40.0)]));
    let g = r.cfg.gameplay.clone();
    let t = CombatTuning::from_config(&g);
    let p = r.pm(1);
    assert!(powers::give_upgrade(p, "Pistol_Accuracy1", &g));
    p.left.item = LeftHand::Pistol;
    assert_eq!(Dispersion::of(p, &g, &t).map(|d| (d.max, d.min, d.speed)), Some((35.0, 5.0, 70.0)));
    p.left.item = LeftHand::Crossbow;
    assert_eq!(Dispersion::of(p, &g, &t).map(|d| (d.max, d.min, d.speed)), Some((30.0, 0.0, 70.0)));
    p.left.item = LeftHand::Pistol;
    // Falling (m_bMaxDispersionOnJump) and sprinting hold it at the maximum.
    p.left.dispersion = 5.0;
    p.mode = MoveMode::Falling;
    update_dispersion(p, &g, &t, 1.0 / 60.0);
    assert_eq!(p.left.dispersion, 35.0);
    p.mode = MoveMode::Walking;
    p.sprinting = true;
    p.left.dispersion = 5.0;
    update_dispersion(p, &g, &t, 1.0 / 60.0);
    assert_eq!(p.left.dispersion, 35.0);
    // Offsets stay inside the radius; the cone of 45 px is about 3.1 degrees.
    for _ in 0..200 {
        let (x, y) = dispersion_offset(p, 45.0);
        assert!((x * x + y * y).sqrt() <= 45.0 + 1e-3);
    }
    let edge = offset_direction(0.0, 0.0, (45.0, 0.0)).angle_between(Vec3::X).to_degrees();
    assert!((edge - 3.09).abs() < 0.05, "{edge}");
    // Positive x is to the right of the view (+Y at yaw 0), positive y downward.
    assert!(offset_direction(0.0, 0.0, (10.0, 0.0)).y > 0.0);
    assert!(offset_direction(0.0, 0.0, (0.0, 10.0)).z < 0.0);
}

#[test]
fn rats_bite_in_groups_and_slow_their_target() {
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 92.), 0.)]);
    let cfg = r.cfg.clone();
    let mut ev = Vec::new();
    for k in 0..20u32 {
        let a = k as f32 * 0.3;
        rats::spawn_rat(&mut r.state, 500 + k, Vec3::new(150.0 * a.cos(), 150.0 * a.sin(), 12.0), 180.0 + a.to_degrees(), None, &cfg, &mut ev);
    }
    let attack: Vec<(u32, PlayerInput)> = (0..20u32).map(|k| (500 + k, PlayerInput { yaw: 180.0 + (k as f32 * 0.3).to_degrees(), buttons: buttons::PRIMARY, ..Default::default() })).collect();
    let h0 = r.p(1).vitals.health;
    r.steps(31, &attack);
    // On the player there is no initial delay: one bite per 0.5 s of clamp(20 x 0.2, 1, 10) = 4.
    let bites = r.events.iter().filter(|e| matches!(e, SimEvent::RatBite { victim: 1, .. })).count();
    assert_eq!(bites, 1);
    assert_eq!(r.p(1).vitals.health, h0 - 4.0);
    assert!(r.p(1).rat_slow > 0.0);
    // Slowed: 40 % off the ground speed for a second.
    let m = powers::effective_movement(r.p(1), &cfg.gameplay);
    assert!((m.run_speed - cfg.gameplay.movement.run_speed * 0.6).abs() < 1e-3);
    // Rats die from any damage (Health 1).
    let rat = r.pm(500);
    dishonored_sim::vitals::apply_damage(rat, 1.0, Some(1), DamageCause::Sword, &mut ev, &cfg.gameplay.vitals);
    assert!(!r.p(500).alive);
}

#[test]
fn possession_takes_a_rat_out_of_a_swarm() {
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 92.), 0.)]);
    r.pm(1).inv.power_levels[power::DEVOURING_SWARM] = 1;
    r.pm(1).inv.power_levels[power::POSSESSION] = 1;
    let swarm = |b| PlayerInput { pitch: -20.0, buttons: b, left_hand: LeftHand::DevouringSwarm, ..Default::default() };
    r.step(&[(1, swarm(buttons::SECONDARY))]);
    r.steps(120, &[(1, swarm(0))]);
    assert_eq!(r.state.swarms.len(), 1);
    let rats0 = r.state.swarms[0].rats;
    r.pm(1).vitals.mana = 100.0;
    let poss = |b| PlayerInput { pitch: -20.0, buttons: b, left_hand: LeftHand::Possession, ..Default::default() };
    r.steps(5, &[(1, poss(buttons::SECONDARY))]);
    r.step(&[(1, poss(0))]);
    r.steps(90, &[(1, poss(0))]);
    assert_eq!(r.state.swarms[0].rats, rats0 - 1);
    assert_eq!(r.p(1).possess.stage, PossessStage::Active);
    let host = r.p(1).possess.host.unwrap();
    assert_eq!(r.p(host).kind, PawnKind::Rat);
    // Rat possession lasts the rat's level-1 duration (20 s).
    assert!((r.p(1).possess.timer - 20.0).abs() < 1.6, "{}", r.p(1).possess.timer);
}

#[test]
fn deterministic_with_water_rats_and_powers() {
    let run = || {
        let mut r = Run::new(&[(1, Vec3::new(1700., 0., 92.), 0.), (2, Vec3::new(0., 0., 92.), 0.)]);
        r.pm(1).inv.power_levels[power::AGILITY] = 2;
        r.pm(2).inv.power_levels[power::BLOOD_THIRSTY] = 2;
        let cfg = r.cfg.clone();
        let mut ev = Vec::new();
        rats::spawn_rat(&mut r.state, 700, Vec3::new(100., 0., 12.), 180.0, None, &cfg, &mut ev);
        for k in 0..300 {
            let b = if k % 50 < 20 { buttons::JUMP } else { 0 };
            r.step(&[(1, input(0., 127, b)), (2, input(0., 0, if k % 40 == 0 { buttons::PRIMARY } else { 0 })), (700, PlayerInput { yaw: 180.0, buttons: buttons::PRIMARY, ..Default::default() })]);
        }
        state_hash(&r.state)
    };
    assert_eq!(run(), run());
}

/// A pain volume over the floor around x in -1000..-600 like Boyle's fence volumes (zone
/// velocity (40, 90, 25), 1 damage per second of `DisDamageType_SpringRazor`).
fn pain_world() -> CollisionWorld {
    let mut w = world();
    let v = dishonored_world::PhysicsVolume {
        name: "PhysicsVolume_55".into(),
        hulls: vec![vec![[0., 0., 1., 300.], [0., 0., -1., 100.], [1., 0., 0., -600.], [-1., 0., 0., 1000.], [0., 1., 0., 200.], [0., -1., 0., 200.]]],
        min: Vec3::new(-1000., -200., -100.),
        max: Vec3::new(-600., 200., 300.),
        priority: 0,
        zone_velocity: Vec3::new(40., 90., 25.),
        velocity_affects_walking: true,
        pain: Some(dishonored_world::VolumePain { damage_per_sec: 1.0, interval: 1.0, entry: true, damage_type: "DisDamageType_SpringRazor".into() }),
    };
    w.volumes = Some(std::sync::Arc::new(vec![v]));
    w
}

#[test]
fn pain_volume_hurts_on_entry_and_every_interval_and_pushes() {
    let mut r = Run::new(&[(1, Vec3::new(-800., 0., 100.), 0.0)]);
    r.world = pain_world();
    let h0 = r.p(1).vitals.health;
    let start = r.p(1).position;
    r.events.clear();
    // Entry pain at once, then one per interval: 1 + 3 over three seconds.
    r.steps((3.0 / dishonored_sim::movement::DT) as usize, &[]);
    let hits = r.events.iter().filter(|e| matches!(e, SimEvent::Damage { victim: 1, cause: DamageCause::SpringRazor, amount, .. } if *amount == 1.0)).count();
    assert!((3..=4).contains(&hits), "pain hits {hits}");
    assert_eq!(r.p(1).vitals.health, h0 - hits as f32);
    // Standing still, the zone velocity walks the pawn along +x/+y (25 x dt of it per step).
    let moved = r.p(1).position - start;
    assert!(moved.y > 20.0 && moved.x > 5.0, "pushed {moved:?}");
}

#[test]
fn outside_pain_volume_no_damage() {
    let mut r = Run::new(&[(1, Vec3::new(0., 0., 100.), 0.0)]);
    r.world = pain_world();
    let h0 = r.p(1).vitals.health;
    r.steps(200, &[]);
    assert_eq!(r.p(1).vitals.health, h0);
}
