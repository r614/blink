//! NPC perception and behaviour tuning, normalized from Arkane's AI tweaks
//! (`DisTweaks_PawnAttention`, `DisTweaks_Vision`, `DisTweaks_AIBehavior_*`,
//! `DisTweaks_AISubState_*`) and `DefaultAI.ini` / `DefaultNPC.ini`.
//! See `research/dishonored/AI.md`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The NPC class a pawn's brain tweak makes it, read from the behaviour in its brain's
/// Combat slot (`m_BehaviorTweak[i]` whose `m_DesignatedSlot` is `EAISlot_Combat`, its
/// `m_pSpawnedObjectClass`). Pawns whose brain has no combat behaviour are civilians.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NpcClass {
    /// No combat behaviour (`BrainTweaks_Civilian`): panic.
    #[default]
    Civilian,
    /// `DisBehaviorCombatEliteGuard` / `DisBehaviorCombatCityGuard` and the other sword
    /// fighters the port plays with the elite guard's combat.
    EliteGuard,
    /// `DisBehaviorMusicalCombat` (the music-box Overseer).
    MusicalOverseer,
    /// `DisBehaviorTallBoyCombat`.
    Tallboy,
    /// `DisBehaviorCombatWolfhound`.
    Wolfhound,
    /// The weepers' combat behaviour.
    Weeper,
}

impl NpcClass {
    /// The class of a combat behaviour class name (`None`: not a combat behaviour).
    pub fn from_combat_behavior(class: &str) -> NpcClass {
        match class {
            "DisBehaviorMusicalCombat" => NpcClass::MusicalOverseer,
            "DisBehaviorTallBoyCombat" => NpcClass::Tallboy,
            "DisBehaviorCombatWolfhound" => NpcClass::Wolfhound,
            c if c.to_ascii_lowercase().contains("weep") => NpcClass::Weeper,
            "" => NpcClass::Civilian,
            _ => NpcClass::EliteGuard,
        }
    }

    /// Has a combat behaviour (spawned as a fighter, never panics).
    pub fn fights(self) -> bool {
        self != NpcClass::Civilian
    }
}

/// Attention levels (`EDisAttentionLevel`).
pub const DAL_UNAWARE: u8 = 0;
pub const DAL_HEAD_TRACK: u8 = 1;
pub const DAL_TURN_TO_FACE: u8 = 2;
pub const DAL_INVESTIGATE: u8 = 3;
pub const DAL_BUSTED: u8 = 4;
/// `DALFI_MaxAttention` (only as an increase target).
pub const DALFI_MAX_ATTENTION: u8 = 5;

/// `DisTweaks_PawnAttention.DisAttentionIncrease`: add `amount`, then raise the attention to
/// at least the start threshold of `to_level` (`EDisAttentionLevelForIncrease`: 0 None,
/// 1 HeadTrack, 2 TurnToFace, 3 Investigate, 4 Busted, 5 MaxAttention). CONFIRMED in the
/// pending-change pass of `DisAttentionInfo_Complex` (0x00b60ca0): the amount is accumulated
/// first, then the value is floored at the threshold, so an increase with no amount (the
/// struct default 0) sets the attention straight to its threshold.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct AttentionIncrease {
    pub amount: f32,
    pub to_level: u8,
}

/// One `DisTweaks_PawnAttention` profile (attention an NPC builds toward one target).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttentionConfig {
    /// Attention lost per second while the target is not perceived.
    pub leak_rate: f32,
    pub max: f32,
    /// `m_AttentionThresholds[HeadTrack, TurnToFace, Investigate, Busted]`.
    pub start: [f32; 4],
    pub stop: [f32; 4],
    pub danger_sound: AttentionIncrease,
    pub threatening_sound: AttentionIncrease,
    pub anomaly_sound: AttentionIncrease,
    pub help_sound: AttentionIncrease,
    pub alarm_sound: AttentionIncrease,
    pub death_rattle: AttentionIncrease,
    pub attacked_by: AttentionIncrease,
    pub attacked_enemy: AttentionIncrease,
    pub discovered_corpse: AttentionIncrease,
    pub ally_combat_target: AttentionIncrease,
    pub ally_panic_target: AttentionIncrease,
    pub ally_search_target: AttentionIncrease,
    /// Attention per second while touching the target.
    pub touching_rate: f32,
    /// `m_SeeingPawnAttentionIncreaseScalar_ToPlayer` (Normal difficulty).
    pub seeing_scalar_player: f32,
    pub seeing_scalar_npc: f32,
    pub distance_to_lose_sight: f32,
    /// `m_DistanceValueTuning`: maps `1 - normalized vision distance` to the seen value; the
    /// target's `Attribute_Visibility` is its operand (`DVCTPAO_Add`).
    #[serde(default = "VisTuning::guard_distance")]
    pub distance_tuning: VisTuning,
    /// `m_bClampTargetVisibility`: clamp the seen value to [0, 1] before the scalar.
    #[serde(default = "yes")]
    pub clamp_target_visibility: bool,
    /// `m_AttentionThresholds[i].m_fStoppingVariance`: each brain rolls its stop thresholds
    /// once as `stop + uniform(-variance, variance)`.
    #[serde(default = "guard_variance")]
    pub stopping_variance: [f32; 4],
    /// `m_ReasonSettings[EDisAttentionChangeReasonType].m_bCanIncreaseToBusted`, one bit per
    /// reason (`DACRT_*`).
    #[serde(default = "guard_can_bust")]
    pub can_bust: u32,
    /// `m_fSeeingPawnAttentionIncreaseScalar_ToDeadNPC`.
    #[serde(default = "ten")]
    pub seeing_scalar_dead_npc: f32,
    /// `m_fMinRangeToUnsightedBustedProxy`: an unsighted target cannot stay Busted once the
    /// NPC is this close to the proxy while the target is farther from it.
    #[serde(default = "min_range_unsighted")]
    pub min_range_unsighted_busted_proxy: f32,
    /// `m_IncreaseForMagic` (+0x180): the change a magic witness that is not Busted on the caster
    /// gets toward it (reason Magic, 0x00b41190).
    #[serde(default = "magic_increase")]
    pub magic: AttentionIncrease,
}

fn magic_increase() -> AttentionIncrease {
    AttentionIncrease { amount: 0.1, to_level: 1 }
}

fn yes() -> bool {
    true
}
fn ten() -> f32 {
    10.0
}
fn min_range_unsighted() -> f32 {
    250.0
}
fn guard_variance() -> [f32; 4] {
    [0.025; 4]
}
/// Reasons allowed to raise attention to Busted in the guard and neutral profiles:
/// HeardHelpRequest (6), SeeAllyCombat (10), TalkedTo (12), Magic (13), HideOutTransition
/// (14), AttackedBy (16), AttackedEnemy (17), Touched (18), Seen (19), Psychic (20).
fn guard_can_bust() -> u32 {
    [6u32, 10, 12, 13, 14, 16, 17, 18, 19, 20].iter().fold(0, |m, b| m | (1 << b))
}

/// `EDisAttentionChangeReasonType` (DisAttentionDefs).
pub mod reason {
    pub const DRAIN: u8 = 0;
    pub const FOUND_CORPSE: u8 = 1;
    pub const HEARD_DEATH: u8 = 2;
    pub const ALARM: u8 = 3;
    pub const HEARD_NOTICE_REQUEST: u8 = 4;
    pub const HEARD_SEARCH_REQUEST: u8 = 5;
    pub const HEARD_HELP_REQUEST: u8 = 6;
    pub const HEARD: u8 = 7;
    pub const SAW_DEATH: u8 = 8;
    pub const SEE_ALLY_SEARCH: u8 = 9;
    pub const SEE_ALLY_COMBAT: u8 = 10;
    pub const SEE_ALLY_PANIC: u8 = 11;
    pub const TALKED_TO: u8 = 12;
    pub const MAGIC: u8 = 13;
    pub const HIDEOUT_TRANSITION: u8 = 14;
    pub const ATTACKED_BY_INDIRECT: u8 = 15;
    pub const ATTACKED_BY: u8 = 16;
    pub const ATTACKED_ENEMY: u8 = 17;
    pub const TOUCHED: u8 = 18;
    pub const SEEN: u8 = 19;
    pub const PSYCHIC: u8 = 20;
}

/// `DisVisCalcTuningParams` (DisAttentionDefs): a curve or a formula of an input in [0, 1],
/// combined with an operand. Native evaluation at 0x00b0b350: with `use_points` the
/// piecewise-linear curve (clamped to the first/last output), otherwise
/// `(x^exponent + bias) * scalar`; then `+ operand` (`DVCTPAO_Add`) or `* operand`
/// (`DVCTPAO_Multiply`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VisTuning {
    pub multiply: bool,
    pub scalar: f32,
    pub bias: f32,
    pub exponent: f32,
    pub use_points: bool,
    pub points: Vec<[f32; 2]>,
}

impl VisTuning {
    /// `m_DistanceValueTuning` of the guard and neutral attention profiles.
    pub fn guard_distance() -> Self {
        VisTuning { multiply: false, scalar: 3.0, bias: 0.0, exponent: 4.0, use_points: true, points: vec![[0.55, 0.0], [0.875, 0.8], [0.93, 2.0]] }
    }

    /// Plain formula `x * scalar`, added to the operand (`Twk_PlayerVisibility` light and
    /// speed tunings).
    pub fn linear_add(scalar: f32) -> Self {
        VisTuning { multiply: false, scalar, bias: 0.0, exponent: 1.0, use_points: false, points: Vec::new() }
    }

