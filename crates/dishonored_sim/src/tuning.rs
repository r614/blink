//! Combat/power values derived from `GameplayConfig`. Timings the original drives from
//! animation notifies are read from the decoded Startup.upk AnimSets (`anim_notifies`,
//! U-021); the remaining SPECULATIVE values are listed in research/UNKNOWNS.md.

use dishonored_ir::gameplay::GameplayConfig;
use serde::{Deserialize, Serialize};

/// The player's gun reload (`DisItemContext_ReloadGunPlayer`, start 0x00C2AA80, tick
/// 0x00C2AC00): the `m_ReloadGunAction_In` action (stage ReloadIn), then the timed stage
/// (`m_fReloadTimeRemaining = m_fReloadTime / Attribute_ReloadSpeed`, 0x00C23620, undivided when
/// the attribute is 0), whose end loads the magazine (0x00C28C50), then `m_ReloadGunAction_Out`
/// (stage ReloadOut); the context ends with that action. With `Attribute_FastReload` > 0 (and a
/// `m_ReloadFast` action) the context plays `m_ReloadFast` instead and its timed stage lasts the
/// attribute's value; the magazine loads when it runs out and the context ends at once (no
/// ReloadOut). Durations are the actions' `DishonoredNotify_AnimStateExit` times over their
/// `m_fPlayRateOverride`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct ReloadTiming {
    pub reload_in: f32,
    /// `m_fReloadTime`.
    pub time: f32,
    pub reload_out: f32,
}

impl ReloadTiming {
    /// Pistol_Reload_In exit 0.426, `m_fReloadTime` 0.75 (`Twk_Inv_PistolCorvo`), Pistol_Reload_Out
    /// exit 0.302.
    pub const PISTOL: ReloadTiming = ReloadTiming { reload_in: 0.426, time: 0.75, reload_out: 0.302 };
    /// Crossbow_Reload_In exit 0.225 and Crossbow_Reload_Out exit 0.169 at play rate 1.3,
    /// `m_fReloadTime` 1.0 (the class default: `Twk_Inv_CrossbowCorvo` and its cooked fallbacks
    /// leave it).
    pub const CROSSBOW: ReloadTiming = ReloadTiming { reload_in: 0.225 / 1.3, time: 1.0, reload_out: 0.169 / 1.3 };

    /// (seconds into the reload when the magazine loads, seconds when the reload ends) for the
    /// weapon's `Attribute_ReloadSpeed` and `Attribute_FastReload`.
    pub fn stages(&self, reload_speed: f32, fast_reload: f32) -> (f32, f32) {
        if fast_reload > 0.0 {
            return (fast_reload, fast_reload);
        }
        let timed = if reload_speed != 0.0 { self.time / reload_speed } else { self.time };
        let load = self.reload_in + timed;
        (load, load + self.reload_out)
    }

    /// Length of a normal reload.
    pub fn total(&self, reload_speed: f32) -> f32 {
        self.stages(reload_speed, 0.0).1
    }

    /// The reload in whole ticks of `dt`, as the context runs it: (ticks from the reload's
    /// start until the magazine loads, ticks until the reload ends).
    ///
    /// - ReloadIn and ReloadOut are actions: each ends on the tick its animation time,
    ///   advanced by `dt` per tick, reaches the `DishonoredNotify_AnimStateExit` time
    ///   ([`action_ticks`]).
    /// - The timed stage is a countdown: the In action's end sets `m_fReloadTimeRemaining`
    ///   (0x00C23620) and the context tick (0x00C2AC00) subtracts `dt` from the next tick on
    ///   and loads the magazine on the tick it reaches zero or less ([`countdown_ticks`]). In
    ///   float steps a whole number of ticks leaves a tiny remainder, so 0.75 s takes 46 ticks
    ///   and 1.0 s 61: the measured 0.767 s and 1.02 s (HOOK_TRIAGE.md, U-501).
    /// - `Attribute_FastReload` > 0: the fast action's countdown is the attribute's value, set
    ///   at the reload's start (0x00C2AA80); the context ends when it loads (no ReloadOut).
    pub fn ticks(&self, reload_speed: f32, fast_reload: f32, dt: f32) -> (u32, u32) {
        if fast_reload > 0.0 {
            let n = countdown_ticks(fast_reload, dt);
            return (n, n);
        }
        let timed = if reload_speed != 0.0 { self.time / reload_speed } else { self.time };
        let load = action_ticks(self.reload_in, dt) + countdown_ticks(timed, dt);
        (load, load + action_ticks(self.reload_out, dt))
    }
}

