//! Predicting client. Runs the same `dishonored_sim` as the server: local inputs are
//! applied immediately to a predicted world; authoritative snapshots rebase that world
//! and unacknowledged inputs are replayed on top (reconciliation). Remote players are
//! presented by interpolating between snapshots.

use crate::protocol::*;
use crate::socket::NetSocket;
use dishonored_multiplayer::MatchView;
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{PlayerId, PlayerState, WorldState};
use dishonored_sim::{SimConfig, step_world};
use dishonored_world::WorldQuery;
use glam::Vec3;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Connected,
    Rejected,
}

#[derive(Debug, Clone, Default)]
pub struct NetMetrics {
    pub rtt_ms: f32,
    pub snapshots: u64,
    pub snapshot_age_ms: f32,
    /// |predicted - authoritative| for the acknowledged input (uu).
    pub prediction_error: f32,
    pub prediction_error_max: f32,
    pub prediction_error_sum: f64,
    pub prediction_error_samples: u64,
    /// How far the current predicted position moved when rebasing (uu).
    pub correction: f32,
    pub correction_max: f32,
    pub corrections_over_10uu: u64,
    pub pending_inputs: usize,
    /// Discontinuities (respawn, server teleport) excluded from the error statistics.
    pub resyncs: u64,
    pub server_tick: u64,
}

struct Interp {
    tick: u64,
    players: Vec<PlayerState>,
}

pub struct NetClient {
    pub socket: NetSocket,
    pub server: SocketAddr,
    pub name: String,
    pub state: ConnState,
    pub player_id: Option<PlayerId>,
    pub reject_reason: Option<String>,
    pub map: String,
    pub server_config_hash: u64,
    /// Locally predicted world (own player predicted, others extrapolated).
    pub predicted: WorldState,
    pub matchv: MatchView,
    /// Private Contracts info for this player (None in FFA).
    pub target_info: Option<dishonored_multiplayer::contracts::TargetInfo>,
    pub metrics: NetMetrics,
    /// Authoritative events received (tick, event).
    pub events: VecDeque<(u64, SimEvent)>,
    /// Predicted events for the local player's own actions this frame.
    pub local_events: Vec<SimEvent>,
    next_seq: u64,
    pending: VecDeque<PlayerInput>,
    /// Predicted own state after each input seq (for error measurement).
    history: VecDeque<(u64, PlayerState)>,
    last_snapshot_seq: u64,
    last_snapshot_at: Option<Instant>,
    last_hello: Option<Instant>,
    last_ping: Option<Instant>,
    started: Instant,
    interp: VecDeque<Interp>,
    pub last_server_hash: u64,
}

impl NetClient {
    pub fn new(socket: NetSocket, server: SocketAddr, name: &str) -> Self {
        Self {
            socket,
            server,
            name: name.to_string(),
            state: ConnState::Connecting,
            player_id: None,
            reject_reason: None,
            map: String::new(),
            server_config_hash: 0,
            predicted: WorldState::default(),
            matchv: MatchView::default(),
            target_info: None,
            metrics: NetMetrics::default(),
            events: VecDeque::new(),
            local_events: Vec::new(),
            next_seq: 1,
            pending: VecDeque::new(),
            history: VecDeque::new(),
            last_snapshot_seq: 0,
            last_snapshot_at: None,
            last_hello: None,
            last_ping: None,
            started: Instant::now(),
            interp: VecDeque::new(),
            last_server_hash: 0,
        }
    }

