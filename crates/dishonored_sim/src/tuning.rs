//! Combat/power values derived from `GameplayConfig`. Timings the original drives from
//! animation notifies are read from the config's sequence table (`GameplayConfig::sequences`,
//! decoded from the AnimSets); the states that play those sequences take them from the playhead
//! (`crate::anim`), these values stand where a rule still counts time. The remaining
//! SPECULATIVE values are listed in research/UNKNOWNS.md.

use crate::anim::{self, ASSASSINATION_SET, CROSSBOW_MESH_SET, CROSSBOW_SET, FATALITY_SET, GUNS_SET, PISTOL_SET, POWERS_SET, SWING_CLIPS, SWORD_ATTACK_SET, SWORD_VERSUS_SET};
use dishonored_ir::gameplay::{ASSASSINATION_MASTERS, GameplayConfig, SequenceInfo};
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

/// `DisTweaks_ReloadGunPlayer.m_fReloadTime` of `Twk_Inv_PistolCorvo` (0.75) and of the crossbow
/// (1.0, the class default: `Twk_Inv_CrossbowCorvo` and its cooked fallbacks leave it); the
/// crossbow's reload actions' `m_fPlayRateOverride` (1.3). Tweak values the config does not load
/// yet (U-501).
const PISTOL_RELOAD_TIME: f32 = 0.75;
const CROSSBOW_RELOAD_TIME: f32 = 1.0;
const CROSSBOW_RELOAD_RATE: f32 = 1.3;

impl ReloadTiming {
    /// `Pistol_Reload_In` / `Pistol_Reload_Out` `AnimStateExit` (`Ply_Pistol_as`) around the
    /// pistol's `m_fReloadTime`.
    pub fn pistol(g: &GameplayConfig) -> Self {
        ReloadTiming { reload_in: anim::exit_time(g, PISTOL_SET, "Pistol_Reload_In"), time: PISTOL_RELOAD_TIME, reload_out: anim::exit_time(g, PISTOL_SET, "Pistol_Reload_Out") }
    }
    /// `Crossbow_Reload_In` / `Crossbow_Reload_Out` exits (`Ply_Crossbow_as`) at the actions' play
    /// rate around the crossbow's `m_fReloadTime`.
    pub fn crossbow(g: &GameplayConfig) -> Self {
        ReloadTiming {
            reload_in: anim::exit_time(g, CROSSBOW_SET, "Crossbow_Reload_In") / CROSSBOW_RELOAD_RATE,
            time: CROSSBOW_RELOAD_TIME,
            reload_out: anim::exit_time(g, CROSSBOW_SET, "Crossbow_Reload_Out") / CROSSBOW_RELOAD_RATE,
        }
    }

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

/// One attack sequence's `DishonoredNotify_AttackZone` and the end of its attack state, in
/// play seconds at rate 1 (sequence times over `RateScale`).
///
/// The notify's tick handler (0x00A8E760) opens the versus zone and then the damage zone
/// on the step of the window whose `[before, after)` holds `Duration - m_fDamageZoneMaxTime -
/// m_fVersusZoneMaxTime` and `Duration - m_fDamageZoneMaxTime` (each clamped to the notify
/// start). Entering the damage zone (melee context slot 115, 0x00C13140) sets
/// `m_bDamageIsPosssible` and clears `m_AlreadyHitActors`; the context tick (0x00C13640) sweeps
/// for victims every tick while that flag and the attack zone hold (0x00BFF7B0), until the notify
/// ends (0x00A8E710). The hit can therefore only land in the last `m_fDamageZoneMaxTime` of the
/// attack zone. The swing's state reads this from its sequence's playhead (`crate::anim`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SwingTiming {
    /// Notify start and duration.
    pub zone_start: f32,
    pub zone_duration: f32,
    pub damage_zone_max: f32,
    /// `DishonoredNotify_AttackInterruptable` (`AnimStateExit` when the sequence has none).
    pub end: f32,
}

