//! Behaviour traces: per-tick channel values plus ordered events, the same shape for the
//! original (ProcessEvent / Kismet / state samples from the hook) and the port (the headless
//! runner). Stored in the oracle cache under [`crate::cache::Probe::Trace`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    /// `pawn.z`, `pawn.vx`, `npc.<name>.awareness`...
    pub name: String,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub tick: u32,
    /// Parallel to [`Trace::channels`]; NaN: not sampled this tick.
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub tick: u32,
    /// `ProcessEvent`, `Kismet`, `Sim`...
    pub kind: String,
    pub object: String,
    pub name: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub scenario: String,
    /// `original` or `port`.
    pub side: String,
    pub tick_hz: u32,
    pub channels: Vec<Channel>,
    pub frames: Vec<Frame>,
    pub events: Vec<TraceEvent>,
}

impl Trace {
    pub fn new(scenario: &str, side: &str, tick_hz: u32, channels: &[(&str, &str)]) -> Self {
        Self {
            scenario: scenario.into(),
            side: side.into(),
            tick_hz,
            channels: channels.iter().map(|(n, u)| Channel { name: n.to_string(), unit: u.to_string() }).collect(),
            frames: Vec::new(),
            events: Vec::new(),
        }
    }
    pub fn channel(&self, name: &str) -> Option<usize> {
        self.channels.iter().position(|c| c.name == name)
    }
    pub fn push(&mut self, tick: u32, values: Vec<f64>) {
        debug_assert_eq!(values.len(), self.channels.len());
        self.frames.push(Frame { tick, values });
    }
    /// (tick, value) of a channel where sampled.
    pub fn series(&self, name: &str) -> Vec<(u32, f64)> {
        let Some(c) = self.channel(name) else { return Vec::new() };
        self.frames.iter().filter_map(|f| f.values.get(c).filter(|v| !v.is_nan()).map(|v| (f.tick, *v))).collect()
    }
}
