//! The player statistics the end-of-mission screen reads (`DishonoredPlayerPawn` +0xB7C: the
//! mission's tracked stats, `{EDisPlayerStat, value, max}` entries).
//!
//! - every stat increment goes through 0x00AB9560, which ends with the chaos deeds (0x00C4D2C0)
//!   and adds the amount to the tracked entry of that stat, if the mission tracks it (0x00AA97E0);
//! - `DisSeqAct_MissionStatsTracking` input 0 (0x009FE090) empties the tracked list (0x00AB1B00)
//!   and adds an entry per row of its `DisTweaks_MissionStats` (0x00AB1B50: the row's stat, and
//!   its additional stat, with the row's `m_MissionStatsMaxValues` entry); input 1 reads them
//!   (0x00AA9700) into the mission's record, then empties the list;
//! - a kill credited to the player (0x00B73990) raises `NumKills` and then `HostileKill` or
//!   `CivilianKill` when the victim's tweak is in `Twk_PlayerStats.m_HostileNPCInfo` /
//!   `m_CivilianNPCInfo` (0x00ACC590 / 0x00ACC5B0 -> 0x00ACC4B0: one of the included tweaks and
//!   none of the excluded, by the tweak-lineage test 0x00C6F450); a knock-out (0x00B738F0)
//!   raises `HostilePutToSleep` / `CivilianPutToSleep` with the same test.

use crate::darkness::inherits;

/// `EDisPlayerStat` names (DisTweaks_PlayerStats.EDisPlayerStat, `DishonoredGame.upk`).
pub const NUM_KILLS: &str = "ePlayerStat_NumKills";
pub const NUM_ALARMS_TRIGGERED: &str = "ePlayerStat_NumAlarmsTriggered";
pub const NUM_CORPSES_DISCOVERED: &str = "ePlayerStat_NumCorpsesDiscovered";
pub const NPCS_ALERTED: &str = "ePlayerStat_NPCsAlerted";
pub const CHAOS_LEVEL: &str = "ePlayerStat_ChaosLevel";
pub const HOSTILE_KILL: &str = "ePlayerStat_HostileKill";
pub const HOSTILE_PUT_TO_SLEEP: &str = "ePlayerStat_HostilePutToSleep";
pub const CIVILIAN_KILL: &str = "ePlayerStat_CivilianKill";
pub const CIVILIAN_PUT_TO_SLEEP: &str = "ePlayerStat_CivilianPutToSleep";
pub const GOLD_FOUND: &str = "ePlayerStat_GoldFound";
pub const RUNE_FOUND: &str = "ePlayerStat_RuneFound";
pub const BONE_CHARM_FOUND: &str = "ePlayerStat_BoneCharmFound";
pub const OUTSIDER_SHRINE_FOUND: &str = "ePlayerStat_OutsiderShrineFound";
pub const SOKOLOV_PAINTING_FOUND: &str = "ePlayerStat_SokolovPaintingFound";
pub const CRACKED_CHARM_FOUND: &str = "ePlayerStat_CrackedCharmFound";