    fn now_s(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub fn me(&self) -> Option<&PlayerState> {
        self.player_id.and_then(|id| self.predicted.player(id))
    }

    /// Receive and process datagrams; (re)send Hello / Ping as needed.
    pub fn poll(&mut self, world: &dyn WorldQuery, sim: &SimConfig) {
        let now = Instant::now();
        if self.state == ConnState::Connecting && self.last_hello.is_none_or(|t| now - t > std::time::Duration::from_millis(250)) {
            let hello = encode(&Packet::Hello { version: PROTOCOL_VERSION, name: self.name.clone() });
            self.socket.send(self.server, hello);
            self.last_hello = Some(now);
        }
        if self.state == ConnState::Connected && self.last_ping.is_none_or(|t| now - t > std::time::Duration::from_millis(500)) {
            let t = self.now_s();
            self.socket.send(self.server, encode(&Packet::Ping { client_time: t }));
            self.last_ping = Some(now);
        }
        let mut buf = vec![0u8; MAX_DATAGRAM];
        let mut newest: Option<Box<Snapshot>> = None;
        while let Some((n, from)) = self.socket.recv(&mut buf) {
            if from != self.server {
                continue;
            }
            match decode(&buf[..n]) {
                Some(Packet::Welcome { player_id, map, config_hash, .. }) => {
                    self.player_id = Some(player_id);
                    self.map = map;
                    self.server_config_hash = config_hash;
                    self.state = ConnState::Connected;
                }
                Some(Packet::Reject { reason }) => {
                    self.state = ConnState::Rejected;
                    self.reject_reason = Some(reason);
                }
                Some(Packet::Pong { client_time, .. }) => {
                    let rtt = ((self.now_s() - client_time) * 1000.0) as f32;
                    self.metrics.rtt_ms = if self.metrics.rtt_ms == 0.0 { rtt } else { self.metrics.rtt_ms * 0.8 + rtt * 0.2 };
                }
                Some(Packet::Snapshot(s)) => {
                    // Events must not be lost even when an older snapshot is superseded.
                    for e in &s.events {
                        self.events.push_back(*e);
                    }
                    if s.seq > self.last_snapshot_seq && newest.as_ref().is_none_or(|n| s.seq > n.seq) {
                        newest = Some(s);
                    }
                }
                _ => {}
            }
        }
        while self.events.len() > 512 {
            self.events.pop_front();
        }
        if let Some(s) = newest {
            self.apply_snapshot(*s, world, sim);
        }
        self.socket.pump();
    }

    fn apply_snapshot(&mut self, s: Snapshot, world: &dyn WorldQuery, sim: &SimConfig) {
        let now = Instant::now();
        if let Some(t) = self.last_snapshot_at {
            self.metrics.snapshot_age_ms = (now - t).as_secs_f32() * 1000.0;
        }
        self.last_snapshot_at = Some(now);
        self.last_snapshot_seq = s.seq;
        self.metrics.snapshots += 1;
        self.metrics.server_tick = s.tick;
        // The NPC look table only rides on some snapshots (it rarely changes): keep the last one.
        let looks = std::mem::take(&mut self.matchv.appearances);
        self.matchv = s.matchv.clone();
        if self.matchv.appearances.is_empty() {
            self.matchv.appearances = looks;
        }
        self.target_info = s.private.clone();
        self.last_server_hash = s.state_hash;
        self.interp.push_back(Interp { tick: s.tick, players: s.players.clone() });
        while self.interp.len() > 32 {
            self.interp.pop_front();
        }

        let Some(me) = self.player_id else { return };
        let before = self.me().map(|p| p.position);

        // Prediction error at the acknowledged input.
        if let Some(auth) = s.players.iter().find(|p| p.id == me) {
            if let Some((_, pred)) = self.history.iter().find(|(seq, _)| *seq == s.ack_input) {
                let err = pred.position.distance(auth.position);
                let m = &mut self.metrics;
                // Respawns and other server-side teleports are resyncs, not mispredictions.
                if err > 500.0 || pred.alive != auth.alive {
                    m.resyncs += 1;
                } else {
                m.prediction_error = err;
                m.prediction_error_max = m.prediction_error_max.max(err);
                m.prediction_error_sum += err as f64;
                m.prediction_error_samples += 1;
                }
            }
        }
        while self.pending.front().is_some_and(|i| i.tick <= s.ack_input) {
            self.pending.pop_front();
        }
        while self.history.front().is_some_and(|(seq, _)| *seq <= s.ack_input) {
            self.history.pop_front();
        }

        // Rebase and replay unacknowledged inputs.
        self.predicted = WorldState { tick: s.tick, players: s.players, projectiles: s.projectiles, next_projectile_id: 0, windblasts: s.windblasts };
        self.history.clear();
        let pending: Vec<PlayerInput> = self.pending.iter().copied().collect();
        for i in pending {
            step_world(&mut self.predicted, &[(me, i)], world, sim);
            if let Some(p) = self.predicted.player(me) {
                self.history.push_back((i.tick, *p));
            }
        }
        if let (Some(b), Some(a)) = (before, self.me().map(|p| p.position)) {
            let c = b.distance(a);
            let m = &mut self.metrics;
            m.correction = c;
            if c <= 500.0 {
                m.correction_max = m.correction_max.max(c);
            }
            if c > 500.0 {
                m.correction = 0.0;
            } else if c > 10.0 {
                m.corrections_over_10uu += 1;
            }
        }
        self.metrics.pending_inputs = self.pending.len();
    }

    /// Apply one local input (one simulation tick) and send it.
    pub fn send_input(&mut self, mut input: PlayerInput, world: &dyn WorldQuery, sim: &SimConfig) {
        if self.state != ConnState::Connected {
            return;
        }
        let Some(me) = self.player_id else { return };
        input.tick = self.next_seq;
        self.next_seq += 1;
        self.pending.push_back(input);
        while self.pending.len() > 256 {
            self.pending.pop_front();
        }
        let out = step_world(&mut self.predicted, &[(me, input)], world, sim);
        self.local_events = out
            .events
            .into_iter()
            .filter(|e| match e {
                SimEvent::Jumped { id } | SimEvent::BlinkStart { id, .. } | SimEvent::SwordSwing { id } | SimEvent::Fired { id, .. } | SimEvent::Landed { id, .. } => *id == me,
                _ => false,
            })
            .collect();
        if let Some(p) = self.predicted.player(me) {
            self.history.push_back((input.tick, *p));
            while self.history.len() > 256 {
                self.history.pop_front();
            }
        }
        let start = self.pending.len().saturating_sub(INPUT_REDUNDANCY);
        let inputs: Vec<PlayerInput> = self.pending.iter().skip(start).copied().collect();
        let pkt = Packet::Inputs { inputs, ack_snapshot: self.last_snapshot_seq };
        self.socket.send(self.server, encode(&pkt));
        self.socket.pump();
    }

    /// Interpolated remote player positions, `delay_ticks` behind the newest snapshot.
    pub fn remote_players(&self, delay_ticks: f32) -> Vec<PlayerState> {
        let Some(newest) = self.interp.back() else { return Vec::new() };
        let target = newest.tick as f32 - delay_ticks;
        let mut out = Vec::new();
        let (mut a, mut b) = (None, None);
        for w in self.interp.iter() {
            if (w.tick as f32) <= target {
                a = Some(w);
            } else {
                b = Some(w);
                break;
            }
        }
        let (a, b) = match (a, b) {
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) => (a, a),
            (None, Some(b)) => (b, b),
            _ => return out,
        };
        let span = (b.tick - a.tick).max(1) as f32;
        let t = ((target - a.tick as f32) / span).clamp(0.0, 1.0);
        for pa in &a.players {
            if Some(pa.id) == self.player_id {
                continue;
            }
            let mut p = *pa;
            if let Some(pb) = b.players.iter().find(|x| x.id == pa.id) {
                // Do not interpolate across teleports (respawn, long Blink).
                if pa.position.distance(pb.position) < 2000.0 {
                    p.position = pa.position.lerp(pb.position, t);
                } else {
                    p.position = if t < 0.5 { pa.position } else { pb.position };
                }
                p.yaw = lerp_angle(pa.yaw, pb.yaw, t);
                p.pitch = pa.pitch + (pb.pitch - pa.pitch) * t;
                p.alive = pb.alive;
                p.mode = pb.mode;
                p.sword = pb.sword;
                p.crouched = pb.crouched;
            }
            out.push(p);
        }
        out
    }

    pub fn disconnect(&mut self) {
        self.socket.send(self.server, encode(&Packet::Disconnect));
        self.socket.pump();
    }

    pub fn own_position(&self) -> Option<Vec3> {
        self.me().map(|p| p.position)
    }
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let mut d = (b - a) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    a + d * t
}

/// Query a server's info over UDP (server browser). Returns (info, rtt ms).
pub fn query_info(addr: SocketAddr, timeout: std::time::Duration) -> Option<(ServerInfo, f32)> {
    let mut s = NetSocket::bind("0.0.0.0:0").ok()?;
    let nonce = 0x5eed_u64 ^ (Instant::now().elapsed().as_nanos() as u64);
    let t0 = Instant::now();
    s.send(addr, encode(&Packet::InfoRequest { nonce }));
    let mut buf = vec![0u8; MAX_DATAGRAM];
    while t0.elapsed() < timeout {
        if let Some((n, from)) = s.recv(&mut buf) {
            if from == addr {
                if let Some(Packet::InfoResponse { nonce: r, info }) = decode(&buf[..n]) {
                    if r == nonce {
                        return Some((info, t0.elapsed().as_secs_f32() * 1000.0));
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    None
}
