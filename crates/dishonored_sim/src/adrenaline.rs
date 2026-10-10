//! Blood Thirsty's adrenaline gauge, decoded from `Dishonored.exe`
//! (research/dishonored/PLAYER.md "Blood Thirsty"):
//!
//! - gain (0x00AAC510): only with the power owned (and `m_bAdrenalineEnabled`); the value grows
//!   by the amount and is capped at `(int)Attribute_AdrenalineMax` (250; level 2 lowers it by
//!   20 % to 200), where the gauge counts as full (`m_bNeedAdrenalineEffect`); each gain resets the
//!   idle time;
//! - sources: a sword hit gives damage x `m_fAdrenalineMultOnHit` (2.0, 0x00C0A110); an
//!   assassination the victim's health x 1.25, a drop assassination x 1.5 (0x00C09640,
//!   0x00C0D9F0, 0x00C0DFD0); a won parry `m_fAdrenalineAddedParryWin` (20); damage taken x
//!   `Attribute_AdrenalineMultWhenTakingDamage` (0 for Corvo, 0x00AB0A30); a rat kill
//!   `Attribute_AdrenalineOnRatKill` (0x00AACE60);
//! - tick (0x00AB2120): after `Attribute_AdrenalineCooldown` (10 s) without a gain, the value
//!   burns at `Attribute_AdrenalineBurnRate` (1/s);
//! - with a full gauge the attack press asks the sword's Bash slot first: the fatality context
//!   as an adrenaline finisher (`crate::fatality`, 0x00C10EF0) kills the NPC in reach with a
//!   finisher, spends the gauge and adds `m_fAdrenalineMultOnHit` x its health. Players are not
//!   finished (the context needs an NPC target, U-113).

use crate::events::SimEvent;
use crate::state::{PlayerState, power};
use dishonored_ir::gameplay::GameplayConfig;

pub fn max(p: &PlayerState, cfg: &GameplayConfig) -> f32 {
    crate::powers::attribute(p, cfg, "Attribute_AdrenalineMax", cfg.adrenaline.max).trunc()
}

fn owned(p: &PlayerState) -> bool {
    p.inv.power_levels[power::BLOOD_THIRSTY] > 0
}

/// Add adrenaline (0x00AAC510).
pub fn gain(p: &mut PlayerState, amount: f32, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    if !owned(p) || !p.alive {
        p.adrenaline = Default::default();
        return;
    }
    let m = max(p, cfg);
    p.adrenaline.value += amount;
    if p.adrenaline.value >= m {
        p.adrenaline.value = m;
        if !p.adrenaline.full {
            p.adrenaline.full = true;
            events.push(SimEvent::AdrenalineFull { id: p.id });
        }
    }
    p.adrenaline.idle = 0.0;
}

/// The gauge is full (0x00AA21E0: owned and `(int)AdrenalineMax <= (int)value`).
pub fn is_full(p: &PlayerState, cfg: &GameplayConfig) -> bool {
    owned(p) && max(p, cfg) <= p.adrenaline.value.trunc()
}

/// Per-tick burn (0x00AB2120).
pub fn tick(p: &mut PlayerState, cfg: &GameplayConfig, dt: f32) {
    if !owned(p) {
        p.adrenaline = Default::default();
        return;
    }
    p.adrenaline.idle += dt;
    let cooldown = crate::powers::attribute(p, cfg, "Attribute_AdrenalineCooldown", cfg.adrenaline.cooldown);
    if p.adrenaline.idle > cooldown {
        let burn = crate::powers::attribute(p, cfg, "Attribute_AdrenalineBurnRate", cfg.adrenaline.burn_rate);
        p.adrenaline.full = false;
        p.adrenaline.value = (p.adrenaline.value - burn * dt).max(0.0);
    }
}

/// Spend the gauge on a finisher.
pub fn spend(p: &mut PlayerState, events: &mut Vec<SimEvent>) {
    p.adrenaline.value = 0.0;
    p.adrenaline.full = false;
    p.adrenaline.idle = 0.0;
    events.push(SimEvent::AdrenalineAttack { id: p.id });
}