/// Ticks a `PawnAction` lasts: its animation time grows by `dt` each tick (f32) and the action
/// ends on the first tick it reaches `exit` (the `DishonoredNotify_AnimStateExit` time over the
/// play rate).
pub fn action_ticks(exit: f32, dt: f32) -> u32 {
    let (mut t, mut n) = (0.0f32, 0u32);
    while t < exit && n < 1_000_000 {
        t += dt;
        n += 1;
    }
    n
}

/// Ticks of a countdown decremented by `dt` each tick (f32) until it is zero or less.
pub fn countdown_ticks(time: f32, dt: f32) -> u32 {
    let (mut r, mut n) = (time, 0u32);
    loop {
        r -= dt;
        n += 1;
        if r <= 0.0 || n >= 1_000_000 {
            return n;
        }
    }
}

fn pistol_stages() -> ReloadTiming {
    ReloadTiming::PISTOL
}
fn crossbow_stages() -> ReloadTiming {
    ReloadTiming::CROSSBOW
}
fn crossbow_fire_recovery() -> f32 {
    0.398
}
fn crossbow_fast_fire() -> f32 {
    0.625 / 1.5
}

/// One player attack clip's `DishonoredNotify_AttackZone` and the end of its attack state.
///
/// The notify's tick handler (0x00A8E760) opens the versus zone and then the damage zone
/// when the clip time crosses `end - m_fDamageZoneMaxTime - m_fVersusZoneMaxTime` and
/// `end - m_fDamageZoneMaxTime` (each clamped to the notify start). Entering the damage
/// zone (melee context slot 115, 0x00C13140) sets `m_bDamageIsPosssible` and clears
/// `m_AlreadyHitActors`; the context tick (0x00C13640) sweeps for victims every tick while
/// that flag and the attack zone hold (0x00BFF7B0), until the notify ends (0x00A8E710).
/// The hit can therefore only land in the last `m_fDamageZoneMaxTime` of the attack zone.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SwingTiming {
    /// Notify start and duration (seconds into the clip).
    pub zone_start: f32,
    pub zone_duration: f32,
    pub damage_zone_max: f32,
    /// `DishonoredNotify_AttackInterruptable` (`AnimStateExit` when the clip has none).
    pub end: f32,
}

impl SwingTiming {
    /// `Ply_Sword_Attack_as` (Startup.upk) in `SwordState::swing_clip` order:
    /// `Sword_Ready_AttackRight_A_Small`, `AttackLeft_A_Small`, `AttackRight_B_Small`,
    /// `AttackLeft_B_Small`, `Sword_Sneak_Attack_Small` (no interruptable notify: its
    /// `AnimStateExit`).
    pub const PLAYER: [SwingTiming; 5] = [
        SwingTiming { zone_start: 0.138, zone_duration: 0.207, damage_zone_max: 0.05, end: 0.632 },
        SwingTiming { zone_start: 0.158, zone_duration: 0.186, damage_zone_max: 0.05, end: 0.579 },
        SwingTiming { zone_start: 0.104, zone_duration: 0.187, damage_zone_max: 0.05, end: 0.566 },
        SwingTiming { zone_start: 0.122, zone_duration: 0.243, damage_zone_max: 0.05, end: 0.594 },
        SwingTiming { zone_start: 0.086, zone_duration: 0.258, damage_zone_max: 0.05, end: 0.779 },
    ];

    /// Clip time at which the damage zone opens.
    pub fn damage_start(&self) -> f32 {
        self.zone_start + (self.zone_duration - self.damage_zone_max).max(0.0)
    }

