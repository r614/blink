//! Dialog hook rules of the original (`eDisDialogHook`): per-hook info, fire groups, refire
//! delays, the bark queue of `DisConvGlobalMan`, and the polled hook-node state machines.
//!
//! Sources (research/dishonored/CONVERSATIONS.md, research/dishonored/systems/conversation.md):
//! - The per-hook table is built natively by `FUN_00c8d820` (0x50 bytes per hook: hook class,
//!   fire group +0x34, refire min +0x38 / max +0x3c / global +0x40, bust time +0x44, flags
//!   +0x48). The numbers below were read from it (engine constants).
//! - `DisTweaks_ConvSettings` (the manager's `m_pConvSettingTweaks`; no `Twk_ConvSettings`
//!   object is shipped, so the class defaults apply): [`ConvSettings`], read from
//!   `Default__DisTweaks_ConvSettings` by the loader (`load.rs`), the cooked values as the
//!   fallback.
//! - Refire (`FUN_00ca6bd0` gate, `FUN_00ca5670` stamp): a successful fire on an NPC stores
//!   `next = now + rand() * (max - min) / 32767`; a later request passes when
//!   `next < 1e-8 || min < (now - next) + 1e-8`, so the effective delay is
//!   `min + u * (max - min)` from the start.
//! - Hook flags: 0x2 interrupts (an NPC answers it even when busy, unless its current hook is
//!   not interruptible), 0x4 not interruptible, 0x8 battle sound, 0x20 search sound,
//!   0x1000 notice sound (the blurb emits an AI noise of the tweak's loudness), 0x40
//!   deprecated, 0x2000 radial (bypasses attention), 0x4000 player used (the NPC turns to the
//!   player for the bust time).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `eDisDialogHook` names in enum order (index = value).
pub const HOOK_NAMES: [&str; 127] = [
    "DDH_NOT_SET",
    "DDH_PLAYER_USED",
    "DDH_PLAYER_APPROACHES",
    "DDH_PLAYER_APPROACHES_RADIAL",
    "DDH_PLAYER_LOITERS",
    "DDH_PLAYER_LOITERS_RADIAL",
    "DDH_PLAYER_LOOKAT",
    "DDH_PLAYER_LOOKAT_RADIAL",
    "DDH_FRIEND_APPROACHES",
    "DDH_NEUTRAL_APPROACHES",
    "DDH_KISMET_ACTIVATED",
    "DDH_PATROL_MISC",
    "DDH_PATROL_MISC_GROUP",
    "DDH_PATROL_MISC_ALERTED",
    "DDH_PATROL_MISC_ALERTED_GROUP",
    "DDH_PATROL_SWITCH_ROUTE",
    "DDH_PATROL_ATTACKED_BY_ENEMY",
    "DDH_PATROL_PROXIMITY_WARNING",
    "DDH_PATROL_PERSONAL_SPACE",
    "DDH_PATROL_PERSONAL_SPACE_WARNING",
    "DDH_PATROL_ATTACK_RATSINGLE",
    "DDH_SEARCH_NEAR_VDETECTION",
    "DDH_SEARCH_FAR_VDETECTION",
    "DDH_SEARCH_VDETECTION_FROM_ALLY",
    "DDH_SEARCH_ADETECTION",
    "DDH_SEARCH_ADETECTION_DANGER",
    "DDH_SEARCH_ADETECTION_FROM_ALLY",
    "DDH_SEARCH_DETECT_CORPSE",
    "DDH_SEARCH_WITNESS_DEATH",
    "DDH_SEARCH_MUSTHAVEBEENARAT",
    "DDH_SEARCH_GIVEUP_POSTCORPSE",
    "DDH_SEARCH",
    "DDH_SEARCH_GROUP",
    "DDH_SEARCH_FROM_ALLY",
    "DDH_SEARCH_FIND_ENEMY",
    "DDH_SEARCH_STOPHUNTING",
    "DDH_SEARCH_STOPHUNTING_GROUP",
    "DDH_DISTRACT_LOOKAT",
    "DDH_DISTRACT_ANIMATE_STARTWALK",
    "DDH_DISTRACT_ANIMATE_STARTANIM",
    "DDH_COMBAT_THREAT",
    "DDH_COMBAT_THREAT_GROUP",
    "DDH_COMBAT_THREAT_OFFSCREEN",
    "DDH_COMBAT_THREAT_VERSUS",
    "DDH_COMBAT_VICTORY",
    "DDH_COMBAT_VICTORY_LOST_ALLIES",
    "DDH_COMBAT_CANT_REACH",
    "DDH_COMBAT_HELP",
    "DDH_COMBAT_HELP_FOUND",
    "DDH_COMBAT_LOST",
    "DDH_COMBAT_LOST_GROUP",
    "DDH_COMBAT_BEGGING",
    "DDH_COMBAT_TAUNT_WITH_GESTURE",
    "DDH_COMBAT_ATTACK",
    "DDH_COMBAT_BIG_ATTACK",
    "DDH_COMBAT_ATTACK_RATSWARM",
    "DDH_COMBAT_BLIND_ATTACK",
    "DDH_COMBAT_DYING",
    "DDH_COMBAT_BACKSTABBED",
    "DDH_COMBAT_STEALTHKILL_PROJECTILE",
    "DDH_COMBAT_OUCH_SMALL",
    "DDH_COMBAT_OUCH_BIG",
    "DDH_COMBAT_IMMOLATED",
    "DDH_COMBAT_SURPRISED",
    "DDH_COMBAT_RANGED_BEGIN",
    "DDH_COMBAT_GOING_HOME",
    "DDH_ALARM_GO_TO_RING_ALARM",
    "DDH_ALARM_GO_TO_RING_ALARM_OTHER",
    "DDH_ALARM_HEAR_AND_RESPOND",
    "DDH_MAGIC_WITNESSED",
    "DDH_MAGIC_DISAPPEARED",
    "DDH_ROBBED",
    "DDH_WITNESSED_INTERACTION",
    "DDH_NOTICE_CARRYING_ALLY_CORPSE",
    "DDH_NOTICE_CARRYING_NEUTRAL_CORPSE",
    "DDH_NOTICE_CARRYING_ENEMY_CORPSE",
    "DDH_NOTICE_BROKEN_STUFF",
    "DDH_NOTICE_SUSPICIOUS_ACTIVITY",
    "DDH_NOTICE_THREATENING_ACTIVITY",
    "DDH_NOTICE_ALLY_BETRAYAL_WARNING",
    "DDH_COMBAT_CONFUSION",
    "DDH_FALLING",
    "DDH_ESCALATE_SUSPICION_LEVEL",
    "DDH_PANIC_START",
    "DDH_PANIC_START_RATSWARM",
    "DDH_FLEEING",
    "DDH_PANIC_NOWHERE_TO_HIDE",
    "DDH_UNREACH_TAUNT",
    "DDH_UNREACH_SEEK_ALLIES",
    "DDH_UNREACH_THROW_STONE",
    "DDH_UNREACH_RANGED_THREAT",
    "DDH_UNREACH_BELOW",
    "DDH_UNREACH_GIVEUP",
    "DDH_UNREACH_GIVEUP_WITHGROUP",
    "DDH_WEEP_NOTICED_TARGET",
    "DDH_WEEP_FOLLOWING",
    "DDH_WEEP_STAND_STILL_WEEPING",
    "DDH_WEEP_GIVEUP",
    "DDH_WEEP_MOAN",
    "DDH_COMBAT_OVS_MELEE_RUSH",
    "DDH_COMBAT_OVS_GRENADE_FLUSH",
    "DDH_COMBAT_OVS_GRENADE_COLLATERAL",
    "DDH_COMBAT_OVS_GRENADE_ALLIES",
    "DDH_COMBAT_OVS_GRENADE_JUMPAWAY",
    "DDH_COMBAT_OVS_COORDCHARGE_ANNOUNCE",
    "DDH_COMBAT_OVS_COORDCHARGE_TRIGGER",
    "DDH_COMBAT_THUG_SCRAMBLE",
    "DDH_ATTENTION_HEADTRACK",
    "DDH_ATTENTION_NOTICE_BEGIN",
    "DDH_ATTENTION_NOTICE_END",
    "DDH_ATTENTION_INVESTIGATE",
    "DDH_ATTENTION_TRACKTARGET_BEGIN",
    "DDH_ATTENTION_SAW_OTHER_HEADTRACK",
    "DDH_ATTENTION_SAW_OTHER_NOTICE",
    "DDH_ATTENTION_SAW_OTHER_INVESTIGATE",
    "DDH_ATTENTION_SAW_OTHER_ATTACK",
    "DDH_ESCAPE_IMPENDING_EXPLOSION",
    "DDH_NOTICE_DEATH_BY_WOL",
    "DDH_PLAY_AUDIO_LOG",
    "DDH_HEART_WORLD_AMBIENT_WHISPER",
    "DDH_HEART_TARGET_WHISPER",
    "DDH_HEART_TARGET_AMBIENT_WHISPER",
    "DDH_SUMMONEDASSASSIN_LEAVE_WIN",
    "DDH_SUMMONEDASSASSIN_LEAVE_LOSE",
    "DDH_SMOKE_BOMBED",
    "DDH_SUMMON_TENTACLE",
    "DDH_INVALID",
];

