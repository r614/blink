//! Inventory, takedowns, powers and gadgets: the decoded rules on a flat floor fixture with
//! the extracted gameplay config (fixtures/config/gameplay_config.json).

use dishonored_ir::items::{AmmoType, ElixirType, PickupKind};
use dishonored_sim::events::{ChokeOutcome, DamageCause, KnockoutCause, SimEvent};
use dishonored_sim::input::{LeftHand, PlayerInput, actions, buttons};
use dishonored_sim::state::{ChokePhase, Condition, MoveMode, PawnKind, PickupState, PossessStage, WorldState, power};
use dishonored_sim::{SimConfig, inventory, powers, spawn_npc, spawn_player, state_hash, step_world};
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn floor() -> CollisionWorld {
    let mut tris = Vec::new();
    let (a, b, c, d) = (Vec3::new(-5000., -5000., 0.), Vec3::new(5000., -5000., 0.), Vec3::new(5000., 5000., 0.), Vec3::new(-5000., 5000., 0.));
    for (x, y, z) in [(a, b, c), (a, c, d)] {
        let n = (y - x).cross(z - x).normalize();
        tris.push(Tri { a: x, b: y, c: z, n, channels: channel::ALL, source: 0 });
    }
    CollisionWorld::from_triangles(tris, vec!["fixture".into()])
}

struct Run {
    state: WorldState,
    cfg: SimConfig,
    world: CollisionWorld,
    events: Vec<SimEvent>,
}

const Z: f32 = 92.0;

impl Run {
    /// Player 1 at the origin facing +X; NPC guards from `npcs` (position, yaw).
    fn new(npcs: &[(Vec3, f32)]) -> Run {
        let cfg = cfg();
        let mut state = WorldState::default();
        spawn_player(&mut state, 1, Vec3::new(0.0, 0.0, Z), 0.0, &cfg);
        for (i, (p, yaw)) in npcs.iter().enumerate() {
            spawn_npc(&mut state, 100 + i as u32, PawnKind::Guard, 1, *p, *yaw, &cfg);
        }
        let mut r = Run { state, cfg, world: floor(), events: Vec::new() };
        r.steps(30, &[]);
        r
    }
    fn step(&mut self, inputs: &[(u32, PlayerInput)]) {
        let tick = self.state.tick;
        let mut ins: Vec<(u32, PlayerInput)> = inputs.to_vec();
        for (_, i) in ins.iter_mut() {
            i.tick = tick;
        }
        // NPCs stand still unless driven.
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
    fn p(&self, id: u32) -> &dishonored_sim::state::PlayerState {
        self.state.player(id).unwrap()
    }
    fn pm(&mut self, id: u32) -> &mut dishonored_sim::state::PlayerState {
        self.state.player_mut(id).unwrap()
    }
    fn pickup(&mut self, kind: PickupKind, pos: Vec3) -> u32 {
        let id = 9000 + self.state.pickups.len() as u32;
        self.state.pickups.push(PickupState { id, name: format!("P{id}"), package: String::new(), position: pos, kind, active: true, ammo_left: [0; 8], carried_by: None });
        id
    }
}

fn input(yaw: f32, pitch: f32, buttons: u16) -> PlayerInput {
    PlayerInput { yaw, pitch, buttons, ..Default::default() }
}

#[test]
fn elixir_pickup_and_drink() {
    let mut r = Run::new(&[]);
    let pk = r.pickup(PickupKind::Elixir(ElixirType::Health), Vec3::new(120.0, 0.0, 40.0));
    r.step(&[(1, input(0.0, -20.0, buttons::USE))]);
    r.step(&[(1, input(0.0, -20.0, 0))]);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::PickedUp { pickup, .. } if *pickup == pk)));
    assert_eq!(r.p(1).inv.elixirs[0], 1);
    // Full health: drinking is refused and nothing is consumed (0x00C04FC0).
    r.step(&[(1, PlayerInput { action: actions::DRINK_HEALTH, ..Default::default() })]);
    assert_eq!(r.p(1).inv.elixirs[0], 1);
    r.pm(1).vitals.health = 20.0;
    r.step(&[(1, PlayerInput { action: actions::DRINK_HEALTH, ..Default::default() })]);
    assert_eq!(r.p(1).vitals.health, 60.0, "20 + m_nHealthElixirValue 40");
    assert_eq!(r.p(1).inv.elixirs[0], 0);
    // Capped at HealthMax.
    r.pm(1).inv.elixirs[0] = 1;
    r.step(&[(1, PlayerInput { action: actions::DRINK_HEALTH, ..Default::default() })]);
    assert_eq!(r.p(1).vitals.health, 70.0);
    // Mana elixir: +50.
    r.pm(1).inv.elixirs[1] = 1;
    r.pm(1).vitals.mana = 10.0;
    r.step(&[(1, PlayerInput { action: actions::DRINK_MANA, ..Default::default() })]);
    assert!(r.p(1).vitals.mana >= 60.0 && r.p(1).vitals.mana <= 61.0);
}