    /// (startup, active, recovery) phase lengths: press to the damage zone, the damage
    /// zone, then the rest of the attack state.
    pub fn phases(&self) -> (f32, f32, f32) {
        let start = self.damage_start();
        let zone_end = self.zone_start + self.zone_duration;
        (start, zone_end - start, (self.end - zone_end).max(0.0))
    }
}

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
    // --- sword timings (LIKELY: Ply_Sword_Attack_as / Ply_Sword_Versus_as notifies;
    // stagger and recoil still SPECULATIVE) ---
    /// Uniform swing phases of AI-driven pawns (guards, civilians): a port approximation,
    /// their own clips are not timed in the simulation (U-600).
    pub swing_startup: f32,
    pub swing_active: f32,
    pub swing_recovery: f32,
    /// The player's attack clips, indexed by `SwordState::swing_clip`.
    pub player_swings: [SwingTiming; 5],
    pub parry_window: f32,
    /// `DisTweaks_MeleeBlockPlayer.m_fTapForPerfectParryWindow`: a block started this
    /// recently always parries (CONFIRMED, 0x00C22B60).
    pub parry_tap_window: f32,
    /// `m_fForceBlockTimeOnPerfectParryTap`: after a tap parry the block is held this long
    /// and further hits in that time are plain blocks (CONFIRMED, 0x00C22B60).
    pub force_block_after_tap: f32,
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
    // pistol timings (LIKELY: Ply_Pistol_as / Ply_Guns_as / CorvoGun_Mag_as notifies)
    pub pistol_equip: f32,
    pub pistol_fire_recovery: f32,
    /// Whole normal reload: `ReloadIn` + `m_fReloadTime` + `ReloadOut` (see [`ReloadTiming`]).
    pub pistol_reload: f32,
    /// `DisItemContext_ReloadGunPlayer` stages of the pistol (`Twk_Inv_PistolCorvo`'s
    /// `DisTweaks_ReloadGunPlayer`): the `ReloadIn` action (Pistol_Reload_In exit), the timed
    /// stage `m_fReloadTime` (divided by `Attribute_ReloadSpeed`), the `ReloadOut` action.
    #[serde(default = "pistol_stages")]
    pub pistol_reload_stages: ReloadTiming,

    // --- crossbow (CONFIRMED: Twk_Proj_Arrow gravity multiplier / stealth mult) ---
    pub bolt_gravity_mult: f32,
    pub bolt_stealth_mult: f32,
    /// `(int)Attribute_RangedDamage` of the crossbow (20, through its fallback chain) times the
    /// projectile's `m_fDamageMultiplier` (`DisProjectile` 0x00C4A310, fired by 0x00C257A0).
    pub bolt_damage: f32,
    /// Sleep dart / incendiary bolt: damage (same formula), speed and gravity multipliers.
    #[serde(default)]
    pub sleep_dart_damage: f32,
    #[serde(default)]
    pub sleep_dart_speed: f32,
    #[serde(default)]
    pub sleep_dart_gravity_mult: f32,
    #[serde(default)]
    pub incendiary_damage: f32,
    #[serde(default)]
    pub incendiary_speed: f32,
    #[serde(default)]
    pub incendiary_gravity_mult: f32,
    /// Melee reach scale from forward speed (0x00AB2050 / 0x00C0A060): factor
    /// `1 + clamp((v . view - m_fMinSpeedRayScale) * m_fRaySpeedScale, 0, m_fRaySpeedScale_Max)`
    /// blended by `m_fRayScalePercent` (DefaultPlayer.ini, `DisTweaks_MeleeAttackPlayer`).
    #[serde(default)]
    pub reach_min_speed: f32,
    #[serde(default)]
    pub reach_speed_scale: f32,
    #[serde(default)]
    pub reach_speed_scale_max: f32,
    #[serde(default)]
    pub reach_scale_percent: f32,
    pub bolt_speed: f32,
    /// `DisTweaks_Arrow.m_fLegsAndArmsDamageCap` (CONFIRMED data; limb hits are not
    /// modelled, the sim has head and body only).
    pub bolt_limb_damage_cap: f32,
    pub crossbow_reserve: u16,
    pub crossbow_equip: f32,
    /// Whole normal reload (see `crossbow_reload_stages`).
    pub crossbow_reload: f32,
    /// The crossbow's reload stages (`Twk_Inv_CrossbowCorvo`: In / Out at play rate 1.3).
    #[serde(default = "crossbow_stages")]
    pub crossbow_reload_stages: ReloadTiming,
    /// `FireCrossbow` action (Crossbow_Fire `DishonoredNotify_AnimStateExit`).
    #[serde(default = "crossbow_fire_recovery")]
    pub crossbow_fire_recovery: f32,
    /// `m_FireReloadFast` (`FireReloadFastCrossbow`: Crossbow_Fire_FastReload exit 0.625 s at
    /// `m_fPlayRateOverride` 1.5): with `Attribute_FastReload`, a shot that leaves a bolt to load
    /// plays this action and loads it at the end, with no reload stage (0x00C2E570, 0x00C2E730).
    #[serde(default = "crossbow_fast_fire")]
    pub crossbow_fast_fire: f32,

    // --- Windblast (CONFIRMED: Twk_WindBlast; per level, index = `m_CurrentLevel - 1`, the
    // block 0x00BF6780 / 0x00BF7090 read) ---
    pub windblast_cost: f32,
    pub windblast_angle: [f32; 2],
    pub windblast_distance: [f32; 2],
    pub windblast_force: [f32; 2],
    pub windblast_up_force: [f32; 2],
    pub windblast_damage: [f32; 2],
    /// Speed of the blast front (`m_Speed`): targets are hit when it reaches them.
    pub windblast_speed: [f32; 2],
    /// `m_bUseDirectionalWindblast`: keep the aim pitch (else flattened).
    pub windblast_directional: bool,
    /// `m_HitWallAdditionalDamage`, `m_fMinSpeedToExplodeNPCs`, `m_fChanceToExplodeNPCs`: a
    /// blasted NPC hitting a wall (0x00B529C0).
    pub windblast_wall_damage: f32,
    #[serde(default)]
    pub windblast_wall_min_speed: f32,
    #[serde(default)]
    pub windblast_explode_chance: f32,

    // --- Dark Vision (CONFIRMED: Twk_DarkVision) ---
    pub dark_vision_cost: f32,
    pub dark_vision_duration: f32,
    pub dark_vision_distance: f32,
    /// `m_PpPerLevelParameters.m_EyeLidClosingDuration` (fade-in before the duration runs),
    /// `m_PowerFadeOutDuration` (after it), `m_fCooldownTimer` (minimum time before it can be
    /// switched off) of `Twk_DarkVision`.
    #[serde(default)]
    pub dark_vision_fade_in: f32,
    #[serde(default)]
    pub dark_vision_fade_out: f32,
    #[serde(default)]
    pub dark_vision_cooldown: f32,
    /// `m_fDesactivateInCombatTimer`: seconds in combat before the effect ends (0x00BF84C0).
    #[serde(default)]
    pub dark_vision_combat_timer: f32,

    // --- ranged dispersion (U-247; `DisTweaks_WeaponRanged_Attributes` of Corvo's weapons,
    // CONFIRMED data) ---
    #[serde(default)]
    pub pistol_dispersion: DispersionTuning,
    #[serde(default)]
    pub crossbow_dispersion: DispersionTuning,
}

