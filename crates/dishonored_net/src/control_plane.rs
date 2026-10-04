//! The control plane: a deliberately tiny server list ("master server").
//!
//! It only answers "which servers exist?". It holds no accounts, no matchmaking and no
//! game state. Dedicated servers POST a heartbeat every few seconds and entries expire
//! after a TTL. Clients GET the list and query each server directly over UDP for live
//! info and ping (`client::query_info`). Anyone can run their own master, or skip it
//! entirely and connect by address.
//!
//! Protocol: HTTP/1.1 + JSON over `std::net` (no async runtime, no framework).
//! - `POST /v1/heartbeat` body `{"port":27015,"info":{...ServerInfo}}`; the server
//!   address is the connection's peer IP plus `port` (no self-reported IPs, which
//!   prevents registering someone else's address).
//! - `POST /v1/deregister` body `{"port":27015}`
//! - `GET /v1/servers` returns `[{"addr":"1.2.3.4:27015","info":{...},"age_s":3.2}]`
//! - `GET /health` returns `ok`

use crate::protocol::ServerInfo;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_MASTER_PORT: u16 = 27900;
pub const MAX_BODY: usize = 4096;
pub const MAX_ENTRIES: usize = 4096;
pub const MAX_PER_IP: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub port: u16,
    pub info: ServerInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListedServer {
    pub addr: String,
    pub info: ServerInfo,
    pub age_s: f32,
}

struct Entry {
    info: ServerInfo,
    seen: Instant,
}

#[derive(Clone)]
pub struct Master {
    entries: Arc<Mutex<HashMap<SocketAddr, Entry>>>,
    ttl: Duration,
}

impl Master {
    pub fn new(ttl: Duration) -> Self {
        Self { entries: Arc::new(Mutex::new(HashMap::new())), ttl }
    }

    fn prune(&self) {
        let ttl = self.ttl;
        self.entries.lock().unwrap().retain(|_, e| e.seen.elapsed() < ttl);
    }

    pub fn heartbeat(&self, ip: IpAddr, hb: Heartbeat) -> Result<()> {
        if hb.port == 0 {
            bail!("port 0");
        }
        self.prune();
        let addr = SocketAddr::new(ip, hb.port);
        let mut m = self.entries.lock().unwrap();
        if !m.contains_key(&addr) {
            if m.len() >= MAX_ENTRIES {
                bail!("server list full");
            }
            if m.keys().filter(|a| a.ip() == ip).count() >= MAX_PER_IP {
                bail!("too many servers from this address");
            }
        }
        let mut info = hb.info;
        info.name = info.name.chars().filter(|c| !c.is_control()).take(48).collect();
        info.map = info.map.chars().take(64).collect();
        info.mode = info.mode.chars().take(16).collect();
        m.insert(addr, Entry { info, seen: Instant::now() });
        Ok(())
    }

    pub fn deregister(&self, ip: IpAddr, port: u16) {
        self.entries.lock().unwrap().remove(&SocketAddr::new(ip, port));
    }

    pub fn list(&self) -> Vec<ListedServer> {
        self.prune();
        let mut v: Vec<ListedServer> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|(a, e)| ListedServer { addr: a.to_string(), info: e.info.clone(), age_s: e.seen.elapsed().as_secs_f32() })
            .collect();
        v.sort_by(|a, b| a.addr.cmp(&b.addr));
        v
    }

    /// Serve forever on `bind` (one short-lived thread per connection).
    pub fn serve(&self, bind: &str) -> Result<()> {
        let listener = TcpListener::bind(bind).with_context(|| format!("binding {bind}"))?;
        self.serve_listener(listener)
    }

    pub fn serve_listener(&self, listener: TcpListener) -> Result<()> {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let me = self.clone();
            std::thread::spawn(move || {
                let _ = me.handle(stream);
            });
        }
        Ok(())
    }

    fn handle(&self, mut stream: TcpStream) -> Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        let peer = stream.peer_addr()?.ip();
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        let mut content_length = 0usize;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
        if content_length > MAX_BODY {
            return respond(&mut stream, 413, "text/plain", "body too large");
        }
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
        match (method.as_str(), path.as_str()) {
            ("GET", "/health") => respond(&mut stream, 200, "text/plain", "ok"),
            ("GET", "/v1/servers") => {
                let json = serde_json::to_string(&self.list())?;
                respond(&mut stream, 200, "application/json", &json)
            }
            ("POST", "/v1/heartbeat") => match serde_json::from_slice::<Heartbeat>(&body) {
                Ok(hb) => match self.heartbeat(peer, hb) {
                    Ok(()) => respond(&mut stream, 200, "application/json", &format!("{{\"ok\":true,\"ttl\":{}}}", self.ttl.as_secs())),
                    Err(e) => respond(&mut stream, 429, "text/plain", &e.to_string()),
                },
                Err(e) => respond(&mut stream, 400, "text/plain", &e.to_string()),
            },
            ("POST", "/v1/deregister") => {
                #[derive(Deserialize)]
                struct D {
                    port: u16,
                }
                if let Ok(d) = serde_json::from_slice::<D>(&body) {
                    self.deregister(peer, d.port);
                }
                respond(&mut stream, 200, "application/json", "{\"ok\":true}")
            }
            _ => respond(&mut stream, 404, "text/plain", "not found"),
        }
    }
}

fn respond(s: &mut TcpStream, code: u16, ctype: &str, body: &str) -> Result<()> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        _ => "Error",
    };
    write!(s, "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    Ok(())
}

fn master_addr(master: &str) -> Result<SocketAddr> {
    let m = master.trim_start_matches("http://").trim_end_matches('/');
    let with_port = if m.contains(':') { m.to_string() } else { format!("{m}:{DEFAULT_MASTER_PORT}") };
    with_port.to_socket_addrs()?.next().context("master address did not resolve")
}

fn http(master: &str, method: &str, path: &str, body: Option<&str>) -> Result<String> {
    let addr = master_addr(master)?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    let body = body.unwrap_or("");
    write!(s, "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp)?;
    let (head, payload) = resp.split_once("\r\n\r\n").unwrap_or((&resp, ""));
    let status: u16 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    if status != 200 {
        bail!("master returned {status}: {payload}");
    }
    Ok(payload.to_string())
}

/// Called by dedicated servers every few seconds.
pub fn send_heartbeat(master: &str, port: u16, info: &ServerInfo) -> Result<()> {
    let body = serde_json::to_string(&Heartbeat { port, info: info.clone() })?;
    http(master, "POST", "/v1/heartbeat", Some(&body)).map(|_| ())
}

pub fn send_deregister(master: &str, port: u16) -> Result<()> {
    http(master, "POST", "/v1/deregister", Some(&format!("{{\"port\":{port}}}"))).map(|_| ())
}

pub fn list_servers(master: &str) -> Result<Vec<ListedServer>> {
    Ok(serde_json::from_str(&http(master, "GET", "/v1/servers", None)?)?)
}