/// `eDisDialogHookFireGroup`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum FireGroup {
    Casual = 0,
    CasualStateChanges,
    Attention,
    Reaction,
    Exertions,
    Agitated,
    CombatCries,
    Combat,
    Threat,
    WitnessedMagic,
    SoundAlarm,
    CombatImportant,
    GameplayImportant,
}

impl FireGroup {
    pub fn from_index(i: u8) -> Self {
        use FireGroup::*;
        [Casual, CasualStateChanges, Attention, Reaction, Exertions, Agitated, CombatCries, Combat, Threat, WitnessedMagic, SoundAlarm, CombatImportant, GameplayImportant][(i as usize).min(12)]
    }
}

pub const FLAG_INTERRUPTS: u32 = 0x2;
pub const FLAG_NOT_INTERRUPTIBLE: u32 = 0x4;
pub const FLAG_BATTLE_SOUND: u32 = 0x8;
pub const FLAG_SEARCH_SOUND: u32 = 0x20;
pub const FLAG_DEPRECATED: u32 = 0x40;
pub const FLAG_NOTICE_SOUND: u32 = 0x1000;
pub const FLAG_RADIAL: u32 = 0x2000;
pub const FLAG_PLAYER_USED: u32 = 0x4000;

/// One row of the native hook table (after the tweak overrides).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct HookInfo {
    pub group: FireGroup,
    pub refire_min: f32,
    pub refire_max: f32,
    pub refire_global: f32,
    pub bust_time: f32,
    pub flags: u32,
}

