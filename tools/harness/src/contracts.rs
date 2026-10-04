//! Phase II verification: Contracts on the real map with the crowd, headless and over the
//! network (dedicated server process + bot clients).

use crate::Ctx;
use crate::netsoak::{KillOnDrop, PROFILES, free_port, spawn_server};
use crate::report::Report;
use anyhow::Result;
use dishonored_ai::BrainState;
use dishonored_bots::{Bot, BotStyle, NavGraph};
use dishonored_content::{DEFAULT_MAP, MatchContent, load_match};
use dishonored_multiplayer::MatchPhase;
use dishonored_multiplayer::contracts::{ContractsRules, PursuitAwareness, validate_cycle};
use dishonored_multiplayer::game::{CrowdSource, Game, GameSetup, crowd_center, default_crowd};
use dishonored_net::client::{ConnState, NetClient};
use dishonored_net::socket::{LinkConditions, NetSocket};
use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::state::PawnKind;
use dishonored_sim::state_hash;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const S: &str = "Phase II: Contracts (social stealth)";

fn setup(mc: &MatchContent, time_limit: f32) -> GameSetup {
    let pts: Vec<(glam::Vec3, f32)> = mc.spawns.iter().map(|s| (s.position, s.yaw)).collect();
    let rules = ContractsRules { warmup_time: 2.0, time_limit, crowd: Some(default_crowd(crowd_center(&pts))), ..Default::default() };
    GameSetup::contracts(rules, pts, Some(CrowdSource { points: &mc.ai_points, guard_spawns: &mc.guard_spawns }), &mc.collision)
}

struct Stats {
    hash: u64,
    contract_kills: usize,
    by_cause: BTreeMap<String, (usize, i32)>,
    npc_deaths: usize,
    pursuits: usize,
    guard_combat_ticks: usize,
    civilian_flee_ticks: usize,
    cycle_errors: Vec<String>,
    ended: bool,
    winner: Option<u32>,
    npcs: usize,
    final_scores: Vec<(String, i32)>,
}

