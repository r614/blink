//! `harness mission [--mission FILE]`: plays a custom mission end to end.
//!
//! Headless: a scripted agent (original-navmesh pathing, back assassination) follows the mission's
//! own structure (informant -> host -> extraction) on the authoritative simulation with
//! the ambient crowd and guards, and the report records every task transition, message and
//! the final statistics. The run is repeated to check determinism. Networked: a dedicated
//! server hosts the mission and a client must receive the replicated mission view.

use crate::Ctx;
use crate::netsoak::{KillOnDrop, free_port, spawn_server};
use crate::report::Report;
use anyhow::Result;
use dishonored_bots::nav::PathOpts;
use dishonored_bots::nav::Follower;
use dishonored_content::{MapContents, MatchContent};
use anyhow::Context;
use dishonored_mission::{MissionDef, TaskState};
use dishonored_multiplayer::game::Game;
use dishonored_net::client::{ConnState, NetClient};
use dishonored_net::socket::NetSocket;
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::state_hash;
use dishonored_world::WorldQuery;
use glam::Vec3;
use std::time::{Duration, Instant};

const S: &str = "Custom missions";
const AGENT: u32 = 1;

struct Run {
    hash: u64,
    outcome: Option<(bool, String)>,
    outcome_tick: Option<usize>,
    timeline: Vec<(f32, String)>,
    stats: String,
    reached_informant: bool,
    host_dead: bool,
    agent_deaths: u32,
    final_pos: Vec3,
    host_killed_by_agent: bool,
}

