//! Combat/power values derived from `GameplayConfig`, plus the explicitly SPECULATIVE
//! timings that the original drives from animation data we cannot decode yet (U-008).
//! Each SPECULATIVE value is listed in research/UNKNOWNS.md and must be replaced by a
//! measurement from the original game.

use dishonored_ir::gameplay::GameplayConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CombatTuning {
    // --- sword (CONFIRMED geometry from DisTweaks_MeleeAttackPlayer / Assassinate) ---
    pub attack_range: f32,
    pub attack_sweep_size: f32,
    pub assassinate_range: f32,
    /// Fraction of victim max health per normal hit (`m_fHealthRatio_NormalDamage`).
    pub normal_damage_ratio: f32,
    /// Fraction for counter-attacks on a staggered target (`m_fHealthRatio_BigDamage`).
    pub big_damage_ratio: f32,
    // --- sword timings (SPECULATIVE until animation notifies are decoded/measured) ---
    pub swing_startup: f32,
    pub swing_active: f32,
    pub swing_recovery: f32,
    pub parry_window: f32,
    pub stagger_time: f32,
    pub blocked_recoil: f32,
    pub assassination_time: f32,
    /// Max angle (deg) between defender facing and attacker direction for a block.
    pub block_half_angle: f32,
    /// Attacker must be within this angle (deg) behind the victim for a back assassination.
    pub back_half_angle: f32,
    /// Minimum fall speed for a drop assassination (SPECULATIVE).
    pub drop_assassination_min_fall_speed: f32,

    // --- pistol (CONFIRMED: Twk_Proj_Bullet) ---
    pub bullet_damage: f32,
    pub bullet_speed: f32,
    pub bullet_headshot_mult: f32,
    pub bullet_close_range: f32,
    pub bullet_close_mult: f32,
    pub pistol_reserve: u16,
    // pistol timings (SPECULATIVE)
    pub pistol_equip: f32,
    pub pistol_fire_recovery: f32,
    pub pistol_reload: f32,

    // --- crossbow (CONFIRMED: Twk_Proj_Arrow gravity multiplier / stealth mult) ---
    pub bolt_gravity_mult: f32,
    pub bolt_stealth_mult: f32,
    // (SPECULATIVE)
    pub bolt_damage: f32,
    pub bolt_speed: f32,
    pub crossbow_reserve: u16,
    pub crossbow_equip: f32,
    pub crossbow_reload: f32,

    // --- Windblast (CONFIRMED: Twk_WindBlast level 2) ---
    pub windblast_cost: f32,
    pub windblast_angle: f32,
    pub windblast_distance: f32,
    pub windblast_force: f32,
    pub windblast_up_force: f32,
    pub windblast_damage: f32,
    pub windblast_wall_damage: f32,
    pub windblast_recovery: f32,

    // --- Dark Vision (CONFIRMED: Twk_DarkVision) ---
    pub dark_vision_cost: f32,
    pub dark_vision_duration: f32,
    pub dark_vision_distance: f32,
}

fn v(map: &std::collections::BTreeMap<String, f32>, key: &str, default: f32) -> f32 {
    map.get(key).copied().unwrap_or(default)
}

impl CombatTuning {
    pub fn from_config(c: &GameplayConfig) -> Self {
        let m = &c.melee.values;
        let pb = &c.pistol.values;
        let cb = &c.crossbow.values;
        let wb = &c.windblast.values;
        let dv = &c.dark_vision.values;
        CombatTuning {
            attack_range: v(m, "attack.m_fMaxContextRange", 210.0),
            attack_sweep_size: v(m, "attack.m_fSweepingAttackSize", 60.0),
            assassinate_range: v(m, "assassinate.m_fMaxContextRange", 275.0),
            normal_damage_ratio: v(m, "attack.m_fHealthRatio_NormalDamage", 0.2),
            big_damage_ratio: v(m, "attack.m_fHealthRatio_BigDamage", 0.7),
            swing_startup: 0.25,
            swing_active: 0.15,
            swing_recovery: 0.35,
            parry_window: 0.2,
            stagger_time: 1.0,
            blocked_recoil: 0.4,
            assassination_time: 1.2,
            block_half_angle: 70.0,
            back_half_angle: 60.0,
            drop_assassination_min_fall_speed: 300.0,

            bullet_damage: v(pb, "projectile.m_fDamage", 20.0),
            bullet_speed: v(pb, "projectile.m_fSpeed", 100000.0),
            bullet_headshot_mult: v(pb, "projectile.m_fDamageMultiplier_Headshot", 3.0),
            bullet_close_range: v(pb, "projectile.m_fCloseRange", 400.0),
            bullet_close_mult: v(pb, "projectile.m_fDamageMultiplier_CloseRange", 1.5),
            pistol_reserve: 10,
            pistol_equip: 0.4,
            pistol_fire_recovery: 0.3,
            pistol_reload: 1.6,

            bolt_gravity_mult: v(cb, "projectile.m_fGravityMultiplier", 1.3),
            bolt_stealth_mult: v(cb, "projectile.m_fDamageMultiplier_Stealth", 2.0),
            bolt_damage: 35.0,
            bolt_speed: 4500.0,
            crossbow_reserve: 10,
            crossbow_equip: 0.4,
            crossbow_reload: 1.0,

            windblast_cost: c.windblast.mana_cost,
            windblast_angle: v(wb, "m_Levels[1].m_Angle", v(wb, "m_Levels.m_Angle", 135.0)),
            windblast_distance: v(wb, "m_Levels[1].m_Distance", 3000.0),
            windblast_force: v(wb, "m_Levels[1].m_Force", 2700.0),
            windblast_up_force: v(wb, "m_Levels[1].m_ExtraUpForce", 1600.0),
            windblast_damage: v(wb, "m_Levels[1].m_Damage", 30.0),
            windblast_wall_damage: v(wb, "m_HitWallAdditionalDamage", 100.0),
            windblast_recovery: 0.6,

            dark_vision_cost: c.dark_vision.mana_cost,
            dark_vision_duration: v(dv, "m_Levels[1].m_fDuration", v(dv, "m_Levels.m_fDuration", 30.0)),
            dark_vision_distance: v(dv, "m_Levels[1].m_fDistance", v(dv, "m_Levels.m_fDistance", 2000.0)),
        }
    }
}
