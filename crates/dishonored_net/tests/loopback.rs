//! End-to-end netcode over real UDP sockets on loopback (no retail data needed):
//! control plane -> server discovery -> connect -> predicted movement -> combat -> score
//! -> replay determinism. Latency/loss come from the link conditioner.

use dishonored_multiplayer::ffa::{FfaRules, SpawnPoint};
use dishonored_net::client::{ConnState, NetClient, query_info};
use dishonored_net::control_plane::{Master, list_servers, send_heartbeat};
use dishonored_net::replay::resimulate;
use dishonored_net::server::{GameServer, ServerConfig};
use dishonored_net::socket::{LinkConditions, NetSocket};
use dishonored_sim::SimConfig;
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;
use std::net::TcpListener;
use std::time::{Duration, Instant};

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn floor() -> CollisionWorld {
    let mut tris = Vec::new();
    let (a, b, c, d) = (Vec3::new(-8000., -8000., 0.), Vec3::new(8000., -8000., 0.), Vec3::new(8000., 8000., 0.), Vec3::new(-8000., 8000., 0.));
    for (x, y, z) in [(a, b, c), (a, c, d)] {
        tris.push(Tri { a: x, b: y, c: z, n: Vec3::Z, channels: channel::ALL, source: 0 });
    }
    CollisionWorld::from_triangles(tris, vec!["floor".into()])
}

struct Net<'w> {
    server: GameServer<'w>,
    clients: Vec<NetClient>,
}

impl<'w> Net<'w> {
    fn run(&mut self, secs: f32, world: &CollisionWorld, sim: &SimConfig, mut input: impl FnMut(usize, &NetClient) -> PlayerInput) {
        let tick = Duration::from_secs_f64(1.0 / 60.0);
        let end = Instant::now() + Duration::from_secs_f32(secs);
        let mut next = Instant::now();
        while Instant::now() < end {
            self.server.tick();
            for (i, c) in self.clients.iter_mut().enumerate() {
                c.poll(world, sim);
                let inp = input(i, c);
                c.send_input(inp, world, sim);
            }
            next += tick;
            if let Some(d) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
    }
}

fn setup<'w>(world: &'w CollisionWorld, sim: &SimConfig, spawns: Vec<SpawnPoint>, cond: LinkConditions, rules: FfaRules) -> Net<'w> {
    let ssock = NetSocket::bind("127.0.0.1:0").unwrap().with_conditions(cond, 1);
    let addr = ssock.local_addr();
    let server = GameServer::new(
        ServerConfig { name: "test".into(), map: "fixture".into(), rules, snapshot_interval: 1 },
        sim.clone(),
        world,
        ssock,
        spawns,
    );
    let clients = (0..2)
        .map(|i| NetClient::new(NetSocket::bind("127.0.0.1:0").unwrap().with_conditions(cond, 10 + i), addr, &format!("bot{i}")))
        .collect();
    Net { server, clients }
}

#[test]
fn control_plane_lists_and_server_answers_info() {
    let master = Master::new(Duration::from_secs(30));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let maddr = listener.local_addr().unwrap().to_string();
    let m2 = master.clone();
    std::thread::spawn(move || m2.serve_listener(listener));

    let world = floor();
    let sim = cfg();
    let mut net = setup(&world, &sim, vec![SpawnPoint { position: Vec3::new(0., 0., 100.), yaw: 0. }], LinkConditions::default(), FfaRules::default());
    let port = net.server.socket.local_addr().port();
    send_heartbeat(&maddr, port, &net.server.info()).unwrap();
    let listed = list_servers(&maddr).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].info.name, "test");
    // Direct UDP info query (what a server browser does after listing).
    let addr: std::net::SocketAddr = listed[0].addr.parse().unwrap();
    let h = std::thread::spawn(move || query_info(addr, Duration::from_secs(2)));
    let t0 = Instant::now();
    while !h.is_finished() && t0.elapsed() < Duration::from_secs(3) {
        net.server.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    let (info, rtt) = h.join().unwrap().expect("info response");
    assert_eq!(info.max_players, 8);
    assert!(rtt < 500.0);
}