    pub fn value(&self, x: f32) -> f32 {
        if !self.use_points {
            return (x.powf(self.exponent) + self.bias) * self.scalar;
        }
        let p = &self.points;
        match p.len() {
            0 => 0.0,
            _ if x <= p[0][0] => p[0][1],
            n if x >= p[n - 1][0] => p[n - 1][1],
            _ => {
                for w in p.windows(2) {
                    if x >= w[0][0] && x <= w[1][0] {
                        let span = w[1][0] - w[0][0];
                        if span == 0.0 {
                            return (w[0][1] + w[1][1]) * 0.5;
                        }
                        return (x - w[0][0]) / span * (w[1][1] - w[0][1]) + w[0][1];
                    }
                }
                0.0
            }
        }
    }

    pub fn apply(&self, x: f32, operand: f32) -> f32 {
        let v = self.value(x);
        if self.multiply { v * operand } else { v + operand }
    }
}

/// `Twk_PlayerVisibility` (`DisTweaks_PlayerVisibility`): how the player's
/// `Attribute_Visibility` is built by `DishonoredVisibilityComponent` (0x00ad1590): the light
/// tuning of the normalized light value is the operand of the speed tuning of the speed value
/// (one modifier), plus the stance bonus (a second modifier).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerVisibilityConfig {
    pub light: VisTuning,
    pub speed: VisTuning,
    pub bonus_autocrouching: f32,
    pub bonus_sneaking: f32,
    pub bonus_sliding: f32,
    pub bonus_standing: f32,
    pub bonus_walking: f32,
    pub bonus_sprinting: f32,
    pub bonus_swimming: f32,
    /// `m_fVisLevelBonus_SwimmingOnSurface` (the swim state with the head out of the water).
    #[serde(default)]
    pub bonus_swimming_surface: f32,
    /// `m_bForceZeroSpeedWhileSneaking`.
    pub zero_speed_sneaking: bool,
    /// `Twk_GlobalPlayerVisSettings.m_fLightValue{Darkest,Brightest}Val`.
    pub light_darkest: f32,
    pub light_brightest: f32,
    /// The player's `Attribute_GroundSpeedSprint`: the speed value is the horizontal speed
    /// divided by it, unclamped (0x00ac33b0).
    #[serde(default = "ground_speed_sprint")]
    pub ground_speed_sprint: f32,
}

fn ground_speed_sprint() -> f32 {
    600.0
}

impl Default for PlayerVisibilityConfig {
    fn default() -> Self {
        PlayerVisibilityConfig {
            light: VisTuning::linear_add(0.1),
            speed: VisTuning::linear_add(0.25),
            bonus_autocrouching: -1.0,
            bonus_sneaking: 0.0,
            bonus_sliding: 0.0,
            bonus_standing: 0.0,
            bonus_walking: 0.0,
            bonus_sprinting: 0.0,
            bonus_swimming: -1.0,
            bonus_swimming_surface: 0.0,
            zero_speed_sneaking: true,
            light_darkest: 0.05,
            light_brightest: 1.0,
            ground_speed_sprint: 600.0,
        }
    }
}

/// One melee attack context of an attack pattern (`DisTweaks_AIAttackPattern
/// .m_ItemContextOverrides`) with the matching `DisGlobalCombatManager.m_fInhibitionTime*`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeleeContextConfig {
    pub name: String,
    /// Per-NPC cooldown after use (`m_ContextCooldown`, seconds).
    pub cooldown: [f32; 2],
    /// After any NPC uses it on a target, other NPCs may not use the same attack class on that
    /// target for this long (`m_NPCAttackHistory*` with `m_fInhibitionTime*`).
    pub inhibition: f32,
    /// Move kind (`dishonored_sim::input::npc_moves`) of the context class.
    #[serde(default)]
    pub kind: u8,
    /// Item-context range (`DisTweaks_ItemContext.m_fMinContextRange` / `m_fMaxContextRange`)
    /// and `m_fMaxContextHeightDifference`, uu.
    #[serde(default)]
    pub min_range: f32,
    #[serde(default = "big_range")]
    pub max_range: f32,
    #[serde(default = "context_height")]
    pub max_height: f32,
    /// `DisTweaks_MeleeAttack.m_fAttackDamageMultiplier` of the context (+0xCC): the hit deals
    /// `round(multiplier x (MeleeDamage + MeleeDamageBonus))` (0x00C07C80). 1.0 for every slash
    /// (class default); the elite guard sword's `NPCJumpAttack` and every `NPCBash` have 0.
    /// `None` (configs written before it was read): the elite sword's value for the class.
    #[serde(default)]
    pub damage_multiplier: Option<f32>,
}

impl MeleeContextConfig {
    /// The context's `m_fAttackDamageMultiplier` (see [`Self::damage_multiplier`]).
    pub fn attack_damage_multiplier(&self) -> f32 {
        self.damage_multiplier.unwrap_or_else(|| elite_sword_multiplier(&self.name))
    }
}

/// `m_fAttackDamageMultiplier` of `Twk_Inv_SwordEliteGuard`'s pawn-specific contexts: 0 for
/// `DisTweaks_NPCJumpAttack_4` and `DisTweaks_NPCBash_5`, 1 for the slashes.
pub fn elite_sword_multiplier(class: &str) -> f32 {
    if matches!(class, "DisItemContext_NPCJumpAttack" | "DisItemContext_NPCBash") { 0.0 } else { 1.0 }
}

fn big_range() -> f32 {
    f32::MAX
}
fn context_height() -> f32 {
    200.0
}

impl MeleeContextConfig {
    /// A context of an attack pattern with its class-default range (`DisTweaks_NPCAttack*`).
    pub fn of_class(name: &str, cooldown: [f32; 2], inhibition: f32) -> Self {
        let (kind, min_range, max_range, max_height) = context_class_defaults(name);
        let damage_multiplier = Some(elite_sword_multiplier(name));
        MeleeContextConfig { name: name.into(), cooldown, inhibition, kind, min_range, max_range, max_height, damage_multiplier }
    }
}

/// Move kind and class-default range of an NPC item context (`DisTweaks_NPCAttackShort` 250,
/// `StepBack` 100, `Medium` 350, `Long` 450, `NPCJumpAttack` 550-800 with a 100 uu height
/// limit; `DisTweaks_MeleeAttackNPCBase.m_fMaxContextHeightDifference` 200 otherwise).
pub fn context_class_defaults(name: &str) -> (u8, f32, f32, f32) {
    match name {
        "DisItemContext_NPCAttackShort" => (1, 0.0, 250.0, 200.0),
        "DisItemContext_NPCAttackStepBack" => (2, 0.0, 100.0, 200.0),
        "DisItemContext_NPCAttackMedium" => (3, 0.0, 350.0, 200.0),
        "DisItemContext_NPCAttackLong" => (4, 0.0, 450.0, 200.0),
        "DisItemContext_NPCJumpAttack" => (5, 550.0, 800.0, 100.0),
        "DisItemContext_NPCSideStep" => (6, 0.0, f32::MAX, 200.0),
        "DisItemContext_NPCBackStep" => (7, 0.0, f32::MAX, 200.0),
        "DisItemContext_NPCFireGun" => (8, 0.0, f32::MAX, 200.0),
        _ => (0, 0.0, f32::MAX, 200.0),
    }
}

/// Elite guard attack contexts (`TwkAttackPattern_BerserkerElite` overrides in tweak order,
/// with `DisGlobalCombatManager.m_fInhibitionTime*`). The dodges and the gun are handled by
/// their own rules ([`DodgeConfig`], [`PistolConfig`]).
pub fn default_melee_contexts() -> Vec<MeleeContextConfig> {
    vec![
        // Ranges of the elite sword's pawn-specific contexts
        // (`Twk_Inv_SwordEliteGuard.DisTweaks_InventoryItem_PawnSpecific_1`).
        MeleeContextConfig { min_range: 50.0, max_range: 235.0, max_height: 300.0, ..MeleeContextConfig::of_class("DisItemContext_NPCAttackShort", [1.0, 3.0], 2.0) },
        MeleeContextConfig { min_range: 0.0, max_range: 170.0, max_height: 100.0, ..MeleeContextConfig::of_class("DisItemContext_NPCAttackStepBack", [0.5, 2.0], 1.0) },
        MeleeContextConfig { min_range: 235.0, max_range: 375.0, max_height: 300.0, ..MeleeContextConfig::of_class("DisItemContext_NPCAttackMedium", [3.0, 5.0], 1.0) },
        MeleeContextConfig { min_range: 375.0, max_range: 570.0, max_height: 200.0, ..MeleeContextConfig::of_class("DisItemContext_NPCAttackLong", [15.0, 20.0], 0.8) },
        // The elite sword's jump attack deals no damage itself (`m_fAttackDamageMultiplier` 0).
        MeleeContextConfig { min_range: 730.0, max_range: 850.0, max_height: 100.0, ..MeleeContextConfig::of_class("DisItemContext_NPCJumpAttack", [15.0, 20.0], 3.0) },
    ]
}

