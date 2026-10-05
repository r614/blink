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
    items(&mc, rep);
    Ok(())
}

/// Inventory, takedowns, powers and gadgets on the real map (research/dishonored/INVENTORY.md).
fn items(mc: &MatchContent, rep: &mut Report) {
    use dishonored_ir::items::{AmmoType, ElixirType, PickupKind};
    use glam::Vec3;
    use dishonored_sim::input::actions;
    use dishonored_sim::state::{Condition, PawnKind, PossessStage, power};
    const T: &str = "Inventory, takedowns, powers, gadgets";
    let cfg = &mc.sim;
    // Spawn 1: open ground in front (it walks 1700+ uu in 5 s).
    let s0 = &mc.spawns[1.min(mc.spawns.len() - 1)];
    let fwd = |yaw: f32| Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
    // A fresh world: player 1 at spawn 0, NPC guards at offsets in front (distance, facing offset).
    let setup = |npcs: &[(f32, f32)]| -> WorldState {
        let mut st = WorldState::default();
        spawn_player(&mut st, 1, s0.position, s0.yaw, cfg);
        for (k, (d, face)) in npcs.iter().enumerate() {
            dishonored_sim::spawn_npc(&mut st, 100 + k as u32, PawnKind::Guard, 1, s0.position + fwd(s0.yaw) * *d, s0.yaw + *face, cfg);
        }
        settle(mc, &mut st, 120);
        st
    };
    let step = |st: &mut WorldState, ins: &[(u32, PlayerInput)], ev: &mut Vec<SimEvent>| {
        let mut all = ins.to_vec();
        for p in &st.players {
            if p.kind != PawnKind::Human && !all.iter().any(|(id, _)| *id == p.id) {
                all.push((p.id, PlayerInput { yaw: p.yaw, ..Default::default() }));
            }
        }
        ev.extend(step_world(st, &all, &mc.collision, cfg).events);
    };
    let me = |b: u16, lh: LeftHand| PlayerInput { yaw: s0.yaw, buttons: b, left_hand: lh, ..Default::default() };

    // 1. Pick up the map's first placed health elixir (walk up to it), then drink it.
    let mut st = setup(&[]);
    let mut ev = Vec::new();
    let placed: Vec<_> = mc.pickups.iter().filter(|p| matches!(p.kind, PickupKind::Elixir(ElixirType::Health))).cloned().collect();
    let ok_pick = if let Some(pp) = placed.first() {
        dishonored_sim::inventory::spawn_pickups(&mut st, std::slice::from_ref(pp), 1, &|_| None);
        // Stand 80 uu from it on its floor (first free side of eight) and look at it.
        let base = st.clone();
        let (mut yaw, mut pitch) = (0.0, 0.0);
        for k in 0..8 {
            st = base.clone();
            let to = fwd(k as f32 * 45.0);
            if let Some(p) = st.player_mut(1) {
                p.position = pp.position - to * 80.0 + Vec3::Z * 60.0;
                p.velocity = Vec3::ZERO;
            }
            settle(mc, &mut st, 60);
            let mut p = *st.player(1).unwrap();
            let eye = dishonored_sim::movement::eye_position(&p, &cfg.gameplay.movement);
            let d = pp.position - eye;
            yaw = d.y.atan2(d.x).to_degrees();
            pitch = (d.z / d.truncate().length().max(1.0)).atan().to_degrees();
            p.yaw = yaw;
            p.pitch = pitch;
            if p.mode == dishonored_sim::state::MoveMode::Walking && dishonored_sim::inventory::pickup_in_view(&st, &p, &mc.collision, &cfg.gameplay).is_some() {
                break;
            }
        }
        step(&mut st, &[(1, PlayerInput { yaw, pitch, buttons: buttons::USE, ..Default::default() })], &mut ev);
        let got = st.player(1).unwrap().inv.elixirs[0];
        st.player_mut(1).unwrap().vitals.health = 15.0;
        step(&mut st, &[(1, PlayerInput { yaw, pitch, action: actions::DRINK_HEALTH, ..Default::default() })], &mut ev);
        let h = st.player(1).unwrap().vitals.health;
        rep.check(T, "pick up a placed health elixir and drink it", got == 1 && h == 55.0, format!("{} ({}) at {:.0},{:.0},{:.0}: elixirs {got}, health 15 -> {h}", pp.name, pp.package, pp.position.x, pp.position.y, pp.position.z));
        got == 1
    } else {
        rep.check(T, "pick up a placed health elixir and drink it", false, "no placed health elixir in the map".to_string());
        false
    };
    let counts = {
        let mut c = std::collections::BTreeMap::new();
        for p in &mc.pickups {
            let k = format!("{:?}", p.kind);
            *c.entry(k.split([' ', '(', '{']).next().unwrap_or("").to_string()).or_insert(0) += 1;
        }
        c
    };
    rep.check(T, "placed pickups extracted", !mc.pickups.is_empty(), format!("{} pickups {:?}, {} carried by NPCs (pickpocketable)", mc.pickups.len(), counts, mc.pickups.iter().filter(|p| p.stealable_from.is_some()).count()));
    let _ = ok_pick;

    // 2. Choke a guard from behind to a knock-out, carry the body, drop it.
    let mut st = setup(&[(110.0, 0.0)]);
    let mut ev = Vec::new();
    let hold = me(buttons::USE, LeftHand::Blink);
    let t0 = st.tick;
    let mut ko_t = None;
    for _ in 0..(8 * 60) {
        step(&mut st, &[(1, hold)], &mut ev);
        if ko_t.is_none() && ev.iter().any(|e| matches!(e, SimEvent::KnockedOut { victim: 100, .. })) {
            ko_t = Some((st.tick - t0) as f32 / 60.0);
        }
        if st.player(1).unwrap().carrying.is_some() {
            break;
        }
    }
    let ko_tick = ev.iter().position(|e| matches!(e, SimEvent::KnockedOut { victim: 100, .. }));
    let carried = st.player(1).unwrap().carrying == Some(100);
    let start = st.player(1).unwrap().position;
    for _ in 0..120 {
        step(&mut st, &[(1, me(0, LeftHand::Blink))], &mut ev);
    }
    let mut speed: f32 = 0.0;
    for _ in 0..120 {
        step(&mut st, &[(1, PlayerInput { forward: 127, ..me(0, LeftHand::Blink) })], &mut ev);
        speed = speed.max(st.player(1).unwrap().velocity.truncate().length());
    }
    let moved = st.player(1).unwrap().position.distance(start);
    step(&mut st, &[(1, me(buttons::USE, LeftHand::Blink))], &mut ev);
    for _ in 0..120 {
        step(&mut st, &[(1, me(0, LeftHand::Blink))], &mut ev);
    }
    let body = *st.player(100).unwrap();
    rep.check(
        T,
        "choke to knock-out, carry and drop the body",
        ko_tick.is_some() && carried && body.carried_by.is_none() && body.alive && body.condition == Condition::Unconscious && (speed - 350.0).abs() < 5.0,
        format!("knocked out after {:.2} s, carried {carried}, carry speed {speed:.0} uu/s, moved {moved:.0} uu, dropped at z {:.0}, alive {} {:?}", ko_t.unwrap_or(0.0), body.position.z, body.alive, body.condition),
    );

    // 3. Possess a guard and release it.
    let mut st = setup(&[(500.0, 180.0)]);
    let mut ev = Vec::new();
    st.player_mut(1).unwrap().inv.power_levels[power::POSSESSION] = 2;
    for _ in 0..5 {
        step(&mut st, &[(1, me(buttons::SECONDARY, LeftHand::Possession))], &mut ev);
    }
    for _ in 0..90 {
        step(&mut st, &[(1, me(0, LeftHand::Possession))], &mut ev);
    }
    let active = st.player(1).unwrap().possess.stage == PossessStage::Active && st.player(100).unwrap().condition == Condition::Possessed;
    let left = st.player(1).unwrap().possess.timer;
    step(&mut st, &[(1, me(buttons::SECONDARY, LeftHand::Possession))], &mut ev);
    for _ in 0..10 {
        step(&mut st, &[(1, me(0, LeftHand::Possession))], &mut ev);
    }
    let ended = ev.iter().any(|e| matches!(e, SimEvent::PossessEnd { id: 1, host: 100 })) && st.player(100).unwrap().condition == Condition::Normal;
    rep.check(T, "possess a guard, then release it", active && ended, format!("active {active} ({left:.1} s of 15 left), released {ended}, mana {:.0}", st.player(1).unwrap().vitals.mana));

    // 4. Stop time (Bend Time level 2): a walking guard does not move.
    let mut st = setup(&[(400.0, 90.0)]);
    let mut ev = Vec::new();
    st.player_mut(1).unwrap().inv.power_levels[power::BEND_TIME] = 2;
    step(&mut st, &[(1, me(buttons::SECONDARY, LeftHand::BendTime))], &mut ev);
    for _ in 0..30 {
        step(&mut st, &[(1, me(0, LeftHand::BendTime))], &mut ev);
    }
    let p0 = st.player(100).unwrap().position;
    let walk = PlayerInput { forward: 127, yaw: s0.yaw + 90.0, ..Default::default() };
    for _ in 0..180 {
        step(&mut st, &[(1, me(0, LeftHand::BendTime)), (100, walk)], &mut ev);
    }
    let frozen = st.player(100).unwrap().position == p0;
    for _ in 0..(7 * 60) {
        step(&mut st, &[(1, me(0, LeftHand::BendTime)), (100, walk)], &mut ev);
    }
    let resumed = st.player(100).unwrap().position.distance(p0) > 50.0;
    rep.check(T, "stop time: the world freezes for 8 s", frozen && resumed, format!("frozen {frozen}, moving again after the end {resumed}"));

    // 5. Spring Razor: placed in front, a guard walks into it.
    let mut st = setup(&[(700.0, 180.0)]);
    let mut ev = Vec::new();
    st.player_mut(1).unwrap().inv.ammo[AmmoType::SpringRazor as usize] = 1;
    let place = |b| PlayerInput { pitch: -40.0, ..me(b, LeftHand::SpringRazor) };
    step(&mut st, &[(1, place(buttons::SECONDARY))], &mut ev);
    for _ in 0..40 {
        step(&mut st, &[(1, place(0))], &mut ev);
    }
    let trap = st.traps.first().map(|t| t.position);
    for _ in 0..90 {
        step(&mut st, &[(1, PlayerInput { forward: -127, ..place(0) })], &mut ev);
    }
    let walk = PlayerInput { forward: 127, yaw: s0.yaw + 180.0, ..Default::default() };
    let mut closest = f32::MAX;
    for _ in 0..(6 * 60) {
        step(&mut st, &[(1, place(0)), (100, walk)], &mut ev);
        if let Some(t) = trap {
            closest = closest.min(st.player(100).unwrap().position.distance(t));
        }
        if !st.player(100).unwrap().alive {
            break;
        }
    }
    let killed = ev.iter().any(|e| matches!(e, SimEvent::Death { victim: 100, cause: dishonored_sim::events::DamageCause::SpringRazor, .. }));
    rep.check(T, "set off a Spring Razor", killed, format!("trap at {:?}, guard came within {closest:.0} uu, killed by it {killed}", trap.map(|t| (t.x.round(), t.y.round(), t.z.round()))));
}