impl SwingTiming {
    /// The timing of an attack sequence (its first `DishonoredNotify_AttackZone`).
    pub fn of(s: &SequenceInfo) -> Option<Self> {
        let z = s.notify("DishonoredNotify_AttackZone")?;
        let r = s.rate_scale_or_one();
        Some(SwingTiming {
            zone_start: z.time / r,
            zone_duration: z.duration / r,
            damage_zone_max: z.payload.get("m_fDamageZoneMaxTime").copied().unwrap_or(0.0) / r,
            end: s.notify_time("DishonoredNotify_AttackInterruptable").unwrap_or_else(|| s.exit_time()),
        })
    }

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
    /// `m_fHealthRatio_NormalDamage` / `_BigDamage`: NOT damage factors. The hit handler
    /// (0x00C131C0) divides the health a hit took by the victim's HealthMax and picks the
    /// contact effect (small below 0.2, normal, big from 0.7); presentation only.
    pub normal_damage_ratio: f32,
    pub big_damage_ratio: f32,
    /// `m_fAttackDamageMultiplier` of the player's attack context (`DisTweaks_MeleeAttackPlayer`).
    #[serde(default = "one")]
    pub attack_damage_multiplier: f32,
    /// `DisTweaks_Assassinate` `m_fRayScalePercent` (the reach's speed blend) and
    /// `DisTweaks_DropAssassinate` `m_fMaxContextRange` / `m_fRayScalePercent`.
    #[serde(default = "assassinate_ray_scale")]
    pub assassinate_ray_scale: f32,
    #[serde(default = "drop_range")]
    pub drop_assassinate_range: f32,
    #[serde(default = "drop_ray_scale")]
    pub drop_assassinate_ray_scale: f32,
    /// `m_AssassinateOnAwareness[EAIAwareness].m_bAssassinate` (0x00C09060): the shipped table
    /// allows Unaware, Surprised, Suspicious and Begging.
    #[serde(default = "assassinate_awareness")]
    pub assassinate_on_awareness: [bool; 8],
    /// `m_bCanAssassinateRunners` (tweak +0x1C00): off, so running targets are refused.
    #[serde(default)]
    pub assassinate_runners: bool,
    /// `DisTweaks_DropAssassinate`: `m_fHitWindowInSeconds`, `m_fMinDropDownVel`,
    /// `m_fMaxAllowedDropJumpVel`, `m_fMinDropDistToTarget`, `m_fMaxDropDistToTarget`
    /// (0x00C09810 / 0x00C09900).
    #[serde(default = "drop_rule")]
    pub drop_rule: DropRule,
    /// `AnimStateExit` of the assassination clips the move sets play, by the direction
    /// 0x00BBA970 returns (0 front, 1 left, 2 right, 3 back): `Sword_Ready_Assassination_*_Master`
    /// and `_Drop*_Master` of `Ply_Sword_Assassination_as` (the move's length when no layer plays
    /// its master: `SwordState::lock_time`).
    pub assassination_times: [f32; 4],
    pub drop_assassination_times: [f32; 4],
    /// The synced short fatality of a killing blow (`DisTweaks_MeleeAttackPlayer`
    /// `m_ForehandKill_Synced` / `m_BackhandKill_Synced`, 0x00C0A590): its chance
    /// (`m_fChanceOfSyncedAttack`), the facing the victim needs (0x00BBF9D0 with 45 degrees) and
    /// the masters' `AnimStateExit` (`Sword_Ready_Fatality_SmallFront_{Right_A, Right_B, Left_A,
    /// Left_B}_Master` of `Ply_Sword_Fatality_as`: forehand A / B, backhand A / B).
    #[serde(default = "synced_attack_chance")]
    pub synced_attack_chance: f32,
    #[serde(default = "synced_attack_angle")]
    pub synced_attack_angle: f32,
    pub short_fatality_times: [f32; 4],
    // --- sword timings (Ply_Sword_Attack_as / Ply_Sword_Versus_as notifies; stagger and recoil
    // still SPECULATIVE) ---
    /// Swing phases of a pawn whose swing plays no sequence (the special classes' moves, whose
    /// sets the table does not hold): the means of the player's four ready small attacks' attack
    /// zone start, zone length and the rest to `AttackInterruptable` (a port approximation, U-600).
    pub swing_startup: f32,
    pub swing_active: f32,
    pub swing_recovery: f32,
    /// The player's attack clips, indexed by `SwordState::swing_clip` (`Ply_Sword_Attack_as`:
    /// `Sword_Ready_AttackRight_A_Small`, `AttackLeft_A_Small`, `AttackRight_B_Small`,
    /// `AttackLeft_B_Small`, `Sword_Sneak_Attack_Small`).
    pub player_swings: [SwingTiming; 5],
    /// `DisNotify_BlockParryZone` of `Sword_Ready_ParryIdle_In` (start, duration): the parry
    /// window of a block that plays no sequence (`combat::parry_zone`).
    pub parry_zone: [f32; 2],
    /// `DisTweaks_MeleeBlockPlayer.m_fTapForPerfectParryWindow`: a block started this
    /// recently always parries (CONFIRMED, 0x00C22B60).
    pub parry_tap_window: f32,
    /// `m_fForceBlockTimeOnPerfectParryTap`: after a tap parry the block is held this long
    /// and further hits in that time are plain blocks (CONFIRMED, 0x00C22B60).
    pub force_block_after_tap: f32,
    pub stagger_time: f32,
    pub blocked_recoil: f32,
    /// Max angle (deg) between defender facing and attacker direction for a block.
    pub block_half_angle: f32,
    /// Port rule (not the original's): attacker within this angle (deg) behind the victim, for
    /// the choke (U-103) and for assassinating another player (the original only assassinates
    /// NPCs; a player has no AI awareness to gate it).
    pub back_half_angle: f32,