/// (hook index, fire group, refire min, refire max, flags) where they differ from the
/// common row (group Casual, refire 1 s / 1 s, flags 0x80).
const TABLE: &[(u8, u8, f32, f32, u32)] = &[
    (1, 12, 0.0, 0.0, 0x4000),
    (2, 12, 0.0, 0.0, 0),
    (3, 12, 1.0, 1.0, 0x2000),
    (4, 0, 1.0, 1.0, 0),
    (5, 0, 1.0, 1.0, 0x2000),
    (6, 0, 1.0, 1.0, 0x10),
    (7, 0, 1.0, 1.0, 0x2010),
    (8, 0, 1.0, 1.0, 0),
    (9, 0, 1.0, 1.0, 0),
    (10, 12, 0.0, 0.0, 0x500),
    (13, 0, 1.0, 1.0, 0xc0),
    (14, 0, 1.0, 1.0, 0xc0),
    (15, 1, 1.0, 1.0, 0x80),
    (16, 7, 1.0, 1.0, 0x80),
    (17, 8, 1.0, 1.0, 0x80),
    (18, 8, 1.0, 1.0, 0x80),
    (19, 8, 1.0, 1.0, 0x80),
    (20, 6, 1.0, 1.0, 0x80),
    (21, 5, 1.0, 1.0, 0x80),
    (22, 1, 1.0, 1.0, 0x80),
    (23, 5, 1.0, 1.0, 0x80),
    (24, 1, 1.0, 1.0, 0x80),
    (25, 6, 1.0, 1.0, 0x82),
    (26, 5, 1.0, 1.0, 0x80),
    (27, 5, 1.0, 1.0, 0x80),
    (28, 5, 1.0, 1.0, 0x80),
    (29, 5, 1.0, 1.0, 0x80),
    (30, 5, 1.0, 1.0, 0x80),
    (31, 5, 1.0, 1.0, 0xa0),
    (32, 5, 1.0, 1.0, 0xa0),
    (33, 5, 1.0, 1.0, 0x80),
    (34, 5, 1.0, 1.0, 0x88),
    (35, 5, 1.0, 1.0, 0x80),
    (36, 5, 1.0, 1.0, 0x80),
    (37, 1, 1.0, 1.0, 0x80),
    (38, 1, 1.0, 1.0, 0x80),
    (39, 1, 1.0, 1.0, 0x80),
    (40, 8, 1.0, 1.0, 0x88),
    (41, 8, 1.0, 1.0, 0x88),
    (42, 11, 1.0, 1.0, 0x88),
    (43, 8, 1.0, 1.0, 0x88),
    (44, 7, 1.0, 1.0, 0x80),
    (45, 7, 1.0, 1.0, 0x80),
    (46, 7, 8.0, 8.0, 0x80),
    (47, 10, 1.0, 1.0, 0x88),
    (48, 12, 1.0, 1.0, 0x88),
    (49, 5, 1.0, 1.0, 0x80),
    (50, 5, 1.0, 1.0, 0x80),
    (51, 7, 1.0, 1.0, 0x88),
    (52, 6, 1.0, 1.0, 0x88),
    (53, 6, 1.0, 1.0, 0x88),
    (54, 6, 1.0, 1.0, 0x88),
    (55, 6, 1.0, 1.0, 0x8a),
    (56, 6, 1.0, 1.0, 0x89),
    (57, 4, 0.0, 0.0, 0x8e),
    (58, 4, 0.0, 0.0, 0x86),
    (59, 4, 0.0, 0.0, 0x86),
    (60, 4, 0.0, 0.0, 0x8a),
    (61, 4, 0.0, 0.0, 0x8a),
    (62, 4, 1.0, 1.0, 0x8e),
    (63, 7, 1.0, 1.0, 0x80),
    (64, 10, 1.0, 1.0, 0x88),
    (65, 7, 8.0, 8.0, 0x80),
    (66, 10, 1.0, 1.0, 0x88),
    (67, 10, 1.0, 1.0, 0xc8),
    (68, 5, 1.0, 1.0, 0x80),
    (69, 9, 1.0, 1.0, 0x80),
    (70, 9, 1.0, 1.0, 0x80),
    (71, 3, 1.0, 1.0, 0x80),
    (72, 12, 1.0, 1.0, 0x80),
    (73, 12, 1.0, 1.0, 0x80),
    (74, 3, 1.0, 1.0, 0x80),
    (75, 3, 1.0, 1.0, 0x80),
    (76, 3, 1.0, 1.0, 0x80),
    (77, 3, 1.0, 1.0, 0x80),
    (78, 3, 1.0, 1.0, 0x80),
    (79, 3, 1.0, 1.0, 0x80),
    (80, 3, 1.0, 1.0, 0x80),
    (81, 3, 1.0, 1.0, 0x8a),
    (82, 2, 1.0, 1.0, 0x80),
    (83, 12, 1.0, 1.0, 0x88),
    (84, 12, 1.0, 1.0, 0x88),
    (85, 7, 1.0, 1.0, 0x88),
    (86, 7, 1.0, 1.0, 0x88),
    (87, 8, 1.0, 1.0, 0x88),
    (88, 8, 1.0, 1.0, 0x88),
    (89, 6, 1.0, 1.0, 0x89),
    (90, 8, 1.0, 1.0, 0x88),
    (91, 8, 1.0, 1.0, 0x88),
    (92, 8, 1.0, 1.0, 0x88),
    (93, 8, 1.0, 1.0, 0x88),
    (94, 8, 1.0, 1.0, 0x88),
    (95, 8, 1.0, 1.0, 0x88),
    (96, 8, 1.0, 1.0, 0x88),
    (99, 6, 1.0, 1.0, 0x88),
    (100, 6, 1.0, 1.0, 0x88),
    (101, 6, 1.0, 1.0, 0x88),
    (102, 6, 1.0, 1.0, 0x88),
    (103, 6, 1.0, 1.0, 0x88),
    (104, 6, 1.0, 1.0, 0x88),
    (105, 6, 1.0, 1.0, 0x88),
    (106, 6, 1.0, 1.0, 0x88),
    (107, 2, 1.0, 1.0, 0x80),
    (108, 2, 1.0, 1.0, 0x1080),
    (109, 2, 1.0, 1.0, 0x80),
    // `DDH_ATTENTION_INVESTIGATE`: the `FUN_00c8bce0` call right after the `< 0x6f` check sets
    // 0xa0 (search sound).
    (110, 2, 1.0, 1.0, 0xa0),
    (111, 2, 1.0, 1.0, 0xa0),
    (112, 2, 1.0, 1.0, 0x80),
    (113, 2, 1.0, 1.0, 0x80),
    (114, 2, 1.0, 1.0, 0x80),
    (115, 6, 1.0, 1.0, 0x88),
    (116, 6, 1.0, 1.0, 0x80),
    (117, 3, 1.0, 1.0, 0x80),
    (118, 12, 0.0, 0.0, 0x300),
    (119, 12, 0.0, 0.0, 0xc04),
    (120, 12, 0.0, 0.0, 0xc04),
    (121, 12, 0.0, 0.0, 0xc04),
    (122, 7, 1.0, 1.0, 0x86),
    (123, 7, 1.0, 1.0, 0x86),
    (124, 7, 1.0, 1.0, 0x86),
    (125, 7, 1.0, 1.0, 0x86),
];

