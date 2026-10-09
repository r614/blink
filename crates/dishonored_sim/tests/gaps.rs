//! Fast reload, the random bone charms, helmets and the Devouring Swarm's eating: the decoded
//! rules on a flat floor with the extracted gameplay config (fixtures/config/gameplay_config.json).

use dishonored_ir::items::{AmmoType, ElixirType, PickupKind};
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::{LeftHand, PlayerInput, actions, buttons};
use dishonored_sim::state::{LeftPhase, PawnKind, PickupState, PlayerState, SwarmState, WorldState};
use dishonored_sim::tuning::ReloadTiming;
use dishonored_sim::{SimConfig, inventory, powers, rats, spawn_npc, spawn_player, state_hash, step_world};
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

const Z: f32 = 92.0;
const TICK: f32 = 1.0 / 60.0;

struct Run {
    state: WorldState,
    cfg: SimConfig,
    world: CollisionWorld,
    events: Vec<SimEvent>,
}

impl Run {
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
    fn p(&self, id: u32) -> &PlayerState {
        self.state.player(id).unwrap()
    }
    fn pm(&mut self, id: u32) -> &mut PlayerState {
        self.state.player_mut(id).unwrap()
    }
    /// Equip the bone charm acting on `attribute` (its first level).
    fn charm(&mut self, attribute: &str) {
        let i = self.cfg.gameplay.items.charms.iter().position(|c| c.attribute == attribute).expect("charm in the config");
        let p = self.pm(1);
        p.inv.charms_owned |= 1 << i;
        p.inv.charms_equipped |= 1 << i;
    }
    /// Ticks until `f` holds (at most `max`), stepping with `input`.
    fn until(&mut self, max: usize, input: PlayerInput, f: impl Fn(&PlayerState) -> bool) -> Option<usize> {
        for n in 1..=max {
            self.step(&[(1, input)]);
            if f(self.p(1)) {
                return Some(n);
            }
        }
        None
    }
}

/// A random stream state whose next `roll()` is below (`below`) or at/above `x`.
fn seed_for(below: bool, x: f32) -> u32 {
    (1u32..).find(|s| {
        let mut p = PlayerState::new(1, Vec3::ZERO, 0.0);
        p.rng = *s;
        (p.roll() < x) == below
    })
    .unwrap()
}

#[test]
fn reload_stages_follow_the_reload_context() {
    // In + m_fReloadTime / ReloadSpeed + Out; the magazine loads at the end of the timed stage.
    let (load, end) = ReloadTiming::PISTOL.stages(0.0, 0.0);
    assert!((load - (0.426 + 0.75)).abs() < 1e-6 && (end - (0.426 + 0.75 + 0.302)).abs() < 1e-6);
    // Pistol Reload 1 (Attribute_ReloadSpeed 2) halves only the timed stage.
    let (load, _) = ReloadTiming::PISTOL.stages(2.0, 0.0);
    assert!((load - (0.426 + 0.375)).abs() < 1e-6);
    // Attribute_FastReload: the fast action's timer is the attribute itself, no ReloadOut.
    assert_eq!(ReloadTiming::PISTOL.stages(202.0, 1.0), (1.0, 1.0));
    assert_eq!(ReloadTiming::CROSSBOW.stages(0.0, 0.3), (0.3, 0.3));
}

#[test]
fn pistol_fast_reload_takes_the_attribute_time() {
    let gun = |b| PlayerInput { buttons: b, left_hand: LeftHand::Pistol, ..Default::default() };
    let reload_ticks = |upgrades: &[&str]| {
        let mut r = Run::new(&[]);
        let g = r.cfg.gameplay.clone();
        for u in upgrades {
            assert!(powers::give_upgrade(r.pm(1), u, &g), "{u}");
        }
        r.steps(40, &[(1, gun(0))]);
        r.step(&[(1, gun(buttons::SECONDARY))]);
        assert_eq!(r.p(1).left.pistol_loaded, 0);
        // Fire recovery, then the automatic reload of the empty gun; count to the round loaded.
        let loaded = r.until(400, gun(0), |p| p.left.pistol_loaded == 1).expect("reloads") as f32 * TICK;
        let idle = r.until(400, gun(0), |p| p.left.phase == LeftPhase::Idle).unwrap_or(0) as f32 * TICK;
        (loaded, idle)
    };
    let (normal, out) = reload_ticks(&[]);
    assert!((normal - (0.3 + 0.426 + 0.75)).abs() < 0.04, "{normal}");
    assert!((out - 0.302).abs() < 0.04, "ReloadOut after loading: {out}");
    let (fast, out) = reload_ticks(&["PistolReloadSpeed1", "PistolReloadSpeed2"]);
    assert!((fast - (0.3 + 1.0)).abs() < 0.04, "Attribute_FastReload 1.0: {fast}");
    assert!(out < 0.02, "no ReloadOut: {out}");
}

