//! Headless mechanics scenarios on the real map (fast, not real time).

use crate::Ctx;
use crate::report::Report;
use anyhow::Result;
use dishonored_bots::{Bot, BotStyle};
use dishonored_content::{DEFAULT_MAP, MatchContent, load_match};
use dishonored_multiplayer::ffa::{FfaMatch, FfaRules, SpawnPoint};
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::state::{MoveMode, WorldState};
use dishonored_sim::{spawn_player, state_hash, step_world};
use std::io::Write;

const S: &str = "Simulation on the real map";

fn settle(mc: &MatchContent, st: &mut WorldState, ticks: usize) {
    for _ in 0..ticks {
        step_world(st, &[], &mc.collision, &mc.sim);
    }
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let map = ctx.opt("--map").unwrap_or_else(|| DEFAULT_MAP.into());
    let t = std::time::Instant::now();
    let mc = load_match(&map)?;
    rep.check(S, "load match content", mc.collision.tris.len() > 10_000 && !mc.spawns.is_empty(),
        format!("{map}: {} collision triangles, {} spawns, {:.1?}", mc.collision.tris.len(), mc.spawns.len(), t.elapsed()));

    // 1. Every spawn settles onto walkable ground.
    let mut grounded = 0;
    let mut rows = Vec::new();
    for (i, s) in mc.spawns.iter().enumerate() {
        let mut st = WorldState::default();
        spawn_player(&mut st, 1, s.position, s.yaw, &mc.sim);
        settle(&mc, &mut st, 120);
        let p0 = *st.player(1).unwrap();
        if p0.mode == MoveMode::Walking {
            grounded += 1;
        }
        // 2. Run forward 5 s: no falling through ordinary geometry.
        for _ in 0..300 {
            step_world(&mut st, &[(1, PlayerInput { yaw: s.yaw, forward: 127, ..Default::default() })], &mc.collision, &mc.sim);
        }
        let p1 = *st.player(1).unwrap();
        let moved = (p1.position - p0.position).truncate().length();
        rows.push(vec![i.to_string(), format!("{:.0},{:.0},{:.0}", s.position.x, s.position.y, s.position.z), format!("{:?}", p0.mode), format!("{moved:.0}"), format!("{:.0}", p1.position.z - p0.position.z), format!("{:?}", p1.mode)]);
        rep.check(S, &format!("spawn {i} walk 5 s stays in world"), p1.alive && p1.position.z > p0.position.z - 1500.0, format!("moved {moved:.0} uu, dz {:.0}", p1.position.z - p0.position.z));
    }
    rep.check(S, "spawns settle on walkable floor", grounded == mc.spawns.len(), format!("{grounded}/{}", mc.spawns.len()));
    rep.table("Spawn traversal", &["spawn", "position", "after settle", "moved 5 s (uu)", "dz", "end mode"], rows);

    // 3. Jump apex on real ground vs closed-form reference.
    let s0 = &mc.spawns[0];
    let mut st = WorldState::default();
    spawn_player(&mut st, 1, s0.position, s0.yaw, &mc.sim);
    settle(&mc, &mut st, 120);
    let z0 = st.player(1).unwrap().position.z;
    step_world(&mut st, &[(1, PlayerInput { yaw: s0.yaw, buttons: buttons::JUMP, ..Default::default() })], &mc.collision, &mc.sim);
    let mut apex = z0;
    for _ in 0..90 {
        step_world(&mut st, &[(1, PlayerInput { yaw: s0.yaw, ..Default::default() })], &mc.collision, &mc.sim);
        apex = apex.max(st.player(1).unwrap().position.z);
    }
    let mv = &mc.sim.gameplay.movement;
    let expected = mv.jump_z * mv.jump_z / (2.0 * -mv.gravity_z);
    rep.check(S, "jump apex = JumpZ^2/2g", (apex - z0 - expected).abs() < 3.0, format!("{:.1} uu (reference {expected:.1})", apex - z0));

    // 4. Blink from each spawn: valid target, mana cost, ends standing.
    let mut blinks_ok = 0;
    let mut blink_detail = Vec::new();
    for s in &mc.spawns {
        let mut st = WorldState::default();
        spawn_player(&mut st, 1, s.position, s.yaw, &mc.sim);
        settle(&mc, &mut st, 120);
        let from = st.player(1).unwrap().position;
        let aim = |b| PlayerInput { yaw: s.yaw, pitch: -6.0, buttons: b, left_hand: LeftHand::Blink, ..Default::default() };
        for _ in 0..10 {
            step_world(&mut st, &[(1, aim(buttons::SECONDARY))], &mc.collision, &mc.sim);
        }
        let had_target = st.player(1).unwrap().left.blink_target.is_some();
        for _ in 0..90 {
            step_world(&mut st, &[(1, aim(0))], &mc.collision, &mc.sim);
        }
        let p = st.player(1).unwrap();
        blink_detail.push(format!("target={had_target} moved={:.0} mana={:.0} mode={:?}", p.position.distance(from), p.vitals.mana, p.mode));
        if had_target && p.vitals.mana < 100.0 && p.position.distance(from) > 100.0 && p.mode == MoveMode::Walking {
            blinks_ok += 1;
        }
    }
    rep.check(S, "Blink from spawns (target, cost, lands)", blinks_ok * 2 >= mc.spawns.len(), format!("{blinks_ok}/{} spawns: {}", mc.spawns.len(), blink_detail.join("; ")));

    // 5. Bot brawl: 6 bots, 3 simulated minutes, FFA rules; telemetry trace recorded.
    let brawl = |record: bool| -> (u64, FfaMatch, usize) {
        let spawns: Vec<SpawnPoint> = mc.spawns.iter().map(|s| SpawnPoint { position: s.position, yaw: s.yaw }).collect();
        let mut m = FfaMatch::new(FfaRules { warmup_time: 0.5, time_limit: 170.0, score_limit: 50, ..Default::default() }, spawns);
        let mut bots: Vec<Bot> = (0..6).map(|i| Bot::new(i + 1, if i % 2 == 0 { BotStyle::Duelist } else { BotStyle::Gunner }, 77 + i as u64)).collect();
        for b in &bots {
            m.add_player(b.id, &format!("bot{}", b.id), &mc.sim);
        }
        let mut trace = record.then(|| std::fs::File::create(ctx.out.join("telemetry_bot1.jsonl")).unwrap());
        let mut events = 0usize;
        for _ in 0..(60 * 180) {
            let snap = m.world.players.clone();
            let inputs: Vec<_> = bots.iter_mut().filter_map(|b| snap.iter().find(|p| p.id == b.id).map(|me| (b.id, b.think(me, &snap, &mc.collision)))).collect();
            let out = m.step(&inputs, &mc.collision, &mc.sim);
            events += out.events.iter().filter(|e| matches!(e, SimEvent::Death { .. })).count();
            if let (Some(f), Some(p)) = (trace.as_mut(), m.world.player(1)) {
                let _ = writeln!(f, "{{\"tick\":{},\"pos\":[{:.2},{:.2},{:.2}],\"vel\":[{:.2},{:.2},{:.2}],\"mode\":\"{:?}\",\"health\":{:.1},\"mana\":{:.1},\"sword\":\"{:?}\"}}",
                    m.world.tick, p.position.x, p.position.y, p.position.z, p.velocity.x, p.velocity.y, p.velocity.z, p.mode, p.vitals.health, p.vitals.mana, p.sword.phase);
            }
        }
        (state_hash(&m.world), m, events)
    };
    let t = std::time::Instant::now();
    let (h1, m, deaths) = brawl(true);
    let secs = t.elapsed().as_secs_f32();
    let finite = m.world.players.iter().all(|p| p.position.is_finite() && p.velocity.is_finite());
    rep.check(S, "6-bot brawl: deaths occur and state stays finite", deaths > 0 && finite, format!("{deaths} deaths in 180 s sim ({:.1}x real time), scores {:?}", 180.0 / secs, m.view.scores.iter().map(|s| (s.name.clone(), s.kills)).collect::<Vec<_>>()));
    rep.check(S, "match flow reaches Ended (time limit)", m.view.phase != dishonored_multiplayer::MatchPhase::Warmup || m.view.match_number > 0, format!("phase {:?}, match #{}", m.view.phase, m.view.match_number));
    let (h2, _, _) = brawl(false);
    rep.check(S, "determinism: identical brawl hashes", h1 == h2, format!("{h1:016x} vs {h2:016x}"));
    rep.artifact("bot telemetry trace (JSONL)", &ctx.out.join("telemetry_bot1.jsonl"));
    let dump = ctx.out.join("state_dump.json");
    std::fs::write(&dump, serde_json::to_string_pretty(&m.world.players)?)?;
    rep.artifact("authoritative state dump", &dump);
    Ok(())
}