/// Dodges and attack pacing (`DisTweaks_AISubProcess_ManageAttacks` of the elite combat
/// behaviour, `DisTweaks_NPCSideStep` / `DisTweaks_NPCBackStep`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DodgeConfig {
    /// `m_AttacksPerFlurry`: stamina-draining attacks before a `m_StaminaRecoveryDuration`
    /// pause (0x00b47880).
    pub attacks_per_flurry: [i32; 2],
    pub stamina_recovery: [f32; 2],
    /// `m_TimeForThreatToCauseSideStep`, `m_fAngleOfPerceivedMeleeThreat` /
    /// `m_fAngleOfPerceivedRangedThreat` (degrees).
    pub time_for_threat_side_step: [f32; 2],
    pub angle_melee_threat: f32,
    pub angle_ranged_threat: f32,
    /// `m_IncomingAttacksBeforeBackStep`, `m_fTimeToForgiveIncomingAttacks`.
    pub incoming_attacks_before_back_step: [i32; 2],
    pub forgive_incoming_attacks: f32,
    pub side_step_distance: f32,
    pub back_step_distance: f32,
    /// `DisGlobalCombatManager.m_fInhibitionTimeSideStep` / `BackStep`.
    pub side_step_inhibition: f32,
    pub back_step_inhibition: f32,
    /// `m_fAggressionDeltaPerDamage`: added to `m_fAggression` (clamped to [0, 1]) by the
    /// `EAIStimID_AttackedByEnemy` handler (0x00b3e070).
    pub aggression_delta_per_damage: f32,
    /// `m_fAggressionRegenRate` per second (0x00b39e50, every tick of the process).
    pub aggression_regen_rate: f32,
    /// `m_fCooldownFactorAtMinAggression`: a chosen attack's cooldown is scaled by
    /// `(1 - aggression) * factor + 1` (0x00b3deb0).
    pub cooldown_factor_at_min_aggression: f32,
    /// `m_fStalemateAllowanceTime`, `m_fBlockedAttackSensitivity` (kept for the formation and
    /// block-breaker rules, which are not ported).
    pub stalemate_allowance: f32,
    pub blocked_attack_sensitivity: f32,
}

impl Default for DodgeConfig {
    fn default() -> Self {
        DodgeConfig {
            aggression_delta_per_damage: -0.5,
            aggression_regen_rate: 0.1,
            cooldown_factor_at_min_aggression: 1.0,
            stalemate_allowance: 3.0,
            blocked_attack_sensitivity: 7.0,
            attacks_per_flurry: [2, 3],
            stamina_recovery: [4.0, 5.0],
            time_for_threat_side_step: [2.0, 3.0],
            angle_melee_threat: 30.0,
            angle_ranged_threat: 10.0,
            incoming_attacks_before_back_step: [1, 1],
            forgive_incoming_attacks: 5.0,
            side_step_distance: 250.0,
            back_step_distance: 300.0,
            side_step_inhibition: 6.0,
            back_step_inhibition: 8.0,
        }
    }
}

/// The elite guard's pistol (`DisTweaks_AISubState_FirePistol`, `DisItemContext_NPCFireGun`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PistolConfig {
    /// `m_TimeToFirstShot` (the fire timer set on entering the sub-state, 0x00b32c50).
    pub time_to_first_shot: [f32; 2],
    /// `m_TimeBeforeGivingUp`.
    pub time_before_giving_up: [f32; 2],
    pub point_blank_distance: f32,
    pub chance_fire_again: f32,
    /// FireGun context range (`m_fMinContextRange` / `m_fMaxContextRange`): starting to shoot
    /// needs the target within (min, max); continuing needs it within max (0x00b0e220).
    pub min_range: f32,
    pub max_range: f32,
}

impl Default for PistolConfig {
    fn default() -> Self {
        PistolConfig { time_to_first_shot: [0.75, 1.25], time_before_giving_up: [0.3, 1.0], point_blank_distance: 80.0, chance_fire_again: 1.0, min_range: 250.0, max_range: 3000.0 }
    }
}

/// `DisTweaks_RatSwarm` (a swarm of rats, `DisRatSwarm`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RatSwarmConfig {
    /// `EDisRatSwarmType`: 0 ambient, 1 random, 2 scripted, 3 Devouring Swarm.
    pub swarm_type: u8,
    pub initial_rat_count_to_be_aggressive: i32,
    pub pawn_detection_radius: f32,
    pub pawn_detection_radius_when_busy: f32,
    pub pawn_escape_radius: f32,
    pub player_detection_delay_after_spawn: f32,
    pub pawn_unreachable_delay: f32,
    pub destination_radius: f32,
    pub pawn_visibility_check_interval: f32,
    pub idle_away_from_pawn_trigger_dist: f32,
    pub idle_away_from_pawn_trigger_duration: f32,
    pub idle_away_from_pawn_move_dist: f32,
    pub idle_away_from_pawn_count: [i32; 2],
    pub idle_roaming_radius: f32,
    pub idle_roaming_max_pause: f32,
    pub min_rat_count_per_attack_group: i32,
    pub min_rat_count_for_new_attack_group: i32,
    pub forced_attack_detection_radius: f32,
    pub initial_delay_before_dealing_damage: f32,
    pub damage_per_bite: f32,
    pub min_damage: f32,
    pub max_damage: f32,
    pub damage_time_interval: f32,
    pub pawn_slowdown_percent: f32,
    pub pawn_slowdown_duration: f32,
    pub eat_required_rat_count: i32,
    pub eat_startup_duration: f32,
    pub eat_startup_delay_after_kill: f32,
    pub eat_per_limb_duration: f32,
    pub never_flee_systemically: bool,
    pub flee_attempt_distance: [f32; 2],
    pub flee_retry_delay: f32,
}

impl Default for RatSwarmConfig {
    /// `Default__DisTweaks_RatSwarm`.
    fn default() -> Self {
        RatSwarmConfig {
            swarm_type: 0,
            initial_rat_count_to_be_aggressive: 10,
            pawn_detection_radius: 1000.0,
            pawn_detection_radius_when_busy: 400.0,
            pawn_escape_radius: 1500.0,
            player_detection_delay_after_spawn: 2.0,
            pawn_unreachable_delay: 3.0,
            destination_radius: 150.0,
            pawn_visibility_check_interval: 1.5,
            idle_away_from_pawn_trigger_dist: 250.0,
            idle_away_from_pawn_trigger_duration: 0.5,
            idle_away_from_pawn_move_dist: 300.0,
            idle_away_from_pawn_count: [3, 5],
            idle_roaming_radius: 50.0,
            idle_roaming_max_pause: 5.0,
            min_rat_count_per_attack_group: 8,
            min_rat_count_for_new_attack_group: 20,
            forced_attack_detection_radius: 250.0,
            initial_delay_before_dealing_damage: 1.0,
            damage_per_bite: 0.2,
            min_damage: 1.0,
            max_damage: 10.0,
            damage_time_interval: 0.5,
            pawn_slowdown_percent: 40.0,
            pawn_slowdown_duration: 1.0,
            eat_required_rat_count: 5,
            eat_startup_duration: 2.0,
            eat_startup_delay_after_kill: 1.0,
            eat_per_limb_duration: 1.85,
            never_flee_systemically: false,
            flee_attempt_distance: [1000.0, 5000.0],
            flee_retry_delay: 3.0,
        }
    }
}

/// A `DishonoredRoute` (an engine `Route` of `DishonoredNavPoint`s) used by `DisBehaviorPatrol`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PatrolRoute {
    pub name: String,
    /// `ERouteType`: 0 Linear (once, then adopt another route), 1 Loop (back and forth),
    /// 2 Circle (wrap; the `DishonoredRoute` default).
    pub kind: u8,
    /// `m_Capacity` (-1 = unlimited).
    pub capacity: i32,
    pub points: Vec<RoutePoint>,
    /// `m_SupportedSquads`: when set, only pawns of these squads may adopt the route.
    #[serde(default)]
    pub squads: Vec<String>,
    /// `m_AdoptionRange` (negative: any distance).
    #[serde(default = "minus_one")]
    pub adoption_range: f32,
    /// `m_NeglectThreshold`: a route nobody walked for longer is preferred.
    #[serde(default = "thirty")]
    pub neglect_threshold: f32,
    /// `m_bIsActive`.
    #[serde(default = "yes")]
    pub active: bool,
    /// The route actor's location (adoption range origin).
    #[serde(default)]
    pub location: glam::Vec3,
}

fn minus_one() -> f32 {
    -1.0
}
fn thirty() -> f32 {
    30.0
}

impl Default for PatrolRoute {
    fn default() -> Self {
        PatrolRoute { name: String::new(), kind: 2, capacity: -1, points: Vec::new(), squads: Vec::new(), adoption_range: -1.0, neglect_threshold: 30.0, active: true, location: glam::Vec3::ZERO }
    }
}