#[test]
fn crossbow_fast_reload_loads_the_next_bolt_with_the_shot() {
    let bow = |b| PlayerInput { buttons: b, left_hand: LeftHand::Crossbow, ..Default::default() };
    let mut r = Run::new(&[]);
    let g = r.cfg.gameplay.clone();
    assert!(powers::give_upgrade(r.pm(1), "Crossbow_ReloadSpeed", &g));
    // The pistol's upgrades do not reach the crossbow and the other way round.
    let t = dishonored_sim::tuning::CombatTuning::from_config(&g);
    r.pm(1).left.item = LeftHand::Pistol;
    assert_eq!(dishonored_sim::left_hand::gun_reload(r.p(1), &g, &t).fast, 0.0);
    r.steps(40, &[(1, bow(0))]);
    assert_eq!(dishonored_sim::left_hand::gun_reload(r.p(1), &g, &t).fast, 0.3);
    r.step(&[(1, bow(buttons::SECONDARY))]);
    assert_eq!(r.p(1).left.bolts_loaded, 0);
    let n = r.until(200, bow(0), |p| p.left.bolts_loaded == 1).expect("loads") as f32 * TICK;
    assert!((n - 0.625 / 1.5).abs() < 0.04, "fast fire action: {n}");
    assert_eq!(r.p(1).left.phase, LeftPhase::Idle);
    // Without the upgrade the bolt loads after the fire action and the normal reload.
    let mut r = Run::new(&[]);
    r.steps(40, &[(1, bow(0))]);
    r.step(&[(1, bow(buttons::SECONDARY))]);
    let n = r.until(300, bow(0), |p| p.left.bolts_loaded == 1).expect("loads") as f32 * TICK;
    let want = 0.398 + ReloadTiming::CROSSBOW.reload_in + 1.0;
    assert!((n - want).abs() < 0.04, "{n} vs {want}");
}

#[test]
fn twist_of_fortune_fills_mana_after_any_elixir() {
    let mut r = Run::new(&[]);
    r.charm("Attribute_PotionManaBonusChanceRatio");
    r.pm(1).inv.elixirs[ElixirType::Health as usize] = 2;
    r.pm(1).vitals.health = 20.0;
    r.pm(1).vitals.mana = 10.0;
    let max = powers::mana_max(r.p(1), &r.cfg.gameplay);
    // A roll below the charm's 0.1 after a health elixir: (int)(ManaMax x 2) added, capped.
    r.pm(1).rng = seed_for(true, 0.1);
    r.step(&[(1, PlayerInput { action: actions::DRINK_HEALTH, ..Default::default() })]);
    assert_eq!(r.p(1).vitals.mana, max);
    // A roll above: nothing.
    r.pm(1).vitals.mana = 10.0;
    r.pm(1).vitals.health = 20.0;
    r.pm(1).rng = seed_for(false, 0.1);
    r.step(&[(1, PlayerInput { action: actions::DRINK_HEALTH, ..Default::default() })]);
    assert!(r.p(1).vitals.mana < 12.0, "{}", r.p(1).vitals.mana);
    // Without the charm the pawn's random stream is not drawn.
    let mut r = Run::new(&[]);
    r.pm(1).inv.elixirs[ElixirType::Health as usize] = 1;
    r.pm(1).vitals.health = 20.0;
    let rng = r.p(1).rng;
    let g = r.cfg.gameplay.clone();
    assert!(inventory::drink(r.pm(1), ElixirType::Health, &g, &mut Vec::new()));
    assert_eq!(r.p(1).rng, rng);
}