#[test]
fn predicted_movement_under_latency_and_loss() {
    let world = floor();
    let sim = cfg();
    let spawns = vec![SpawnPoint { position: Vec3::new(0., 0., 100.), yaw: 0. }, SpawnPoint { position: Vec3::new(0., 1000., 100.), yaw: 0. }];
    let mut net = setup(&world, &sim, spawns, LinkConditions::rtt(100.0, 0.05), FfaRules { warmup_time: 0.2, ..Default::default() });
    // Client 0 runs forward and strafes; client 1 idles and watches.
    net.run(6.0, &world, &sim, |i, _| {
        if i == 0 {
            PlayerInput { yaw: 0.0, forward: 127, right: 40, buttons: buttons::SPRINT, left_hand: LeftHand::Blink, ..Default::default() }
        } else {
            PlayerInput { yaw: 0.0, left_hand: LeftHand::Blink, ..Default::default() }
        }
    });
    let c0 = &net.clients[0];
    let c1 = &net.clients[1];
    assert_eq!(c0.state, ConnState::Connected);
    assert_eq!(c1.state, ConnState::Connected);
    let m = &c0.metrics;
    let mean_err = m.prediction_error_sum / m.prediction_error_samples.max(1) as f64;
    println!("rtt {:.0}ms snapshots {} mean pred err {:.3} max {:.2} corrections>10uu {} max corr {:.2}", m.rtt_ms, m.snapshots, mean_err, m.prediction_error_max, m.corrections_over_10uu, m.correction_max);
    assert!(m.rtt_ms > 60.0 && m.rtt_ms < 200.0, "rtt {}", m.rtt_ms);
    assert!(mean_err < 2.0, "mean prediction error {mean_err}");
    // Authoritative position moved ~ sprinting for ~5 s.
    let server_p0 = net.server.game.world.players.iter().find(|p| p.id == c0.player_id.unwrap()).unwrap().position;
    assert!(server_p0.x > 1500.0, "server x {}", server_p0.x);
    // Remote view on client 1 is close to the server (interpolation delay ~ 100 ms of travel).
    let remote = c1.remote_players(6.0);
    let seen = remote.iter().find(|p| p.id == c0.player_id.unwrap()).expect("remote player visible");
    let lag = seen.position.distance(server_p0);
    assert!(lag < 250.0, "remote lag {lag}");
    // Replay of the whole session re-simulates bit-exactly.
    let replay = net.server.replay.as_ref().unwrap().replay.clone();
    let rep = resimulate(&replay, &world, &sim, |_, _| {});
    assert!(rep.frames > 300);
    assert_eq!(rep.mismatches, 0, "first mismatch {:?}", rep.first_mismatch_tick);
}

#[test]
fn networked_combat_kill_and_score() {
    let world = floor();
    let sim = cfg();
    // Facing each other 300 uu apart.
    let spawns = vec![SpawnPoint { position: Vec3::new(0., 0., 100.), yaw: 0. }, SpawnPoint { position: Vec3::new(300., 0., 100.), yaw: 180. }];
    let rules = FfaRules { warmup_time: 0.2, respawn_delay: 3.0, score_limit: 1, post_match_time: 30.0, ..Default::default() };
    let mut net = setup(&world, &sim, spawns, LinkConditions::rtt(60.0, 0.02), rules);
    let mut frame = 0u64;
    net.run(7.0, &world, &sim, |i, c| {
        frame += 1;
        if i == 0 {
            // Aim at the opponent's chest and fire the pistol every ~half second.
            let target = c.remote_players(6.0).first().map(|p| p.position);
            let me = c.own_position();
            let (yaw, pitch) = match (target, me) {
                (Some(t), Some(m)) => {
                    let d = t - (m + Vec3::Z * 85.0);
                    (d.y.atan2(d.x).to_degrees(), (d.z / d.truncate().length()).atan().to_degrees())
                }
                _ => (0.0, 0.0),
            };
            let fire = (frame / 2) % 30 == 0;
            PlayerInput { yaw, pitch, buttons: if fire { buttons::SECONDARY } else { 0 }, left_hand: LeftHand::Pistol, ..Default::default() }
        } else {
            PlayerInput { yaw: 180.0, left_hand: LeftHand::Blink, ..Default::default() }
        }
    });
    let mv = &net.server.game.view;
    println!("scores {:?} phase {:?}", mv.scores, mv.phase);
    let shooter = net.clients[0].player_id.unwrap();
    let s = mv.scores.iter().find(|s| s.id == shooter).unwrap();
    assert!(s.kills >= 1, "kills {}", s.kills);
    assert_eq!(mv.winner, Some(shooter));
    // Clients learned about the death from authoritative events.
    let saw_death = net.clients[1].events.iter().any(|(_, e)| matches!(e, dishonored_sim::events::SimEvent::Death { .. }));
    assert!(saw_death);
    assert_eq!(net.clients[1].matchv.winner, Some(shooter));
}