fn play(maps: &MapContents, def: &MissionDef, max_secs: f32) -> Result<Run> {
    let mc: &MatchContent = maps.get(&def.map).context("mission map")?;
    maps.set.set_active(&def.map);
    let mut def = def.clone();
    // The agent is not a careful player: let it respawn so the mission logic is exercised
    // to the end (death without respawn is covered by the interpreter tests).
    def.settings.respawn = true;
    let setup = dishonored_content::mission_setup(maps, def.clone()).map_err(|e| anyhow::anyhow!(e))?;
    let mut g = setup.build(&mc.sim);
    g.add_player(AGENT, "agent", 0, &mc.collision, &mc.sim);
    // The agent paths on the map's original navmesh (carried by the collision world).
    let nav = mc.collision.nav();
    if std::env::var_os("MISSION_NAV_DEBUG").is_some() {
        if let (Game::Mission(m), Some(nav)) = (&g, nav) {
            let pts: Vec<(String, Vec3)> = m.setup.markers.iter().map(|(k, v)| (k.clone(), v.0)).chain(m.setup.npcs.iter().map(|(n, s)| (format!("npc:{n}"), s.position))).collect();
            let start = std::env::var("MISSION_NAV_FROM").ok().and_then(|n| m.setup.npcs.iter().find(|x| x.0 == n).map(|x| x.1.position)).unwrap_or(m.setup.starts[0].0);
            for (name, p) in pts {
                let path = nav.find_path(start, p, &PathOpts::default());
                eprintln!("nav start->{name} at {p:.0}: start on mesh {} goal on mesh {} path {:?}", nav.closest_point(start, 200.0).is_some(), nav.closest_point(p, 300.0).is_some(), path.map(|v| v.len()));
            }
        }
    }
    let mut follow = Follower::default();
    let mut run = Run { hash: 0, outcome: None, outcome_tick: None, timeline: Vec::new(), stats: String::new(), reached_informant: false, host_dead: false, agent_deaths: 0, final_pos: Vec3::ZERO, host_killed_by_agent: false };
    let mut last_tasks: Vec<(String, TaskState)> = Vec::new();
    let mut seen_msgs = 0u32;
    let mut last_pos = Vec3::ZERO;
    let mut stuck = 0u32;
    let mut sidestep = 0u32;
    for tick in 0..(max_secs * 60.0) as usize {
        let Game::Mission(m) = &g else { anyhow::bail!("not a mission game") };
        let host_id = m.setup.npcs.iter().find(|(n, _)| n == "host").map(|(_, s)| s.id);
        let informant = m.setup.npcs.iter().find(|(n, _)| n == "informant").and_then(|(_, s)| m.world.player(s.id)).map(|p| p.position);
        let exfil = m.state.markers.get("exfil").map(|x| x.0);
        let know = m.state.flag("know_host");
        let host = host_id.and_then(|id| m.world.player(id)).filter(|p| p.alive).cloned();
        let me = m.world.player(AGENT).cloned();
        let mut input = PlayerInput { left_hand: LeftHand::Empty, ..Default::default() };
        if let Some(me) = me.filter(|p| p.alive) {
            let (goal, strike) = match (&host, know) {
                (_, false) => (informant.unwrap_or(me.position), None),
                (Some(h), true) => {
                    let behind = h.position - Vec3::new(h.yaw.to_radians().cos(), h.yaw.to_radians().sin(), 0.0) * 80.0;
                    (behind, Some(h.clone()))
                }
                (None, true) => (exfil.unwrap_or(me.position), None),
            };
            let talk_to = informant.filter(|_| !know);
            let near = strike.as_ref().is_some_and(|h| h.position.distance(me.position) < 150.0);
            if let Some(ip) = talk_to.filter(|ip| ip.distance(me.position) < 130.0) {
                // Face the informant and press F (edge: press every 12 ticks).
                let to = ip - me.position;
                input.yaw = to.y.atan2(to.x).to_degrees();
                input.pitch = -10.0;
                if tick % 12 == 0 {
                    input.buttons |= buttons::USE;
                }
            } else if near {
                let h = strike.unwrap();
                let to = h.position - me.position;
                input.yaw = to.y.atan2(to.x).to_degrees();
                if tick % 15 == 0 {
                    input.buttons |= buttons::PRIMARY;
                }
            } else {
                let p = follow.steer(nav, me.position, goal);
                let to = p - me.position;
                input.yaw = to.y.atan2(to.x).to_degrees();
                input.forward = 127;
                // Walk among the guests near the host (blend in), otherwise move normally.
                if strike.as_ref().is_some_and(|h| h.position.distance(me.position) < 900.0) {
                    input.buttons |= buttons::WALK;
                }
                // Horizontal progress only: a pawn bobbing against a step changes height every tick.
                if me.position.truncate().distance(last_pos.truncate()) < 0.5 {
                    stuck += 1;
                } else {
                    stuck = 0;
                }
                if stuck > 40 {
                    stuck = 0;
                    sidestep = 25;
                    follow.force_replan();
                }
                if sidestep > 0 {
                    sidestep -= 1;
                    input.yaw += 90.0;
                    if sidestep == 24 {
                        input.buttons |= buttons::JUMP;
                    }
                }
            }
            last_pos = me.position;
        }
        let out = g.step(&[(AGENT, input)], &maps.set, &mc.sim);
        for e in &out.events {
            if let dishonored_sim::events::SimEvent::Death { victim, killer, cause } = e {
                if Some(*victim) == host_id {
                    run.host_killed_by_agent |= *killer == Some(AGENT);
                    run.timeline.push((tick as f32 / 60.0, format!("host died: killer {killer:?} cause {cause:?}")));
                }
            }
        }
        let Game::Mission(m) = &g else { unreachable!() };
        let t = tick as f32 / 60.0;
        // Timeline: task transitions and new messages.
        let tasks: Vec<(String, TaskState)> = m.state.def.objectives.iter().flat_map(|o| o.tasks.iter().map(move |x| format!("{}.{}", o.id, x.id))).map(|p| {
            let s = m.state.task(&p).unwrap_or_default();
            (p, s)
        }).collect();
        for (p, s) in &tasks {
            if last_tasks.iter().find(|(q, _)| q == p).map(|x| x.1) != Some(*s) && !last_tasks.is_empty() {
                run.timeline.push((t, format!("task {p} -> {s:?}")));
            }
        }
        last_tasks = tasks;
        for (id, msg) in &m.state.view.messages {
            if *id > seen_msgs {
                seen_msgs = *id;
                run.timeline.push((t, format!("message \"{msg}\"")));
            }
        }
        run.reached_informant |= m.state.flag("know_host");
        run.host_dead |= m.state.flag("host_dead");
        run.agent_deaths = m.state.view.stats.player_deaths;
        if let Some(o) = m.state.outcome() {
            run.outcome = Some((o.success, o.text.clone()));
            run.outcome_tick = Some(tick);
            let st = &m.state.view.stats;
            run.stats = format!(
                "time {:.0}s, hostiles killed {}, civilians killed {}, detected {}, alarms {}, agent deaths {}, chaos {}, ghost {}, clean hands {}",
                st.time, st.hostiles_killed, st.civilians_killed, st.detected, st.alarms, st.player_deaths, if m.state.view.high_chaos { "high" } else { "low" }, st.ghost(), st.clean_hands()
            );
            break;
        }
    }
    if let Game::Mission(m) = &g {
        run.final_pos = m.world.player(AGENT).map(|p| p.position).unwrap_or_default();
    }
    run.hash = state_hash(g.world());
    Ok(run)
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let path = ctx.opt("--mission").unwrap_or_else(|| "missions/boyle_masked_host.ron".into());
    let def = MissionDef::load(std::path::Path::new(&path))?;
    let errs = def.validate();
    rep.check(S, "mission file parses and validates", errs.is_empty(), if errs.is_empty() { format!("{path}: \"{}\"", def.title) } else { errs.join("; ") });
    let t = Instant::now();
    let maps = MapContents::load(&dishonored_content::mission_maps(&def))?;
    let mc = maps.get(&def.map).context("mission map")?.clone();
    let setup = dishonored_content::mission_setup(&maps, def.clone());
    rep.check(S, "markers and NPCs resolve against the level", setup.is_ok(), match &setup {
        Ok(dishonored_multiplayer::game::GameSetup::Mission(s)) => format!("{} markers, {} mission NPCs, crowd {} in {:.1?}", s.markers.len(), s.npcs.len(), s.crowd.as_ref().map(|c| c.brains.len()).unwrap_or(0), t.elapsed()),
        Ok(_) => String::new(),
        Err(e) => e.clone(),
    });
    let max: f32 = ctx.opt("--seconds").and_then(|s| s.parse().ok()).unwrap_or(600.0);
    let a = play(&maps, &def, max)?;
    rep.check(S, "agent finds the informant (clue reveals the host)", a.reached_informant, a.timeline.iter().find(|x| x.1.contains("host.kill -> Active")).map(|x| format!("at {:.0}s", x.0)).unwrap_or_default());
    rep.check(S, "host eliminated by the agent", a.host_killed_by_agent, a.timeline.iter().find(|x| x.1.contains("host.kill -> Completed")).map(|x| format!("at {:.0}s", x.0)).unwrap_or_default());
    rep.check(S, "mission completes successfully", a.outcome.as_ref().is_some_and(|o| o.0), format!("{:?}; {}; agent ended at {:.0}", a.outcome, a.stats, a.final_pos));
    rep.table("Mission timeline (simulated seconds)", &["t", "event"], a.timeline.iter().map(|(t, e)| vec![format!("{t:.1}"), e.clone()]).collect());
    let b = play(&maps, &def, max)?;
    rep.check(S, "determinism: identical mission runs", a.hash == b.hash && a.outcome_tick == b.outcome_tick, format!("{:016x} @ {:?} vs {:016x} @ {:?}", a.hash, a.outcome_tick, b.hash, b.outcome_tick));
    // Dedicated server hosting the mission; a client must see the replicated mission view.
    let port = free_port();
    let _server: KillOnDrop = spawn_server(ctx, port, None, 40, &["--mission", &path], "mission_server.log")?;
    std::thread::sleep(Duration::from_secs(4));
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let mut c = NetClient::new(NetSocket::bind("127.0.0.1:0")?, addr, "coop");
    let end = Instant::now() + Duration::from_secs(25);
    let mut got = None;
    while Instant::now() < end {
        c.poll(&mc.collision, &mc.sim);
        if c.state == ConnState::Connected {
            c.send_input(PlayerInput::default(), &mc.collision, &mc.sim);
            if let Some(mv) = c.matchv.mission.as_ref() {
                if !mv.messages.is_empty() {
                    got = Some((mv.title.clone(), mv.objectives.len(), mv.messages.len(), c.predicted.players.len()));
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    c.disconnect();
    rep.check(S, "dedicated server hosts the mission; client receives objectives and messages", got.as_ref().is_some_and(|g| g.0 == def.title && g.1 == def.objectives.len()), format!("{got:?}"));
    Ok(())
}