fn stat_pickup(r: &mut Run, health: i32, food: bool, bullets: u16) -> usize {
    let mut ammo_left = [0u16; 8];
    ammo_left[AmmoType::Bullet as usize] = bullets;
    let id = 9000 + r.state.pickups.len() as u32;
    r.state.pickups.push(PickupState {
        id,
        name: format!("P{id}"),
        package: String::new(),
        position: Vec3::new(100.0, 0.0, 40.0),
        kind: PickupKind::Stat { health, mana: 0, food, ammo: [(0, 0); 8] },
        active: true,
        ammo_left,
        carried_by: None,
        bounds_min: Vec3::ZERO,
        bounds_max: Vec3::ZERO,
    });
    r.state.pickups.len() - 1
}

#[test]
fn scavenger_adds_a_round_to_ammo_piles_of_two_or_more() {
    let g = cfg().gameplay;
    for (seed_below, bullets, want) in [(true, 3u16, 4u16), (false, 3, 3), (true, 1, 1)] {
        let mut r = Run::new(&[]);
        r.charm("Attribute_StatPickupCapacityBonusChance");
        r.pm(1).left.pistol_reserve = 0;
        r.pm(1).left.pistol_loaded = 0;
        r.pm(1).rng = seed_for(seed_below, 0.5);
        let pi = stat_pickup(&mut r, 0, false, bullets);
        assert!(inventory::take(&mut r.state, pi, 0, &g, &mut Vec::new()));
        assert_eq!(inventory::ammo(r.p(1), AmmoType::Bullet), want, "{bullets} bullets, roll below: {seed_below}");
        assert_eq!(r.state.pickups[pi].ammo_left[AmmoType::Bullet as usize], 0);
    }
}

#[test]
fn food_heals_the_food_bonus() {
    let g = cfg().gameplay;
    let mut r = Run::new(&[]);
    r.charm("Attribute_FoodHealBonus");
    let bonus = powers::attribute(r.p(1), &g, "Attribute_FoodHealBonus", 0.0).trunc();
    assert!(bonus > 0.0);
    r.pm(1).vitals.health = 10.0;
    let pi = stat_pickup(&mut r, 5, true, 0);
    inventory::take(&mut r.state, pi, 0, &g, &mut Vec::new());
    assert_eq!(r.p(1).vitals.health, 10.0 + 5.0 + bonus);
    // Not food: no bonus.
    r.pm(1).vitals.health = 10.0;
    let pi = stat_pickup(&mut r, 5, false, 0);
    inventory::take(&mut r.state, pi, 0, &g, &mut Vec::new());
    assert_eq!(r.p(1).vitals.health, 15.0);
}

#[test]
fn eating_takes_the_startup_and_one_interval_per_limb_plus_one() {
    // Lvl1: startup 1.5, 1.5 per limb, 6 limbs: 1.5 + 7 x 1.5.
    assert_eq!(rats::eat_duration(true, rats::CORPSE_LIMBS, 1.5, 1.5), 12.0);
    assert_eq!(rats::eat_duration(false, rats::CORPSE_LIMBS, 1.5, 1.5), 9.0);
    assert_eq!(rats::eat_clock_start(true, 0.5), -0.5);
    assert_eq!(rats::eat_clock_start(false, 0.5), 0.0);
}

