//! NPC perception and behaviour tuning, normalized from Arkane's AI tweaks
//! (`DisTweaks_PawnAttention`, `DisTweaks_Vision`, `DisTweaks_AIBehavior_*`,
//! `DisTweaks_AISubState_*`) and `DefaultAI.ini` / `DefaultNPC.ini`.
//! See `research/dishonored/AI.md`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Attention levels (`EDisAttentionLevel`).
pub const DAL_UNAWARE: u8 = 0;
pub const DAL_HEAD_TRACK: u8 = 1;
pub const DAL_TURN_TO_FACE: u8 = 2;
pub const DAL_INVESTIGATE: u8 = 3;
pub const DAL_BUSTED: u8 = 4;
/// `DALFI_MaxAttention` (only as an increase target).
pub const DALFI_MAX_ATTENTION: u8 = 5;

/// `DisTweaks_PawnAttention.DisAttentionIncrease`: raise attention by `amount`, but not past
/// the start threshold of `to_level` (`EDisAttentionLevelForIncrease`: 0 None, 1 HeadTrack,
/// 2 TurnToFace, 3 Investigate, 4 Busted, 5 MaxAttention). The struct default is
/// `amount = 0` (read from the `DisAttentionIncrease` ScriptStruct defaults); with no amount
/// the attention is raised straight to that threshold (LIKELY: otherwise `AttackedBy` would
/// do nothing).
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
    /// `m_DistanceValueTuning` points (input, output), applied with `DVCTPAO_Add`.
    pub distance_points: Vec<[f32; 2]>,
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
}

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
    pub provenance: BTreeMap<String, String>,
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
            distance_points: vec![[0.55, 0.0], [0.875, 0.8], [0.93, 2.0]],
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
            ..Self::guard_unsuspecting()
        }
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
            search: SearchConfig { crumbs: 25, crumb_spacing: 800.0, crumb_lifetime: 150.0, heartbeat: 0.8, wander_min: 500.0, wander_max: 1000.0, wander_angle_deg: 60.0, cancel_investigate_wait: [0.5, 1.0], proximity: 250.0, reached_broadcast_range: 2000.0, stationary_duration: [2.0, 3.0] },
            panic: PanicConfig { cower_distance: 350.0, flee_point_claim_radius: 500.0, flee_interception_distance: 300.0, free_flight_time: 2.0, forbidden_angle_deg: 50.0, forbidden_radius: 500.0 },
            combat: CombatAiConfig { reachable_melee_range: 150.0, reachable_melee_height: 150.0, aim_time_before_shooting: 0.75, gun_usage_start: [0.0, 0.5], gun_usage_cooldown: [1.0, 2.0], skirmish_inner: 2, skirmish_total: 3 },
            guard_attention_span: 3.0,
            alarm_companion_range: 2000.0,
            alarm_fight_radius: 500.0,
            civilian_talking_loudness: 1500.0,
            death_rattle_loudness: 300.0,
            auto_glance_radius: 1000.0,
            auto_look_radius: 400.0,
            glance_time: 2.5,
            provenance: BTreeMap::new(),
        }
    }
}
