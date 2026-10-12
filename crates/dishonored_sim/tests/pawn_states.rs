//! The NPC pawn states (`dishonored_sim::pawn_states`) on the fixture config, without the install:
//! the AI's item contexts and GenericActions request states, the states play their chains on the
//! full body and end with them, and the chains' root motion moves the pawn the way the original's
//! locomotion and pose barrier do (research/dishonored/systems/tick_order.md section 5).

use dishonored_sim::anim::layer;
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::movement::DT;
use dishonored_sim::pawn_states::{Master, Request, context, root_mode};
use dishonored_sim::state::{PawnKind, SwordPhase, WorldState, awareness};
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
    events: Vec<(u64, SimEvent)>,
}

impl Run {
    /// A player at the origin facing +X and a guard `d` uu ahead facing it, settled.
    fn new(d: f32) -> Run {
        let cfg = cfg();
        let mut state = WorldState::default();
        spawn_player(&mut state, 1, Vec3::new(0., 0., Z), 0.0, &cfg);
        spawn_npc(&mut state, GUARD, PawnKind::Guard, 1, Vec3::new(d, 0., Z), 180.0, &cfg);
        state.player_mut(GUARD).unwrap().ai_awareness = awareness::IN_COMBAT;
        let mut r = Run { state, cfg, world: floor(), events: Vec::new() };
        for _ in 0..30 {
            r.step(None);
        }
        r.events.clear();
        r
    }
    fn step(&mut self, req: Option<Request>) -> u64 {
        let t = self.state.tick;
        self.state.requests = req.map(|q| vec![(GUARD, q)]).unwrap_or_default();
        let ins = [(1, PlayerInput { tick: t, ..Default::default() }), (GUARD, PlayerInput { tick: t, yaw: 180.0, ..Default::default() })];
        let out = step_world(&mut self.state, &ins, &self.world, &self.cfg);
        self.events.extend(out.events.into_iter().map(|e| (t, e)));
        t
    }
    fn guard(&self) -> dishonored_sim::state::PlayerState {
        *self.state.player(GUARD).unwrap()
    }
    fn master(&self) -> Master {
        dishonored_sim::pawn_states::master(&self.state, GUARD)
    }
    fn chain_seq(&self) -> Option<String> {
        let l = self.state.anim.get(&GUARD)?.layers[layer::FULL_BODY].as_ref()?;
        self.cfg.anim.key(l.seq).map(str::to_string)
    }
}

/// An elite guard's long attack: the context starts ActionMelee, whose `AttackLong` chain plays on
/// the full body with the chain's `RMM_Accel`; its root translation reaches the pawn as velocity
/// one tick late and carries it about the chain's travel toward its facing (the root motion's own
/// keys at the chain's stop, `m_fBlendOutTime` before the end), the locomotion switched off; the
/// state hands back to Walk on the tick after the stop and the swing is over.
#[test]
fn a_long_attack_lunges_by_its_root_motion() {
    let mut r = Run::new(500.0);
    let x0 = r.guard().position.x;
    let t0 = r.step(Some(Request { context: context::ATTACK_LONG, start: true, ..Default::default() }));
    assert_eq!(r.master(), Master::ActionMelee);
    let key = r.chain_seq().expect("the attack's sequence plays");
    assert!(key.contains("Sword_Attack_Long"), "{key}");
    let s = &r.state.pawn_states[&GUARD];
    assert_eq!((s.rm_mode, s.loco_off), (root_mode::ACCEL, true));
    assert_eq!(r.guard().sword.phase, SwordPhase::Startup);
    // The first tick's root motion moves the pawn on the next.
    assert_eq!(r.guard().position.x, x0);
    let info = r.cfg.gameplay.sequences[&key].clone();
    let rm = r.cfg.gameplay.root_motion[info.root_motion.as_ref().unwrap()].clone();
    let blend_out = 0.2;
    let mut ended = None;
    for _ in 0..240 {
        let t = r.step(None);
        if ended.is_none() && r.master() == Master::Walk {
            ended = Some(t - t0);
        }
    }
    let ended = ended.expect("the attack ends");
    // The stop at `blend_out` before the end, the exit on the next tick.
    let stop = ((info.duration - blend_out) / DT).ceil() as u64;
    assert!((ended as i64 - (stop + 1) as i64).abs() <= 1, "ended {ended} ticks after the start, stop at {stop}");
    let travel = x0 - r.guard().position.x;
    let authored = rm.at(info.duration - blend_out)[0];
    assert!(travel > 0.0 && (travel - authored).abs() < 0.15 * authored, "travel {travel} against the chain's {authored}");
    assert_eq!(r.guard().sword.phase, SwordPhase::Idle);
    assert!(!r.state.pawn_states[&GUARD].loco_off);
}