impl PatrolRoute {
    /// `DishonoredRoute` adoption check (0x00a4bc70): active, has points, the pawn's squad is
    /// supported, capacity left, and the pawn within `m_AdoptionRange`.
    pub fn adoptable(&self, users: i32, squad: &str, pos: glam::Vec3) -> bool {
        self.active
            && !self.points.is_empty()
            && (self.squads.is_empty() || self.squads.iter().any(|s| s.eq_ignore_ascii_case(squad)))
            && (self.capacity < 0 || users < self.capacity)
            && (self.adoption_range < 0.0 || pos.distance(self.location) < self.adoption_range)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RoutePoint {
    pub position: glam::Vec3,
    /// `DishonoredNavPoint.m_fGuardDuration` (0 = walk past).
    pub guard_duration: f32,
    /// `m_bGuardForever`.
    pub guard_forever: bool,
}

/// `DisTweaks_Vision`: two view cones.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VisionConfig {
    pub inner_fov_deg: f32,
    pub inner_range: f32,
    pub inner_up: f32,
    pub inner_down: f32,
    pub outer_fov_deg: f32,
    pub outer_range: f32,
    pub outer_up: f32,
    pub outer_down: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchConfig {
    /// `DishonoredSearchCrumbsComponent` (DefaultAI.ini).
    pub crumbs: u32,
    pub crumb_spacing: f32,
    pub crumb_lifetime: f32,
    pub heartbeat: f32,
    pub wander_min: f32,
    pub wander_max: f32,
    pub wander_angle_deg: f32,
    /// `TweaksBehavior_Search_Default`: wait after reaching the proxy before searching.
    pub cancel_investigate_wait: [f32; 2],
    /// `DisTweaks_AISubState_Investigate.m_fProximityThresholds` (most reasons).
    pub proximity: f32,
    pub reached_broadcast_range: f32,
    /// `TwkBehavior_StationarySearch_Default` duration (civilians).
    pub stationary_duration: [f32; 2],
    /// `m_fInvestigateStareAtUnreachableRadius` / `m_fInvestigateStareAtUnreachableMaxTime_s`
    /// (`DisBehaviorSearch` StareAtUnreachable, 0x00af4bd0 / 0x00af4c40 / 0x00af4440).
    #[serde(default = "stare_radius")]
    pub stare_radius: f32,
    #[serde(default = "stare_time")]
    pub stare_max_time: f32,
    /// `m_fSearchAnimCooldown`: set when the search animation (GenericAction) ends (0x00af4ab0).
    #[serde(default = "anim_cooldown")]
    pub anim_cooldown: f32,
    /// `DisTweaks_AISubState_Investigate`: `m_PathingFailRetryCount`,
    /// `m_fPathingFailDistanceScale`, `m_fPathingFailPointerSearchRange_Horiz/Up/Down`
    /// (the PathingFail handler 0x00b33850).
    #[serde(default = "one_u32")]
    pub pathing_fail_retries: u32,
    #[serde(default = "one_f32")]
    pub pathing_fail_scale: f32,
    #[serde(default = "pathing_fail_range")]
    pub pathing_fail_range: [f32; 3],
    /// `DisTweaks_AISubState_TrackTarget.m_fProximityThreshold` (a wander step is reached).
    #[serde(default = "track_proximity")]
    pub track_proximity: f32,
    /// Lengths (s) of the search GenericAction animation groups of the elite guard
    /// (`DisNPCAnimDefinitions_EliteGuard` SearchLostTarget / CorpseCheck with the sword prefix);
    /// 0 when the animation sets were not loaded.
    #[serde(default)]
    pub anim_lost_target: f32,
    #[serde(default)]
    pub anim_corpse_check: f32,
}

fn stare_radius() -> f32 {
    500.0
}
fn stare_time() -> f32 {
    5.0
}
fn anim_cooldown() -> f32 {
    10.0
}
fn one_u32() -> u32 {
    1
}
fn one_f32() -> f32 {
    1.0
}
fn pathing_fail_range() -> [f32; 3] {
    [500.0, 25.0, 500.0]
}
fn track_proximity() -> f32 {
    150.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PanicConfig {
    pub cower_distance: f32,
    pub flee_point_claim_radius: f32,
    pub flee_interception_distance: f32,
    pub free_flight_time: f32,
    pub forbidden_angle_deg: f32,
    pub forbidden_radius: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CombatAiConfig {
    pub reachable_melee_range: f32,
    pub reachable_melee_height: f32,
    pub aim_time_before_shooting: f32,
    pub gun_usage_start: [f32; 2],
    pub gun_usage_cooldown: [f32; 2],
    /// `DisGlobalCombatManager` skirmish caps (Normal): melee attackers per target / total.
    pub skirmish_inner: u32,
    pub skirmish_total: u32,
    /// Melee attack contexts in pattern order with cooldowns and group inhibition.
    #[serde(default = "default_melee_contexts")]
    pub melee_contexts: Vec<MeleeContextConfig>,
    #[serde(default)]
    pub dodge: DodgeConfig,
    #[serde(default)]
    pub pistol: PistolConfig,
    /// `Attribute_MeleeDamage` of the guards' sword by difficulty (Easy, Normal, Hard, Very Hard:
    /// `Twk_Inv_SwordEliteGuard`'s attributes, cooked into the level script packages; read when
    /// they are loaded, else these values read from them).
    #[serde(default = "d_npc_sword_damage")]
    pub npc_sword_damage: [f32; 4],
    /// The guard pawns' `Attribute_MeleeDamageBonus` (Normal).
    #[serde(default)]
    pub npc_melee_damage_bonus: f32,
    /// `Attribute_HealthMax` (Normal) of the guard archetype mission guards take: every
    /// `Pwn_EliteGuard_*` pawn tweak of the Boyle levels resolves to 50 (Lord Shaw 10, tallboys
    /// 150, weepers 30, aristocrats 10-11 are other archetypes).
    #[serde(default = "d_guard_health_max")]
    pub guard_health_max: f32,
}

/// Move codes of the special NPC classes' item contexts (`PlayerInput::npc_move`, after the
/// elite guard's in `dishonored_sim::input::npc_moves`).
pub mod special_moves {
    /// Wolfhound bite (`DisItemContext_NPCAttackMedium` of `Twk_Inv_WolfhoundBite`).
    pub const WH_BITE: u8 = 20;
    /// `DisItemContext_WHJumpAttack` (knockdown).
    pub const WH_JUMP_ATTACK: u8 = 21;
    /// `DisItemContext_WHArmAttack` (sync grab; not ported).
    pub const WH_ARM_ATTACK: u8 = 22;
    /// Tallboy leg stomp (`DisItemContext_AttackNPCTallboyMelee`).
    pub const TALLBOY_STOMP: u8 = 23;
    /// Tallboy bow (`DisItemContext_NPCFireBow`): aiming, the shot on the Secondary press.
    pub const TALLBOY_FIRE_BOW: u8 = 24;
    /// Music-box Overseer bash (`DisItemContext_NPCBash` of the music box).
    pub const OVERSEER_BASH: u8 = 25;
    /// Music-box Overseer playing the combat tune (`DisItemContext_NPCTune_Combat`).
    pub const OVERSEER_TUNE: u8 = 26;
    /// Weeper arm grab (`DisItemContext_WeeperGrab`).
    pub const WEEPER_GRAB: u8 = 27;
    /// Weeper coughing the plague cloud (`DishonoredNotify_StartPlague`).
    pub const WEEPER_COUGH: u8 = 28;
}

/// An attack item context of a special NPC class (`DisTweaks_ItemContext` of its weapon's
/// pawn-specific tweaks): range, height limit, facing angle, cooldown, the combat manager's group
/// inhibition and the damage of one hit (`ceil(m_fAttackDamageMultiplier x Attribute_MeleeDamage)`
/// of the weapon, Normal).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SpecialAttack {
    /// Item context class (`DisItemContext_WHJumpAttack`).
    pub name: String,
    /// Move code carried in `PlayerInput::npc_move` (`dishonored_sim::input::npc_moves`).
    pub kind: u8,
    pub min_range: f32,
    pub max_range: f32,
    /// `m_fIdealContextRange` (the attack pattern's ideal range, the shooting position's distance).
    pub ideal_range: f32,
    /// `m_fMaxContextHeightDifference` (no limit: `f32::MAX`).
    pub max_height: f32,
    /// `m_fAllowedAngle` (degrees off the pawn's facing).
    pub angle: f32,
    pub cooldown: [f32; 2],
    pub inhibition: f32,
    pub damage: f32,
    /// `m_MainAttack.m_bUnblockable`.
    pub unblockable: bool,
}

/// `DisTweaks_AISubProcess_ManageAttacks` of a special class's combat behaviour.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ManageAttacksConfig {
    pub attacks_per_flurry: [i32; 2],
    pub stamina_recovery: [f32; 2],
    pub aggression_regen_rate: f32,
    pub aggression_delta_per_damage: f32,
    pub cooldown_factor_at_min_aggression: f32,
}

impl Default for ManageAttacksConfig {
    fn default() -> Self {
        ManageAttacksConfig { attacks_per_flurry: [2, 3], stamina_recovery: [3.0, 4.0], aggression_regen_rate: 0.1, aggression_delta_per_damage: -0.5, cooldown_factor_at_min_aggression: 1.0 }
    }
}

/// The special NPC classes' combat tuning (research/dishonored/systems/ai.md F6), read from
/// their pawn, brain, behaviour and weapon tweaks (`dishonored_assets::ai_config`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SpecialNpcConfig {
    pub wolfhound: WolfhoundConfig,
    pub weeper: WeeperConfig,
    pub tallboy: TallboyConfig,
    pub overseer: MusicalOverseerConfig,
}

impl SpecialNpcConfig {
    /// `Attribute_HealthMax` (Normal) of a class's pawn tweak; `None`: the kind's default.
    pub fn health_max(&self, c: NpcClass) -> Option<f32> {
        let h = match c {
            NpcClass::Wolfhound => self.wolfhound.health_max,
            NpcClass::Weeper => self.weeper.health_max,
            NpcClass::Tallboy => self.tallboy.health_max,
            NpcClass::MusicalOverseer => self.overseer.health_max,
            _ => 0.0,
        };
        (h > 0.0).then_some(h)
    }

    /// The special attack with a move code (any class).
    pub fn attack(&self, kind: u8) -> Option<&SpecialAttack> {
        self.wolfhound.attacks.iter().chain(self.tallboy.attacks.iter()).chain(self.overseer.attacks.iter()).find(|a| a.kind == kind)
    }
}

/// `DisBehaviorCombatWolfhound` (`AI_Tweak_Behavior_WolfhoundCombat`) and the hound's bite
/// (`Twk_Inv_Wolfhound.Twk_Inv_WolfhoundBite`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WolfhoundConfig {
    pub health_max: f32,
    /// The bite's contexts in tweak order (AttackMedium, Left90, Right90, WHJumpAttack, WHArmAttack).
    pub attacks: Vec<SpecialAttack>,
    pub manage: ManageAttacksConfig,
    /// `m_fWalkDistance` of the engage sub-state: within it the hound walks (WalkAttack).
    pub walk_distance: f32,
    /// The hound's attention profiles (`Twk_Attn_Wolfhound_*`).
    pub unsuspecting: Option<AttentionConfig>,
    pub suspecting: Option<AttentionConfig>,
}

