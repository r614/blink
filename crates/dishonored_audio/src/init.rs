//! `Init.bnk`: global settings of the Wwise project (STMG section) and the bus hierarchy.
//! Layout measured on the retail file (bank version 65); see research/dishonored/AUDIO_FORMAT.md.

use crate::bank::Bank;
use crate::hirc::{Bus, CurvePoint, Fx, Hierarchy, Node};
use anyhow::{Context, Result, ensure};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct StateGroup {
    pub id: u32,
    /// Default transition time between states (ms) for state-driven property offsets.
    pub default_transition_ms: u32,
    /// (from, to, time ms).
    pub transitions: Vec<(u32, u32, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct SwitchGroup {
    pub id: u32,
    /// Game parameter driving the switch (switch groups mapped on an RTPC).
    pub rtpc: u32,
    /// (parameter value, switch id, interpolation).
    pub points: Vec<(f32, u32, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct InitBank {
    pub volume_threshold: f32,
    pub max_voices: u16,
    pub state_groups: Vec<StateGroup>,
    pub switch_groups: Vec<SwitchGroup>,
    /// Game parameter id -> default value.
    pub params: HashMap<u32, f32>,
    /// Bus hierarchy (HIRC type 8), by id.
    pub buses: HashMap<u32, Bus>,
    /// Effect sharesets (environments: RoomVerb, Matrix Reverb, Delay...), by id.
    pub fx: HashMap<u32, Fx>,
    /// State objects of the buses: id -> property offsets.
    pub state_props: HashMap<u32, Vec<(u8, f32)>>,
    /// ENVS: obstruction volume, obstruction LPF, occlusion volume, occlusion LPF curves
    /// (enabled, scaling, points over 0..100 %).
    pub env_curves: Vec<(bool, u8, Vec<CurvePoint>)>,
    /// HIRC objects of Init.bnk that did not decode exactly.
    pub inexact: usize,
}

struct R<'a> {
    b: &'a [u8],
    o: usize,
}

impl R<'_> {
    fn u16(&mut self) -> Result<u16> {
        let v = u16::from_le_bytes(self.b.get(self.o..self.o + 2).context("STMG truncated")?.try_into()?);
        self.o += 2;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32> {
        let v = u32::from_le_bytes(self.b.get(self.o..self.o + 4).context("STMG truncated")?.try_into()?);
        self.o += 4;
        Ok(v)
    }
    fn f32(&mut self) -> Result<f32> {
        self.u32().map(f32::from_bits)
    }
    fn count(&mut self) -> Result<usize> {
        let n = self.u32()? as usize;
        ensure!(n < 1 << 16, "implausible count {n}");
        Ok(n)
    }
}

impl InitBank {
    pub fn parse(bank: &Bank) -> Result<Self> {
        let mut r = R { b: &bank.stmg, o: 0 };
        let mut i = InitBank { volume_threshold: r.f32()?, max_voices: r.u16()?, ..Default::default() };
        for _ in 0..r.count()? {
            let mut g = StateGroup { id: r.u32()?, default_transition_ms: r.u32()?, ..Default::default() };
            for _ in 0..r.count()? {
                g.transitions.push((r.u32()?, r.u32()?, r.u32()?));
            }
            i.state_groups.push(g);
        }
        for _ in 0..r.count()? {
            let mut g = SwitchGroup { id: r.u32()?, rtpc: r.u32()?, ..Default::default() };
            for _ in 0..r.count()? {
                g.points.push((r.f32()?, r.u32()?, r.u32()?));
            }
            i.switch_groups.push(g);
        }
        for _ in 0..r.count()? {
            let id = r.u32()?;
            i.params.insert(id, r.f32()?);
        }
        ensure!(r.o == r.b.len(), "STMG: {} of {} bytes decoded", r.o, r.b.len());
        let h = Hierarchy::from_bank(bank);
        i.inexact = h.inexact.len();
        for (id, n) in h.nodes {
            match n {
                Node::Bus(b) => {
                    i.buses.insert(id, b);
                }
                Node::Fx(f) => {
                    i.fx.insert(id, f);
                }
                Node::State { props } => {
                    i.state_props.insert(id, props);
                }
                _ => {}
            }
        }
        // ENVS: four curves {u8 enabled, u8 scaling, u16 n, {f32 x, f32 y, u32 interp}}.
        let mut e = R { b: &bank.envs, o: 0 };
        while e.o + 4 <= e.b.len() {
            let en = e.b[e.o] != 0;
            let sc = e.b[e.o + 1];
            e.o += 2;
            let n = e.u16()? as usize;
            let mut pts = Vec::with_capacity(n);
            for _ in 0..n {
                pts.push(CurvePoint { x: e.f32()?, y: e.f32()?, interp: e.u32()? });
            }
            i.env_curves.push((en, sc, pts));
        }
        Ok(i)
    }

    pub fn load(root: &std::path::Path) -> Result<Self> {
        for d in crate::library::pck_dirs(root) {
            let p = d.join("Init.bnk");
            if p.is_file() {
                return Self::parse(&Bank::parse(&std::fs::read(&p)?)?);
            }
        }
        anyhow::bail!("Init.bnk not found")
    }
}

impl InitBank {
    /// The bus and its ancestors up to the master bus (the bus first).
    pub fn bus_chain(&self, bus: u32) -> Vec<u32> {
        let mut v = Vec::new();
        let mut b = bus;
        while b != 0 && !v.contains(&b) {
            let Some(x) = self.buses.get(&b) else { break };
            v.push(b);
            b = x.parent;
        }
        v
    }

    /// The master bus (the one without a parent).
    pub fn master_bus(&self) -> Option<u32> {
        self.buses.iter().find(|(_, b)| b.parent == 0).map(|(id, _)| *id)
    }
}
