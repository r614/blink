//! Animation state in the simulation (`dishonored_sim::anim`) on the fixture config, without the
//! install: the sequences the states play, their notifies fired from the playhead in the
//! original's order (research/dishonored/systems/tick_order.md), the gameplay they drive, the
//! state's round trip through serialization.

use dishonored_ir::items::AmmoType;
use dishonored_sim::anim::{SeqTable, layer, token};
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::movement::DT;
use dishonored_sim::state::{LeftPhase, PawnKind, SwordPhase, WorldState, awareness, power};
use dishonored_sim::{SimConfig, spawn_npc, spawn_player, state_hash, step_world};
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn floor() -> CollisionWorld {
    let (a, b, c, d) = (Vec3::new(-5000., -5000., 0.), Vec3::new(5000., -5000., 0.), Vec3::new(5000., 5000., 0.), Vec3::new(-5000., 5000., 0.));
    let tri = |x: Vec3, y: Vec3, z: Vec3| Tri { a: x, b: y, c: z, n: (y - x).cross(z - x).normalize(), channels: channel::ALL, source: 0 };
    CollisionWorld::from_triangles(vec![tri(a, b, c), tri(a, c, d)], vec!["fixture".into()])
}

const Z: f32 = 88.5;

struct Run {
    state: WorldState,
    cfg: SimConfig,
    world: CollisionWorld,
    /// Events by the tick (0-based from the run's start) that produced them.
    events: Vec<(u64, SimEvent)>,
}

impl Run {
    fn new() -> Run {
        let cfg = cfg();
        let mut state = WorldState::default();
        spawn_player(&mut state, 1, Vec3::new(0., 0., Z), 0.0, &cfg);
        let mut r = Run { state, cfg, world: floor(), events: Vec::new() };
        for _ in 0..90 {
            r.step(PlayerInput::default());
        }
        r.events.clear();
        r
    }
    fn step(&mut self, i: PlayerInput) -> u64 {
        self.step_with(&[(1, i)])
    }
    fn step_with(&mut self, ins: &[(u32, PlayerInput)]) -> u64 {
        let t = self.state.tick;
        let ins: Vec<(u32, PlayerInput)> = ins.iter().map(|(id, i)| (*id, PlayerInput { tick: t, ..*i })).collect();
        let out = step_world(&mut self.state, &ins, &self.world, &self.cfg);
        self.events.extend(out.events.into_iter().map(|e| (t, e)));
        t
    }
    fn table(&self) -> &SeqTable {
        &self.cfg.anim
    }
    /// The ticks of the `AnimNotify`s of `class` on `pawn`'s sequence `key`.
    fn notify_ticks(&self, pawn: u32, key: &str, class: &str) -> Vec<u64> {
        let id = self.table().id(key).unwrap_or_else(|| panic!("{key}"));
        let info = self.table().info(id).unwrap();
        self.events
            .iter()
            .filter_map(|(t, e)| match e {
                SimEvent::AnimNotify { pawn: p, sequence, notify, .. } if *p == pawn && *sequence == id && info.notifies[*notify as usize].class == class => Some(*t),
                _ => None,
            })
            .collect()
    }
}

/// The first tick (0-based from `press`) whose step `[k dt, (k + 1) dt)` of a sequence started on
/// the press tick holds the play time `at` (f32 accumulation, as the advance does).
fn crossing_tick(at: f32) -> u64 {
    let (mut t, mut k) = (0.0f32, 0u64);
    loop {
        let t1 = t + DT;
        if at >= t && at < t1 {
            return k;
        }
        t = t1;
        k += 1;
    }
}