/// A side step plays `SideStep` in the requested side's group through PlayAnim; its root motion
/// moves the pawn sideways (the right group to the pawn's right), and the state ends with the chain.
#[test]
fn a_side_step_moves_by_its_chain() {
    let mut r = Run::new(300.0);
    let y0 = r.guard().position.y;
    r.step(Some(Request { context: context::SIDE_STEP, start: true, variant: 1, ..Default::default() }));
    assert_eq!(r.master(), Master::PlayAnim);
    assert!(r.chain_seq().unwrap().ends_with("Sword_SideStep_Right"));
    for _ in 0..120 {
        r.step(None);
    }
    assert_eq!(r.master(), Master::Walk);
    // The guard faces -X (yaw 180): its right is -Y.
    let moved = r.guard().position.y - y0;
    assert!(moved < -100.0, "moved {moved} uu along Y");
}

/// A behaviour's GenericAction (the alarm's `TriggerAlarm`) is a PlayAnim of its chain, taken once
/// per request serial; the state returns to Walk when the chain ends.
#[test]
fn a_generic_action_plays_its_chain() {
    let mut r = Run::new(600.0);
    let req = Request { play_anim: Some((35, 1)), ..Default::default() };
    r.step(Some(req));
    assert_eq!(r.master(), Master::PlayAnim);
    assert!(r.chain_seq().unwrap().ends_with("Sword_TriggerAlarm"));
    // The same serial again plays nothing new.
    for _ in 0..100 {
        r.step(Some(req));
    }
    assert_eq!(r.master(), Master::Walk);
    assert_eq!(r.state.pawn_states[&GUARD].play_serial, 1);
}

/// A death: the Dead state, requested after the pawn group, plays the directional `Death` step
/// from the next tick; its `RMM_Translate` moves the body at the barrier until the chain's
/// `DishonoredNotify_Ragdoll`, and no further.
#[test]
fn a_death_plays_its_chain_and_moves_until_the_ragdoll() {
    let mut r = Run::new(600.0);
    let p0 = r.guard().position;
    dishonored_sim::vitals::kill(r.state.player_mut(GUARD).unwrap(), None, dishonored_sim::events::DamageCause::World, &mut Vec::new());
    dishonored_sim::pawn_states::request_death(&mut r.state, GUARD, dishonored_ir::npc_anim::NpcAnim::Death, 0, &r.cfg.clone());
    r.step(None);
    assert_eq!(r.master(), Master::Dead);
    let key = r.chain_seq().unwrap();
    assert!(key.ends_with("Generic_DeathFrontA"), "{key}");
    let ragdoll = r.cfg.gameplay.sequences[&key].notify_time("DishonoredNotify_Ragdoll").unwrap();
    let mut at_ragdoll = None;
    for k in 0..300 {
        r.step(None);
        if at_ragdoll.is_none() && (k as f32 + 2.0) * DT > ragdoll {
            at_ragdoll = Some(r.guard().position);
        }
    }
    let moved = (r.guard().position - p0).truncate().length();
    assert!(moved > 10.0, "the death's root motion moved the body {moved} uu");
    assert!((r.guard().position - at_ragdoll.unwrap()).truncate().length() < 3.0, "no root motion after the ragdoll");
}

