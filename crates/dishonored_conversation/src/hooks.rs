//! Dialog hook rules of the original (`eDisDialogHook`): per-hook info, fire groups, refire
//! delays, and the distance-hook band logic.
//!
//! Sources (research/dishonored/CONVERSATIONS.md):
//! - The per-hook table is built natively by `FUN_00c8d820` (0x50 bytes per hook: hook class,
//!   fire group, refire min/max/global, bust time, flags). The numbers below were read from it.
//! - `DisTweaks_ConvSettings` defaults (its `Twk_ConvSettings` object is not shipped, so the
//!   class defaults apply): `m_DialogHookOverrides`, `m_FireGroupConfig`,
//!   `m_MaxDialogHooksAtOnce` 5, `m_fMaxDialogDistance` 5000.
//! - A successful fire on an NPC sets that NPC's next allowed time for the hook to
//!   `now + rand * (max - min)` (`FUN_00ca5670`; the minimum is not added, as in the original).
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
    fn from_index(i: u8) -> Self {
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
    (110, 2, 1.0, 1.0, 0x80),
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

/// `DisTweaks_ConvSettings.m_DialogHookOverrides` defaults: (hook, min, max, global, group).
const OVERRIDES: &[(u8, f32, f32, f32, u8)] = &[
    (31, 8.0, 12.0, 0.0, 5),
    (32, 8.0, 12.0, 0.0, 5),
    (85, 2.0, 5.0, 0.0, 7),
    (40, 5.0, 13.0, 0.0, 8),
    (41, 5.0, 13.0, 0.0, 8),
    (118, -1.0, 0.0, 0.0, 12),
    (11, 5.0, 12.0, 0.0, 0),
    (12, 20.0, 60.0, 0.0, 0),
];

/// `DisFireGroupConfig` defaults: (group, max playing per group, per initiator, ignore global
/// cap, min seconds between firings). -1 = unlimited.
const FIRE_GROUPS: &[(u8, i32, i32, bool, f32)] = &[
    (0, 3, 1, false, 3.0),
    (1, 3, 1, false, 12.0),
    (2, 2, 2, false, 0.2),
    (4, 2, -1, false, 0.0),
    (7, 5, 2, false, 0.0),
    (8, 1, 1, false, 0.0),
    (5, 1, 1, false, 2.5),
    (9, 3, 3, false, 3.0),
    (12, -1, -1, true, 0.0),
    (3, 1, 1, false, 0.0),
    (10, 2, 1, false, 0.0),
    (6, 2, 2, false, 2.0),
    (11, 2, 2, false, 1.5),
];

/// `m_MaxDialogHooksAtOnce`.
pub const MAX_HOOKS_AT_ONCE: usize = 5;

/// Enum index of a `DDH_*` name.
pub fn hook_index(name: &str) -> Option<u8> {
    HOOK_NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| i as u8)
}

/// The hook's info row (native table, then the tweak overrides).
pub fn hook_info(name: &str) -> HookInfo {
    let idx = hook_index(name).unwrap_or(126);
    let mut info = HookInfo { group: FireGroup::Casual, refire_min: 1.0, refire_max: 1.0, refire_global: 0.0, bust_time: 0.0, flags: 0x80 };
    if let Some(&(_, g, mn, mx, f)) = TABLE.iter().find(|r| r.0 == idx) {
        info = HookInfo { group: FireGroup::from_index(g), refire_min: mn, refire_max: mx, refire_global: 0.0, bust_time: if idx == 1 { 1.0 } else { 0.0 }, flags: f };
    }
    if let Some(&(_, mn, mx, gl, g)) = OVERRIDES.iter().find(|r| r.0 == idx) {
        info.refire_min = mn;
        info.refire_max = mx;
        info.refire_global = gl;
        info.group = FireGroup::from_index(g);
    }
    info
}

/// `DisFireGroupConfig` of a group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FireGroupConfig {
    pub max_per_group: i32,
    pub max_per_initiator: i32,
    pub ignore_global_cap: bool,
    pub min_time_between: f32,
}

pub fn fire_group(g: FireGroup) -> FireGroupConfig {
    FIRE_GROUPS
        .iter()
        .find(|r| r.0 == g as u8)
        .map(|&(_, a, b, c, d)| FireGroupConfig { max_per_group: a, max_per_initiator: b, ignore_global_cap: c, min_time_between: d })
        .unwrap_or(FireGroupConfig { max_per_group: -1, max_per_initiator: -1, ignore_global_cap: false, min_time_between: 0.0 })
}

/// Refire bookkeeping for hooks fired on NPCs (what `DisConversationComponent` and
/// `DisConvGlobalMan` keep). Deterministic: the random refire delay comes from the caller.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HookClock {
    /// (npc, hook) -> earliest time the hook may fire on that NPC again.
    pub next_allowed: BTreeMap<(String, u8), f64>,
    /// hook -> last time it fired on anyone (for `m_fRefire_Delay_Global`).
    pub last_global: BTreeMap<u8, f64>,
    /// fire group -> last time any hook of the group fired.
    pub last_group: BTreeMap<u8, f64>,
}

/// Why a hook did not fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookRejection {
    /// `eDisDialogDebugFailReason_RejectedByRefireDelay`.
    RefireDelay,
    /// `..._RejectedByGlobalRefire`.
    GlobalRefire,
    /// `..._FiringGroupLocked` (minimum time between firings of the group).
    GroupLocked,
    /// `..._FiringGroupInitiatorCap` / group cap / global cap.
    GroupCap,
    /// `..._FiredButNoContent`: the NPC's trees have no such hook, or it led nowhere.
    NoContent,
    /// The NPC could not take the conversation (busy with a higher priority one).
    Busy,
}

impl HookClock {
    /// Gate for firing `hook` on `npc` at `now`.
    pub fn allowed(&self, npc: &str, hook: &str, now: f64) -> Result<(), HookRejection> {
        let idx = hook_index(hook).unwrap_or(126);
        let info = hook_info(hook);
        if self.next_allowed.get(&(npc.to_string(), idx)).is_some_and(|t| now < *t) {
            return Err(HookRejection::RefireDelay);
        }
        if info.refire_global > 0.0 && self.last_global.get(&idx).is_some_and(|t| now - t < info.refire_global as f64) {
            return Err(HookRejection::GlobalRefire);
        }
        let cfg = fire_group(info.group);
        if cfg.min_time_between > 0.0 && self.last_group.get(&(info.group as u8)).is_some_and(|t| now - t < cfg.min_time_between as f64) {
            return Err(HookRejection::GroupLocked);
        }
        Ok(())
    }

    /// Record a successful fire. `unit` is a uniform roll in [0, 1).
    pub fn fired(&mut self, npc: &str, hook: &str, now: f64, unit: f32) {
        let idx = hook_index(hook).unwrap_or(126);
        let info = hook_info(hook);
        self.next_allowed.insert((npc.to_string(), idx), now + (unit * (info.refire_max - info.refire_min)) as f64);
        self.last_global.insert(idx, now);
        self.last_group.insert(info.group as u8, now);
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
