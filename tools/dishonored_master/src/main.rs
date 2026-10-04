//! `dishonored_master [--bind 0.0.0.0:27900] [--ttl 30]`
//!
//! The entire control plane: an in-memory list of dedicated servers kept alive by
//! heartbeats. No database, no accounts, no game state. Restarting it loses nothing that
//! servers will not re-announce within one heartbeat interval.
use anyhow::Result;
use dishonored_net::control_plane::{DEFAULT_MASTER_PORT, Master};
use std::time::Duration;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let bind = get("--bind").unwrap_or_else(|| format!("0.0.0.0:{DEFAULT_MASTER_PORT}"));
    let ttl: u64 = get("--ttl").and_then(|s| s.parse().ok()).unwrap_or(30);
    println!("dishonored_master listening on {bind} (ttl {ttl}s)");
    Master::new(Duration::from_secs(ttl)).serve(&bind)
}
