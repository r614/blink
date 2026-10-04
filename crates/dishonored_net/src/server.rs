//! Authoritative game server. Owns damage, death, projectiles, Blink validation, score
//! and match state. Driven by `tick()` from a fixed-rate loop (binary) or a test harness.

use crate::protocol::*;
use crate::replay::{ReplayFrame, ReplayHeader, ReplayRecorder, Roster};
use crate::socket::NetSocket;
use dishonored_multiplayer::ffa::{FfaMatch, FfaRules, SpawnPoint};
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::PlayerId;
use dishonored_sim::{SimConfig, state_hash};
use dishonored_world::WorldQuery;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub const JITTER_BUFFER_MAX: usize = 6;
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub name: String,
    pub map: String,
    pub rules: FfaRules,
    /// Send a snapshot every N ticks (1 = 60 Hz).
    pub snapshot_interval: u64,
}

struct Client {
    addr: SocketAddr,
    id: PlayerId,
    name: String,
    queue: VecDeque<PlayerInput>,
    last_received: u64,
    last_applied: u64,
    last_input: PlayerInput,
    last_heard: Instant,
    pending_events: Vec<(u64, dishonored_sim::events::SimEvent)>,
}

#[derive(Debug, Default, Clone)]
pub struct ServerStats {
    pub ticks: u64,
    pub snapshots_sent: u64,
    pub inputs_starved: u64,
    pub inputs_dropped_overflow: u64,
}

pub struct GameServer<'w> {
    pub cfg: ServerConfig,
    pub sim: SimConfig,
    pub world: &'w dyn WorldQuery,
    pub socket: NetSocket,
    pub game: FfaMatch,
    clients: Vec<Client>,
    next_id: PlayerId,
    snapshot_seq: u64,
    config_hash: u64,
    pub stats: ServerStats,
    pub replay: Option<ReplayRecorder>,
    roster_this_tick: Vec<Roster>,
}

impl<'w> GameServer<'w> {
    pub fn new(cfg: ServerConfig, sim: SimConfig, world: &'w dyn WorldQuery, socket: NetSocket, spawns: Vec<SpawnPoint>) -> Self {
        let config_hash = config_hash(&sim);
        let game = FfaMatch::new(cfg.rules.clone(), spawns.clone());
        let replay = Some(ReplayRecorder::new(ReplayHeader {
            map: cfg.map.clone(),
            config_hash,
            rules: cfg.rules.clone(),
            spawns,
        }));
        Self { cfg, sim, world, socket, game, clients: Vec::new(), next_id: 1, snapshot_seq: 0, config_hash, stats: ServerStats::default(), replay, roster_this_tick: Vec::new() }
    }

    pub fn info(&self) -> ServerInfo {
        ServerInfo {
            name: self.cfg.name.clone(),
            map: self.cfg.map.clone(),
            mode: self.game.view.mode.clone(),
            players: self.clients.len() as u32,
            max_players: self.cfg.rules.max_players as u32,
            version: PROTOCOL_VERSION,
        }
    }

    pub fn player_count(&self) -> usize {
        self.clients.len()
    }

