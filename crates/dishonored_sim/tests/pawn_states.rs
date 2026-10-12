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