/// A thrown NPC at rest gets up through `StateNPCMasterStunned`: the `Stun` chain's group 1
/// (`Generic_RagdollToFront`, cut in), its loop left on the next tick (`m_fStunDuration` 0), the
/// recovery the rest test rolled, then Walk.
#[test]
fn a_thrown_npc_gets_up_through_the_stun_chain() {
    let mut r = Run::new(600.0);
    dishonored_sim::pawn_states::enter_thrown(&mut r.state, GUARD);
    r.state.player_mut(GUARD).unwrap().thrown.getup = 2;
    r.step(None);
    assert_eq!(r.master(), Master::Stunned);
    assert!(r.chain_seq().unwrap().ends_with("Generic_RagdollToFront"));
    let l = r.state.anim[&GUARD].layers[layer::FULL_BODY].as_ref().unwrap();
    assert_eq!(l.blend_in, 0.0);
    let mut seqs = vec![r.chain_seq().unwrap()];
    for _ in 0..400 {
        r.step(None);
        if let Some(k) = r.chain_seq() {
            if seqs.last() != Some(&k) {
                seqs.push(k);
            }
        }
    }
    assert_eq!(r.master(), Master::Walk);
    assert!(seqs.iter().any(|k| k.ends_with("Sword_Stun_Loop")), "{seqs:?}");
    assert!(seqs.last().unwrap().ends_with("Sword_RecoveryB"), "the rolled recovery: {seqs:?}");
}

/// The brain's ambient idle is a full-body PlayAnim of its `AmbientIdle` step, taken once per
/// serial, from how long ago it began; dropping it exits the state.
#[test]
fn an_ambient_idle_plays_through_the_pawn_states() {
    let mut r = Run::new(600.0);
    r.state.player_mut(GUARD).unwrap().ai_awareness = awareness::UNAWARE;
    let req = Request { ambient: Some((0, 3, 1, 0.1)), ..Default::default() };
    r.step(Some(req));
    assert_eq!(r.master(), Master::PlayAnim);
    assert!(r.chain_seq().unwrap().ends_with("Empty_Idle_04"));
    let l = r.state.anim[&GUARD].layers[layer::FULL_BODY].as_ref().unwrap();
    assert!((l.time - (0.1 + DT)).abs() < 2e-3, "from 0.1 s in: {}", l.time);
    r.step(Some(req));
    assert_eq!(r.state.pawn_states[&GUARD].ambient_serial, 1);
    r.step(None);
    r.step(None);
    assert_eq!(r.master(), Master::Walk);
}
/// A choke: the victim's `StateNPCMasterBeingChoked` plays the `Choke` group of the choker's state,
/// In first, the Loop after the victim's own In (0x00A74720), the Win at the won choke; the Win's
/// end puts the body down.
#[test]
fn a_choked_guard_plays_the_choke_slaves() {
    let mut r = Run::new(60.0);
    r.state.player_mut(GUARD).unwrap().ai_awareness = awareness::UNAWARE;
    let (ai, vi) = (r.state.players.iter().position(|p| p.id == 1).unwrap(), r.state.players.iter().position(|p| p.id == GUARD).unwrap());
    let g = r.cfg.gameplay.clone();
    dishonored_sim::takedown::start_choke(&mut r.state, ai, vi, &g, &mut Vec::new());
    // The choke button held through the choke.
    let hold = |r: &mut Run| {
        let t = r.state.tick;
        r.state.requests.clear();
        let ins = [(1, PlayerInput { tick: t, buttons: dishonored_sim::input::buttons::BLOCK, ..Default::default() }), (GUARD, PlayerInput { tick: t, yaw: 180.0, ..Default::default() })];
        step_world(&mut r.state, &ins, &r.world, &r.cfg);
    };
    hold(&mut r);
    assert_eq!(r.master(), Master::BeingChoked);
    assert!(r.chain_seq().unwrap().ends_with("Generic_Choke_In_Slave"), "{:?}", r.chain_seq());
    let mut seen = vec![r.chain_seq().unwrap()];
    for _ in 0..900 {
        hold(&mut r);
        if let Some(k) = r.chain_seq() {
            if seen.last() != Some(&k) {
                seen.push(k);
            }
        }
    }
    assert!(seen.iter().any(|k| k.ends_with("Generic_Choke_Loop_Slave")), "{seen:?}");
    assert!(seen.iter().any(|k| k.ends_with("Generic_Choke_Win_Slave")), "{seen:?}");
    assert_eq!(r.master(), Master::Dead);
    assert!(r.state.pawn_states[&GUARD].ragdoll);
}
/// One step with the player's and the guard's own inputs (the requests empty).
fn step_inputs(r: &mut Run, player: PlayerInput, guard_buttons: u16) -> u64 {
    let t = r.state.tick;
    r.state.requests.clear();
    let ins = [(1, PlayerInput { tick: t, ..player }), (GUARD, PlayerInput { tick: t, yaw: 180.0, buttons: guard_buttons, ..Default::default() })];
    let out = step_world(&mut r.state, &ins, &r.world, &r.cfg);
    r.events.extend(out.events.into_iter().map(|e| (t, e)));
    t
}