    fn handle(&mut self, from: SocketAddr, p: Packet) {
        let now = Instant::now();
        match p {
            Packet::InfoRequest { nonce } => {
                let info = self.info();
                self.socket.send(from, encode(&Packet::InfoResponse { nonce, info }));
            }
            Packet::Hello { version, name } => {
                if version != PROTOCOL_VERSION {
                    self.socket.send(from, encode(&Packet::Reject { reason: format!("protocol {version} != {PROTOCOL_VERSION}") }));
                    return;
                }
                let id = if let Some(c) = self.clients.iter_mut().find(|c| c.addr == from) {
                    c.last_heard = now;
                    c.id
                } else {
                    if self.clients.len() >= self.cfg.rules.max_players {
                        self.socket.send(from, encode(&Packet::Reject { reason: "server full".into() }));
                        return;
                    }
                    let id = self.next_id;
                    self.next_id += 1;
                    let name: String = name.chars().filter(|c| !c.is_control()).take(24).collect();
                    self.game.add_player(id, &name, &self.sim);
                    self.roster_this_tick.push(Roster::Join(id, name.clone()));
                    self.clients.push(Client {
                        addr: from,
                        id,
                        name,
                        queue: VecDeque::new(),
                        last_received: 0,
                        last_applied: 0,
                        last_input: PlayerInput::default(),
                        last_heard: now,
                        pending_events: Vec::new(),
                    });
                    id
                };
                let welcome = Packet::Welcome {
                    player_id: id,
                    map: self.cfg.map.clone(),
                    tick: self.game.world.tick,
                    tick_hz: dishonored_sim::movement::TICK_HZ,
                    config_hash: self.config_hash,
                    server_name: self.cfg.name.clone(),
                };
                self.socket.send(from, encode(&welcome));
            }
            Packet::Inputs { inputs, .. } => {
                if let Some(c) = self.clients.iter_mut().find(|c| c.addr == from) {
                    c.last_heard = now;
                    let mut fresh: Vec<PlayerInput> = inputs.into_iter().filter(|i| i.tick > c.last_received).collect();
                    fresh.sort_by_key(|i| i.tick);
                    for i in fresh {
                        c.last_received = i.tick;
                        c.queue.push_back(i);
                    }
                    while c.queue.len() > JITTER_BUFFER_MAX {
                        c.queue.pop_front();
                        self.stats.inputs_dropped_overflow += 1;
                    }
                }
            }
            Packet::Ping { client_time } => {
                if let Some(c) = self.clients.iter_mut().find(|c| c.addr == from) {
                    c.last_heard = now;
                }
                let tick = self.game.world.tick;
                self.socket.send(from, encode(&Packet::Pong { client_time, server_tick: tick }));
            }
            Packet::Disconnect => self.drop_client(from),
            _ => {}
        }
    }

    fn drop_client(&mut self, addr: SocketAddr) {
        if let Some(i) = self.clients.iter().position(|c| c.addr == addr) {
            let c = self.clients.remove(i);
            self.game.remove_player(c.id);
            self.roster_this_tick.push(Roster::Leave(c.id));
        }
    }

    /// Receive everything pending, step one tick, and send snapshots when due.
    pub fn tick(&mut self) {
        let mut buf = vec![0u8; MAX_DATAGRAM];
        while let Some((n, from)) = self.socket.recv(&mut buf) {
            if let Some(p) = decode(&buf[..n]) {
                self.handle(from, p);
            }
        }
        let now = Instant::now();
        let stale: Vec<SocketAddr> = self.clients.iter().filter(|c| now - c.last_heard > CLIENT_TIMEOUT).map(|c| c.addr).collect();
        for a in stale {
            self.drop_client(a);
        }

        // One input per client per tick from the jitter buffer.
        let mut inputs = Vec::with_capacity(self.clients.len());
        for c in self.clients.iter_mut() {
            let input = match c.queue.pop_front() {
                Some(i) => {
                    c.last_applied = i.tick;
                    c.last_input = i;
                    i
                }
                None => {
                    self.stats.inputs_starved += 1;
                    c.last_input
                }
            };
            inputs.push((c.id, input));
        }
        let tick = self.game.world.tick;
        let out = self.game.step(&inputs, self.world, &self.sim);
        let hash = state_hash(&self.game.world);
        if let Some(r) = self.replay.as_mut() {
            r.push(ReplayFrame {
                tick,
                roster: std::mem::take(&mut self.roster_this_tick),
                inputs: inputs.clone(),
                hash,
            });
        }
        for c in self.clients.iter_mut() {
            c.pending_events.extend(out.events.iter().map(|e| (tick, *e)));
        }
        self.stats.ticks += 1;

        if self.game.world.tick % self.cfg.snapshot_interval.max(1) == 0 {
            self.snapshot_seq += 1;
            for i in 0..self.clients.len() {
                let snap = Snapshot {
                    seq: self.snapshot_seq,
                    tick: self.game.world.tick,
                    ack_input: self.clients[i].last_applied,
                    players: self.game.world.players.clone(),
                    projectiles: self.game.world.projectiles.clone(),
                    matchv: self.game.view.clone(),
                    events: std::mem::take(&mut self.clients[i].pending_events),
                    state_hash: hash,
                };
                let addr = self.clients[i].addr;
                self.socket.send(addr, encode(&Packet::Snapshot(Box::new(snap))));
                self.stats.snapshots_sent += 1;
            }
        }
        self.socket.pump();
    }

    pub fn client_names(&self) -> Vec<(PlayerId, String)> {
        self.clients.iter().map(|c| (c.id, c.name.clone())).collect()
    }
}