#[test]
fn elixir_pickup_refused_at_max() {
    let mut r = Run::new(&[]);
    r.pm(1).inv.elixirs[0] = r.cfg.gameplay.items.inventory.max_health_elixirs;
    r.pickup(PickupKind::Elixir(ElixirType::Health), Vec3::new(120.0, 0.0, 40.0));
    r.step(&[(1, input(0.0, -20.0, buttons::USE))]);
    assert!(r.state.pickups[0].active);
}

#[test]
fn ammo_capacity_and_partial_stat_pickup() {
    let mut r = Run::new(&[]);
    let cfg = r.cfg.gameplay.clone();
    let p = r.pm(1);
    assert_eq!(inventory::add_ammo(p, AmmoType::Grenade, 3, &cfg), 3);
    assert_eq!(inventory::add_ammo(p, AmmoType::Grenade, 5, &cfg), 2, "capacity 5");
    assert!(!inventory::remove_ammo(p, AmmoType::Grenade, 6));
    assert!(inventory::remove_ammo(p, AmmoType::Grenade, 5));
    // A stat pickup with 8 sleep darts given to a player with 5: 5 fit, 3 stay in it.
    r.pm(1).inv.ammo[AmmoType::SleepDart as usize] = 5;
    let mut ammo = [(0i16, 0i16); 8];
    ammo[AmmoType::SleepDart as usize] = (8, 8);
    r.pickup(PickupKind::Stat { health: 0, mana: 0, food: false, ammo }, Vec3::new(120.0, 0.0, 40.0));
    r.state.pickups[0].ammo_left[AmmoType::SleepDart as usize] = 8;
    r.step(&[(1, input(0.0, -20.0, buttons::USE))]);
    assert_eq!(r.p(1).inv.ammo[AmmoType::SleepDart as usize], 10);
    assert!(r.state.pickups[0].active);
    assert_eq!(r.state.pickups[0].ammo_left[AmmoType::SleepDart as usize], 3);
}

#[test]
fn coins_runes_and_buying_powers() {
    let mut r = Run::new(&[]);
    r.pickup(PickupKind::Abstract { item: "Money_twk_Abs.AbstractItems.Coins_AbsItm".into(), quantity: 150, note: false, key_item: true }, Vec3::new(120.0, 0.0, 40.0));
    r.pickup(PickupKind::Abstract { item: "Runes_twk.AbstractItems.Rune_AbsItm".into(), quantity: 1, note: false, key_item: true }, Vec3::new(120.0, 8.0, 40.0));
    r.pickup(PickupKind::Abstract { item: "Boyle.Key_AbsItm".into(), quantity: 1, note: false, key_item: true }, Vec3::new(120.0, -8.0, 40.0));
    for _ in 0..3 {
        r.step(&[(1, input(0.0, -20.0, buttons::USE))]);
        r.step(&[(1, input(0.0, -20.0, 0))]);
    }
    assert_eq!(r.p(1).inv.coins, 150);
    assert_eq!(r.p(1).inv.runes, 1, "{:?}", r.state.pickups);
    assert_eq!(inventory::item_count(&r.state, 1, "Boyle.Key_AbsItm"), 1);
    // Vitality level 1 costs 1 rune and adds 20 to HealthMax (DefaultPlayer.ini m_Powers).
    let cfg = r.cfg.gameplay.clone();
    assert!(powers::buy_power(r.pm(1), power::VITALITY, &cfg));
    assert_eq!(r.p(1).inv.runes, 0);
    assert_eq!(powers::health_max(r.p(1), &cfg), 90.0);
    assert!(!powers::buy_power(r.pm(1), power::VITALITY, &cfg), "no runes left");
}