/// MEASURED 2026-10-08: a level 1 Devouring Swarm's spawner releases 30 individual rats over 16
/// ticks; after `m_fDuration` (30 s) the swarm ends and its rats despawn over about 10 s.
#[test]
fn a_swarm_releases_rat_pawns_and_they_despawn_after_its_duration() {
    let mut r = Run::new(&[(Vec3::new(3000.0, 0.0, Z), 180.0)]);
    let l = r.cfg.gameplay.items.swarm.levels[0];
    r.state.swarms.push(SwarmState { id: 7000, owner: 1, level: 0, position: Vec3::new(0.0, 0.0, 10.0), spawn_delay: 1e-4, age: 0.0, rats: l.rat_count, release_ticks: 0, ending: false, rats_at_end: 0 });
    let members = |r: &Run| r.state.players.iter().filter(|p| p.swarm == Some(7000) && p.kind == dishonored_sim::state::PawnKind::Rat).count();
    let mut counts = Vec::new();
    for _ in 0..20 {
        r.step(&[]);
        counts.push(members(&r));
    }
    assert_eq!(counts[15], l.rat_count as usize, "{counts:?}");
    assert!(counts[0] > 0 && counts[0] < l.rat_count as usize, "{counts:?}");
    assert!(r.events.iter().filter(|e| matches!(e, SimEvent::RatSpawned { swarm: Some(7000), .. })).count() == l.rat_count as usize);
    // Alive for the duration, then gone within the despawn tail.
    r.steps(((l.duration - 1.0) / TICK) as usize, &[]);
    assert_eq!(members(&r), l.rat_count as usize);
    r.steps((6.0 / TICK) as usize, &[]);
    let mid = members(&r);
    assert!(mid > 0 && mid < l.rat_count as usize, "{mid}");
    r.steps((6.0 / TICK) as usize, &[]);
    assert_eq!(members(&r), 0);
    assert!(r.state.swarms.is_empty());
}
#[test]
fn armour_absorbs_on_its_region_side_and_types() {
    use dishonored_sim::vitals::{ArmorSide, armor_absorbs};
    assert!(armor_absorbs(true, ArmorSide::All, 0.5, true));
    assert!(armor_absorbs(true, ArmorSide::Front, -0.5, true));
    assert!(!armor_absorbs(true, ArmorSide::Front, 0.5, true));
    assert!(armor_absorbs(true, ArmorSide::Back, 0.5, true));
    assert!(!armor_absorbs(false, ArmorSide::All, 0.5, true));
    // The shipped helmets list no absorbed type.
    assert!(!armor_absorbs(true, ArmorSide::All, -1.0, false));
}

#[test]
fn charm_rolls_are_deterministic() {
    let run = || {
        let mut r = Run::new(&[]);
        r.charm("Attribute_PotionManaBonusChanceRatio");
        r.charm("Attribute_StatPickupCapacityBonusChance");
        let g = r.cfg.gameplay.clone();
        for _ in 0..20 {
            r.pm(1).inv.elixirs[1] = 1;
            r.pm(1).vitals.mana = 5.0;
            r.step(&[(1, PlayerInput { action: actions::DRINK_MANA, ..Default::default() })]);
            r.pm(1).left.pistol_reserve = 0;
            let pi = stat_pickup(&mut r, 0, false, 3);
            inventory::take(&mut r.state, pi, 0, &g, &mut Vec::new());
        }
        state_hash(&r.state)
    };
    assert_eq!(run(), run());
}

#[test]
fn attack_drops_a_carried_body_without_throwing_it() {
    use dishonored_sim::state::Condition;
    let mut r = Run::new(&[(Vec3::new(60.0, 0.0, Z), 0.0)]);
    r.pm(100).condition = Condition::Unconscious;
    let look = |b| PlayerInput { pitch: -40.0, buttons: b, ..Default::default() };
    // The Use acts on the crosshair the previous camera update chose (measured on the original,
    // research/dishonored/systems/crosshair.md): the view is held a tick before the press.
    r.step(&[(1, look(0))]);
    r.step(&[(1, look(buttons::USE))]);
    r.step(&[(1, look(0))]);
    assert_eq!(r.p(1).carrying, Some(100));
    r.steps(90, &[(1, PlayerInput::default())]);
    r.step(&[(1, PlayerInput { pitch: 20.0, buttons: buttons::PRIMARY, ..Default::default() })]);
    assert_eq!(r.p(1).carrying, None);
    // Released in front of the carrier with no impulse (no ThrowStrength throw).
    assert_eq!(r.p(100).velocity.truncate().length(), 0.0);
    r.steps(120, &[(1, PlayerInput::default())]);
    assert!(r.p(100).position.truncate().distance(r.p(1).position.truncate()) < 120.0);
}
