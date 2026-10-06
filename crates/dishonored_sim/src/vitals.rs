//! Health and mana: damage application and Dishonored's partial regeneration, as decoded
//! from `Dishonored.exe` (research/dishonored/PLAYER.md, "Vitals").
//!
//! Mana (`ADishonoredPlayerPawn` 0x00AA93E0 spend, 0x00AA7A20 tick): a spend sets the
//! regen pool to `ManaRegenAdditivePortion` and the timer to `ManaRegenInitialDelay`. Each
//! tick the timer counts down; when the pool is empty and mana is below the portion, the
//! pool becomes `portion - mana` (mana always refills to the portion). While the pool is
//! positive and the timer is below zero, `min(ManaRegenAmount, pool)` points are added and
//! the timer is set (not added) to `ManaRegenStepTime`.
//!
//! Health (`ADishonoredPawn` 0x00B57FD0 tick, damage adjustment 0x00B5CCD0): taking damage
//! that leaves health below `HealthRegenLimit` sets the countdown to
//! `HealthRegenInitialDelay`; while health is below the limit the countdown runs and every
//! time it reaches zero `HealthRegenAmount` is added (capped at the limit) and
//! `HealthRegenRate` seconds are added back to the countdown.

use crate::events::{DamageCause, SimEvent};
use crate::state::{MoveMode, PlayerId, PlayerState};
use dishonored_ir::gameplay::VitalsConfig;

pub fn reset(p: &mut PlayerState, cfg: &VitalsConfig) {
    p.vitals.health = cfg.health_max;
    p.vitals.mana = cfg.mana_max;
    p.vitals.mana_regen_pool = 0.0;
    p.vitals.mana_regen_timer = 0.0;
    p.vitals.mana_regen_cap = cfg.mana_max;
    p.vitals.health_regen_timer = 0.0;
    p.vitals.dark_vision_remaining = 0.0;
    p.dark = Default::default();
}

/// Spend mana if available. Returns false (and spends nothing) otherwise.
pub fn spend_mana(p: &mut PlayerState, cost: f32, cfg: &VitalsConfig) -> bool {
    if p.vitals.mana + 1e-3 < cost {
        return false;
    }
    // `Attribute_ChanceUseNoManaForSpell` (a bone charm): the spend is skipped when the roll is
    // below it (0x00AA93E0).
    if p.free_spell_chance > 0.0 && p.roll() < p.free_spell_chance {
        return true;
    }
    let v = &mut p.vitals;
    v.mana -= cost;
    v.mana_regen_pool = cfg.mana_regen_additive_portion;
    v.mana_regen_timer = cfg.mana_regen_initial_delay;
    v.mana_regen_cap = (v.mana + v.mana_regen_pool).min(cfg.mana_max);
    true
}

pub fn tick(p: &mut PlayerState, cfg: &VitalsConfig, dt: f32) {
    if !p.alive {
        return;
    }
    let v = &mut p.vitals;
    // Mana.
    v.mana_regen_timer -= dt;
    let portion = cfg.mana_regen_additive_portion;
    if v.mana_regen_pool <= 0.0 && v.mana < portion {
        v.mana_regen_pool = portion - v.mana;
    }
    v.mana_regen_pool = v.mana_regen_pool.min(portion);
    if v.mana_regen_pool > 0.0 && v.mana_regen_timer < 0.0 {
        let amount = cfg.mana_regen_amount.min(v.mana_regen_pool);
        v.mana = (v.mana + amount).min(cfg.mana_max);
        v.mana_regen_pool -= amount;
        v.mana_regen_timer = cfg.mana_regen_step_time;
    }
    v.mana_regen_cap = (v.mana + v.mana_regen_pool).min(cfg.mana_max);
    // Health.
    if cfg.health_regen_amount > 0.0 && v.health < cfg.health_regen_limit {
        v.health_regen_timer -= dt;
        while v.health_regen_timer <= 0.0 && v.health < cfg.health_regen_limit {
            v.health = (v.health + cfg.health_regen_amount).min(cfg.health_regen_limit);
            v.health_regen_timer += cfg.health_regen_rate.max(1e-3);
        }
    }
}

/// `DisTweaks_Armor.m_ProtectedSide` (`eDisArmorProtectionDirection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmorSide {
    All,
    Front,
    Back,
}

/// Whether an NPC armour piece absorbs a hit (the damage filter 0x00B78610, run for each
/// `m_PotentialNPCArmor` piece the pawn still wears): the hit region must be
/// `m_ProtectedBodyRegion`, the side must match (`facing_dot_dir` = the pawn's facing . the hit's
/// direction: Front covers hits travelling against the facing, < 0; Back the others), and the
/// damage type must be in `m_DamageTypesToAbsorb` (`type_listed`); the damage then becomes 0.
pub fn armor_absorbs(region_match: bool, side: ArmorSide, facing_dot_dir: f32, type_listed: bool) -> bool {
    let side_ok = match side {
        ArmorSide::All => true,
        ArmorSide::Front => facing_dot_dir < 0.0,
        ArmorSide::Back => facing_dot_dir >= 0.0,
    };
    region_match && side_ok && type_listed
}