/// A ranged weapon's dispersion: `Attribute_DispersionMax` / `Min` /
/// `InterpolationSpeed` base values (before the workshop upgrades) and the
/// `DisTweaks_WeaponRanged.m_bMaxDispersionOn*` flags (bit 0 sprint, 1 jump, 2 block, 3 melee,
/// the tweak's own bit order at +0x118).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct DispersionTuning {
    pub max: f32,
    pub min: f32,
    pub speed: f32,
    pub max_on: u8,
}

impl DispersionTuning {
    pub const SPRINT: u8 = 1;
    pub const JUMP: u8 = 2;
    pub const BLOCK: u8 = 4;
    pub const MELEE: u8 = 8;

    fn from_values(w: &std::collections::BTreeMap<String, f32>, max: f32, min: f32, speed: f32) -> Self {
        let flag = |k: &str, bit: u8, d: bool| if v(w, &format!("item.m_bMaxDispersionOn{k}"), if d { 1.0 } else { 0.0 }) != 0.0 { bit } else { 0 };
        DispersionTuning {
            max: v(w, "disp.m_DispersionMax.m_fBaseValue2_Normal", max),
            min: v(w, "disp.m_DispersionMin.m_fBaseValue2_Normal", min),
            speed: v(w, "disp.m_DispersionInterpolationSpeed.m_fBaseValue2_Normal", speed),
            max_on: flag("Sprint", Self::SPRINT, true) | flag("Jump", Self::JUMP, true) | flag("Block", Self::BLOCK, true) | flag("Melee", Self::MELEE, false),
        }
    }
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
            // "Small" attacks: DishonoredNotify_AttackZone starts 0.104-0.158 s (mean 0.13),
            // lasts 0.186-0.243 s (mean 0.21); AttackInterruptable at ~0.6 s.
            swing_startup: 0.13,
            swing_active: 0.21,
            swing_recovery: 0.26,
            player_swings: SwingTiming::PLAYER,
            // DisNotify_BlockParryZone on Sword_Block_*: t=0.001, duration 0.430.
            parry_window: 0.43,
            parry_tap_window: v(m, "block.m_fTapForPerfectParryWindow", 0.2),
            force_block_after_tap: v(m, "block.m_fForceBlockTimeOnPerfectParryTap", 0.3),
            stagger_time: 1.0,
            blocked_recoil: 0.4,
            // Sword_Ready_Assassination_Back_Master AnimStateExit at 1.886 s.
            assassination_time: 1.886,
            // DisTweaks_MeleeBlock.m_fBlockAngle (horizontal half-angle of the block cone).
            block_half_angle: v(m, "block.m_fBlockAngle", 50.0),
            back_half_angle: 60.0,
            drop_assassination_min_fall_speed: 300.0,

