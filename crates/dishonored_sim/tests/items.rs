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
    /// A pickup with a 16 uu box mesh (a pickup is hit by the crosshair's sweeps like any
    /// mesh, `dishonored_sim::crosshair`).
    fn pickup(&mut self, kind: PickupKind, pos: Vec3) -> u32 {
        let id = 9000 + self.state.pickups.len() as u32;
        self.state.pickups.push(PickupState { id, name: format!("P{id}"), package: String::new(), position: pos, kind, active: true, ammo_left: [0; 8], carried_by: None, bounds_min: Vec3::splat(-8.0), bounds_max: Vec3::splat(8.0), local_min: Vec3::ZERO, local_max: Vec3::ZERO, rotation: Vec3::ZERO });
        id
    }
    /// Hold a view for a tick so the camera update chooses its crosshair target; the next
    /// tick's Use acts on it (the original's Use reads `m_pCrosshairActor`, measured).
    fn look(&mut self, yaw: f32, pitch: f32) {
        self.step(&[(1, input(yaw, pitch, 0))]);
    }
}

/// Where the crosshair at pitch -20 meets x = 120 from the standing pawn at the origin (the
/// camera's arc: `CameraRig`).
const AIM_Z: f32 = 128.0;

fn input(yaw: f32, pitch: f32, buttons: u16) -> PlayerInput {
    PlayerInput { yaw, pitch, buttons, ..Default::default() }
}

#[test]
fn elixir_pickup_and_drink() {
    let mut r = Run::new(&[]);
    let pk = r.pickup(PickupKind::Elixir(ElixirType::Health), Vec3::new(120.0, 0.0, AIM_Z));
    r.look(0.0, -20.0);
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
    r.pickup(PickupKind::Elixir(ElixirType::Health), Vec3::new(120.0, 0.0, AIM_Z));
    r.look(0.0, -20.0);
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
    r.pickup(PickupKind::Stat { health: 0, mana: 0, food: false, ammo }, Vec3::new(120.0, 0.0, AIM_Z));
    r.state.pickups[0].ammo_left[AmmoType::SleepDart as usize] = 8;
    r.look(0.0, -20.0);
    r.step(&[(1, input(0.0, -20.0, buttons::USE))]);
    assert_eq!(r.p(1).inv.ammo[AmmoType::SleepDart as usize], 10);
    assert!(r.state.pickups[0].active);
    assert_eq!(r.state.pickups[0].ammo_left[AmmoType::SleepDart as usize], 3);
}

#[test]
fn coins_runes_and_buying_powers() {
    let mut r = Run::new(&[]);
    r.pickup(PickupKind::Abstract { item: "Money_twk_Abs.AbstractItems.Coins_AbsItm".into(), quantity: 150, note: false, key_item: true }, Vec3::new(120.0, 0.0, AIM_Z));
    r.pickup(PickupKind::Abstract { item: "Runes_twk.AbstractItems.Rune_AbsItm".into(), quantity: 1, note: false, key_item: true }, Vec3::new(150.0, 0.0, AIM_Z - 10.0));
    r.pickup(PickupKind::Abstract { item: "Boyle.Key_AbsItm".into(), quantity: 1, note: false, key_item: true }, Vec3::new(180.0, 0.0, AIM_Z - 20.0));
    r.look(0.0, -20.0);
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
    // The choke button (Block) held; Use held too for the auto-carry after the win.
    let hold = input(0.0, 0.0, buttons::BLOCK | buttons::USE);
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
    r.steps(60, &[(1, input(0.0, 0.0, buttons::BLOCK))]);
    r.step(&[(1, input(0.0, 0.0, 0))]);
    assert_eq!(r.p(1).choke.phase, ChokePhase::Lose);
    assert_eq!(r.p(100).condition, Condition::Normal);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::ChokeEnd { outcome: ChokeOutcome::Lose, .. })));
}