fn run_headless(mc: &MatchContent, minutes: f32, seed: u64) -> Stats {
    let mut g = setup(mc, minutes * 60.0 - 20.0).build(&mc.sim);
    let nav = Arc::new(NavGraph::build(&mc.collision, mc.ai_points.iter().map(|p| p.0).chain(mc.spawns.iter().map(|s| s.position))));
    if std::env::var_os("CONTRACTS_DEBUG").is_some() {
        eprintln!("nav graph: {} nodes, {} edges", nav.nodes.len(), nav.edge_count());
    }
    let mut bots: Vec<Bot> = (1..=8).map(|i| Bot::new(i, BotStyle::Duelist, seed + i as u64).with_nav(nav.clone())).collect();
    for b in &bots {
        g.add_player(b.id, &format!("bot{}", b.id), [0u8, 2, 3][b.id as usize % 3], &mc.collision, &mc.sim);
    }
    let npcs = g.world().players.iter().filter(|p| p.kind != PawnKind::Human).count();
    let mut st = Stats { hash: 0, contract_kills: 0, by_cause: BTreeMap::new(), npc_deaths: 0, pursuits: 0, guard_combat_ticks: 0, civilian_flee_ticks: 0, cycle_errors: Vec::new(), ended: false, winner: None, npcs, final_scores: Vec::new() };
    let mut prev_kills = 0;
    let mut prev_aw: BTreeMap<u32, PursuitAwareness> = BTreeMap::new();
    for tick in 0..((minutes * 60.0 * 60.0) as usize) {
        let snap = g.world().players.clone();
        let inputs: Vec<_> = bots.iter_mut().filter_map(|b| {
            let me = snap.iter().find(|p| p.id == b.id)?;
            let t = g.target_info(b.id).and_then(|t| t.target);
            Some((b.id, b.think_contract(me, &snap, &mc.collision, t)))
        }).collect();
        if std::env::var_os("CONTRACTS_DEBUG").is_some() && tick % 600 == 0 {
            for b in &bots {
                let Some(me) = snap.iter().find(|p| p.id == b.id) else { continue };
                let t = g.target_info(b.id).and_then(|t| t.target);
                let d = t.and_then(|t| snap.iter().find(|p| p.id == t)).map(|o| o.position.distance(me.position));
                eprintln!("t={:.0}s bot{} alive {} pos {:.0?} mode {:?} target {t:?} dist {d:.0?}", tick as f32 / 60.0, b.id, me.alive, me.position, me.mode);
            }
        }
        let before: BTreeMap<u32, i32> = g.view().scores.iter().map(|s| (s.id, s.score)).collect();
        let out = g.step(&inputs, &mc.collision, &mc.sim);
        if let Game::Contracts(m) = &g {
            if m.view.phase == MatchPhase::Playing {
                if let Err(e) = validate_cycle(&m.contracts) {
                    if st.cycle_errors.len() < 5 {
                        st.cycle_errors.push(format!("tick {tick}: {e}"));
                    }
                }
            }
            for c in &m.contracts {
                let was = prev_aw.insert(c.id, c.awareness).unwrap_or_default();
                if c.awareness == PursuitAwareness::Pursuit && was != PursuitAwareness::Pursuit {
                    st.pursuits += 1;
                }
            }
            if let Some(cr) = &m.crowd {
                st.guard_combat_ticks += cr.brains.iter().filter(|b| b.kind == PawnKind::Guard && b.state == BrainState::Combat).count();
                st.civilian_flee_ticks += cr.brains.iter().filter(|b| b.kind == PawnKind::Civilian && b.state == BrainState::Flee).count();
            }
            if m.view.phase == MatchPhase::Ended && !st.ended {
                st.ended = true;
                st.winner = m.view.winner;
                st.final_scores = m.view.scores.iter().map(|s| (s.name.clone(), s.score)).collect();
            }
        }
        let kills: i32 = g.view().scores.iter().map(|s| s.kills).sum();
        for e in &out.events {
            if let SimEvent::Death { victim, killer: Some(k), cause } = e {
                let vk = g.world().player(*victim).map(|p| p.kind);
                if vk != Some(PawnKind::Human) {
                    st.npc_deaths += 1;
                } else if kills > prev_kills {
                    let gained = g.view().scores.iter().find(|s| s.id == *k).map(|s| s.score).unwrap_or(0) - before.get(k).copied().unwrap_or(0);
                    let key = match cause {
                        DamageCause::Assassination | DamageCause::DropAssassination => "assassination",
                        DamageCause::Pistol | DamageCause::PistolHeadshot => "pistol",
                        DamageCause::Crossbow | DamageCause::CrossbowHeadshot => "crossbow",
                        _ => "sword",
                    };
                    let ent = st.by_cause.entry(key.into()).or_default();
                    ent.0 += 1;
                    ent.1 += gained;
                }
            }
        }
        if kills > prev_kills {
            st.contract_kills += (kills - prev_kills) as usize;
        }
        prev_kills = kills;
    }
    st.hash = state_hash(g.world());
    st
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let map = ctx.opt("--map").unwrap_or_else(|| DEFAULT_MAP.into());
    let mc = Arc::new(load_match(&map)?);
    let minutes: f32 = ctx.opt("--minutes").and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let t = Instant::now();
    let st = run_headless(&mc, minutes, 9);
    let secs = t.elapsed().as_secs_f32();
    rep.check(S, "S0 crowd populated from original placement data", st.npcs >= 20, format!("{} NPCs (civilians + guards) on {map}", st.npcs));
    rep.check(S, "S1 contract cycle valid every tick (8 players)", st.cycle_errors.is_empty(), if st.cycle_errors.is_empty() { format!("{:.0} min simulated ({:.1}x real time)", minutes, minutes * 60.0 / secs) } else { st.cycle_errors.join("; ") });
    rep.check(S, "S1/S10 contracts resolve (kills, reassignment)", st.contract_kills >= 5, format!("{} contract kills", st.contract_kills));
    let mut rows = Vec::new();
    for (k, (n, pts)) in &st.by_cause {
        rows.push(vec![k.clone(), n.to_string(), format!("{:.0}", *pts as f32 / (*n).max(1) as f32)]);
    }
    rep.table("Contract kills by method (mean score)", &["method", "count", "mean score"], rows);
    let mean = |k: &str| st.by_cause.get(k).map(|(n, p)| *p as f32 / (*n).max(1) as f32);
    let ordering_ok = match (mean("assassination"), mean("pistol")) {
        (Some(a), Some(p)) => a > p,
        (Some(a), None) => a >= 400.0,
        _ => true,
    };
    rep.check(S, "S4 assassinations outscore loud kills", ordering_ok, format!("assassination {:?} vs pistol {:?}", mean("assassination"), mean("pistol")));
    rep.check(S, "S7 detection becomes chases", st.pursuits > 0, format!("{} pursuit transitions", st.pursuits));
    rep.check(S, "S8 guards respond to violence", st.guard_combat_ticks > 0, format!("{} guard-combat ticks", st.guard_combat_ticks));
    rep.check(S, "S8 civilians react to disturbances", st.civilian_flee_ticks > 0, format!("{} civilian-flee ticks, {} NPC deaths", st.civilian_flee_ticks, st.npc_deaths));
    rep.check(S, "S10 full match completes with a winner", st.ended && st.winner.is_some(), format!("winner {:?} final {:?}", st.winner, st.final_scores));
    let st2 = run_headless(&mc, minutes.min(3.0), 9);
    let st3 = run_headless(&mc, minutes.min(3.0), 9);
    rep.check(S, "determinism with crowd AI (identical runs)", st2.hash == st3.hash, format!("{:016x} vs {:016x}", st2.hash, st3.hash));
    net(ctx, rep, &mc)?;
    Ok(())
}