/// `DisBehaviorWeep` (`TweakBehaviors_Weep_Default`), the arm grab and the plague cloud of the
/// weeper pawn tweaks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WeeperConfig {
    pub health_max: f32,
    /// `m_fHomeRadius`, `m_LostTimer`.
    pub home_radius: f32,
    pub lost_timer: [f32; 2],
    /// `m_fCoughingCooldown`.
    pub coughing_cooldown: [f32; 2],
    /// `m_SimultaneousRunnerAllowed`, `m_fTimeBeforeRunning`.
    pub runners_allowed: u32,
    pub time_before_running: f32,
    /// `m_StandStillWeeping` after a grab.
    pub stand_still: [f32; 2],
    /// TakePosition arrival threshold of the follow sub-state.
    pub follow_threshold: f32,
    /// `DisTweaks_WeeperGrab`: range, feet height difference, angle, length, group inhibition.
    pub grab_range: f32,
    pub grab_height: f32,
    pub grab_angle: f32,
    pub grab_duration: f32,
    pub grab_inhibition: f32,
    /// Plague cloud of the pawn tweak: `m_fPlagueRadius`, `m_fPlagueDmgAngle`,
    /// `m_iPlagueDmgPerTick`, `m_fPlagueDmgePeriod`, `m_fPlagueDmgLength`.
    pub plague_radius: f32,
    pub plague_angle: f32,
    pub plague_damage: f32,
    pub plague_period: f32,
    pub plague_length: [f32; 2],
}

impl Default for WeeperConfig {
    /// `Pwn_WeeperMiddle_MTall_1` / `TweakBehaviors_Weep_Default` / `DisTweaks_WeeperGrab_2` values.
    fn default() -> Self {
        WeeperConfig {
            health_max: 30.0,
            home_radius: 1500.0,
            lost_timer: [1.0, 3.0],
            coughing_cooldown: [2.0, 5.0],
            runners_allowed: 3,
            time_before_running: 0.0,
            stand_still: [2.0, 5.0],
            follow_threshold: 110.0,
            grab_range: 400.0,
            grab_height: 50.0,
            grab_angle: 90.0,
            grab_duration: 1.33,
            grab_inhibition: 20.0,
            plague_radius: 350.0,
            plague_angle: 90.0,
            plague_damage: 4.0,
            plague_period: 0.5,
            plague_length: [2.0, 3.0],
        }
    }
}

/// `DisBehaviorTallBoyCombat` (`AI_BehaviorTweaks_TallBoy_Combat`) and its bow.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TallboyConfig {
    pub health_max: f32,
    /// Melee contexts (the leg stomp).
    pub attacks: Vec<SpecialAttack>,
    pub manage: ManageAttacksConfig,
    /// `DisTweaks_NPCFireBow`: ranges, angle, cooldown, projectile speed.
    pub bow: SpecialAttack,
    pub arrow_speed: f32,
    pub arrow_gravity_mult: f32,
    /// `m_fAlliesHoldFireRadius`: an ally within it around the target denies the shot (0x00bf7550).
    pub allies_hold_fire_radius: f32,
    /// `StateNPCMasterTallboyAim`: the shot needs the aim yaw within `m_fAimAlignYawDegrees` of the
    /// target for `m_fAimAlignTimeBeforeShooting` (0x00a87700, 0x00a7cd80).
    pub aim_align_deg: f32,
    pub aim_time: f32,
    /// `Attribute_MaxAccuracy` / `Attribute_MinAccuracy` (Normal) of the pawn: the hit chance
    /// runs from the first at the bow's min range to the second at its max range (0x00b732e0).
    pub accuracy: [f32; 2],
    /// `m_fCheatAngleDeviation`: a miss aims within this angle of the line to the target.
    pub miss_angle: f32,
    /// `GravityZ` of the world (the ballistic solve, 0x00c3f2a0).
    pub gravity_z: f32,
    /// The arrow's explosion (`TallboyArrowExplosion_Twk`).
    pub explosion: crate::items::ExplosionConfig,
    /// FindShootingPosition: avoidance radius, give-up time without progress.
    pub fsp_avoidance: f32,
    pub fsp_give_up: f32,
}

/// `DisBehaviorMusicalCombat` and the music box (`Twk_Weapon_MusicBox_Default`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MusicalOverseerConfig {
    pub health_max: f32,
    /// The bash (`DisTweaks_NPCBash`).
    pub attacks: Vec<SpecialAttack>,
    /// The combat tune (`DisTweaks_NPCTune_Combat`): range, angle, cooldown.
    pub tune: SpecialAttack,
    /// `m_fMusicEffectPeriod` of its `DisTweaks_Tune`, `m_iMusicEffectDamage`.
    pub tune_period: f32,
    pub tune_damage: f32,
    /// `m_fStopWhenNotSeeingTimer`, `m_fStopWhenOutOfRangeTimer`.
    pub stop_not_seeing: f32,
    pub stop_out_of_range: f32,
    /// `m_fRepulseForceRange` (uu/s per second, at the far and near end of the range).
    pub repulse: [f32; 2],
    /// `DisTweaks_Tune_Protection.m_fMusicEffectRadius_InhibitPowers`: within it a pawn cannot use
    /// powers while either tune plays (0x00bd7390).
    pub inhibit_powers_radius: f32,
    /// `m_fIdealContextRange` of the protection tune: the distance MaintainDistance closes to.
    pub ideal_distance: f32,
    /// The player's `Attribute_MusicDamageReduction` (Normal, `Twk_Pawn_Corvo_Release`): Musical
    /// damage is `round(dmg x (1 - reduction))` (0x00b5ccd0).
    pub player_music_reduction: f32,
}

/// `EAINoiseContext` (DisGlobalEnums, in enum order).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NoiseContext {
    AmbientWhiteNoise,
    #[default]
    Anomaly,
    Footfall,
    Threatening,
    Danger,
    Help,
    AlarmBell,
    RatSwarm,
    ImpendingExplosion,
    WolZap,
    AllySearchRequest,
    DeathRattle,
    AllyNotice,
    FlareCombustion,
    WatchTowerAttack,
}

impl NoiseContext {
    pub const ALL: [NoiseContext; 15] = [
        NoiseContext::AmbientWhiteNoise,
        NoiseContext::Anomaly,
        NoiseContext::Footfall,
        NoiseContext::Threatening,
        NoiseContext::Danger,
        NoiseContext::Help,
        NoiseContext::AlarmBell,
        NoiseContext::RatSwarm,
        NoiseContext::ImpendingExplosion,
        NoiseContext::WolZap,
        NoiseContext::AllySearchRequest,
        NoiseContext::DeathRattle,
        NoiseContext::AllyNotice,
        NoiseContext::FlareCombustion,
        NoiseContext::WatchTowerAttack,
    ];

    /// From the enum name (`EAINoiseContext_Anomaly`).
    pub fn from_name(n: &str) -> Option<Self> {
        let s = n.strip_prefix("EAINoiseContext_")?;
        Self::ALL.iter().copied().find(|c| format!("{c:?}") == s)
    }
}

/// One AI noise source: the radius (`m_LoudnessDistanceArray[loudness]`, 0 = Silent: nothing
/// is emitted), the context, and the contact speed below which a speed-scaled contact makes no
/// noise (`m_VolumeSettings.m_fMinSpeed` when `m_bScaleWithSpeed`, else 0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AiNoise {
    pub radius: f32,
    pub context: NoiseContext,
    #[serde(default)]
    pub min_speed: f32,
}

impl AiNoise {
    pub const fn new(radius: f32, context: NoiseContext, min_speed: f32) -> Self {
        AiNoise { radius, context, min_speed }
    }
    pub const SILENT: AiNoise = AiNoise { radius: 0.0, context: NoiseContext::Anomaly, min_speed: 0.0 };

    /// Whether a contact at `speed` makes this noise (0x00BD6C20: silent below the minimum
    /// speed of a speed-scaled sub, or for loudness Silent).
    pub fn emits(&self, speed: f32) -> bool {
        self.radius > 0.0 && speed >= self.min_speed
    }
}

/// The AI noises the simulation emits, from the contact system's `DisContactSub_AINoise`
/// objects (`Phy_ContactSystem.ContactSystem.Dis_ContactSystem`, struck type
/// `DisContactType_Environment` unless named) and the weapon tweaks, with radii from
/// `m_LoudnessDistanceArray`. See research/dishonored/systems/stealth.md section 5.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseTable {
    /// Player footfalls by striker (`DishonoredPlayerPawn` vslot 398, 0x00AAB7D0):
    /// `Footfall_Run_Player` (the body tweak's `m_pFootfallContactType`), `_Sprint_Player`
    /// while sprinting, `_Sneak_Player` while sneaking, `_Run_Player_Silent` with
    /// `Attribute_SilentRunning` / `Attribute_SilentSprinting`.
    pub footfall_run: AiNoise,
    pub footfall_sprint: AiNoise,
    pub footfall_sneak: AiNoise,
    pub footfall_silent: AiNoise,
    /// Player landings (0x00AB53A0): `JumpLand`, `JumpLand_Stealth` (a fall begun sneaking,
    /// `StatePlayerMasterFalling.m_bIsStealthFall`), `JumpLand_Big` (the stun landing).
    pub land: AiNoise,
    pub land_stealth: AiNoise,
    pub land_big: AiNoise,
    /// `StatePlayerMasterFalling.m_fFall_Stun_Vel` (DefaultPlayerState.ini): a landing faster
    /// than this is the stun landing.
    pub fall_stun_speed: f32,
    /// `StatePlayerMasterFalling.m_fAnimateFallTime` (DefaultPlayerState.ini): a fall that did not
    /// come from a jump and lasted at most this long lands with no landing contact (0x00A5FF10).
    #[serde(default = "animate_fall_time")]
    pub animate_fall_time: f32,
    /// `Sword` -> `Body` (a hit) and `Sword` -> `Weapon` (a clash / block).
    pub sword_body: AiNoise,
    pub sword_weapon: AiNoise,
    /// `Bullet` -> `Environment` (the pistol bullet's impact).
    pub bullet_impact: AiNoise,
    /// `Corpse` -> `Environment` (a dead body touching the world) and `SleepingBody` ->
    /// `Environment` (an unconscious one). The sub's `m_fMinSpeed` 200 is inert: it only gates
    /// speed-scaled subs (`m_bScaleWithSpeed` false here), so every such contact makes the noise.
    pub corpse: AiNoise,
    pub sleeping_body: AiNoise,
    /// `DisTweaks_FirePistol.m_AINoiseLoudnessWhenFiring`, context Danger, at the shooter
    /// (0x00C2A3E0). The crossbow's fire tweak has no AI noise: firing a bolt is silent.
    pub pistol_fire: AiNoise,
    /// The player's locomotion clips' `DishonoredNotify_Footfall` times (Startup
    /// `Ply_Sword_Locomotion_as`): `Sword_Ready_Walk`, `_Run`, `_Sprint`.
    pub walk_clip: FootfallClip,
    pub run_clip: FootfallClip,
    pub sprint_clip: FootfallClip,
}