/// The player's swing plays `Sword_Ready_AttackRight_A_Small` on the upper body from the press's
/// own tick: its notifies are emitted on the ticks whose steps hold them, the attack zone moves the
/// swing to Active in the step holding its damage time and to Recovery at its end, and the swing
/// ends on the tick after its `AttackInterruptable`.
#[test]
fn a_swing_is_its_sequence() {
    let mut r = Run::new();
    let press = r.step(PlayerInput { buttons: buttons::PRIMARY, ..Default::default() });
    let key = "Ply_Sword_Attack_as.Sword_Ready_AttackRight_A_Small";
    let a = r.state.anim.get(&1).unwrap();
    let up = a.layers[layer::UPPER_BODY].as_ref().expect("the swing plays");
    assert_eq!(r.table().key(up.seq), Some(key));
    assert_eq!(up.token, token::SWING | r.state.player(1).unwrap().sword.swings as u32);
    assert!((up.time - DT).abs() < 1e-6, "the press tick's advance: {}", up.time);
    let info = r.cfg.gameplay.sequences[key].clone();
    let zone = info.notify("DishonoredNotify_AttackZone").unwrap().clone();
    let damage = zone.time + zone.duration - zone.payload["m_fDamageZoneMaxTime"];
    let mut phases = Vec::new();
    for _ in 0..70 {
        let t = r.step(PlayerInput::default());
        phases.push((t - press, r.state.player(1).unwrap().sword.phase));
    }
    let first = |ph: SwordPhase| phases.iter().find(|(_, p)| *p == ph).map(|(k, _)| *k).unwrap();
    assert_eq!(first(SwordPhase::Active), crossing_tick(damage), "the damage zone opens in its step");
    // The window ends in the step where its time runs out (`remaining <= 0`).
    let end = zone.time + zone.duration;
    let rec = first(SwordPhase::Recovery);
    assert!(rec == crossing_tick(end) || rec == crossing_tick(end) + 1, "recovery at {rec}");
    let interruptible = info.notify_time("DishonoredNotify_AttackInterruptable").unwrap();
    assert_eq!(phases.iter().rev().find(|(_, p)| *p != SwordPhase::Idle).map(|(k, _)| *k + 1).unwrap(), crossing_tick(interruptible) + 1, "idle the tick after the flag");
    assert_eq!(r.notify_ticks(1, key, "DishonoredNotify_AttackZone"), vec![press + crossing_tick(zone.time)]);
    assert_eq!(r.notify_ticks(1, key, "DishonoredNotify_AttackInterruptable"), vec![press + crossing_tick(interruptible)]);
    // The sequence plays on to its `AnimStateExit`.
    assert_eq!(r.notify_ticks(1, key, "DishonoredNotify_AnimStateExit"), vec![press + crossing_tick(info.exit_time())]);
}

/// The crossbow's bolt leaves at the crossbow mesh's `Crossbow_Fire` `DishonoredNotify_FireProjectile`
/// (0.0336 s: the third tick of the action, as the original's capture, ranged.md F14); its arm's
/// `Crossbow_Fire` plays on the left arm.
#[test]
fn the_bolt_leaves_at_the_crossbow_meshs_notify() {
    let mut r = Run::new();
    let bow = |b| PlayerInput { buttons: b, left_hand: LeftHand::Crossbow, ..Default::default() };
    for _ in 0..40 {
        r.step(bow(0));
    }
    let press = r.step(bow(buttons::SECONDARY));
    for _ in 0..10 {
        r.step(bow(0));
    }
    let fired: Vec<u64> = r.events.iter().filter_map(|(t, e)| matches!(e, SimEvent::Fired { id: 1, bolt: true, .. }).then_some(*t)).collect();
    let at = r.cfg.tuning.crossbow_fire_notify;
    assert_eq!(fired, vec![press + crossing_tick(at)], "bolt at {at} s");
    assert_eq!(crossing_tick(at), 2);
    assert_eq!(r.notify_ticks(1, "Crossbow_as.Crossbow_Fire", "DishonoredNotify_FireProjectile"), fired);
    let arm = r.state.anim.get(&1).unwrap().layers[layer::LEFT_ARM].as_ref().unwrap();
    assert_eq!(r.table().key(arm.seq), Some("Ply_Crossbow_as.Crossbow_Fire"));
}

/// Bend Time's cast: `Powers_Cast_BendTime` on the left arm; its `DisNotify_BendTime` takes the
/// cost and starts the power in its step; its `AnimStateExit` ends the cast on the next tick.
#[test]
fn bend_time_starts_at_its_cast_notify() {
    let mut r = Run::new();
    r.state.player_mut(1).unwrap().inv.power_levels[power::BEND_TIME] = 1;
    r.state.player_mut(1).unwrap().vitals.mana = 100.0;
    let cast = |b| PlayerInput { buttons: b, left_hand: LeftHand::BendTime, ..Default::default() };
    r.step(cast(0));
    let press = r.step(cast(buttons::SECONDARY));
    let mut exited = Vec::new();
    let mut idle = None;
    for _ in 0..150 {
        let t = r.step(cast(0));
        if idle.is_none() && r.state.player(1).unwrap().left.phase == LeftPhase::Idle {
            idle = Some(t - press);
        }
        exited.push(r.state.anim.get(&1).unwrap().layers[layer::LEFT_ARM].as_ref().is_some_and(|l| l.exited));
    }
    let info = &r.cfg.gameplay.sequences["Ply_Powers_as.Powers_Cast_BendTime"];
    let at = info.notify_time("DisNotify_BendTime").unwrap();
    let started: Vec<u64> = r.events.iter().filter_map(|(t, e)| matches!(e, SimEvent::BendTimeStart { id: 1, .. }).then_some(*t)).collect();
    assert_eq!(started, vec![press + crossing_tick(at)], "the cast notify at {at} s");
    // The rest of the cast runs at the caster's Bend Time rate (`DT x time_scale`); it ends on the
    // tick after the advance that crossed its `AnimStateExit` (`exited[k]` is tick k + 1's).
    let idle = idle.expect("the cast ends") as usize;
    assert!(exited[idle - 2] && !exited[idle - 3], "idle at {idle}");
    assert!(idle as u64 > crossing_tick(info.exit_time()), "slowed by its own Bend Time");
}

