//! `Init.bnk`: global settings of the Wwise project (STMG section) and the bus hierarchy.
//! Layout measured on the retail file (bank version 65); see research/dishonored/AUDIO_FORMAT.md.

use crate::bank::Bank;
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