fn animate_fall_time() -> f32 {
    0.25
}

/// A looping locomotion clip's length and its footfall notify times (seconds).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FootfallClip {
    pub length: f32,
    pub footfalls: Vec<f32>,
}

impl Default for NoiseTable {
    /// The retail values (GOG; `dishonored_assets::ai_config` reloads them).
    fn default() -> Self {
        use NoiseContext::*;
        NoiseTable {
            footfall_run: AiNoise::new(600.0, Footfall, 0.0),
            footfall_sprint: AiNoise::new(1200.0, Anomaly, 0.0),
            footfall_sneak: AiNoise::new(0.0, Footfall, 0.0),
            footfall_silent: AiNoise::new(0.0, Footfall, 0.0),
            land: AiNoise::new(400.0, Anomaly, 0.0),
            land_stealth: AiNoise::new(0.0, Footfall, 0.0),
            land_big: AiNoise::new(800.0, Anomaly, 0.0),
            fall_stun_speed: 2200.0,
            animate_fall_time: 0.25,
            sword_body: AiNoise::new(800.0, Threatening, 0.0),
            sword_weapon: AiNoise::new(800.0, Threatening, 0.0),
            bullet_impact: AiNoise::new(3000.0, Danger, 0.0),
            corpse: AiNoise::new(300.0, Anomaly, 0.0),
            sleeping_body: AiNoise::new(300.0, Anomaly, 0.0),
            pistol_fire: AiNoise::new(6000.0, Danger, 0.0),
            walk_clip: FootfallClip { length: 2.0, footfalls: vec![1.0000004, 1.9150001] },
            run_clip: FootfallClip { length: 0.96666664, footfalls: vec![0.466667, 0.933333] },
            sprint_clip: FootfallClip { length: 0.56666666, footfalls: vec![0.266667, 0.533333] },
        }
    }
}
fn d_npc_sword_damage() -> [f32; 4] {
    [10.0, 15.0, 20.0, 30.0]
}
fn d_guard_health_max() -> f32 {
    50.0
}

/// The difficulty index the port plays at (`EDifficulty_Normal`): the tweak attributes'
/// `m_fBaseValue2_Normal` column and the `[difficulty]` arrays' index 1.
pub const DIFFICULTY_NORMAL: usize = 1;

/// NPC perception and behaviour tuning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AiConfig {
    pub vision: VisionConfig,
    pub guard_unsuspecting: AttentionConfig,
    pub guard_suspecting: AttentionConfig,
    pub civilian_unsuspecting: AttentionConfig,
    pub civilian_suspecting: AttentionConfig,
    /// Toward neutral pawns (guests at the party, players not yet hostile).
    pub neutral: AttentionConfig,
    /// `DisAINoiseManager.m_LoudnessDistanceArray` indexed by `EAINoiseLoudness`.
    pub loudness_distances: Vec<f32>,
    /// `DishonoredGlobalAIManager.m_ReactionDelay`.
    pub reaction_delay: [f32; 2],
    pub search: SearchConfig,
    pub panic: PanicConfig,
    pub combat: CombatAiConfig,
    /// `TweaksBehavior_Guard_Default.m_fAttentionSpan`.
    pub guard_attention_span: f32,
    /// `TweaksBehavior_TriggerAlarm_Party`.
    pub alarm_companion_range: f32,
    pub alarm_fight_radius: f32,
    /// `BrainTweaks_Civilian.m_TalkingLoudness` (help barks while fleeing), uu.
    pub civilian_talking_loudness: f32,
    /// `m_DeathRattleLoudness` of the guard pawn tweaks, uu.
    pub death_rattle_loudness: f32,
    /// `DishonoredNPCController` glance radii and time.
    pub auto_glance_radius: f32,
    pub auto_look_radius: f32,
    pub glance_time: f32,
    /// How the player's `Attribute_Visibility` is built (light, speed, stance).
    #[serde(default)]
    pub player_visibility: PlayerVisibilityConfig,
    /// `DishonoredGlobalAIManager.m_fGroupDistance` (DefaultAI.ini): range of the group
    /// stimuli such as `EAIStimID_AllyBusted`.
    #[serde(default = "group_distance")]
    pub group_distance: f32,
    /// `TweaksBehavior_TriggerAlarm_Party` (`DisTweaks_AIBehavior_TriggerAlarm`).
    #[serde(default)]
    pub trigger_alarm: TriggerAlarmConfig,
    /// `DisTweaks_AIBrain.m_fVisualDetectionRatioNear`: a sighting closer than this fraction
    /// of the vision range is a "near" detection (`DDH_SEARCH_NEAR_VDETECTION`).
    #[serde(default = "near_ratio")]
    pub visual_detection_ratio_near: f32,
    /// The default rat swarm tweak (`DisTweaks_RatSwarm`; Devouring Swarm and the level swarms
    /// use their own when loaded).
    #[serde(default)]
    pub rats: RatSwarmConfig,
    #[serde(default)]
    pub devouring_swarm: RatSwarmConfig,
    /// The AI noises the simulation emits (contact system and weapon tweaks).
    #[serde(default)]
    pub noise: NoiseTable,
    /// `DishonoredGlobalAIManager.m_MagicReactionDelay`: the delay of `EAIStimID_WitnessMagic`
    /// (its stimulus class's delay getter 0x00b016e0 -> 0x00c47840).
    #[serde(default = "magic_delay")]
    pub magic_reaction_delay: [f32; 2],
    /// Length (s) of the elite guard's MagicLostTarget animation (`Sword_Magic_OutOfSight`),
    /// the GenericAction of `DisBehaviorMagicResponse` when the caster disappeared; 0 unloaded.
    #[serde(default)]
    pub anim_magic_lost_target: f32,
    /// NPC locomotion configs (`ArkComponentLocomotionConfig`): the authored loop speeds and
    /// stance modifiers NPC moves use. Empty: NPCs move at the player's speeds.
    #[serde(default)]
    pub locomotion: Vec<crate::npc_anim::LocoConfig>,
    /// `DisTweaks_AISubProcess_AmbientBarks.m_fBarkInhibitionDuration` of the Patrol / Guard
    /// behaviours (`TweaksBehavior_Patrol_Default`): the silence after the behaviour starts
    /// before the ambient `DDH_PATROL_MISC` barks (0x00b47020).
    #[serde(default = "bark_inhibition")]
    pub ambient_bark_inhibition: f32,
    /// The special NPC classes (tallboys, hounds, weepers, music-box Overseers).
    #[serde(default)]
    pub special: SpecialNpcConfig,
    pub provenance: BTreeMap<String, String>,
}

fn bark_inhibition() -> f32 {
    12.0
}

fn magic_delay() -> [f32; 2] {
    [0.2, 0.3]
}

fn group_distance() -> f32 {
    1000.0
}
fn near_ratio() -> f32 {
    0.3
}

/// `DisTweaks_AIBehavior_TriggerAlarm`: when a guard goes to ring an alarm bell. The chance is
/// picked by what made the guard aware (sighting alone / in a group, found corpse, witnessed
/// death, help request); a guard is "alone" when no other pawn within `companion_range` is an
/// ally (`m_bCompanionMustBeAlly`) of the same squad (`m_bCompanionMustMatchSquad`)
/// (0x00af5fe0). An enemy within `fight_radius` is fought instead (0x00af6bd0).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TriggerAlarmConfig {
    pub chance_sighting_alone: f32,
    pub chance_sighting_in_group: f32,
    pub chance_found_corpse: f32,
    pub chance_witness_death: f32,
    pub chance_help: f32,
    pub fight_radius: f32,
    pub wait_before_ring: f32,
    pub companion_range: f32,
    pub companion_must_match_squad: bool,
    pub companion_must_be_ally: bool,
}

impl Default for TriggerAlarmConfig {
    /// `TweaksBehavior_TriggerAlarm_Party` values; the help chance keeps the class default 0.
    fn default() -> Self {
        TriggerAlarmConfig {
            chance_sighting_alone: 1.0,
            chance_sighting_in_group: 1.0,
            chance_found_corpse: 1.0,
            chance_witness_death: 1.0,
            chance_help: 0.0,
            fight_radius: 500.0,
            wait_before_ring: 0.25,
            companion_range: 2000.0,
            companion_must_match_squad: false,
            companion_must_be_ally: true,
        }
    }
}

/// `ERelationship` (DisGlobalEnums).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Relationship {
    Neutral,
    Ally,
    Enemy,
}

/// A `DisTweaks_Faction`: the factions it treats as allies and enemies, and the tweaks it
/// falls back to (a faction "is" every faction on its fallback chain).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Faction {
    pub name: String,
    pub allies: Vec<String>,
    pub enemies: Vec<String>,
    #[serde(default)]
    pub parents: Vec<String>,
}