#[test]
fn carrying_is_slower_and_drop_puts_the_body_down() {
    let mut r = Run::new(&[(Vec3::new(60.0, 0.0, Z), 0.0)]);
    r.pm(100).condition = Condition::Unconscious;
    r.look(0.0, -40.0);
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
    // The release plays `Powers_Cast_Possession_Out` from its own tick; its
    // `DisNotify_Possession` takes the host in the step that crosses it.
    assert_eq!(r.p(1).possess.stage, PossessStage::Inactive);
    let at = r.cfg.gameplay.sequence("Ply_Powers_as", "Powers_Cast_Possession_Out").unwrap().notify_time("DisNotify_Possession").unwrap();
    let mut n = 0;
    while r.p(1).possess.stage != PossessStage::PrePossess && n < 60 {
        r.step(&[(1, aim(0))]);
        n += 1;
    }
    assert_eq!(n, (at / dishonored_sim::movement::DT).floor() as usize, "the notify at {at} s");
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
/// U-102, measured on the original (engine hook, 60 Hz): a thrown grenade is armed from its
/// first tick and `m_fTimeToDetonation` counts 3.0 s down to the explosion, bounces or not.
fn grenade_explodes_three_seconds_after_the_throw() {
    let mut r = Run::new(&[(Vec3::new(500.0, 0.0, Z), 180.0)]);
    r.pm(1).inv.ammo[AmmoType::Grenade as usize] = 1;
    let throw = |b| PlayerInput { pitch: -10.0, buttons: b, left_hand: LeftHand::Grenade, ..Default::default() };
    r.step(&[(1, throw(buttons::SECONDARY))]);
    let mut thrown = None;
    let mut boom = None;
    for t in 0..400 {
        r.step(&[(1, throw(0))]);
        if thrown.is_none() && !r.state.projectiles.is_empty() {
            assert!(r.state.projectiles.iter().all(|p| p.armed), "armed when thrown");
            thrown = Some(t);
        }
        if boom.is_none() && r.events.iter().any(|e| matches!(e, SimEvent::Explosion { .. })) {
            boom = Some(t);
        }
    }
    let (i, b) = (thrown.expect("thrown"), boom.expect("exploded"));
    assert!((179..=181).contains(&(b - i)), "3.0 s after the throw: {} ticks", b - i);
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
    // The guard's own HealthMax (the guard archetype's 50, melee.md F9).
    assert_eq!(r.p(100).vitals.health, r.cfg.gameplay.ai.combat.guard_health_max, "no damage");
}

#[test]
fn shadow_kill_turns_a_stealth_kill_to_ash() {
    let mut r = Run::new(&[(Vec3::new(100.0, 0.0, Z), 0.0)]);
    r.pm(1).inv.power_levels[power::SHADOW_KILL] = 1;
    // Level 1 needs the victim unaware (the AI's hook, 0x00A87B60); the back assassination needs an
    // awareness the assassinate table allows (Unaware, melee.md F4).
    r.pm(100).ai_unaware = true;
    r.pm(100).ai_awareness = dishonored_sim::state::awareness::UNAWARE;
    r.step(&[(1, input(0.0, 0.0, buttons::PRIMARY))]);
    r.steps(10, &[(1, input(0.0, 0.0, 0))]);
    assert!(!r.p(100).alive);
    assert!(r.p(100).no_body);
    assert!(r.events.iter().any(|e| matches!(e, SimEvent::BodyRemoved { body: 100, ash: true })));
}

#[test]
fn shadow_kill_level_one_spares_an_aware_victim() {
    let mut r = Run::new(&[(Vec3::new(100.0, 0.0, Z), 0.0)]);
    r.pm(1).inv.power_levels[power::SHADOW_KILL] = 1;
    // A fighting guard (InCombat: not assassinable, not unaware) killed by five sword hits of 10 on its 50.
    r.pm(100).ai_awareness = dishonored_sim::state::awareness::IN_COMBAT;
    for _ in 0..5 {
        r.step(&[(1, input(0.0, 0.0, buttons::PRIMARY))]);
        r.steps(60, &[(1, input(0.0, 0.0, 0))]);
    }
    assert!(!r.p(100).alive);
    assert!(!r.p(100).no_body, "an aware victim leaves a body at level 1");
}

#[test]
fn sleep_dart_is_instant_on_an_unaware_npc() {
    let mut r = Run::new(&[(Vec3::new(800.0, 0.0, Z), 180.0)]);
    r.pm(1).inv.ammo[AmmoType::SleepDart as usize] = 1;
    r.pm(100).ai_unaware = true;
    let aim = |b| PlayerInput { buttons: b, left_hand: LeftHand::SleepDart, ..Default::default() };
    r.steps(40, &[(1, aim(0))]);
    r.step(&[(1, aim(buttons::SECONDARY))]);
    r.steps(10, &[(1, aim(0))]);
    assert_eq!(r.p(100).condition, Condition::Unconscious);
}

#[test]
fn workshop_upgrades_raise_capacity_and_make_darts_instant() {
    let mut r = Run::new(&[(Vec3::new(800.0, 0.0, Z), 180.0)]);
    let cfg = r.cfg.gameplay.clone();
    assert!(!cfg.items.upgrades.is_empty());
    // Grenade Belt: `Attribute_GrenadeCapacity` +4.
    assert!(powers::give_upgrade(r.pm(1), "GrenadeBelt", &cfg));
    assert_eq!(inventory::capacity(r.p(1), AmmoType::Grenade, &cfg), 9);
    // Instant Sleep: `Attribute_SleepDelayReduction` 1000 removes `m_fTimeBeforeSleep`.
    assert!(powers::give_upgrade(r.pm(1), "InstantSleepArrow", &cfg));
    r.pm(1).inv.ammo[AmmoType::SleepDart as usize] = 1;
    let aim = |b| PlayerInput { buttons: b, left_hand: LeftHand::SleepDart, ..Default::default() };
    r.steps(40, &[(1, aim(0))]);
    r.step(&[(1, aim(buttons::SECONDARY))]);
    r.steps(10, &[(1, aim(0))]);
    assert_eq!(r.p(100).condition, Condition::Unconscious);
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

/// F-5: all 34 workshop upgrades fit the inventory mask (the 33rd and 34th were dropped by a
/// 32-bit mask).
#[test]
fn every_workshop_upgrade_can_be_given() {
    let cfg = cfg();
    let mut p = dishonored_sim::state::PlayerState::new(1, Vec3::ZERO, 0.0);
    for (name, _) in &cfg.gameplay.items.upgrades {
        assert!(powers::give_upgrade(&mut p, name, &cfg.gameplay), "{name}");
    }
    assert_eq!(p.inv.upgrades.count_ones() as usize, cfg.gameplay.items.upgrades.len());
}

/// F-7: taking a charm effect already owned raises it to its second level (0x00ABBA90), whose
/// value then applies; an effect id from 10,000,000 up is a scripted charm (0x00AA8EF0).
#[test]
fn a_charm_taken_twice_gives_its_level_two_value() {
    let cfg = cfg();
    let g = &cfg.gameplay;
    let Some(slot) = g.items.charms.iter().position(|c| c.values.len() > 1) else { return };
    let c = g.items.charms[slot].clone();
    let mut p = dishonored_sim::state::PlayerState::new(1, Vec3::ZERO, 0.0);
    assert_eq!(powers::charm_slot(g, slot as i32), Some(slot));
    inventory::give_charm_effect(&mut p, g, Some(slot as i32), 0);
    // Corvo's `m_bAutoEquipBoneCharm` is false: the player equips it from the menu.
    p.inv.charms_equipped |= 1u64 << slot;
    assert_eq!(powers::attribute(&p, g, &c.attribute, 0.0), c.values[0]);
    inventory::give_charm_effect(&mut p, g, Some(slot as i32), 0);
    assert_eq!(powers::attribute(&p, g, &c.attribute, 0.0), c.values[1]);
    assert_eq!(powers::charm_slot(g, -1), None);
    let base = dishonored_ir::items::SCRIPTED_CHARM_BASE;
    assert_eq!(powers::charm_slot(g, base), (!g.items.scripted_charms.is_empty()).then_some(g.items.charms.len()));
}

/// DLC07 cracked charms (0x00D026B0): each modifier applies with its own type while equipped
/// (Tank: `AddBasePercent` -30 of a 200 ground speed; Statuesque: `SetVal` 0 wins over the
/// passive powers' and regular charms' adds); their slots follow the regular ones in the masks.
#[test]
fn a_cracked_charm_applies_its_typed_modifiers() {
    use dishonored_ir::items::{AttributeModifier, CrackedCharm, ModifierType};
    let mut cfg = cfg();
    let m = |a: &str, kind: ModifierType, value: f32| AttributeModifier { attribute: a.into(), kind, value };
    cfg.gameplay.items.cracked_charms = vec![
        CrackedCharm { name: "Tank".into(), text: String::new(), modifiers: vec![m("Attribute_GroundSpeed", ModifierType::AddBasePercent, -30.0), m("Attribute_DamageTakenMultiplier", ModifierType::AddVal, -0.3)] },
        CrackedCharm { name: "Statuesque".into(), text: String::new(), modifiers: vec![m("Attribute_ManaRegenAdditivePortion", ModifierType::SetVal, 0.0)] },
    ];
    cfg.gameplay.items.cracked_auto_equip = true;
    let g = &cfg.gameplay;
    let mut p = dishonored_sim::state::PlayerState::new(1, Vec3::ZERO, 0.0);
    assert_eq!(powers::attribute(&p, g, "Attribute_GroundSpeed", 200.0), 200.0);
    let tank = powers::cracked_charm_slot(g, 0).unwrap();
    let statuesque = powers::cracked_charm_slot(g, 1).unwrap();
    assert!(tank >= powers::charm_slots(g) && statuesque == tank + 1 && statuesque < 64);
    assert!((0..powers::charm_slots(g) as i32).all(|e| powers::charm_slot(g, e) != Some(tank)));
    inventory::give_cracked_charm_effect(&mut p, g, Some(0), 0);
    assert!((powers::attribute(&p, g, "Attribute_GroundSpeed", 200.0) - 140.0).abs() < 1e-4);
    inventory::give_cracked_charm_effect(&mut p, g, Some(1), 0);
    assert_eq!(powers::attribute(&p, g, "Attribute_ManaRegenAdditivePortion", 0.5), 0.0);
    p.inv.charms_equipped = 0;
    assert_eq!(powers::attribute(&p, g, "Attribute_GroundSpeed", 200.0), 200.0);
}

/// A cracked charm pickup without `m_Effect` gives one not owned yet (the original's pool,
/// 0x00CFCB80; the port's order from the pickup id), and is not equipped without
/// `m_bAutoEquipBoneCharm` (Daud).
#[test]
fn cracked_charm_without_effect_picks_an_unowned_one() {
    use dishonored_ir::items::CrackedCharm;
    let mut cfg = cfg();
    cfg.gameplay.items.cracked_charms = (0..3).map(|i| CrackedCharm { name: format!("C{i}"), ..Default::default() }).collect();
    cfg.gameplay.items.cracked_auto_equip = false;
    let g = &cfg.gameplay;
    let mut p = dishonored_sim::state::PlayerState::new(1, Vec3::ZERO, 0.0);
    for _ in 0..3 {
        inventory::give_cracked_charm_effect(&mut p, g, None, 1);
    }
    let r = powers::cracked_slots(g);
    assert!(r.clone().all(|s| p.inv.charms_owned & (1u64 << s) != 0));
    assert_eq!(p.inv.charms_equipped, 0);
}