            bullet_damage: v(pb, "projectile.m_fDamage", 20.0),
            bullet_speed: v(pb, "projectile.m_fSpeed", 100000.0),
            bullet_headshot_mult: v(pb, "projectile.m_fDamageMultiplier_Headshot", 3.0),
            bullet_close_range: v(pb, "projectile.m_fCloseRange", 400.0),
            bullet_close_mult: v(pb, "projectile.m_fDamageMultiplier_CloseRange", 1.5),
            pistol_reserve: 10,
            // Guns_Ready_Equip AnimStateExit 0.43; Pistol_Fire exit 0.30.
            pistol_equip: 0.43,
            pistol_fire_recovery: 0.3,
            pistol_reload: ReloadTiming::PISTOL.total(0.0),
            pistol_reload_stages: ReloadTiming::PISTOL,

            bolt_gravity_mult: v(cb, "projectile.m_fGravityMultiplier", 1.3),
            bolt_stealth_mult: v(cb, "projectile.m_fDamageMultiplier_Stealth", 2.0),
            bolt_damage: c.items.inventory.crossbow_ranged_damage.trunc() * c.items.gadgets.bolt.damage_mult,
            sleep_dart_damage: c.items.inventory.crossbow_ranged_damage.trunc() * c.items.gadgets.sleep_dart.damage_mult,
            sleep_dart_speed: v(cb, "fire.m_fArrowFireSpeed", 20000.0) * c.items.gadgets.sleep_dart.speed_mult,
            sleep_dart_gravity_mult: c.items.gadgets.sleep_dart.gravity_mult,
            incendiary_damage: c.items.inventory.crossbow_ranged_damage.trunc() * c.items.gadgets.incendiary_bolt.damage_mult,
            incendiary_speed: v(cb, "fire.m_fArrowFireSpeed", 20000.0) * c.items.gadgets.incendiary_bolt.speed_mult,
            incendiary_gravity_mult: c.items.gadgets.incendiary_bolt.gravity_mult,
            reach_min_speed: c.items.inventory.melee_ray_speed[0],
            reach_speed_scale: c.items.inventory.melee_ray_speed[1],
            reach_speed_scale_max: c.items.inventory.melee_ray_speed[2],
            reach_scale_percent: v(m, "attack.m_fRayScalePercent", 0.5),
            // DisTweaks_FireCrossbow.m_fArrowFireSpeed x DisTweaks_Projectile.m_fSpeedMultiplier.
            bolt_speed: v(cb, "fire.m_fArrowFireSpeed", 20000.0) * v(cb, "projectile.m_fSpeedMultiplier", 1.0),
            bolt_limb_damage_cap: v(cb, "projectile.m_fLegsAndArmsDamageCap", 15.0),
            crossbow_reserve: 10,
            // Guns_Ready_Equip exit 0.43; Crossbow_Fire exit 0.398 (Ply_Crossbow_as).
            crossbow_equip: 0.43,
            crossbow_reload: ReloadTiming::CROSSBOW.total(0.0),
            crossbow_reload_stages: ReloadTiming::CROSSBOW,
            crossbow_fire_recovery: 0.398,
            crossbow_fast_fire: 0.625 / 1.5,