/// The factions of a level (global `DisFaction_Defaults` plus the level-cooked ones such as
/// `Boyle_Factions`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FactionTable {
    pub factions: Vec<Faction>,
}

impl FactionTable {
    pub fn index(&self, name: &str) -> Option<u16> {
        self.factions.iter().position(|f| f.name.eq_ignore_ascii_case(name)).map(|i| i as u16)
    }

    /// Index of `name`, adding an empty faction when it is unknown (a faction referenced by a
    /// spawner but not loaded relates to nothing but itself).
    pub fn intern(&mut self, name: &str) -> u16 {
        match self.index(name) {
            Some(i) => i,
            None => {
                self.factions.push(Faction { name: name.to_string(), ..Default::default() });
                (self.factions.len() - 1) as u16
            }
        }
    }

    /// `a` is `b` or falls back to it (0x00c6f360: compares, then walks the fallback chain).
    pub fn isa(&self, a: u16, b: u16) -> bool {
        if a == b {
            return true;
        }
        let Some(fb) = self.factions.get(b as usize) else { return false };
        let mut seen = 0;
        let mut cur = a;
        while seen < 16 {
            let Some(fa) = self.factions.get(cur as usize) else { return false };
            let Some(p) = fa.parents.first().and_then(|p| self.index(p)) else { return false };
            if p == b || self.factions[p as usize].name.eq_ignore_ascii_case(&fb.name) {
                return true;
            }
            cur = p;
            seen += 1;
        }
        false
    }

    fn isa_name(&self, a: u16, b: &str) -> bool {
        self.index(b).is_some_and(|b| self.isa(a, b))
    }

    /// Relationship of faction `a` toward faction `b` (0x00c4ceb0, mapped by 0x00c5fd00): the
    /// same faction (either way along the fallback chain) is an ally; otherwise `a`'s
    /// `m_EnemyFactions` are checked before its `m_AlliedFactions`; anything else is neutral.
    /// Only `a`'s lists count: relationships are not symmetric.
    pub fn relation(&self, a: u16, b: u16) -> Relationship {
        if self.isa(b, a) || self.isa(a, b) {
            return Relationship::Ally;
        }
        let Some(fa) = self.factions.get(a as usize) else { return Relationship::Neutral };
        if fa.enemies.iter().any(|e| self.isa_name(b, e)) {
            return Relationship::Enemy;
        }
        if fa.allies.iter().any(|e| self.isa_name(b, e)) {
            return Relationship::Ally;
        }
        Relationship::Neutral
    }
}

/// A `DisForbiddenZone`: pawns of a forbidden faction inside it are trespassers, and the
/// owning factions treat trespassers as enemies (0x00c953f0 registers the trespasser for each
/// owning faction; 0x00c5fc20 makes them enemies of those factions).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ForbiddenZone {
    pub name: String,
    /// `m_bEnabled` (toggleable volume; Kismet can switch it).
    pub enabled: bool,
    pub owning: Vec<String>,
    pub forbidden: Vec<String>,
    /// World-space convex hulls of the brush, as planes (nx, ny, nz, d): inside when
    /// `n . p <= d` for every plane of one hull.
    pub hulls: Vec<Vec<[f32; 4]>>,
}

impl ForbiddenZone {
    pub fn contains(&self, p: glam::Vec3) -> bool {
        self.hulls.iter().any(|h| !h.is_empty() && h.iter().all(|pl| pl[0] * p.x + pl[1] * p.y + pl[2] * p.z <= pl[3] + 1e-3))
    }
}

/// A `DisAlarmBell` with its `DisTweaks_AlarmBell`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AlarmBellDef {
    pub name: String,
    pub package: String,
    pub position: glam::Vec3,
    /// `m_AlarmLoudness` as a radius (`m_LoudnessDistanceArray`).
    pub loudness: f32,
    /// `m_fNPCGoToAlarmCloseness`: guards farther than this do not go to ring it.
    pub go_closeness: f32,
    /// `m_fNPCUseCloseness`: distance at which a guard rings it.
    pub use_closeness: f32,
    pub time_between_uses: f32,
    /// `m_fSpawnerMinDistFromPlayer`: triggered spawners at least this far from the enemy are
    /// preferred (all are used when none is).
    pub spawner_min_dist: f32,
    /// `m_MaxSpawnedPawnsPerRing` (or the bell's `m_MaxSpawnedPawnsPerRingOverride`).
    pub max_spawns: i32,
    /// `m_fAlarmRingDelayBeforeAINoise`.
    pub ring_delay: f32,
    /// `m_TriggeredSpawners` (spawner actor names).
    pub spawners: Vec<String>,
}

/// A light for the AI light probe (the original samples the pawn's light environment).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProbeLight {
    /// 0 point, 1 spot, 2 directional.
    pub kind: u8,
    pub position: glam::Vec3,
    pub direction: glam::Vec3,
    /// Brightness times the colour's luminance.
    pub intensity: f32,
    pub radius: f32,
    pub falloff_exponent: f32,
    /// Spot cone half angles (degrees).
    pub inner_cone: f32,
    pub outer_cone: f32,
    pub casts_shadows: bool,
}

/// Per-map AI data from the level packages (cached by the content loader).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AiLevel {
    pub factions: FactionTable,
    /// Pawn tweak path -> its `m_pFactionTweak`.
    pub tweak_factions: BTreeMap<String, String>,
    /// The player pawn tweak's faction (`Faction_Corvo_Default`).
    pub player_faction: String,
    pub zones: Vec<ForbiddenZone>,
    pub alarms: Vec<AlarmBellDef>,
    #[serde(default)]
    pub lights: Vec<ProbeLight>,
    /// Sky lights of the level as an ambient brightness term for the light probe.
    #[serde(default)]
    pub sky_light: f32,
    /// The level's audio cells and portals (AI hearing between cells).
    #[serde(default)]
    pub audio: AudioCells,
    /// Pawn tweak path -> its generic dialog trees (`DisTweaks_NPCPawn.m_GenericDialogData`
    /// voices' `m_pDialogTree`), for AI barks.
    #[serde(default)]
    pub generic_trees: BTreeMap<String, Vec<String>>,
    /// The map's player light normalization (`DishonoredMapInfo.m_pMapPlayerVisSettings`:
    /// `m_fLightValueDarkestVal`, `m_fLightValueBrightestVal`), which replaces the global
    /// `Twk_GlobalPlayerVisSettings` (0x00BC8E60; Boyle Ext: `Boyle_Twk.Pvs_BoyleExterior`
    /// 0.2 / 0.5, measured on the original).
    #[serde(default)]
    pub player_light_range: Option<[f32; 2]>,
    /// The level's `DisStealthVolume`s.
    #[serde(default)]
    pub stealth_volumes: Vec<StealthVolume>,
    /// Pawn tweak path -> the class name of its brain's Combat-slot behaviour
    /// (`m_pBrainTweak` -> `m_BehaviorTweak[i].m_Difficulty[1]` with `m_DesignatedSlot`
    /// `EAISlot_Combat` -> `m_pSpawnedObjectClass`); absent: no combat behaviour.
    #[serde(default)]
    pub tweak_combat: BTreeMap<String, String>,
}

impl AiLevel {
    /// The NPC class of a pawn tweak (see [`NpcClass`]); `None` when the tweak's brain was not
    /// resolved.
    pub fn npc_class(&self, tweak: &str) -> Option<NpcClass> {
        self.tweak_combat.get(tweak).map(|c| NpcClass::from_combat_behavior(c))
    }
}

/// A `DisStealthVolume`: while the player touches it, the visibility component normalizes the
/// light with the volume's `m_pVisSettings` instead of the map's (0x00BD0120). Touching volumes
/// are kept in a list ordered by `m_StealthVolumePriority` (0x00C8C280 inserts before the first
/// lower priority), whose head is used; a head without settings falls through to the map's.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StealthVolume {
    pub name: String,
    /// `m_StealthVolumePriority`.
    pub priority: i32,
    /// `m_pVisSettings` darkest / brightest (`None`: no settings).
    pub light_range: Option<[f32; 2]>,
    /// Convex hulls (world planes `n.p <= d`, like [`ForbiddenZone`]).
    pub hulls: Vec<Vec<[f32; 4]>>,
}

impl StealthVolume {
    pub fn contains(&self, p: glam::Vec3) -> bool {
        self.hulls.iter().any(|h| !h.is_empty() && h.iter().all(|pl| pl[0] * p.x + pl[1] * p.y + pl[2] * p.z <= pl[3] + 1e-3))
    }
}

/// The light range of the stealth volumes at `p`: the highest-priority volume containing `p`
/// (the first of equal priorities, as the list keeps insertion order among them) and its
/// settings; `None` outside every volume or when that volume has no settings, where the map's
/// range applies (0x00BD0120 -> 0x00BC8E60).
pub fn stealth_light_range(volumes: &[StealthVolume], p: glam::Vec3) -> Option<[f32; 2]> {
    let mut head: Option<&StealthVolume> = None;
    for v in volumes.iter().filter(|v| v.contains(p)) {
        if head.is_none_or(|h| v.priority > h.priority) {
            head = Some(v);
        }
    }
    head.and_then(|v| v.light_range)
}

impl AiLevel {
    /// The player light range at `p` (stealth volume, else the map's; `None`: the global one).
    pub fn light_range_at(&self, p: glam::Vec3) -> Option<[f32; 2]> {
        stealth_light_range(&self.stealth_volumes, p).or(self.player_light_range)
    }
}

/// A `DishonoredAudioVolume`: an audio cell.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioCell {
    pub name: String,
    /// `m_AudioPropagationIndex`.
    pub index: u8,
    /// Convex hulls (world planes `n.p <= d`, like [`ForbiddenZone`]).
    pub hulls: Vec<Vec<[f32; 4]>>,
    /// `m_LinkedPortals` (indices into [`AudioCells::portals`]).
    pub portals: Vec<usize>,
}

