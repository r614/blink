//! Wire protocol. Every datagram is `MAGIC (u32) | bincode(Packet)`.
//!
//! Model (Quake/Source style): clients number their inputs with a sequence and send the
//! last few unacknowledged inputs every packet (loss tolerance). The server consumes one
//! input per client per tick from a small jitter buffer and reports, in each snapshot,
//! the last input sequence it applied. Clients re-simulate their unacknowledged inputs on
//! top of the authoritative state (prediction + reconciliation) using the same
//! `dishonored_sim` code.

use dishonored_multiplayer::MatchView;
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{PlayerId, PlayerState, ProjectileState, WindblastState};
use serde::{Deserialize, Serialize};

pub const MAGIC: u32 = 0x4449_5331; // "DIS1"
pub const PROTOCOL_VERSION: u32 = 8; // 4: PlayerState.climb, SimEvent::Climb / ClimbSound; 5: LeftHandState.dispersion; 6: MissionView.doors / movers; 7: SwordState.swings / swing_clip; 8: Look.tweak (NPC names)
pub const MAX_DATAGRAM: usize = 60_000;
/// Inputs resent per packet.
pub const INPUT_REDUNDANCY: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Packet {
    // ---- connectionless ----
    /// Server browser query; answered without a connection.
    InfoRequest { nonce: u64 },
    InfoResponse { nonce: u64, info: ServerInfo },

    // ---- client -> server ----
    Hello { version: u32, name: String },
    Inputs { inputs: Vec<PlayerInput>, ack_snapshot: u64 },
    Ping { client_time: f64 },
    Disconnect,

    // ---- server -> client ----
    Welcome { player_id: PlayerId, map: String, tick: u64, tick_hz: u32, config_hash: u64, server_name: String },
    Reject { reason: String },
    Snapshot(Box<Snapshot>),
    Pong { client_time: f64, server_tick: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ServerInfo {
    pub name: String,
    pub map: String,
    pub mode: String,
    pub players: u32,
    pub max_players: u32,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub seq: u64,
    pub tick: u64,
    /// Last input sequence the server applied for the receiving client.
    pub ack_input: u64,
    pub players: Vec<PlayerState>,
    pub projectiles: Vec<ProjectileState>,
    /// Windblasts in flight (warmup and expanding front).
    pub windblasts: Vec<WindblastState>,
    /// Pickups, held abstract items, Spring Razors, Bend Time and swarms.
    pub extras: dishonored_sim::state::WorldExtras,
    pub matchv: MatchView,
    /// Private contract info for the receiving player only (Contracts mode).
    pub private: Option<dishonored_multiplayer::contracts::TargetInfo>,
    /// Gameplay events since the previous snapshot (tick, event).
    pub events: Vec<(u64, SimEvent)>,
    /// Authoritative state hash at `tick` (desync detection).
    pub state_hash: u64,
}

pub fn encode(p: &Packet) -> Vec<u8> {
    let mut out = MAGIC.to_le_bytes().to_vec();
    out.extend(bincode::serde::encode_to_vec(p, bincode::config::standard()).expect("encode"));
    out
}

pub fn decode(b: &[u8]) -> Option<Packet> {
    if b.len() < 4 || u32::from_le_bytes(b[..4].try_into().ok()?) != MAGIC {
        return None;
    }
    bincode::serde::decode_from_slice(&b[4..], bincode::config::standard()).ok().map(|(p, _)| p)
}

/// Stable hash of the gameplay config so clients refuse to predict with different data.
pub fn config_hash(cfg: &dishonored_sim::SimConfig) -> u64 {
    let s = serde_json::to_string(&cfg.gameplay).unwrap_or_default() + &serde_json::to_string(&cfg.tuning).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