/// The player's bullet knocks a guard down (`Twk_Proj_Bullet.m_pDamageType` =
/// `DishonoredDamageType_Bullet`, `eDisPawnHitReactionType_Knockdown`, slot 423): a
/// `StateNPCMasterThrown` request taken at the guard's next machine tick, whose enter plays
/// `PushToRagdoll` group 0 at the shot's side (`Generic_PushToRagdoll_Front` from the front) and
/// flies the guard away from the shooter at the minimum momentum over the throw mass (800 / 2), so
/// that it moves from the tick after; its ragdoll notify (0.296 s) ends the flight on the 18th
/// update, as the capture of `combat/pistol_hit` shows (hit tick 8, `PHYS_Flying` from tick 9,
/// 6.6 uu a tick from tick 10, `PHYS_RigidBody` at tick 26); at rest it gets up through the stun
/// chain.
#[test]
fn a_pistol_hit_knocks_a_guard_down() {
    use dishonored_sim::input::{LeftHand, buttons};
    let mut r = Run::new(600.0);
    r.state.player_mut(GUARD).unwrap().vitals.health = 500.0;
    let gun = |b| PlayerInput { yaw: 0.0, pitch: -5.0, buttons: b, left_hand: LeftHand::Pistol, ..Default::default() };
    for _ in 0..40 {
        step_inputs(&mut r, gun(0), 0);
    }
    r.events.clear();
    let mut hit = None;
    for k in 0..10 {
        let t = step_inputs(&mut r, gun(if k == 0 { buttons::SECONDARY } else { 0 }), 0);
        if r.events.iter().any(|(_, e)| matches!(e, SimEvent::Damage { victim: GUARD, .. })) {
            hit = Some(t);
            break;
        }
    }
    let hit = hit.expect("the shot hits the guard");
    assert!(r.guard().alive);
    assert_eq!(r.master(), Master::Walk, "the request waits for the next machine tick");
    let x0 = r.guard().position.x;
    step_inputs(&mut r, gun(0), 0);
    assert_eq!(r.master(), Master::Thrown);
    assert!(r.chain_seq().unwrap().ends_with("Generic_PushToRagdoll_Front"), "{:?}", r.chain_seq());
    let g = r.guard();
    assert!(g.thrown.active && g.thrown.by_notify);
    assert!((g.velocity.x - 400.0).abs() < 1.0 && g.velocity.y.abs() < 40.0, "flies away from the shooter at 800 / 2: {:?}", g.velocity);
    assert_eq!(g.position.x, x0, "it moves from the next tick");
    let mut ragdoll = None;
    let mut x1 = None;
    for k in 0..40 {
        step_inputs(&mut r, gun(0), 0);
        if k == 0 {
            x1 = Some(r.guard().position.x);
        }
        if ragdoll.is_none() && r.guard().thrown.ragdoll {
            ragdoll = Some(r.state.tick - 1 - hit);
        }
    }
    let step1 = x1.unwrap() - x0;
    assert!((step1 - 400.0 * 0.995 * DT).abs() < 0.2, "the first flight step {step1}");
    assert_eq!(ragdoll, Some(18), "the ragdoll notify on the 18th update");
    let mut seen = Vec::new();
    for _ in 0..900 {
        step_inputs(&mut r, gun(0), 0);
        seen.push(r.master());
    }
    assert!(seen.contains(&Master::Stunned), "the get-up");
    assert_eq!(r.master(), Master::Walk);
}