/// Networked Contracts: dedicated server process in contracts mode + 8 bot clients.
fn net(ctx: &Ctx, rep: &mut Report, mc: &Arc<MatchContent>) -> Result<()> {
    let secs: f32 = ctx.opt("--seconds").and_then(|s| s.parse().ok()).unwrap_or(75.0);
    let port = free_port();
    let _server: KillOnDrop = spawn_server(ctx, port, None, secs as u64 + 12, &["--mode", "contracts", "--time-limit", "300", "--snapshot-hz", "30"], "contracts_server.log")?;
    std::thread::sleep(Duration::from_secs(3));
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let nav = Arc::new(NavGraph::build(&mc.collision, mc.ai_points.iter().map(|p| p.0).chain(mc.spawns.iter().map(|s| s.position))));
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let mc = mc.clone();
            let nav = nav.clone();
            std::thread::spawn(move || {
                let sock = NetSocket::bind("127.0.0.1:0").unwrap().with_conditions(LinkConditions::rtt(PROFILES[i % 5].0, PROFILES[i % 5].1), 300 + i as u64);
                let mut c = NetClient::new(sock, addr, &format!("cbot{i}"));
                let mut bot = Bot::new(0, BotStyle::Duelist, 900 + i as u64).with_nav(nav);
                let (mut got_target, mut kills) = (0usize, 0usize);
                let end = Instant::now() + Duration::from_secs_f32(secs);
                let tick = Duration::from_secs_f64(1.0 / 60.0);
                let mut next = Instant::now();
                while Instant::now() < end {
                    c.poll(&mc.collision, &mc.sim);
                    for (_, e) in c.events.drain(..) {
                        if matches!(e, SimEvent::Death { .. }) {
                            kills += 1;
                        }
                    }
                    if let (Some(me), true) = (c.me().copied(), c.state == ConnState::Connected) {
                        bot.id = me.id;
                        let target = c.target_info.as_ref().and_then(|t| t.target);
                        if target.is_some() {
                            got_target += 1;
                        }
                        let mut others = c.remote_players(3.0);
                        others.retain(|p| p.id != me.id);
                        let inp = bot.think_contract(&me, &others, &mc.collision, target);
                        c.send_input(inp, &mc.collision, &mc.sim);
                    }
                    next += tick;
                    if let Some(d) = next.checked_duration_since(Instant::now()) {
                        std::thread::sleep(d);
                    }
                }
                c.disconnect();
                let npcs = c.predicted.players.iter().filter(|p| p.kind != PawnKind::Human).count();
                let mean = c.metrics.prediction_error_sum / c.metrics.prediction_error_samples.max(1) as f64;
                (c.state == ConnState::Connected, got_target, kills, npcs, mean, c.matchv.scores.iter().map(|s| (s.name.clone(), s.score)).collect::<Vec<_>>())
            })
        })
        .collect();
    let res: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let connected = res.iter().filter(|r| r.0).count();
    rep.check(S, "networked Contracts: 8 clients connected", connected == 8, format!("{connected}/8"));
    let with_target = res.iter().filter(|r| r.1 > 0).count();
    rep.check(S, "private target info delivered per client", with_target >= 7, format!("{with_target}/8 clients received a contract"));
    let npcs = res.iter().map(|r| r.3).max().unwrap_or(0);
    rep.check(S, "NPC crowd replicated to clients", npcs >= 20, format!("{npcs} NPCs in client world"));
    let deaths = res.iter().map(|r| r.2).max().unwrap_or(0);
    rep.check(S, "networked kills resolved by the server", deaths > 0, format!("{deaths} deaths observed"));
    let worst = res.iter().map(|r| r.4).fold(0.0, f64::max);
    rep.check(S, "prediction stays tight with 30+ pawns", worst < 25.0, format!("worst client mean prediction error {worst:.2} uu"));
    let boards: Vec<_> = res.iter().map(|r| r.5.clone()).collect();
    let agree = boards.windows(2).all(|w| w[0] == w[1] || w[0].is_empty() || w[1].is_empty());
    rep.check(S, "scoreboards agree across clients", agree, format!("{} boards", boards.len()));
    // Replay of the contracts session (crowd included) must re-simulate bit-exactly.
    let t0 = Instant::now();
    let dir = ctx.out.join("replays");
    let mut file = None;
    while t0.elapsed() < Duration::from_secs(40) && file.is_none() {
        file = std::fs::read_dir(&dir).ok().and_then(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "replay")).max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()));
        std::thread::sleep(Duration::from_millis(500));
    }
    if let Some(f) = file {
        std::thread::sleep(Duration::from_millis(800));
        crate::netsoak::verify_replay(&f, mc, rep)?;
    } else {
        rep.check(S, "contracts replay written", false, "none");
    }
    Ok(())
}