    // --- pistol (CONFIRMED: Twk_Proj_Bullet) ---
    /// `(int)Attribute_RangedDamage` of the pistol (the projectile's `m_Damage`, set by the
    /// fire 0x00C2A3E0) plus the bullet tweak's `m_fDamage` (0x00C29820:
    /// `cvtsi2ss [this+0x27C]; addss [tweak+0xE8]`): 10 + 20.
    pub bullet_damage: f32,
    pub bullet_speed: f32,
    pub bullet_headshot_mult: f32,
    /// `m_fCloseRange` / `m_fDamageMultiplier_CloseRange` (distance from the spawn point
    /// strictly below the range), `m_fLongRange` / `m_fDamageMultiplier_LongRange` (strictly
    /// above), `m_fDamageMultiplier_Stealth` (any awareness but `InCombat`), 0x00C29820.
    pub bullet_close_range: f32,
    pub bullet_close_mult: f32,
    #[serde(default)]
    pub bullet_long_range: f32,
    #[serde(default)]
    pub bullet_long_mult: f32,
    #[serde(default)]
    pub bullet_stealth_mult: f32,
    /// `m_fPiercingChance` and `m_fPostPiercingDamageRatio` (0x00C3A660, 0x00C29820).
    #[serde(default)]
    pub bullet_pierce_chance: f32,
    #[serde(default)]
    pub bullet_post_pierce_ratio: f32,
    /// `DisTweaks_FirePistol.m_fBulletSpawnDistance` / `DisTweaks_FireCrossbow.m_fArrowSpawnDistance`
    /// (`+0x420` of the fire tweak): the projectile starts this far in front of the camera
    /// (0x00C2A3E0, 0x00C38230).
    #[serde(default)]
    pub bullet_spawn_distance: f32,
    #[serde(default)]
    pub bolt_spawn_distance: f32,
    /// `m_AINoiseLoudnessWhenFiring` of the pistol as a distance (`EAINoiseLoudness4`, 6000).
    #[serde(default)]
    pub pistol_noise_radius: f32,
    pub pistol_reserve: u16,
    // pistol timings (Ply_Pistol_as / Ply_Guns_as notifies)
    /// `Guns_Ready_Equip` and `Pistol_Fire` `AnimStateExit`.
    pub pistol_equip: f32,
    pub pistol_fire_recovery: f32,
    /// Whole normal reload: `ReloadIn` + `m_fReloadTime` + `ReloadOut` (see [`ReloadTiming`]).
    pub pistol_reload: f32,
    /// `DisItemContext_ReloadGunPlayer` stages of the pistol (`Twk_Inv_PistolCorvo`'s
    /// `DisTweaks_ReloadGunPlayer`): the `ReloadIn` action (Pistol_Reload_In exit), the timed
    /// stage `m_fReloadTime` (divided by `Attribute_ReloadSpeed`), the `ReloadOut` action.
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
    /// `DisTweaks_Projectile.m_fSpeedMultiplier` of the bolts (the homing's ballistic aim,
    /// 0x00C49EE0, uses it again): arrow, sleep dart, incendiary bolt; 1 for the other kinds.
    #[serde(default = "one")]
    pub bolt_speed_mult: f32,
    #[serde(default = "one")]
    pub sleep_dart_speed_mult: f32,
    #[serde(default = "one")]
    pub incendiary_speed_mult: f32,
    /// A projectile's Windblast reaction (`DisTweaks_Projectile`, 0x00C4F290; no shipped projectile
    /// tweak overrides the class defaults): `m_WindblastBehavior` (0 none, 1 normal, 2 push
    /// toward the suggested target, 3 homing), `m_fMass` (the push's divisor),
    /// `m_fWindblastHomingTime`, `m_fWindblastVelocityMultiplier`.
    #[serde(default = "homing_behavior")]
    pub projectile_windblast_behavior: u8,
    #[serde(default = "one")]
    pub projectile_mass: f32,
    #[serde(default = "homing_time")]
    pub projectile_windblast_homing_time: f32,
    #[serde(default = "homing_mult")]
    pub projectile_windblast_velocity_mult: f32,
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
    /// The hit rules of the bolt, the sleep dart and the incendiary bolt, in that order
    /// (`DisProjectile_Arrow` damage 0x00C503D0 over the base 0x00C4A310).
    #[serde(default)]
    pub arrows: [ArrowDamage; 3],
    /// `(int)Attribute_RangedHeadshotMultiplier` of the crossbow (`(int)1.2` = 1): the
    /// projectile's headshot factor (0x00C38230), used when it does not kill on a headshot.
    #[serde(default)]
    pub bolt_headshot_mult: f32,
    pub crossbow_reserve: u16,
    /// `Guns_Ready_Equip` `AnimStateExit`.
    pub crossbow_equip: f32,
    /// Whole normal reload (see `crossbow_reload_stages`).
    pub crossbow_reload: f32,
    /// The crossbow's reload stages (`Twk_Inv_CrossbowCorvo`: In / Out at play rate 1.3).
    pub crossbow_reload_stages: ReloadTiming,
    /// `FireCrossbow` action (`Ply_Crossbow_as` Crossbow_Fire `DishonoredNotify_AnimStateExit`).
    pub crossbow_fire_recovery: f32,
    /// `m_FireReloadFast` (`FireReloadFastCrossbow`: Crossbow_Fire_FastReload exit 0.625 s at
    /// `m_fPlayRateOverride` 1.5): with `Attribute_FastReload`, a shot that leaves a bolt to load
    /// plays this action and loads it at the end, with no reload stage (0x00C2E570, 0x00C2E730).
    pub crossbow_fast_fire: f32,
    /// `DishonoredNotify_FireProjectile` of `Crossbow_Fire` in `Crossbows_as` (0.0336 s): the
    /// bolt leaves when the fire action's animation passes it, not on the press
    /// (0x00A96890 -> the item context's fire, 0x00C38230); the crossbow mesh's layer fires it
    /// (`crate::anim`).
    pub crossbow_fire_notify: f32,
    /// The same notify of `Crossbow_Fire_FastReload` (0.0 s: the shot leaves on the press).
    pub crossbow_fast_fire_notify: f32,

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
    /// Seconds from the cast until the blast starts (origin and direction taken, 0x00BF9570):
    /// the `DisNotify_WindBlast` of the player's `Powers_Cast_Windblast` (0x00A96AE0). The
    /// power's own `m_fWarmupTime` only leads to the fire mode (0x00BF9720), which waits for the
    /// notify while the cast action plays.
    pub windblast_start_delay: f32,

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

/// One crossbow projectile's hit rules (`DisTweaks_Arrow`): `m_fDamageMultiplier_Stealth`,
/// `m_fLegsAndArmsDamageCap`, `m_bKillOnHeadshot`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct ArrowDamage {
    pub stealth_mult: f32,
    pub limb_cap: f32,
    pub kill_on_headshot: bool,
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

/// `DisTweaks_DropAssassinate` target rule (0x00C09810 / 0x00C09900).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct DropRule {
    pub hit_window: f32,
    pub min_drop_down_vel: f32,
    pub max_allowed_drop_jump_vel: f32,
    pub min_drop_dist: f32,
    pub max_drop_dist: f32,
}