/// `EDisPlayerStat` in enum order (0 .. 40; `ePlayerStat_MAX` is 41).
pub const ALL: [&str; 41] = [
    "ePlayerStat_NumKills",
    "ePlayerStat_NumAssassinations",
    "ePlayerStat_NumFlees",
    "ePlayerStat_NumAlarmsTriggered",
    "ePlayerStat_NumCorpsesDiscovered",
    "ePlayerStat_NumExplosions",
    "ePlayerStat_NumRatsSummoned",
    "ePlayerStat_LongestDistanceFallen",
    "ePlayerStat_AssassinationTargetKilled",
    "ePlayerStat_AmountStolen",
    "ePlayerStat_NPCsAlerted",
    "ePlayerStat_PowersAcquired",
    "ePlayerStat_UnawareKill",
    "ePlayerStat_SameFactionKill",
    "ePlayerStat_DropKill",
    "ePlayerStat_MercyKill",
    "ePlayerStat_Suicide",
    "ePlayerStat_TimePossessing",
    "ePlayerStat_DistanceTravelled",
    "ePlayerStat_UpgradesAcquired",
    "ePlayerStat_ItemsCollected",
    "ePlayerStat_MaxPursuersCountEscaped",
    "ePlayerStat_MaxSimultaneousEnemyCountKilled",
    "ePlayerStat_FishPossessed",
    "ePlayerStat_GrenadeThrowbackKill",
    "ePlayerStat_RatTunnelUsed",
    "ePlayerStat_ChaosScore",
    "ePlayerStat_ChaosLevel",
    "ePlayerStat_HostileKill",
    "ePlayerStat_HostilePutToSleep",
    "ePlayerStat_CivilianKill",
    "ePlayerStat_CivilianPutToSleep",
    "ePlayerStat_NPCsEscapedFrom",
    "ePlayerStat_GoldFound",
    "ePlayerStat_RuneFound",
    "ePlayerStat_BoneCharmFound",
    "ePlayerStat_OutsiderShrineFound",
    "ePlayerStat_SokolovPaintingFound",
    "ePlayerStat_ArcMineBasedOnRatKill",
    "ePlayerStat_CrackedCharmFound",
    "ePlayerStat_DamageTakenByPullLiftShield",
];

/// The stats the stats screen shows as found pickups with an icon (`m_Stat` 0x21 .. 0x25 and
/// 0x27 in 0x00BCBEA0, icon names from 0x00BB9BD0).
pub fn pickup_icon(stat: &str) -> Option<&'static str> {
    Some(match stat {
        GOLD_FOUND => "coins",
        RUNE_FOUND => "runes",
        BONE_CHARM_FOUND => "boneCharms",
        OUTSIDER_SHRINE_FOUND => "outsider",
        SOKOLOV_PAINTING_FOUND => "paintings",
        CRACKED_CHARM_FOUND => "crackedCharms",
        _ => return None,
    })
}

/// `NPCTypeInfo`: `m_TweaksNamePairs` / `m_ExcludedTweaksNamePairs` (`m_TweaksName`).
#[derive(Debug, Clone, PartialEq)]
pub struct NpcTypeInfo {
    pub tweaks: &'static [&'static str],
    pub excluded: &'static [&'static str],
}

impl NpcTypeInfo {
    /// 0x00ACC4B0: the victim is (or inherits from) an included tweak and none of the excluded.
    pub fn matches(&self, lineage: &[String]) -> bool {
        self.tweaks.iter().any(|t| inherits(lineage, t)) && !self.excluded.iter().any(|t| inherits(lineage, t))
    }
}

/// `Twk_PlayerStats.Twk_PlayerStats.m_HostileNPCInfo` (Startup.upk).
pub const HOSTILE: NpcTypeInfo = NpcTypeInfo {
    tweaks: &["Pwn_Thug_Base.Pwn_Thug_Base", "Pwn_Guard_Base.Pwn_Guard_Base", "Pwn_Overseer_Base.pwn_overseer_houndmaster_Base", "Pwn_Overseer_Base.Pwn_Overseer_MTBase"],
    excluded: &[],
};

/// `Twk_PlayerStats.Twk_PlayerStats.m_CivilianNPCInfo` (Startup.upk).
pub const CIVILIAN: NpcTypeInfo = NpcTypeInfo {
    tweaks: &["Pwn_Civ_Base.Pwn_Civ_Base"],
    excluded: &["Twk_Pawn_LordPendleton.Twk_Pawn_LordPendleton", "Pwn_Weeper_Base.Pwn_Weeper_Base", "Lighthouse_Tweaks.Characters.Twk_Pawn_Pendleton_Lighthouse"],
};