            windblast_cost: c.windblast.mana_cost,
            windblast_angle: [v(wb, "m_Levels.m_Angle", 135.0), v(wb, "m_Levels[1].m_Angle", 135.0)],
            windblast_distance: [v(wb, "m_Levels.m_Distance", 3000.0), v(wb, "m_Levels[1].m_Distance", 3000.0)],
            windblast_force: [v(wb, "m_Levels.m_Force", 1800.0), v(wb, "m_Levels[1].m_Force", 2700.0)],
            windblast_up_force: [v(wb, "m_Levels.m_ExtraUpForce", 1000.0), v(wb, "m_Levels[1].m_ExtraUpForce", 1600.0)],
            windblast_damage: [v(wb, "m_Levels.m_Damage", 20.0), v(wb, "m_Levels[1].m_Damage", 30.0)],
            windblast_speed: [v(wb, "m_Levels.m_Speed", 5000.0), v(wb, "m_Levels[1].m_Speed", 5000.0)],
            windblast_directional: v(wb, "m_bUseDirectionalWindblast", 1.0) != 0.0,
            windblast_wall_damage: v(wb, "m_HitWallAdditionalDamage", 100.0),
            windblast_wall_min_speed: v(wb, "m_fMinSpeedToExplodeNPCs", 500.0),
            windblast_explode_chance: v(wb, "m_fChanceToExplodeNPCs", 0.3),

            dark_vision_cost: c.dark_vision.mana_cost,
            dark_vision_duration: v(dv, "m_Levels[1].m_fDuration", v(dv, "m_Levels.m_fDuration", 30.0)),
            dark_vision_distance: v(dv, "m_Levels[1].m_fDistance", v(dv, "m_Levels.m_fDistance", 2000.0)),
            dark_vision_fade_in: v(dv, "m_Levels[1].m_PpPerLevelParameters.m_EyeLidClosingDuration", 0.5),
            dark_vision_fade_out: v(dv, "m_Levels[1].m_PpPerLevelParameters.m_PowerFadeOutDuration", 2.0),
            dark_vision_cooldown: v(dv, "m_fCooldownTimer", 1.0),
            dark_vision_combat_timer: v(dv, "m_fDesactivateInCombatTimer", 1.0),

            // Corvo's pistol 45 / 15 / 70, crossbow 30 / 0 / 70 (Normal difficulty column).
            pistol_dispersion: DispersionTuning::from_values(pb, 45.0, 15.0, 70.0),
            crossbow_dispersion: DispersionTuning::from_values(cb, 30.0, 0.0, 70.0),
        }
    }
}
