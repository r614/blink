//! Non-blocking UDP socket with an optional link conditioner (latency, jitter, loss)
//! applied on send. Deterministic for a given seed so network tests are reproducible.

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LinkConditions {
    /// One-way latency added to every outgoing datagram.
    pub latency_ms: f32,
    pub jitter_ms: f32,
    /// 0..1 probability of dropping a datagram.
    pub loss: f32,
}

impl LinkConditions {
    pub fn rtt(rtt_ms: f32, loss: f32) -> Self {
        Self { latency_ms: rtt_ms * 0.5, jitter_ms: rtt_ms * 0.05, loss }
    }
}

struct Delayed {
    due: Instant,
    to: SocketAddr,
    bytes: Vec<u8>,
}

pub struct NetSocket {
    pub udp: UdpSocket,
    pub conditions: LinkConditions,
    rng: u64,
    queue: VecDeque<Delayed>,
    pub sent_bytes: u64,
    pub recv_bytes: u64,
    pub dropped: u64,
}

impl NetSocket {
    pub fn bind(addr: &str) -> std::io::Result<Self> {
        let udp = UdpSocket::bind(addr)?;
        udp.set_nonblocking(true)?;
        Ok(Self { udp, conditions: LinkConditions::default(), rng: 0x9E37_79B9_7F4A_7C15, queue: VecDeque::new(), sent_bytes: 0, recv_bytes: 0, dropped: 0 })
    }

    pub fn with_conditions(mut self, c: LinkConditions, seed: u64) -> Self {
        self.conditions = c;
        self.rng ^= seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1;
        self
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.udp.local_addr().expect("local addr")
    }

    fn rand(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        ((self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
    }

    pub fn send(&mut self, to: SocketAddr, bytes: Vec<u8>) {
        let c = self.conditions;
        if c.loss > 0.0 && self.rand() < c.loss {
            self.dropped += 1;
            return;
        }
        let delay = c.latency_ms + (self.rand() * 2.0 - 1.0) * c.jitter_ms;
        if delay <= 0.0 {
            self.sent_bytes += bytes.len() as u64;
            let _ = self.udp.send_to(&bytes, to);
            return;
        }
        let due = Instant::now() + Duration::from_secs_f32(delay / 1000.0);
        // Keep the queue ordered by due time (jitter may reorder packets, like real links).
        let pos = self.queue.iter().position(|d| d.due > due).unwrap_or(self.queue.len());
        self.queue.insert(pos, Delayed { due, to, bytes });
    }

    /// Flush delayed datagrams whose time has come.
    pub fn pump(&mut self) {
        let now = Instant::now();
        while self.queue.front().is_some_and(|d| d.due <= now) {
            let d = self.queue.pop_front().unwrap();
            self.sent_bytes += d.bytes.len() as u64;
            let _ = self.udp.send_to(&d.bytes, d.to);
        }
    }

    pub fn recv(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
        match self.udp.recv_from(buf) {
            Ok((n, a)) => {
                self.recv_bytes += n as u64;
                Some((n, a))
            }
            Err(_) => None,
        }
    }
}