fn one() -> f32 {
    1.0
}
fn assassinate_ray_scale() -> f32 {
    1.25
}
fn drop_range() -> f32 {
    250.0
}
fn drop_ray_scale() -> f32 {
    2.0
}
fn assassinate_awareness() -> [bool; 8] {
    [true, false, true, true, false, false, true, false]
}
/// `DisPWB_Homing`, `m_fWindblastHomingTime` 1.6, `m_fWindblastVelocityMultiplier` 1.2
/// (`Default__DisTweaks_Projectile`).
fn homing_behavior() -> u8 {
    3
}
fn homing_time() -> f32 {
    1.6
}
fn homing_mult() -> f32 {
    1.2
}

fn drop_rule() -> DropRule {
    DropRule { hit_window: 4.0, min_drop_down_vel: 100.0, max_allowed_drop_jump_vel: 0.0, min_drop_dist: 0.0, max_drop_dist: 350.0 }
}
fn synced_attack_chance() -> f32 {
    1.0
}
fn synced_attack_angle() -> f32 {
    45.0
}

/// The four ready small attacks' mean attack zone start, zone length and rest to the swing's end
/// (`CombatTuning::swing_startup`).
fn mean_small_phases(swings: &[SwingTiming; 5]) -> (f32, f32, f32) {
    let n = 4.0;
    let ready = &swings[..4];
    let start = ready.iter().map(|w| w.zone_start).sum::<f32>() / n;
    let active = ready.iter().map(|w| w.zone_duration).sum::<f32>() / n;
    let rest = ready.iter().map(|w| (w.end - w.zone_start - w.zone_duration).max(0.0)).sum::<f32>() / n;
    (start, active, rest)
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
        let player_swings: [SwingTiming; 5] = std::array::from_fn(|i| SwingTiming::of(anim::info(c, SWORD_ATTACK_SET, SWING_CLIPS[i])).unwrap_or_else(|| panic!("{} has no DishonoredNotify_AttackZone", SWING_CLIPS[i])));
        let (swing_startup, swing_active, swing_recovery) = mean_small_phases(&player_swings);
        let masters = |set: &str, from: usize| -> [f32; 4] { std::array::from_fn(|i| anim::exit_time(c, set, ASSASSINATION_MASTERS[from + i])) };
        let parry = anim::info(c, SWORD_VERSUS_SET, "Sword_Ready_ParryIdle_In");
        let parry_zone = parry.notify("DisNotify_BlockParryZone").map(|n| [n.time / parry.rate_scale_or_one(), n.duration / parry.rate_scale_or_one()]).expect("Sword_Ready_ParryIdle_In has no DisNotify_BlockParryZone");
        let notify = |set: &str, seq: &str, class: &str| anim::info(c, set, seq).notify_time(class).unwrap_or_else(|| panic!("{set}.{seq} has no {class}"));
        let pistol_stages = ReloadTiming::pistol(c);
        let crossbow_stages = ReloadTiming::crossbow(c);
        CombatTuning {
            attack_range: v(m, "attack.m_fMaxContextRange", 210.0),
            attack_sweep_size: v(m, "attack.m_fSweepingAttackSize", 60.0),
            assassinate_range: v(m, "assassinate.m_fMaxContextRange", 275.0),
            normal_damage_ratio: v(m, "attack.m_fHealthRatio_NormalDamage", 0.2),
            big_damage_ratio: v(m, "attack.m_fHealthRatio_BigDamage", 0.7),
            attack_damage_multiplier: v(m, "attack.m_fAttackDamageMultiplier", 1.0),
            assassinate_ray_scale: v(m, "assassinate.m_fRayScalePercent", 1.25),
            drop_assassinate_range: v(m, "drop_assassinate.m_fMaxContextRange", 250.0),
            drop_assassinate_ray_scale: v(m, "drop_assassinate.m_fRayScalePercent", 2.0),
            // The flattened tweak lists only the true entries (struct default false).
            assassinate_on_awareness: std::array::from_fn(|i| {
                let k = if i == 0 { "assassinate.m_AssassinateOnAwareness.m_bAssassinate".to_string() } else { format!("assassinate.m_AssassinateOnAwareness[{i}].m_bAssassinate") };
                match m.get(&k) {
                    Some(x) => *x != 0.0,
                    None if m.keys().any(|k| k.starts_with("assassinate.")) => false,
                    None => assassinate_awareness()[i],
                }
            }),
            assassinate_runners: v(m, "assassinate.m_bCanAssassinateRunners", 0.0) != 0.0,
            drop_rule: DropRule {
                hit_window: v(m, "drop_assassinate.m_fHitWindowInSeconds", 4.0),
                min_drop_down_vel: v(m, "drop_assassinate.m_fMinDropDownVel", 100.0),
                max_allowed_drop_jump_vel: v(m, "drop_assassinate.m_fMaxAllowedDropJumpVel", 0.0),
                min_drop_dist: v(m, "drop_assassinate.m_fMinDropDistToTarget", 0.0),
                max_drop_dist: v(m, "drop_assassinate.m_fMaxDropDistToTarget", 350.0),
            },
            // The masters' AnimStateExit (Ply_Sword_Assassination_as; the synced short fatalities'
            // in Ply_Sword_Fatality_as).
            assassination_times: masters(ASSASSINATION_SET, 0),
            drop_assassination_times: masters(ASSASSINATION_SET, 4),
            // Twk_Inv_SwordCorvo `m_fChanceOfSyncedAttack` 1.0.
            synced_attack_chance: synced_attack_chance(),
            synced_attack_angle: synced_attack_angle(),
            short_fatality_times: masters(FATALITY_SET, 8),
            swing_startup,
            swing_active,
            swing_recovery,
            player_swings,
            parry_zone,
            parry_tap_window: v(m, "block.m_fTapForPerfectParryWindow", 0.2),
            force_block_after_tap: v(m, "block.m_fForceBlockTimeOnPerfectParryTap", 0.3),
            stagger_time: 1.0,
            blocked_recoil: 0.4,
            // DisTweaks_MeleeBlock.m_fBlockAngle (horizontal half-angle of the block cone).
            block_half_angle: v(m, "block.m_fBlockAngle", 50.0),
            back_half_angle: 60.0,

            bullet_damage: c.items.inventory.pistol_ranged_damage.trunc() + v(pb, "projectile.m_fDamage", 20.0),
            bullet_speed: v(pb, "projectile.m_fSpeed", 100000.0),
            bullet_headshot_mult: v(pb, "projectile.m_fDamageMultiplier_Headshot", 3.0),
            bullet_close_range: v(pb, "projectile.m_fCloseRange", 400.0),
            bullet_close_mult: v(pb, "projectile.m_fDamageMultiplier_CloseRange", 1.5),
            bullet_long_range: v(pb, "projectile.m_fLongRange", 1500.0),
            bullet_long_mult: v(pb, "projectile.m_fDamageMultiplier_LongRange", 1.0),
            bullet_stealth_mult: v(pb, "projectile.m_fDamageMultiplier_Stealth", 1.0),
            bullet_pierce_chance: v(pb, "projectile.m_fPiercingChance", 1.0),
            bullet_post_pierce_ratio: v(pb, "projectile.m_fPostPiercingDamageRatio", 0.5),
            bullet_spawn_distance: v(pb, "fire.m_fBulletSpawnDistance", 150.0),
            bolt_spawn_distance: v(cb, "fire.m_fArrowSpawnDistance", 150.0),
            // `EAINoiseLoudness4` = `m_LoudnessDistanceArray[15]` when the config predates the field.
            pistol_noise_radius: v(pb, "fire.ai_noise_radius", c.ai.loudness_distances.get(15).copied().unwrap_or(6000.0)),
            pistol_reserve: 10,
            pistol_equip: anim::exit_time(c, GUNS_SET, "Guns_Ready_Equip"),
            pistol_fire_recovery: anim::exit_time(c, PISTOL_SET, "Pistol_Fire"),
            pistol_reload: pistol_stages.total(0.0),
            pistol_reload_stages: pistol_stages,

            bolt_gravity_mult: v(cb, "projectile.m_fGravityMultiplier", 1.3),
            bolt_stealth_mult: v(cb, "projectile.m_fDamageMultiplier_Stealth", 2.0),
            bolt_damage: c.items.inventory.crossbow_ranged_damage.trunc() * c.items.gadgets.bolt.damage_mult,
            sleep_dart_damage: c.items.inventory.crossbow_ranged_damage.trunc() * c.items.gadgets.sleep_dart.damage_mult,
            sleep_dart_speed: v(cb, "fire.m_fArrowFireSpeed", 20000.0) * c.items.gadgets.sleep_dart.speed_mult,
            sleep_dart_gravity_mult: c.items.gadgets.sleep_dart.gravity_mult,
            incendiary_damage: c.items.inventory.crossbow_ranged_damage.trunc() * c.items.gadgets.incendiary_bolt.damage_mult,
            incendiary_speed: v(cb, "fire.m_fArrowFireSpeed", 20000.0) * c.items.gadgets.incendiary_bolt.speed_mult,
            incendiary_gravity_mult: c.items.gadgets.incendiary_bolt.gravity_mult,
            bolt_speed_mult: v(cb, "projectile.m_fSpeedMultiplier", 1.0),
            sleep_dart_speed_mult: c.items.gadgets.sleep_dart.speed_mult,
            incendiary_speed_mult: c.items.gadgets.incendiary_bolt.speed_mult,
            projectile_windblast_behavior: homing_behavior(),
            projectile_mass: v(cb, "projectile.m_fMass", 1.0),
            projectile_windblast_homing_time: v(cb, "projectile.m_fWindblastHomingTime", homing_time()),
            projectile_windblast_velocity_mult: v(cb, "projectile.m_fWindblastVelocityMultiplier", homing_mult()),
            reach_min_speed: c.items.inventory.melee_ray_speed[0],
            reach_speed_scale: c.items.inventory.melee_ray_speed[1],
            reach_speed_scale_max: c.items.inventory.melee_ray_speed[2],
            reach_scale_percent: v(m, "attack.m_fRayScalePercent", 0.5),
            // DisTweaks_FireCrossbow.m_fArrowFireSpeed x DisTweaks_Projectile.m_fSpeedMultiplier.
            bolt_speed: v(cb, "fire.m_fArrowFireSpeed", 20000.0) * v(cb, "projectile.m_fSpeedMultiplier", 1.0),
            bolt_limb_damage_cap: v(cb, "projectile.m_fLegsAndArmsDamageCap", 15.0),
            arrows: {
                let gd = &c.items.gadgets;
                let a = |b: &dishonored_ir::items::BoltConfig| ArrowDamage { stealth_mult: b.stealth_mult, limb_cap: b.limb_damage_cap, kill_on_headshot: b.kill_on_headshot };
                // The bolt's own values come from the crossbow's projectile tweak (the same
                // `Twk_Proj_Arrow`).
                let bolt = ArrowDamage { stealth_mult: v(cb, "projectile.m_fDamageMultiplier_Stealth", 2.0), limb_cap: v(cb, "projectile.m_fLegsAndArmsDamageCap", 15.0), ..a(&gd.bolt) };
                [bolt, a(&gd.sleep_dart), a(&gd.incendiary_bolt)]
            },
            bolt_headshot_mult: c.items.inventory.crossbow_headshot_mult.trunc(),
            crossbow_reserve: 10,
            crossbow_equip: anim::exit_time(c, GUNS_SET, "Guns_Ready_Equip"),
            crossbow_reload: crossbow_stages.total(0.0),
            crossbow_reload_stages: crossbow_stages,
            crossbow_fire_recovery: anim::exit_time(c, CROSSBOW_SET, "Crossbow_Fire"),
            // Crossbow_Fire_FastReload `AnimStateExit` over `m_FireReloadFast.m_fPlayRateOverride`.
            crossbow_fast_fire: anim::exit_time(c, CROSSBOW_SET, "Crossbow_Fire_FastReload") / v(cb, "fire.m_FireReloadFast.m_fPlayRateOverride", 1.0),
            crossbow_fire_notify: notify(CROSSBOW_MESH_SET, "Crossbow_Fire", "DishonoredNotify_FireProjectile"),
            crossbow_fast_fire_notify: notify(CROSSBOW_MESH_SET, "Crossbow_Fire_FastReload", "DishonoredNotify_FireProjectile"),

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
            windblast_start_delay: notify(POWERS_SET, "Powers_Cast_Windblast", "DisNotify_WindBlast"),

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