#[test]
fn choke_knocks_out_after_choke_time_plus_animation() {
    // Guard facing +X at x=120: the player at the origin is behind it.
    let mut r = Run::new(&[(Vec3::new(120.0, 0.0, Z), 0.0)]);
    let hold = input(0.0, 0.0, buttons::USE);
    r.step(&[(1, hold)]);
    assert_eq!(r.p(1).choke.phase, ChokePhase::In);
    assert_eq!(r.p(100).condition, Condition::BeingChoked);
    let required = r.p(1).choke.required;
    assert!((required - (1.5 + 5.033)).abs() < 1e-3);
    let ticks = (required * 60.0).ceil() as usize + 2;
    r.steps(ticks, &[(1, hold)]);
    assert_eq!(r.p(100).condition, Condition::Unconscious);
    assert!(r.p(100).alive, "non-lethal");
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::KnockedOut { victim: 100, cause: KnockoutCause::Choke, .. })));
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::ChokeEnd { outcome: ChokeOutcome::Win, .. })));
    // Holding Use past `m_fChokeHoldToAutoCarry` lifts the body.
    r.steps(30, &[(1, hold)]);
    assert_eq!(r.p(1).carrying, Some(100));
    // Stays unconscious, deterministically, whatever its AI input says.
    r.steps(600, &[(1, input(0.0, 0.0, 0)), (100, PlayerInput { forward: 127, ..Default::default() })]);
    assert_eq!(r.p(100).condition, Condition::Unconscious);
}

#[test]
fn releasing_the_choke_lets_the_victim_go() {
    let mut r = Run::new(&[(Vec3::new(120.0, 0.0, Z), 0.0)]);
    r.steps(60, &[(1, input(0.0, 0.0, buttons::USE))]);
    r.step(&[(1, input(0.0, 0.0, 0))]);
    assert_eq!(r.p(1).choke.phase, ChokePhase::Lose);
    assert_eq!(r.p(100).condition, Condition::Normal);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::ChokeEnd { outcome: ChokeOutcome::Lose, .. })));
}

#[test]
fn carrying_is_slower_and_drop_puts_the_body_down() {
    let mut r = Run::new(&[(Vec3::new(60.0, 0.0, Z), 0.0)]);
    r.pm(100).condition = Condition::Unconscious;
    r.step(&[(1, input(0.0, -40.0, buttons::USE))]);
    r.step(&[(1, input(0.0, 0.0, 0))]);
    assert_eq!(r.p(1).carrying, Some(100));
    assert_eq!(r.p(100).carried_by, Some(1));
    r.steps(60, &[]);
    // Run with the body: `Attribute_GroundSpeedCarryingCorpse` 350 (sword sheathed while carrying).
    r.steps(120, &[(1, PlayerInput { forward: 127, ..Default::default() })]);
    let v = r.p(1).velocity.truncate().length();
    assert!((v - 350.0).abs() < 2.0, "carry speed {v}");
    assert!(r.p(100).position.distance(r.p(1).position) < 100.0, "body follows");
    // Drop: on the floor in front of the carrier.
    r.steps(30, &[(1, input(0.0, 0.0, 0))]);
    r.step(&[(1, input(0.0, 0.0, buttons::USE))]);
    r.steps(120, &[(1, input(0.0, 0.0, 0))]);
    assert_eq!(r.p(1).carrying, None);
    assert_eq!(r.p(100).carried_by, None);
    assert!(r.p(100).position.z < 120.0, "fell to the floor: z {}", r.p(100).position.z);
}

#[test]
fn possession_runs_for_the_host_duration() {
    let mut r = Run::new(&[(Vec3::new(500.0, 0.0, Z), 180.0)]);
    r.pm(1).inv.power_levels[power::POSSESSION] = 2;
    let aim = |b| PlayerInput { yaw: 0.0, buttons: b, left_hand: LeftHand::Possession, ..Default::default() };
    r.steps(5, &[(1, aim(buttons::SECONDARY))]);
    r.step(&[(1, aim(0))]);
    assert_eq!(r.p(1).possess.stage, PossessStage::PrePossess);
    assert_eq!(r.p(1).vitals.mana, 40.0, "m_ManaCost 60");
    r.steps(70, &[(1, aim(0))]);
    assert_eq!(r.p(1).possess.stage, PossessStage::Active);
    assert_eq!(r.p(100).condition, Condition::Possessed);
    // The host walks where the possessor steers it, at m_fGroundSpeedWhilePossessed.
    let x0 = r.p(100).position.x;
    r.steps(120, &[(1, PlayerInput { forward: 127, yaw: 0.0, left_hand: LeftHand::Possession, ..Default::default() })]);
    assert!(r.p(100).position.x > x0 + 200.0);
    let v = r.p(100).velocity.truncate().length();
    assert!(v <= 300.0 + 1.0, "possessed speed {v}");
    // Level 2 NPC duration: 15 s.
    r.steps(15 * 60, &[(1, aim(0))]);
    assert_eq!(r.p(1).possess.stage, PossessStage::Inactive, "ended, cooldown over");
    assert_eq!(r.p(100).condition, Condition::Normal);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::PossessEnd { id: 1, host: 100 })));
}

