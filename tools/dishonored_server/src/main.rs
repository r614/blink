//! Headless dedicated server.
//!
//! ```text
//! dishonored_server [--map L_Boyle_Ext_P] [--port 27015] [--name "My Server"] [--mode contracts | --mission FILE.ron]
//!                   [--max-players 8] [--score-limit 20] [--time-limit 600]
//!                   [--master host:27900] [--snapshot-hz 60] [--replay-dir replays]
//!                   [--duration SECONDS]
//! ```
//! Requires a local Dishonored install (collision and tuning are read from it; nothing
//! retail is shipped with the server). Without `--master` the server is still reachable
//! by direct address; with it, it heartbeats the control plane every 5 s.
use anyhow::Result;
use dishonored_content::load_match;
use dishonored_multiplayer::contracts::ContractsRules;
use dishonored_multiplayer::ffa::{FfaRules, SpawnPoint};
use dishonored_multiplayer::game::{CrowdSource, GameSetup, crowd_center, default_crowd};
use dishonored_net::control_plane::{send_deregister, send_heartbeat};
use dishonored_net::server::{GameServer, ServerConfig};
use dishonored_net::socket::NetSocket;
use dishonored_sim::movement::TICK_HZ;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    // `--mission FILE`: host a custom mission (co-op); the map comes from the mission.
    let mission = get("--mission").map(|p| dishonored_mission::MissionDef::load(std::path::Path::new(&p))).transpose()?;
    let map = get("--map").or_else(|| mission.as_ref().map(|m| m.map.clone())).unwrap_or_else(|| dishonored_content::DEFAULT_MAP.to_string());
    let port: u16 = get("--port").and_then(|s| s.parse().ok()).unwrap_or(27015);
    let name = get("--name").unwrap_or_else(|| "Blink server".into());
    let mut rules = FfaRules::default();
    if let Some(v) = get("--max-players").and_then(|s| s.parse().ok()) { rules.max_players = v; }
    if let Some(v) = get("--score-limit").and_then(|s| s.parse().ok()) { rules.score_limit = v; }
    if let Some(v) = get("--time-limit").and_then(|s| s.parse().ok()) { rules.time_limit = v; }
    let snapshot_hz: u64 = get("--snapshot-hz").and_then(|s| s.parse().ok()).unwrap_or(60);
    let master = get("--master");
    let replay_dir = get("--replay-dir");
    let duration: Option<f64> = get("--duration").and_then(|s| s.parse().ok());

    if let Some(file) = get("--verify") {
        // Re-simulate a replay with this binary (determinism check).
        let r = dishonored_net::replay::Replay::load(std::path::Path::new(&file))?;
        let mc = load_match(&r.header.map)?;
        let v = dishonored_net::replay::resimulate(&r, &mc.collision, &mc.sim, |_, _| {});
        println!("{} ticks, {} mismatches, first {:?}", v.frames, v.mismatches, v.first_mismatch_tick);
        return Ok(());
    }
    let t = Instant::now();
    // A mission may travel between persistent maps (all loaded up front, see MapContents).
    let names = match &mission {
        Some(def) => dishonored_content::mission_maps(def),
        None => vec![map.clone()],
    };
    let maps = dishonored_content::MapContents::load(&names)?;
    let mc = maps.get(&map).cloned().unwrap_or_else(|| maps.maps[0].clone());
    maps.set.set_active(&mc.map_name);
    println!("loaded {map}: {} collision tris, {} spawns in {:.1?}", mc.collision.tris.len(), mc.spawns.len(), t.elapsed());
    let spawns: Vec<SpawnPoint> = mc.spawns.iter().map(|s| SpawnPoint { position: s.position, yaw: s.yaw }).collect();
    let setup = if let Some(def) = mission {
        dishonored_content::mission_setup(&maps, def).map_err(|e| anyhow::anyhow!("mission: {e}"))?
    } else if get("--mode").as_deref() == Some("contracts") {
        let mut crules = ContractsRules { max_players: rules.max_players, time_limit: rules.time_limit.max(300.0), ..Default::default() };
        let center = crowd_center(&mc.spawns.iter().map(|s| (s.position, s.yaw)).collect::<Vec<_>>());
        if !args.iter().any(|a| a == "--no-crowd") {
            crules.crowd = Some(default_crowd(center));
        }
        let pts: Vec<(glam::Vec3, f32)> = mc.spawns.iter().map(|s| (s.position, s.yaw)).collect();
        GameSetup::contracts(crules, pts, Some(CrowdSource { points: &mc.ai_points, guard_spawns: &mc.guard_spawns, flee_points: &mc.flee_points, routes: &mc.routes, ai: Some((&mc.ai_level, &mc.spawners)) }), &mc.collision)
    } else {
        GameSetup::Ffa { rules, spawns }
    };
    let socket = NetSocket::bind(&format!("0.0.0.0:{port}"))?;
    let cfg = ServerConfig { name: name.clone(), map: map.clone(), setup, snapshot_interval: (TICK_HZ as u64 / snapshot_hz.max(1)).max(1) };
    let mut server = GameServer::new(cfg, mc.sim.clone(), &maps.set, socket);
    println!("listening on udp/{port} as \"{name}\"");

    let shared_info = Arc::new(Mutex::new(server.info()));
    if let Some(m) = master.clone() {
        let info = shared_info.clone();
        std::thread::spawn(move || loop {
            let i = info.lock().unwrap().clone();
            if let Err(e) = send_heartbeat(&m, port, &i) {
                eprintln!("heartbeat to {m} failed: {e:#}");
            }
            std::thread::sleep(Duration::from_secs(5));
        });
    }

    let tick = Duration::from_secs_f64(1.0 / TICK_HZ as f64);
    let start = Instant::now();
    let mut next = Instant::now();
    let mut last_report = Instant::now();
    loop {
        server.tick();
        // Level travel: collide against the map the mission is on now.
        if let Some(m) = server.game.current_map() {
            if !maps.set.active_name().eq_ignore_ascii_case(m) && maps.set.set_active(m) {
                println!("level travel -> {m}");
            }
        }
        *shared_info.lock().unwrap() = server.info();
        if last_report.elapsed() > Duration::from_secs(10) {
            last_report = Instant::now();
            println!("tick {} players {} phase {:?} starved {} snapshots {}", server.game.world().tick, server.player_count(), server.game.view().phase, server.stats.inputs_starved, server.stats.snapshots_sent);
        }
        if duration.is_some_and(|d| start.elapsed().as_secs_f64() >= d) {
            break;
        }
        next += tick;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_millis(250) {
            next = now; // fell far behind; do not spiral
        }
    }
    if let (Some(dir), Some(r)) = (replay_dir, server.replay.as_ref()) {
        std::fs::create_dir_all(&dir)?;
        let path = std::path::Path::new(&dir).join(format!("{map}-{}.replay", server.game.world().tick));
        r.replay.save(&path)?;
        println!("replay saved to {}", path.display());
    }
    if let Some(m) = master {
        let _ = send_deregister(&m, port);
    }
    Ok(())
}
