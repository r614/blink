//! Network soak: real `dishonored_master` and `dishonored_server` processes, bot clients
//! over UDP at the latency profiles from the brief, FFA played to completion.

use crate::Ctx;
use crate::report::Report;
use anyhow::{Context, Result};
use dishonored_bots::{Bot, BotStyle};
use dishonored_content::{DEFAULT_MAP, load_match};
use dishonored_net::client::{ConnState, NetClient, NetMetrics, query_info};
use dishonored_net::control_plane::list_servers;
use dishonored_net::replay::{Replay, resimulate};
use dishonored_net::socket::{LinkConditions, NetSocket};
use dishonored_sim::events::SimEvent;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const S: &str = "Networking (dedicated server + control plane)";
pub const PROFILES: [(f32, f32); 5] = [(0.0, 0.0), (30.0, 0.0), (60.0, 0.02), (100.0, 0.02), (150.0, 0.03)];

pub struct KillOnDrop(pub Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

pub fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

pub fn spawn_server(ctx: &Ctx, port: u16, master: Option<&str>, secs: u64, extra: &[&str], log: &str) -> Result<KillOnDrop> {
    let mut cmd = Command::new(ctx.bin("dishonored_server"));
    cmd.args(["--port", &port.to_string(), "--name", "harness", "--duration", &secs.to_string()])
        .args(["--replay-dir", &ctx.out.join("replays").display().to_string()])
        .args(extra);
    if let Some(m) = master {
        cmd.args(["--master", m]);
    }
    let logf = std::fs::File::create(ctx.out.join(log))?;
    cmd.stdout(Stdio::from(logf.try_clone()?)).stderr(Stdio::from(logf));
    Ok(KillOnDrop(cmd.spawn().context("starting dishonored_server (build it with cargo build --release)")?))
}

pub struct BotResult {
    pub profile: (f32, f32),
    pub metrics: NetMetrics,
    pub connected: bool,
    pub kills_seen: usize,
    /// Deaths with another player as the killer.
    pub player_kills_seen: usize,
    /// Deaths with no killer or the victim as killer (world, falls, drowning, suicide).
    pub env_deaths_seen: usize,
    pub final_scores: Vec<(String, i32)>,
    pub matches_ended: u32,
}

/// Run one headless bot client in real time.
pub fn run_bot(server: std::net::SocketAddr, idx: usize, profile: (f32, f32), secs: f32, mc: Arc<dishonored_content::MatchContent>) -> BotResult {
    let sock = NetSocket::bind("127.0.0.1:0").unwrap().with_conditions(LinkConditions::rtt(profile.0, profile.1), 100 + idx as u64);
    let mut client = NetClient::new(sock, server, &format!("bot{idx}"));
    let style = if idx % 2 == 0 { BotStyle::Duelist } else { BotStyle::Gunner };
    let mut bot = Bot::new(0, style, 500 + idx as u64);
    let tick = Duration::from_secs_f64(1.0 / 60.0);
    let end = Instant::now() + Duration::from_secs_f32(secs);
    let mut next = Instant::now();
    let mut kills_seen = 0;
    let (mut player_kills_seen, mut env_deaths_seen) = (0, 0);
    let mut matches_ended = 0;
    let mut last_phase = None;
    while Instant::now() < end {
        client.poll(&mc.collision, &mc.sim);
        for (_, e) in client.events.drain(..) {
            if let SimEvent::Death { victim, killer, .. } = e {
                kills_seen += 1;
                if killer.is_some_and(|k| k != victim) {
                    player_kills_seen += 1;
                } else {
                    env_deaths_seen += 1;
                }
            }
        }
        if last_phase != Some(client.matchv.phase) {
            if client.matchv.phase == dishonored_multiplayer::MatchPhase::Ended {
                matches_ended += 1;
            }
            last_phase = Some(client.matchv.phase);
        }
        if let (Some(me), true) = (client.me().copied(), client.state == ConnState::Connected) {
            bot.id = me.id;
            let others = client.remote_players(6.0);
            let input = bot.think(&me, &others, &mc.collision);
            client.send_input(input, &mc.collision, &mc.sim);
        }
        next += tick;
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    client.disconnect();
    BotResult {
        profile,
        connected: client.state == ConnState::Connected,
        final_scores: client.matchv.scores.iter().map(|s| (s.name.clone(), s.score)).collect(),
        metrics: client.metrics,
        kills_seen,
        player_kills_seen,
        env_deaths_seen,
        matches_ended,
    }
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let players: usize = ctx.opt("--players").and_then(|s| s.parse().ok()).unwrap_or(8);
    let secs: f32 = ctx.opt("--seconds").and_then(|s| s.parse().ok()).unwrap_or(75.0);
    let map = ctx.opt("--map").unwrap_or_else(|| DEFAULT_MAP.into());
    let mc = Arc::new(load_match(&map)?);

    // Control plane.
    let mport = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let maddr = format!("127.0.0.1:{mport}");
    let mlog = std::fs::File::create(ctx.out.join("master.log"))?;
    let _master = KillOnDrop(Command::new(ctx.bin("dishonored_master")).args(["--bind", &maddr, "--ttl", "15"]).stdout(Stdio::from(mlog.try_clone()?)).stderr(Stdio::from(mlog)).spawn().context("starting dishonored_master")?);

    // Dedicated server process.
    let sport = free_port();
    let server_secs = secs as u64 + 12;
    let _server = spawn_server(ctx, sport, Some(&maddr), server_secs, &["--score-limit", "8", "--time-limit", "45", "--max-players", &players.max(2).to_string()], "server.log")?;

    // Discovery through the control plane, then direct UDP info.
    let t0 = Instant::now();
    let mut listed = Vec::new();
    while t0.elapsed() < Duration::from_secs(20) {
        if let Ok(l) = list_servers(&maddr) {
            if !l.is_empty() {
                listed = l;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    rep.check(S, "server registers with control plane", !listed.is_empty(), format!("{:?} after {:.1?}", listed.first().map(|l| (&l.addr, &l.info.name, &l.info.map)), t0.elapsed()));
    let server_addr: std::net::SocketAddr = format!("127.0.0.1:{sport}").parse()?;
    let info = query_info(server_addr, Duration::from_secs(2));
    rep.check(S, "UDP info query (server browser ping)", info.is_some(), format!("{:?}", info.as_ref().map(|(i, rtt)| (i.players, i.max_players, format!("{rtt:.1}ms")))));

    // Bot clients.
    let handles: Vec<_> = (0..players)
        .map(|i| {
            let mc = mc.clone();
            let prof = PROFILES[i % PROFILES.len()];
            std::thread::spawn(move || run_bot(server_addr, i, prof, secs, mc))
        })
        .collect();
    let results: Vec<BotResult> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let connected = results.iter().filter(|r| r.connected).count();
    rep.check(S, &format!("{players} clients connect and stay connected"), connected == players, format!("{connected}/{players}"));
    let mut rows = Vec::new();
    let mut err_ok = true;
    for r in &results {
        let m = &r.metrics;
        let mean = m.prediction_error_sum / m.prediction_error_samples.max(1) as f64;
        // Bound grows with latency: more inputs in flight -> more divergence on contact.
        let bound = 4.0 + r.profile.0 as f64 * 0.1;
        err_ok &= mean <= bound;
        rows.push(vec![
            format!("{:.0} ms / {:.0}%", r.profile.0, r.profile.1 * 100.0),
            format!("{:.0}", m.rtt_ms),
            m.snapshots.to_string(),
            format!("{:.1}", m.snapshot_age_ms),
            format!("{mean:.2}"),
            format!("{:.1}", m.prediction_error_max),
            m.corrections_over_10uu.to_string(),
            format!("{:.1}", m.correction_max),
            r.kills_seen.to_string(),
            r.player_kills_seen.to_string(),
        ]);
    }
    rep.table("Per-client network metrics", &["profile (rtt/loss)", "measured RTT", "snapshots", "snapshot interval ms", "mean pred err (uu)", "max pred err", "corrections >10uu", "max correction", "deaths seen", "player kills seen"], rows);
    rep.check(S, "prediction error within latency-scaled bounds", err_ok, "mean error <= 4 uu + 0.1 uu per ms RTT");
    let deaths = results.iter().map(|r| r.kills_seen).max().unwrap_or(0);
    rep.check(S, "server-authoritative deaths replicated to clients", deaths > 0, format!("{deaths} deaths observed"));
    // The bots hunt each other over the network: deaths must include kills by players, not
    // only falls out of the world.
    let best = results.iter().max_by_key(|r| r.kills_seen);
    let (kills, env) = best.map_or((0, 0), |r| (r.player_kills_seen, r.env_deaths_seen));
    rep.check(S, "networked bots kill each other (player kills, not only world deaths)", kills >= 3 && env * 4 <= kills, format!("{kills} player kills, {env} self / world deaths seen by one client in {secs:.0} s"));
    let ended = results.iter().map(|r| r.matches_ended).max().unwrap_or(0);
    rep.check(S, "FFA completes end to end (score/time limit -> winner)", ended >= 1, format!("{ended} match(es) ended during soak"));
    // Scoreboards agree across clients (replicated match state).
    let boards: Vec<_> = results.iter().filter(|r| r.connected).map(|r| r.final_scores.clone()).collect();
    let agree = boards.windows(2).all(|w| {
        let a: std::collections::HashMap<_, _> = w[0].iter().cloned().collect();
        w[1].iter().all(|(n, s)| a.get(n).is_none_or(|x| (x - s).abs() <= 1))
    });
    rep.check(S, "scoreboards agree across clients", agree, format!("{} boards compared", boards.len()));

    // Server writes its replay on exit; verify determinism.
    let t0 = Instant::now();
    let replay_dir = ctx.out.join("replays");
    let mut file = None;
    while t0.elapsed() < Duration::from_secs(30) {
        if let Ok(rd) = std::fs::read_dir(&replay_dir) {
            file = rd.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "replay"));
            if file.is_some() {
                std::thread::sleep(Duration::from_millis(500));
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    match file {
        Some(f) => verify_replay(&f, &mc, rep)?,
        None => rep.check(S, "server replay written", false, "no replay file"),
    }
    rep.artifact("server log", &ctx.out.join("server.log"));
    rep.artifact("master log", &ctx.out.join("master.log"));
    Ok(())
}

pub fn verify_replay(path: &Path, mc: &dishonored_content::MatchContent, rep: &mut Report) -> Result<()> {
    let r = Replay::load(path)?;
    let joins = r.frames.iter().map(|f| f.roster.len()).sum::<usize>();
    let v = resimulate(&r, &mc.collision, &mc.sim, |_, _| {});
    rep.check(S, "replay re-simulates bit-exactly", v.mismatches == 0 && v.frames > 0, format!("{} ticks, {} roster changes, {} mismatches (first {:?})", v.frames, joins, v.mismatches, v.first_mismatch_tick));
    rep.artifact("server replay", path);
    Ok(())
}

pub fn verify_replay_file(path: &Path, rep: &mut Report) -> Result<()> {
    let r = Replay::load(path)?;
    let mc = load_match(&r.header.map)?;
    verify_replay(path, &mc, rep)
}