/// Apply damage; emits Damage and (if lethal) Death. Returns true if the victim died.
/// Damage is whole points (`TakeDamage(int Damage, ...)`): callers round as the original
/// does; this truncates what remains.
pub fn apply_damage(
    p: &mut PlayerState,
    amount: f32,
    attacker: Option<PlayerId>,
    cause: DamageCause,
    events: &mut Vec<SimEvent>,
    cfg: &VitalsConfig,
) -> bool {
    let mut amount = amount.trunc();
    if !p.alive || amount <= 0.0 {
        return false;
    }
    // The player's armour (0x00AB0A30): `damage - (int)Attribute_DamageReduction`, at least 1, for
    // damage types with `m_bApplyDamageReduction` (all but bites, electricity, plague, whiskey fire,
    // music and energy of the base game's types).
    if p.damage_reduction > 0.0 && applies_damage_reduction(cause) {
        amount = (amount - p.damage_reduction).max(1.0);
    }
    if cfg.health_regen_amount > 0.0 && (p.vitals.health - amount).max(0.0) < cfg.health_regen_limit {
        p.vitals.health_regen_timer = cfg.health_regen_initial_delay;
    }
    p.vitals.health -= amount;
    events.push(SimEvent::Damage { victim: p.id, attacker, amount, cause });
    if p.vitals.health <= 0.0 {
        kill(p, attacker, cause, events);
        return true;
    }
    false
}

/// Damage types of the sim's causes that carry `m_bApplyDamageReduction` (DefaultWeapon.ini:
/// `DishonoredDamageType_Bite` is one of the few that do not).
pub fn applies_damage_reduction(cause: DamageCause) -> bool {
    !matches!(cause, DamageCause::Rats | DamageCause::RatBite | DamageCause::World | DamageCause::Drowning | DamageCause::Adrenaline)
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
    p.dark = Default::default();
    events.push(SimEvent::Death { victim: p.id, killer, cause });
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn cfg() -> VitalsConfig {
        VitalsConfig {
            health_max: 70.0,
            health_regen_rate: 0.03,
            health_regen_amount: 1.0,
            health_regen_limit: 10.0,
            health_regen_initial_delay: 10.0,
            mana_max: 100.0,
            mana_regen_amount: 1.0,
            mana_regen_additive_portion: 20.0,
            mana_regen_step_time: 0.1,
            mana_regen_initial_delay: 3.0,
        }
    }

    fn player() -> PlayerState {
        let mut p = PlayerState::new(1, Vec3::ZERO, 0.0);
        reset(&mut p, &cfg());
        p
    }

    #[test]
    fn mana_regen_refunds_the_portion_after_the_delay() {
        let c = cfg();
        let mut p = player();
        assert!(spend_mana(&mut p, 50.0, &c));
        let dt = 1.0 / 60.0;
        // Nothing before the 3 s delay has run out.
        for _ in 0..179 {
            tick(&mut p, &c, dt);
        }
        assert_eq!(p.vitals.mana, 50.0);
        // Then one point per step (timer set to 0.1 s, so every 7th tick at 60 Hz) up to
        // the 20-point portion.
        for _ in 0..600 {
            tick(&mut p, &c, dt);
        }
        assert_eq!(p.vitals.mana, 70.0);
    }

    #[test]
    fn mana_always_refills_to_the_portion() {
        let c = cfg();
        let mut p = player();
        p.vitals.mana = 5.0;
        for _ in 0..600 {
            tick(&mut p, &c, 1.0 / 60.0);
        }
        assert_eq!(p.vitals.mana, 20.0);
    }

    #[test]
    fn health_regen_only_below_the_limit_after_the_delay() {
        let c = cfg();
        let mut p = player();
        let mut ev = Vec::new();
        apply_damage(&mut p, 65.0, None, DamageCause::Sword, &mut ev, &c);
        assert_eq!(p.vitals.health, 5.0);
        let dt = 1.0 / 60.0;
        for _ in 0..590 {
            tick(&mut p, &c, dt);
        }
        assert_eq!(p.vitals.health, 5.0, "still within the 10 s delay");
        // After the delay, a point every 0.03 s: the limit is reached within a few ticks.
        for _ in 0..30 {
            tick(&mut p, &c, dt);
        }
        assert_eq!(p.vitals.health, 10.0);
        // Damage that leaves health above the limit does not regenerate.
        let mut q = player();
        apply_damage(&mut q, 20.4, None, DamageCause::Sword, &mut ev, &c);
        assert_eq!(q.vitals.health, 50.0, "damage is whole points");
        for _ in 0..1200 {
            tick(&mut q, &c, dt);
        }
        assert_eq!(q.vitals.health, 50.0);
    }
}