/// The grenade leaves at its deploy's `DishonoredNotify_FireProjectile` and the deploy ends at its
/// `AnimStateAdvance` (the gadget's next state).
#[test]
fn the_grenade_leaves_at_the_deploys_notify() {
    let mut r = Run::new();
    let g = r.cfg.gameplay.clone();
    dishonored_sim::inventory::add_ammo(r.state.player_mut(1).unwrap(), AmmoType::Grenade, 2, &g);
    let throw = |b| PlayerInput { buttons: b, left_hand: LeftHand::Grenade, ..Default::default() };
    r.step(throw(0));
    let press = r.step(throw(buttons::SECONDARY));
    for _ in 0..60 {
        r.step(throw(0));
    }
    let info = &r.cfg.gameplay.sequences["Ply_Gadgets_Grenade_as.Gadgets_Grenade_Deploy"];
    let at = info.notify_time("DishonoredNotify_FireProjectile").unwrap();
    let fired: Vec<u64> = r.events.iter().filter_map(|(t, e)| matches!(e, SimEvent::Fired { id: 1, bolt: false, .. }).then_some(*t)).collect();
    assert_eq!(fired, vec![press + crossing_tick(at)]);
    assert_eq!(r.state.player(1).unwrap().left.phase, LeftPhase::Idle);
}

/// An elite guard's swing plays its attack chain's sequence (`AttackShort`, a group picked from the
/// pawn's own stream) on the full body; the hit lands in the step that opens its damage zone.
#[test]
fn a_guard_swing_is_its_chain_sequence() {
    let mut r = Run::new();
    spawn_npc(&mut r.state, 100, PawnKind::Guard, 1, Vec3::new(120., 0., Z), 180.0, &r.cfg.clone());
    r.state.player_mut(100).unwrap().ai_awareness = awareness::IN_COMBAT;
    let idle = |yaw| PlayerInput { yaw, ..Default::default() };
    for _ in 0..30 {
        r.step_with(&[(1, idle(0.0)), (100, idle(180.0))]);
    }
    let h0 = r.state.player(1).unwrap().vitals.health;
    let press = r.step_with(&[(1, idle(0.0)), (100, PlayerInput { yaw: 180.0, buttons: buttons::PRIMARY, ..Default::default() })]);
    let full = r.state.anim.get(&100).unwrap().layers[layer::FULL_BODY].clone().expect("the guard's swing plays");
    let key = r.table().key(full.seq).unwrap().to_string();
    assert!(key.starts_with("Npc_EliteGuard_Sword_as.Sword_Attack_"), "{key}");
    let info = r.cfg.gameplay.sequences[&key].clone();
    let zone = info.notify("DishonoredNotify_AttackZone").unwrap().clone();
    let damage = zone.time + zone.duration - zone.payload["m_fDamageZoneMaxTime"];
    let mut hit = None;
    for _ in 0..150 {
        let t = r.step_with(&[(1, idle(0.0)), (100, idle(180.0))]);
        if hit.is_none() && r.state.player(1).unwrap().vitals.health < h0 {
            hit = Some(t - press);
        }
    }
    assert_eq!(hit, Some(crossing_tick(damage)), "{key}: damage zone at {damage} s");
}

/// The animation state rides in the world's snapshot block: a world restored mid-swing steps on
/// identically (the same notifies, the same hashes).
#[test]
fn the_animation_state_round_trips_through_the_snapshot() {
    let mut r = Run::new();
    r.step(PlayerInput { buttons: buttons::PRIMARY, ..Default::default() });
    for _ in 0..5 {
        r.step(PlayerInput::default());
    }
    let extras = r.state.extras();
    assert!(!extras.anim.is_empty());
    let json = serde_json::to_string(&r.state).unwrap();
    let mut restored: WorldState = serde_json::from_str(&json).unwrap();
    let mut copy = WorldState { tick: r.state.tick, players: r.state.players.clone(), ..Default::default() };
    copy.set_extras(serde_json::from_str(&serde_json::to_string(&extras).unwrap()).unwrap());
    assert_eq!(restored.anim, r.state.anim);
    assert_eq!(copy.anim, r.state.anim);
    for _ in 0..40 {
        let i = [(1, PlayerInput { tick: r.state.tick, ..Default::default() })];
        let a = step_world(&mut r.state, &i, &r.world, &r.cfg).events;
        let b = step_world(&mut restored, &i, &r.world, &r.cfg).events;
        let c = step_world(&mut copy, &i, &r.world, &r.cfg).events;
        assert_eq!(state_hash(&r.state), state_hash(&restored));
        assert_eq!(state_hash(&r.state), state_hash(&copy));
        let notifies = |e: &[SimEvent]| e.iter().filter(|e| matches!(e, SimEvent::AnimNotify { .. })).count();
        assert_eq!(notifies(&a), notifies(&b));
        assert_eq!(notifies(&a), notifies(&c));
    }
}