#[test]
fn possession_needs_level_two_for_people() {
    let mut r = Run::new(&[(Vec3::new(500.0, 0.0, Z), 180.0)]);
    r.pm(1).inv.power_levels[power::POSSESSION] = 1;
    let aim = |b| PlayerInput { yaw: 0.0, buttons: b, left_hand: LeftHand::Possession, ..Default::default() };
    r.steps(5, &[(1, aim(buttons::SECONDARY))]);
    r.step(&[(1, aim(0))]);
    assert_eq!(r.p(1).possess.stage, PossessStage::Inactive);
    assert_eq!(r.p(1).vitals.mana, 100.0);
}

#[test]
fn bend_time_slows_then_stops_the_world() {
    let mut r = Run::new(&[(Vec3::new(500.0, 0.0, Z), 90.0)]);
    r.pm(1).inv.power_levels[power::BEND_TIME] = 1;
    let cast = |b| PlayerInput { buttons: b, left_hand: LeftHand::BendTime, ..Default::default() };
    r.step(&[(1, cast(buttons::SECONDARY))]);
    r.steps(30, &[(1, cast(0))]);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::BendTimeStart { id: 1, level: 0 })));
    assert!((r.p(100).time_scale - 0.15).abs() < 1e-4, "world dilation {}", r.p(100).time_scale);
    assert!((r.p(1).time_scale - 0.85).abs() < 1e-4);
    r.steps(12 * 60, &[(1, cast(0))]);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::BendTimeEnd { id: 1 })));
    assert_eq!(r.p(100).time_scale, 1.0);
    // Level 2 stops time: a walking NPC does not move.
    r.pm(1).inv.power_levels[power::BEND_TIME] = 2;
    r.pm(1).vitals.mana = 100.0;
    r.step(&[(1, cast(buttons::SECONDARY))]);
    r.steps(30, &[(1, cast(0))]);
    let y0 = r.p(100).position.y;
    r.steps(120, &[(1, cast(0)), (100, PlayerInput { forward: 127, yaw: 90.0, ..Default::default() })]);
    assert_eq!(r.p(100).position.y, y0, "frozen");
    // Using it again ends it after the dilation cooldown.
    r.step(&[(1, cast(buttons::SECONDARY))]);
    r.steps(30, &[(1, cast(0))]);
    assert_eq!(r.p(100).time_scale, 1.0);
}

#[test]
fn spring_razor_cuts_down_an_npc_but_ignores_players() {
    let mut r = Run::new(&[(Vec3::new(600.0, 0.0, Z), 180.0)]);
    r.pm(1).inv.ammo[AmmoType::SpringRazor as usize] = 1;
    let place = |b| PlayerInput { pitch: -45.0, buttons: b, left_hand: LeftHand::SpringRazor, ..Default::default() };
    r.step(&[(1, place(buttons::SECONDARY))]);
    r.steps(40, &[(1, place(0))]);
    assert_eq!(r.state.traps.len(), 1);
    assert_eq!(r.p(1).inv.ammo[AmmoType::SpringRazor as usize], 0);
    // The player stands on it: not a target (`DisSpringRazorTargetInterface` is NPC-only).
    r.steps(120, &[(1, PlayerInput { pitch: -45.0, left_hand: LeftHand::SpringRazor, ..Default::default() })]);
    assert!(!r.state.traps[0].triggered);
    // Move away, the NPC walks into it.
    r.steps(150, &[(1, PlayerInput { forward: -127, left_hand: LeftHand::SpringRazor, ..Default::default() })]);
    r.steps(240, &[(100, PlayerInput { forward: 127, yaw: 180.0, ..Default::default() })]);
    assert!(!r.p(100).alive);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Death { victim: 100, cause: DamageCause::SpringRazor, .. })));
}