/// `DisDialogHookOverride` (`m_DialogHookOverrides[]`): replaces a hook row's refire delays and
/// fire group.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct HookOverride {
    pub hook: u8,
    pub min: f32,
    pub max: f32,
    pub global: f32,
    pub group: u8,
}

/// `DisFireGroupConfig` (`m_FireGroupConfig[]`). Index 0 = the Loud list, 1 = the Quiet list
/// (`m_MaxPlaying_PerGroup` / `_Quiet`, `m_MaxPlaying_PerInitiator` / `_Quiet`); a cap below 1
/// is unlimited.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct FireGroupConfig {
    pub max_per_group: [i32; 2],
    pub max_per_initiator: [i32; 2],
    /// `m_bIgnoreGlobalCap`.
    pub ignore_global_cap: bool,
    /// `m_fMinTimeBetweenFiring`: the group's list locks for this long after one of its hooks
    /// ends.
    pub min_time_between: f32,
}

impl Default for FireGroupConfig {
    fn default() -> Self {
        Self { max_per_group: [-1, -1], max_per_initiator: [-1, -1], ignore_global_cap: false, min_time_between: 0.0 }
    }
}

/// `DisTweaks_ConvSettings` (`DisConvGlobalMan.m_pConvSettingTweaks`). The loader reads
/// `Default__DisTweaks_ConvSettings` (`DishonoredGame.upk`); `Default` holds the same cooked
/// values for databases built without it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConvSettings {
    /// `m_BattleLoudness` / `m_SearchLoudness` / `m_NoticeLoudness` (`EAINoiseLoudness<n>`): the
    /// AI noise a blurb of a hook with flag 0x8 / 0x20 / 0x1000 makes.
    pub battle_loudness: u8,
    pub search_loudness: u8,
    pub notice_loudness: u8,
    pub overrides: Vec<HookOverride>,
    /// `m_MaxDialogHooksAtOnce`, `m_MaxDialogHooksAtOnce_Quiet` (tweak +0x9c / +0xa0).
    pub max_at_once: [i32; 2],
    /// `m_fMaxDialogDistance` (+0xa4): speakers farther from the player never bark (except
    /// `DDH_KISMET_ACTIVATED`).
    pub max_distance: f32,
    /// `m_fDialogDistanceConsideredLoud` (+0xa8): audio path distance from which a speaker
    /// uses the Quiet lists.
    pub loud_distance: f32,
    /// `m_fHookWeightRandRange` (+0xac).
    pub weight_rand_range: f32,
    /// By `eDisDialogHookFireGroup`.
    pub fire_groups: [FireGroupConfig; 13],
    /// `m_fNonDominantVolumeScale`, `m_fDominantMaxDistance_Percent` (+0xc0),
    /// `m_fAudioDuckBlendSpeed` (presentation: [`dominance`]).
    pub non_dominant_volume_scale: f32,
    pub dominant_max_distance_percent: f32,
    pub duck_blend_speed: f32,
}