/// `m_ThrowDirection` of a knock-down (0x00BBE6D0): the cardinal sector the push came from, far
/// along its horizontal direction; without a momentum, the hit location's own sector.
#[test]
fn a_knockdown_falls_away_from_the_side_it_was_hit_on() {
    let r = Run::new(600.0);
    let g = r.guard();
    let fwd = dishonored_sim::movement::forward_vector(g.yaw);
    let right = dishonored_sim::movement::right_vector(g.yaw);
    let dir = |loc: Vec3, m: Vec3| dishonored_sim::pawn_states::knockdown_direction(&g, loc, m);
    assert_eq!(dir(g.position + fwd * 30.0, -fwd * 500.0), 0, "shot from the front");
    assert_eq!(dir(g.position - fwd * 30.0, fwd * 500.0), 3, "shot from behind");
    assert_eq!(dir(g.position + right * 30.0, -right * 500.0), 2, "from the right");
    assert_eq!(dir(g.position - right * 30.0, Vec3::ZERO), 1, "no momentum: the hit's side");
}

/// An NPC's block is its parry context awaiting the blow (`DisItemContext_ParryNPC`,
/// `eDisWeaponMeleeState_AwaitingParry`): `Parry_AwaitingImpact` (`Sword_Ready_ParryIdle`) plays
/// while it holds; the player's swing meets it as a clash (PostParry_Neutral): the guard plays its
/// `Parry` group 0 step, takes no damage, the player is not staggered and no parry is reported;
/// letting go of the block ends the context.
#[test]
fn a_blocking_guard_awaits_the_blow_and_parries_it_neutral() {
    use dishonored_sim::input::buttons;
    let mut r = Run::new(110.0);
    let hold = |r: &mut Run, b: u16, g: u16| step_inputs(r, PlayerInput { buttons: b, ..Default::default() }, g);
    hold(&mut r, 0, buttons::BLOCK);
    hold(&mut r, 0, buttons::BLOCK);
    assert_eq!(r.guard().sword.phase, SwordPhase::Blocking);
    assert_eq!(r.master(), Master::ActionParry);
    assert!(r.chain_seq().unwrap().ends_with("Sword_Ready_ParryIdle"), "{:?}", r.chain_seq());
    let h0 = r.guard().vitals.health;
    r.events.clear();
    hold(&mut r, buttons::PRIMARY, buttons::BLOCK);
    let mut seqs = Vec::new();
    for _ in 0..40 {
        hold(&mut r, 0, buttons::BLOCK);
        if let Some(k) = r.chain_seq() {
            if seqs.last() != Some(&k) {
                seqs.push(k);
            }
        }
    }
    assert!(r.events.iter().any(|(_, e)| matches!(e, SimEvent::SwordBlocked { defender: GUARD, .. })), "the clash");
    assert!(!r.events.iter().any(|(_, e)| matches!(e, SimEvent::SwordParried { .. })), "no parry window on an NPC");
    assert_eq!(r.guard().vitals.health, h0);
    assert_ne!(r.state.player(1).unwrap().sword.phase, SwordPhase::Staggered);
    assert!(seqs.iter().any(|k| k.contains("Sword_Ready_Parry_")), "the neutral parry: {seqs:?}");
    for _ in 0..80 {
        hold(&mut r, 0, 0);
    }
    assert_eq!(r.master(), Master::Walk);
}