/// The stats a kill credited to the player raises after `NumKills` (0x00B73990): the hostile
/// test first, the civilian one only when it fails.
pub fn kill_stat(lineage: &[String]) -> Option<&'static str> {
    if HOSTILE.matches(lineage) {
        Some(HOSTILE_KILL)
    } else if CIVILIAN.matches(lineage) {
        Some(CIVILIAN_KILL)
    } else {
        None
    }
}

/// The stat a knock-out by the player raises (0x00B738F0).
pub fn sleep_stat(lineage: &[String]) -> Option<&'static str> {
    if HOSTILE.matches(lineage) {
        Some(HOSTILE_PUT_TO_SLEEP)
    } else if CIVILIAN.matches(lineage) {
        Some(CIVILIAN_PUT_TO_SLEEP)
    } else {
        None
    }
}

/// The stat an abstract item pickup raises when its tweak has `m_bCountForStats` (0x00C4BD60:
/// the player's coin item -> `GoldFound`, the rune item -> `RuneFound`, by the quantity given).
/// `DisTweaks_AbstractItemPickup.m_bCountForStats` is true in the class defaults and no shipped
/// pickup tweak clears it.
pub fn abstract_pickup_stat(item: &str) -> Option<&'static str> {
    let short = item.rsplit(['.', ':']).next().unwrap_or(item);
    if short.eq_ignore_ascii_case(crate::inventory::COINS_ITEM) {
        Some(GOLD_FOUND)
    } else if short.eq_ignore_ascii_case(crate::inventory::RUNE_ITEM) {
        Some(RUNE_FOUND)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lin(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn enum_order_matches_the_decoded_indices() {
        // The screen's special cases are by index: 0x1B chaos, 0x21 .. 0x25 and 0x27 pickups.
        assert_eq!(ALL[0x1b], CHAOS_LEVEL);
        assert_eq!(ALL[0x21], GOLD_FOUND);
        assert_eq!(ALL[0x25], SOKOLOV_PAINTING_FOUND);
        assert_eq!(ALL[0x27], CRACKED_CHARM_FOUND);
        assert_eq!(ALL[10], NPCS_ALERTED);
        assert!(pickup_icon(ALL[0x26]).is_none());
        assert_eq!(pickup_icon(ALL[0x27]), Some("crackedCharms"));
    }

    #[test]
    fn guards_are_hostile_civilians_civilian_weepers_neither() {
        let guard = lin(&["Twk_Pawn_Boyle_Party.Pwn_EliteGuard_MTall_2_Party", "Pwn_EliteGuard_Base.pwn_eliteguard_mtbase", "Pwn_Guard_Base.Pwn_Guard_Base"]);
        assert_eq!(kill_stat(&guard), Some(HOSTILE_KILL));
        assert_eq!(sleep_stat(&guard), Some(HOSTILE_PUT_TO_SLEEP));
        let civ = lin(&["Pwn_Aristo_MTall_1.Pwn_Aristo_MTall_1", "Pwn_Civ_Base.Pwn_Civ_Base"]);
        assert_eq!(kill_stat(&civ), Some(CIVILIAN_KILL));
        let weeper = lin(&["Pwn_WeeperElite_MTall_1.Pwn_WeeperElite_MTall_1", "Pwn_Weeper_MTBase.Pwn_Weeper_MTBase", "Pwn_Weeper_Base.Pwn_Weeper_Base", "Pwn_Civ_Base.Pwn_Civ_Base"]);
        assert_eq!(kill_stat(&weeper), None);
        assert_eq!(kill_stat(&lin(&["Pwn_Rat.Pwn_Rat"])), None);
    }

    #[test]
    fn coins_and_runes_count_as_found() {
        assert_eq!(abstract_pickup_stat("Money_twk_Abs.AbstractItems.Coins_AbsItm"), Some(GOLD_FOUND));
        assert_eq!(abstract_pickup_stat("Runes_twk.AbstractItems.Rune_AbsItm"), Some(RUNE_FOUND));
        assert_eq!(abstract_pickup_stat("Boyle.Key_AbsItm"), None);
    }
}