impl Default for ConvSettings {
    /// The cooked `Default__DisTweaks_ConvSettings` (obj_dump of DishonoredGame.upk).
    fn default() -> Self {
        let ov = |hook, min, max, group| HookOverride { hook, min, max, global: 0.0, group };
        let g = |a: i32, b: i32, ignore: bool, t: f32| FireGroupConfig { max_per_group: [a, a], max_per_initiator: [b, b], ignore_global_cap: ignore, min_time_between: t };
        Self {
            battle_loudness: 3,
            search_loudness: 2,
            notice_loudness: 1,
            overrides: vec![ov(31, 8.0, 12.0, 5), ov(32, 8.0, 12.0, 5), ov(85, 2.0, 5.0, 7), ov(40, 5.0, 13.0, 8), ov(41, 5.0, 13.0, 8), ov(118, -1.0, 0.0, 12), ov(11, 5.0, 12.0, 0), ov(12, 20.0, 60.0, 0)],
            max_at_once: [5, 5],
            max_distance: 5000.0,
            loud_distance: 2000.0,
            weight_rand_range: 0.2,
            fire_groups: [
                g(3, 1, false, 3.0),   // Casual
                g(3, 1, false, 12.0),  // CasualStateChanges
                g(2, 2, false, 0.2),   // Attention
                g(1, 1, false, 0.0),   // Reaction
                g(2, -1, false, 0.0),  // Exertions
                g(1, 1, false, 2.5),   // Agitated
                g(2, 2, false, 2.0),   // CombatCries
                g(5, 2, false, 0.0),   // Combat
                g(1, 1, false, 0.0),   // Threat
                g(3, 3, false, 3.0),   // WitnessedMagic
                g(2, 1, false, 0.0),   // SoundAlarm
                g(2, 2, false, 1.5),   // CombatImportant
                g(-1, -1, true, 0.0),  // GameplayImportant
            ],
            non_dominant_volume_scale: 0.3,
            dominant_max_distance_percent: 0.4,
            duck_blend_speed: 5.0,
        }
    }
}