impl AudioCell {
    pub fn contains(&self, p: glam::Vec3) -> bool {
        self.hulls.iter().any(|h| !h.is_empty() && h.iter().all(|pl| pl[0] * p.x + pl[1] * p.y + pl[2] * p.z <= pl[3] + 1e-3))
    }
}

/// A `DishonoredAudioPortal` (`GenericPortal`): a rectangle between two cells.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioPortal {
    pub name: String,
    /// `m_AudioPropagationIndex`.
    pub index: u8,
    /// Centre, in-plane axes (unit), normal (unit) and half extents along the axes.
    pub origin: glam::Vec3,
    pub x_axis: glam::Vec3,
    pub y_axis: glam::Vec3,
    pub normal: glam::Vec3,
    pub half: [f32; 2],
    /// `m_pCellA` / `m_pCellB` (indices into [`AudioCells::cells`]).
    pub cell_a: Option<usize>,
    pub cell_b: Option<usize>,
    /// `m_fOcclusion_HeardByAI`.
    pub occlusion_ai: f32,
}

/// `DisAudioPropagationInfo`: precomputed portal paths between cells. `keys` are sorted
/// (`portal_b | cell_b << 8 | portal_a << 16 | cell_a << 24` with propagation indices, cell A
/// the lower one); `values[i]` is `(byte offset into data) << 8 | intermediate portal count`;
/// at the offset: the path length as a u16 times `dist_coeff`, then the intermediate portals as
/// indices into `portals` (`m_Portals`, which hold indices into [`AudioCells::portals`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioPropagation {
    pub dist_coeff: f32,
    pub max_audible_distance: f32,
    pub keys: Vec<u32>,
    pub values: Vec<u32>,
    pub data: Vec<u8>,
    pub portals: Vec<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioCells {
    pub cells: Vec<AudioCell>,
    pub portals: Vec<AudioPortal>,
    #[serde(default)]
    pub propagation: Option<AudioPropagation>,
}

fn inc(amount: f32, to_level: u8) -> AttentionIncrease {
    AttentionIncrease { amount, to_level }
}

impl AttentionConfig {
    /// `Twk_Attn_Guard_Unsuspecting` (DishonoredGame.upk), also the fallback of the civilian
    /// `Twk_Attn_Noncombatant_Uns`. Used when the install is not available.
    pub fn guard_unsuspecting() -> Self {
        AttentionConfig {
            leak_rate: 0.025,
            max: 1.0,
            start: [0.125, 0.35, 0.6, 0.85],
            stop: [0.05, 0.25, 0.3, 0.8],
            danger_sound: inc(0.4, 3),
            threatening_sound: inc(0.3, 2),
            anomaly_sound: inc(0.2, 1),
            help_sound: inc(1.0, 3),
            alarm_sound: inc(0.0, 3),
            death_rattle: inc(0.4, 3),
            attacked_by: inc(0.0, 5),
            attacked_enemy: inc(0.0, 5),
            discovered_corpse: inc(0.0, 3),
            ally_combat_target: inc(1.0, 3),
            ally_panic_target: inc(0.0, 2),
            ally_search_target: inc(0.0, 2),
            touching_rate: 1.5,
            seeing_scalar_player: 1.0,
            seeing_scalar_npc: 10.0,
            distance_to_lose_sight: 300.0,
            distance_tuning: VisTuning::guard_distance(),
            clamp_target_visibility: true,
            stopping_variance: guard_variance(),
            can_bust: guard_can_bust(),
            seeing_scalar_dead_npc: 10.0,
            min_range_unsighted_busted_proxy: 250.0,
            magic: inc(0.1, 1),
        }
    }

    /// `Twk_Attn_Guard_Suspecting`.
    pub fn guard_suspecting() -> Self {
        AttentionConfig {
            leak_rate: 0.015,
            start: [0.125, 0.25, 0.5, 0.85],
            stop: [0.05, 0.15, 0.1, 0.8],
            danger_sound: inc(0.6, 3),
            threatening_sound: inc(0.4, 2),
            anomaly_sound: inc(0.2, 2),
            ally_search_target: inc(0.0, 3),
            ally_panic_target: inc(0.0, 3),
            ally_combat_target: inc(0.0, 3),
            touching_rate: 0.75,
            ..Self::guard_unsuspecting()
        }
    }

    /// `Twk_Attn_Default_Neutral`.
    pub fn neutral() -> Self {
        AttentionConfig {
            leak_rate: 2.0,
            start: [0.25, 1.0, 1.0, 1.0],
            stop: [0.2, 0.9, 0.9, 0.9],
            danger_sound: inc(0.4, 4),
            threatening_sound: inc(0.3, 2),
            anomaly_sound: inc(0.25, 1),
            help_sound: inc(0.1, 4),
            alarm_sound: inc(0.1, 4),
            death_rattle: inc(0.1, 4),
            attacked_by: inc(0.1, 4),
            attacked_enemy: inc(0.1, 4),
            discovered_corpse: inc(0.1, 4),
            ally_combat_target: inc(1.0, 3),
            ally_panic_target: inc(0.0, 2),
            ally_search_target: inc(0.0, 2),
            magic: inc(0.1, 4),
            // Thresholds are its own (fallback skip), so their variance is the struct default 0.
            stopping_variance: [0.0; 4],
            ..Self::guard_unsuspecting()
        }
    }

    /// May a change of this reason (`reason::*`) raise attention to Busted?
    pub fn reason_can_bust(&self, reason: u8) -> bool {
        self.can_bust & (1 << reason) != 0
    }

    /// Start threshold of an increase target level (`to_level` 5 = max attention).
    pub fn level_value(&self, to_level: u8) -> f32 {
        match to_level {
            0 => 0.0,
            1..=4 => self.start[to_level as usize - 1],
            _ => self.max,
        }
    }
}

impl Default for AiConfig {
    /// Values read from the GOG Definitive Edition (the tweak or INI named on each field);
    /// `dishonored_assets::gameplay` reloads them from the install.
    fn default() -> Self {
        AiConfig {
            vision: VisionConfig { inner_fov_deg: 45.0, inner_range: 3500.0, inner_up: 0.25, inner_down: 0.7, outer_fov_deg: 160.0, outer_range: 1700.0, outer_up: 0.1, outer_down: 0.7 },
            guard_unsuspecting: AttentionConfig::guard_unsuspecting(),
            guard_suspecting: AttentionConfig::guard_suspecting(),
            civilian_unsuspecting: AttentionConfig::guard_unsuspecting(),
            civilian_suspecting: AttentionConfig::guard_suspecting(),
            neutral: AttentionConfig::neutral(),
            loudness_distances: vec![0.0, 200.0, 300.0, 400.0, 500.0, 600.0, 800.0, 1000.0, 1200.0, 1500.0, 2000.0, 2500.0, 3000.0, 4000.0, 5000.0, 6000.0, 0.0],
            reaction_delay: [0.18, 0.3],
            search: SearchConfig {
                crumbs: 25,
                crumb_spacing: 800.0,
                crumb_lifetime: 150.0,
                heartbeat: 0.8,
                wander_min: 500.0,
                wander_max: 1000.0,
                wander_angle_deg: 60.0,
                cancel_investigate_wait: [0.5, 1.0],
                proximity: 250.0,
                reached_broadcast_range: 2000.0,
                stationary_duration: [2.0, 3.0],
                stare_radius: 500.0,
                stare_max_time: 5.0,
                anim_cooldown: 10.0,
                pathing_fail_retries: 1,
                pathing_fail_scale: 1.0,
                pathing_fail_range: [500.0, 25.0, 500.0],
                track_proximity: 150.0,
                // The elite guard's search animations (Startup AnimSets; the loader finds them even
                // without level packages).
                anim_lost_target: 3.7,
                anim_corpse_check: 5.233333,
            },
            panic: PanicConfig { cower_distance: 350.0, flee_point_claim_radius: 500.0, flee_interception_distance: 300.0, free_flight_time: 2.0, forbidden_angle_deg: 50.0, forbidden_radius: 500.0 },
            combat: CombatAiConfig {
                reachable_melee_range: 150.0,
                reachable_melee_height: 150.0,
                aim_time_before_shooting: 0.75,
                gun_usage_start: [0.0, 0.5],
                gun_usage_cooldown: [1.0, 2.0],
                skirmish_inner: 2,
                skirmish_total: 3,
                melee_contexts: default_melee_contexts(),
                dodge: DodgeConfig::default(),
                pistol: PistolConfig::default(),
                npc_sword_damage: d_npc_sword_damage(),
                npc_melee_damage_bonus: 0.0,
                guard_health_max: d_guard_health_max(),
            },
            guard_attention_span: 3.0,
            alarm_companion_range: 2000.0,
            alarm_fight_radius: 500.0,
            civilian_talking_loudness: 1500.0,
            death_rattle_loudness: 300.0,
            auto_glance_radius: 1000.0,
            auto_look_radius: 400.0,
            glance_time: 2.5,
            player_visibility: PlayerVisibilityConfig::default(),
            group_distance: 1000.0,
            trigger_alarm: TriggerAlarmConfig::default(),
            visual_detection_ratio_near: 0.3,
            rats: RatSwarmConfig::default(),
            devouring_swarm: RatSwarmConfig { swarm_type: 3, ..RatSwarmConfig::default() },
            noise: NoiseTable::default(),
            magic_reaction_delay: [0.2, 0.3],
            anim_magic_lost_target: 0.0,
            locomotion: Vec::new(),
            ambient_bark_inhibition: bark_inhibition(),
            special: SpecialNpcConfig::default(),
            provenance: BTreeMap::new(),
        }
    }
}
