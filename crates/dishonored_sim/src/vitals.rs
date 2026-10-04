//! Health and mana: damage application and Dishonored's partial regeneration.
//!
//! Mana (LIKELY): after spending, regeneration starts once `ManaRegenInitialDelay` has
//! elapsed and adds `ManaRegenAmount` every `ManaRegenStepTime`, but only up to
//! `ManaRegenAdditivePortion` points above the level left after the spend (so a Blink
//! (cost 20 = portion 20) refunds fully while Windblast (50) refunds 20).
//! Health (SPECULATIVE): regenerates 1 point/s up to `HealthRegenLimit` after the delay.

use crate::events::{DamageCause, SimEvent};
use crate::state::{MoveMode, PlayerId, PlayerState};
use dishonored_ir::gameplay::VitalsConfig;

pub fn reset(p: &mut PlayerState, cfg: &VitalsConfig) {
    p.vitals.health = cfg.health_max;
    p.vitals.mana = cfg.mana_max;
    p.vitals.mana_regen_cap = cfg.mana_max;
    p.vitals.mana_idle = 0.0;
    p.vitals.health_idle = 0.0;
    p.vitals.dark_vision_remaining = 0.0;
}

/// Spend mana if available. Returns false (and spends nothing) otherwise.
pub fn spend_mana(p: &mut PlayerState, cost: f32, cfg: &VitalsConfig) -> bool {
    if p.vitals.mana + 1e-3 < cost {
        return false;
    }
    p.vitals.mana -= cost;
    p.vitals.mana_regen_cap = (p.vitals.mana + cfg.mana_regen_additive_portion).min(cfg.mana_max);
    p.vitals.mana_idle = 0.0;
    p.vitals.mana_step_timer = 0.0;
    true
}

pub fn tick(p: &mut PlayerState, cfg: &VitalsConfig, dt: f32) {
    if !p.alive {
        return;
    }
    let v = &mut p.vitals;
    v.mana_idle += dt;
    if v.mana_idle >= cfg.mana_regen_initial_delay && v.mana < v.mana_regen_cap {
        v.mana_step_timer += dt;
        while v.mana_step_timer >= cfg.mana_regen_step_time && v.mana < v.mana_regen_cap {
            v.mana_step_timer -= cfg.mana_regen_step_time;
            v.mana = (v.mana + cfg.mana_regen_amount).min(v.mana_regen_cap);
        }
    }
    v.health_idle += dt;
    if v.health_idle >= cfg.health_regen_initial_delay && v.health < cfg.health_regen_limit {
        v.health_step_timer += dt;
        while v.health_step_timer >= 1.0 && v.health < cfg.health_regen_limit {
            v.health_step_timer -= 1.0;
            v.health = (v.health + cfg.health_regen_amount).min(cfg.health_regen_limit);
        }
    }
    if v.dark_vision_remaining > 0.0 {
        v.dark_vision_remaining = (v.dark_vision_remaining - dt).max(0.0);
    }
}

/// Apply damage; emits Damage and (if lethal) Death. Returns true if the victim died.
pub fn apply_damage(
    p: &mut PlayerState,
    amount: f32,
    attacker: Option<PlayerId>,
    cause: DamageCause,
    events: &mut Vec<SimEvent>,
) -> bool {
    if !p.alive || amount <= 0.0 {
        return false;
    }
    p.vitals.health -= amount;
    p.vitals.health_idle = 0.0;
    events.push(SimEvent::Damage { victim: p.id, attacker, amount, cause });
    if p.vitals.health <= 0.0 {
        kill(p, attacker, cause, events);
        return true;
    }
    false
}

pub fn kill(p: &mut PlayerState, killer: Option<PlayerId>, cause: DamageCause, events: &mut Vec<SimEvent>) {
    if !p.alive {
        return;
    }
    p.vitals.health = 0.0;
    p.alive = false;
    p.mode = MoveMode::Dead;
    p.velocity = glam::Vec3::ZERO;
    p.vitals.dark_vision_remaining = 0.0;
    events.push(SimEvent::Death { victim: p.id, killer, cause });
}