impl ConvSettings {
    /// The hook's info row (native table, then the tweak overrides).
    pub fn hook_info(&self, idx: u8) -> HookInfo {
        let mut info = HookInfo { group: FireGroup::Casual, refire_min: 1.0, refire_max: 1.0, refire_global: 0.0, bust_time: 0.0, flags: 0x80 };
        if let Some(&(_, g, mn, mx, f)) = TABLE.iter().find(|r| r.0 == idx) {
            info = HookInfo { group: FireGroup::from_index(g), refire_min: mn, refire_max: mx, refire_global: 0.0, bust_time: if idx == 1 { 1.0 } else { 0.0 }, flags: f };
        }
        if let Some(o) = self.overrides.iter().find(|o| o.hook == idx) {
            info.refire_min = o.min;
            info.refire_max = o.max;
            info.refire_global = o.global;
            info.group = FireGroup::from_index(o.group);
        }
        info
    }

    pub fn fire_group(&self, g: FireGroup) -> FireGroupConfig {
        self.fire_groups[g as usize]
    }
}

/// Enum index of a `DDH_*` name (126 `DDH_INVALID` for unknown names).
pub fn hook_index(name: &str) -> Option<u8> {
    HOOK_NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| i as u8)
}

/// The hook's info row with the cooked tweak defaults.
pub fn hook_info(name: &str) -> HookInfo {
    ConvSettings::default().hook_info(hook_index(name).unwrap_or(126))
}

/// `DisFireGroupConfig` of a group (cooked tweak defaults).
pub fn fire_group(g: FireGroup) -> FireGroupConfig {
    ConvSettings::default().fire_group(g)
}

/// Refire bookkeeping: the NPC's `m_DialogHookInfo_InGame[h]` (+0x98, the time stamp `next_h`)
/// and the manager's `m_DialogHookInfo_InGame[h].m_fLastFireTime` (+0x558).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HookClock {
    /// (npc, hook) -> `next_h` = fire time + `u * (max - min)`.
    pub next_allowed: BTreeMap<(String, u8), f64>,
    /// hook -> last time it fired on anyone (for `m_fRefire_Delay_Global`).
    pub last_global: BTreeMap<u8, f64>,
}

/// Why a hook did not fire (`eDisDialogDebugFailReason` where one exists).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookRejection {
    /// 0 `RejectedByRefireDelay`.
    RefireDelay,
    /// 1 `RejectedByGlobalRefire`.
    GlobalRefire,
    /// 3 `FiringGroupLocked`: the list is locked for the group's minimum spacing.
    GroupLocked,
    /// 4 `FiringGroupInitiatorCap`.
    GroupCap,
    /// 5: queued, but not fired on the manager tick (group room, global slots or weight); the
    /// request is dropped.
    Dropped,
    /// The NPC's trees have no such hook, the hook node gives no output, or the walk led
    /// nowhere (`FiredButNoContent`).
    NoContent,
    /// The speaker cannot take it (`Available` 4 / 5, R4).
    Busy,
    /// The owner is dead and the hook is not one of 57-59.
    Dead,
}