#[test]
fn grenade_explodes_half_a_second_after_impact() {
    let mut r = Run::new(&[(Vec3::new(500.0, 0.0, Z), 180.0)]);
    r.pm(1).inv.ammo[AmmoType::Grenade as usize] = 1;
    let throw = |b| PlayerInput { pitch: -10.0, buttons: b, left_hand: LeftHand::Grenade, ..Default::default() };
    r.step(&[(1, throw(buttons::SECONDARY))]);
    let mut impact = None;
    let mut boom = None;
    for t in 0..400 {
        r.step(&[(1, throw(0))]);
        if impact.is_none() && r.state.projectiles.iter().any(|p| p.armed) {
            impact = Some(t);
        }
        if boom.is_none() && r.events.iter().any(|e| matches!(e, SimEvent::Explosion { .. })) {
            boom = Some(t);
        }
    }
    let (i, b) = (impact.expect("armed on impact"), boom.expect("exploded"));
    assert!((29..=31).contains(&(b - i)), "0.5 s after impact: {} ticks", b - i);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::Damage { victim: 100, cause: DamageCause::Grenade, .. })));
}

#[test]
fn sleep_dart_knocks_out_after_one_and_a_half_seconds() {
    let mut r = Run::new(&[(Vec3::new(800.0, 0.0, Z), 180.0)]);
    r.pm(1).inv.ammo[AmmoType::SleepDart as usize] = 2;
    let aim = |b| PlayerInput { buttons: b, left_hand: LeftHand::SleepDart, ..Default::default() };
    r.steps(40, &[(1, aim(0))]);
    r.step(&[(1, aim(buttons::SECONDARY))]);
    r.steps(30, &[(1, aim(0))]);
    assert!(r.p(100).sleep_timer > 0.0);
    assert_eq!(r.p(100).condition, Condition::Normal);
    r.steps(90, &[(1, aim(0))]);
    assert_eq!(r.p(100).condition, Condition::Unconscious);
    assert_eq!(r.p(100).vitals.health, r.cfg.gameplay.vitals.health_max, "no damage");
}

#[test]
fn shadow_kill_turns_a_stealth_kill_to_ash() {
    let mut r = Run::new(&[(Vec3::new(100.0, 0.0, Z), 0.0)]);
    r.pm(1).inv.power_levels[power::SHADOW_KILL] = 1;
    r.step(&[(1, input(0.0, 0.0, buttons::PRIMARY))]);
    r.steps(10, &[(1, input(0.0, 0.0, 0))]);
    assert!(!r.p(100).alive);
    assert!(r.p(100).no_body);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::BodyRemoved { body: 100, ash: true })));
}

#[test]
fn melee_reach_grows_with_forward_speed() {
    let c = cfg();
    let mut p = dishonored_sim::state::PlayerState::new(1, Vec3::ZERO, 0.0);
    assert_eq!(dishonored_sim::combat::reach(&p, &c.tuning), 210.0);
    p.velocity = Vec3::new(540.0, 0.0, 0.0);
    // factor 1 + (540 - 400) * 0.005 = 1.7, blended by m_fRayScalePercent 0.5.
    assert!((dishonored_sim::combat::reach(&p, &c.tuning) - 283.5).abs() < 0.01);
}

#[test]
fn bolt_damage_is_ranged_damage_times_multiplier() {
    let c = cfg();
    assert_eq!(c.tuning.bolt_damage, 20.0);
    assert_eq!(c.tuning.sleep_dart_damage, 25.0);
    assert_eq!(c.tuning.incendiary_damage, 50.0);
}

#[test]
fn explosion_falloff() {
    let e = dishonored_ir::items::ExplosionConfig::default();
    assert_eq!(dishonored_sim::gadgets::explosion_damage(&e, 100.0, false), 50.0);
    assert_eq!(dishonored_sim::gadgets::explosion_damage(&e, 450.0, false), 25.0);
    assert_eq!(dishonored_sim::gadgets::explosion_damage(&e, 600.0, false), 0.0);
    assert_eq!(dishonored_sim::gadgets::explosion_damage(&e, 150.0, true), 50.0);
}

#[test]
fn takedown_scenario_is_deterministic() {
    let run = || {
        let mut r = Run::new(&[(Vec3::new(120.0, 0.0, Z), 0.0)]);
        r.steps(500, &[(1, input(0.0, 0.0, buttons::USE))]);
        r.steps(100, &[(1, PlayerInput { forward: 127, ..Default::default() })]);
        state_hash(&r.state)
    };
    assert_eq!(run(), run());
    let _ = MoveMode::Walking;
}