impl HookClock {
    /// Per-NPC and global refire gates of `FUN_00ca6bd0`:
    /// pass iff `next < 1e-8 || min < (now - next) + 1e-8` (the minimum is added to the stamp
    /// `next = fire + u * (max - min)`), then the same with the global delay.
    pub fn allowed(&self, npc: &str, idx: u8, info: &HookInfo, now: f64) -> Result<(), HookRejection> {
        let gate = |stamp: Option<&f64>, delay: f32| stamp.is_none_or(|t| *t < 1e-8 || (delay as f64) < (now - t) + 1e-8);
        if !gate(self.next_allowed.get(&(npc.to_string(), idx)), info.refire_min) {
            return Err(HookRejection::RefireDelay);
        }
        if !gate(self.last_global.get(&idx), info.refire_global) {
            return Err(HookRejection::GlobalRefire);
        }
        Ok(())
    }

    /// The stamp of a successful fire (`FUN_00ca5670`): `unit` = `rand() / 32767`.
    pub fn fired(&mut self, npc: &str, idx: u8, info: &HookInfo, now: f64, unit: f32) {
        self.next_allowed.insert((npc.to_string(), idx), now + (unit * (info.refire_max - info.refire_min)) as f64);
        self.last_global.insert(idx, now);
    }
}

/// `DisConvGlobalMan.m_QueuedHookGroups`: 13 fire groups x {Loud, Quiet}
/// (`DisQueuedFireDialogHookGroup`, 0x30 bytes each from +0x78).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HookQueue {
    /// Index `group * 2 + quiet`.
    pub lists: Vec<HookList>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HookList {
    /// `m_PendingHooks` (+0x78): requests of this manager tick.
    pub pending: Vec<QueuedHook>,
    /// `m_FiringHooks` (+0x84): hooks this list started that still play.
    pub firing: Vec<QueuedHook>,
    /// `m_bIsLocked` (+0xa0 bit 0) and `m_fGroupLockoutTimer` (+0xa4).
    pub locked: bool,
    pub lockout: f32,
}

/// A queued request (`0x4c`-byte entry: hook, sub-index, fire tag, component, initiator,
/// hook params).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueuedHook {
    pub request: crate::runner::HookRequest,
    pub hook: u8,
    /// The hook node's output found by the trial walk (the sub-index).
    pub output: usize,
    /// The instance it started (firing entries); it plays while that instance runs.
    pub instance: Option<u32>,
}

impl HookQueue {
    pub fn list(&mut self, group: FireGroup, quiet: bool) -> &mut HookList {
        if self.lists.len() < 26 {
            self.lists.resize_with(26, HookList::default);
        }
        &mut self.lists[group as usize * 2 + quiet as usize]
    }

    /// `m_InitiatorCountMap[quiet]`: firing hooks of every group of this list type whose
    /// initiator is `who` (an initiator that is the owner itself is not counted).
    pub fn initiator_count(&self, quiet: bool, who: u32) -> usize {
        self.lists.iter().enumerate().filter(|(i, _)| (i % 2 == 1) == quiet).flat_map(|(_, l)| l.firing.iter()).filter(|q| q.request.initiator == Some(who) && q.request.owner != Some(who)).count()
    }
}

/// Bark weight of a pending request (`FUN_00ca2580`) before the initiator cap: `d` is the
/// speaker's straight-line distance to the player (0 for the player's own pawn), `roll` a
/// `rand()` draw.
pub fn bark_weight(settings: &ConvSettings, hook: u8, d: f32, roll: u32) -> f32 {
    let max = settings.max_distance;
    if d < max || hook == 10 {
        (1.0 - d.clamp(0.0, max) / max) + roll as f32 * settings.weight_rand_range * (1.0 / 32767.0)
    } else {
        0.0
    }
}

/// Dominance of a firing hook (`FUN_00c99f80`) from its speaker's interface flags (+0xbc) and
/// its playing voices: flag 0x10 -> 0, 0x2 -> 1, 0x8 -> the best `1 - d / (pct * radius)` over
/// the voices within `pct * radius` (path distance `d`, voice radius `radius`).
pub fn dominance(settings: &ConvSettings, flags: u32, voices: &[(f32, f32)]) -> f32 {
    if flags & 0x10 != 0 {
        return 0.0;
    }
    if flags & 0x2 != 0 {
        return 1.0;
    }
    let mut best = 0.0f32;
    if flags & 0x8 != 0 {
        for &(d, radius) in voices {
            let r = settings.dominant_max_distance_percent * radius;
            if d <= r && r > 0.0 {
                best = best.max(1.0 - d / r);
            }
        }
    }
    best
}

/// `DisConv_Hook_PlayerLoiter` (+0x158, `FUN_00cb53c0`), per (NPC, hook node): polled every
/// tick. The player counts as loitering while a speaker is within `radius` (2D); entering
/// arms a timer of `min + rand() * (max - min) / 32767`; staying past it fires output 0 and
/// re-arms on the next poll. A gap of more than 0.5 s between polls counts as having left.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct LoiterHook {
    pub last_poll: f64,
    pub target: f32,
    pub elapsed: f32,
    pub inside: bool,
}

impl LoiterHook {
    /// `dist2d`: the nearest speaker's horizontal distance to the player; `roll` a `rand()`.
    pub fn update(&mut self, now: f64, dist2d: f32, radius: f32, min: f32, max: f32, roll: u32) -> Option<usize> {
        let dt = (now - self.last_poll) as f32;
        let was = self.inside && dt <= 0.5;
        self.last_poll = now;
        self.inside = dist2d < radius;
        if !was && self.inside {
            self.target = roll as f32 * (max - min) * (1.0 / 32767.0) + min;
            self.elapsed = 0.0;
        }
        if self.target > 0.0001 && self.inside && was {
            self.elapsed += dt;
            if self.target < self.elapsed {
                self.inside = false;
                return Some(0);
            }
        }
        None
    }
}

/// `DisConv_Hook_PlayerLookAt` (+0x158, `FUN_00cb5680`), per (NPC, hook node): while the
/// player is within `radius` (2D) of a speaker, the look-at target with the smallest view angle
/// under its own angle becomes current (a change restarts the clock); it fires its output once
/// after its time.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct LookAtHook {
    pub current: i32,
    pub since: f64,
    pub fired: bool,
}

impl Default for LookAtHook {
    fn default() -> Self {
        Self { current: -1, since: 0.0, fired: false }
    }
}

impl LookAtHook {
    /// `angles[i]`: the view angle to target i in degrees (None = no actor); `targets[i]` =
    /// (max angle, time).
    pub fn update(&mut self, now: f64, dist2d: f32, radius: f32, angles: &[Option<f32>], targets: &[(f32, f32)]) -> Option<usize> {
        if dist2d >= radius {
            self.current = -1;
        } else {
            let mut pick = -1i32;
            let mut best = f32::MAX;
            for (i, (a, t)) in angles.iter().zip(targets).enumerate() {
                if let Some(a) = a {
                    if *a < t.0 && *a < best {
                        best = *a;
                        pick = i as i32;
                    }
                }
            }
            if pick != self.current {
                self.since = now;
                self.fired = false;
                self.current = pick;
            }
        }
        if self.current >= 0 && !self.fired {
            let k = self.current as usize;
            if targets.get(k).is_some_and(|t| t.1 as f64 <= now - self.since) {
                self.fired = true;
                return Some(k);
            }
        }
        None
    }
}

/// `DisConv_Hook_Distance` (`DDH_PLAYER_APPROACHES*`, slot +0x158 `FUN_00cb4ff0`): which output
/// fires as the nearest speaker-to-initiator distance changes.
///
/// The tracker keeps the last distance seen inside the far edge. With the new distance `d`
/// (only considered when `d <= far`): the close output fires when it is linked, `d <= close`
/// and the last distance was beyond close; else the medium output under the same rule; else
/// the far output when `far < last`. The last distance then becomes `d`. Moving out past the
/// far edge leaves it unchanged.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct DistanceHook {
    pub last: f32,
}

impl Default for DistanceHook {
    fn default() -> Self {
        Self { last: f32::MAX }
    }
}

impl DistanceHook {
    /// `edges` = close, medium, far; `linked[i]` = output i has a link.
    pub fn update(&mut self, d: f32, edges: [f32; 3], linked: [bool; 3]) -> Option<usize> {
        if d > edges[2] {
            return None;
        }
        let last = self.last;
        self.last = d;
        if linked[0] && d <= edges[0] && last > edges[0] {
            return Some(0);
        }
        if linked[1] && d <= edges[1] && last > edges[1] {
            return Some(1);
        }
        if linked[2] && edges[2] < last {
            return Some(2);
        }
        None
    }
}
