//! Chaos, which the game calls "darkness" (`DisDarknessManager`, class record 0x01187CF8;
//! research/dishonored/systems/world.md section 2):
//!
//! - one integer score (`m_DarknessScore`, +0x38) for the whole game, never below 0, kept
//!   across missions (serialized, 0x00C52CA0);
//! - `Add(n)` (0x00C41B50): `score = max(0, score + n)`, used by `DisSeqAct_AddDarkness`
//!   (0x00BB3360) and the cheat `adddarkness` (0x00B753A0);
//! - the level (0x00C487D0): the largest index `i` of the player tweak's `m_DarknessLevels`
//!   with `score >= levels[i]` (-1 below the first entry);
//! - deeds (0x00C4D2C0, called at the end of every stat increment 0x00AB9560): for each
//!   `m_Deeds` entry on the incremented stat whose victim filter accepts the victim, the deed's
//!   counter (`m_DeedCounters`, +0x3C) grows by the amount; reaching `m_Threshold` adds
//!   `m_Value` to the score (clamped at 0) and resets the counter to 0 (no carry);
//! - the counters are cleared at mission end (`DisSeqAct_NotifyMissionEnd` 0x00B8B110 ->
//!   0x00C4D500); the score is kept.

use dishonored_ir::items::DarknessConfig;
use serde::{Deserialize, Serialize};

/// `EDisPlayerStat` names the deeds and the hooks use.
pub mod stat {
    pub const NUM_KILLS: &str = "ePlayerStat_NumKills";
    pub const NUM_ALARMS_TRIGGERED: &str = "ePlayerStat_NumAlarmsTriggered";
    pub const NUM_CORPSES_DISCOVERED: &str = "ePlayerStat_NumCorpsesDiscovered";
}

/// The game's `DisDarknessManager` state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Darkness {
    /// `m_DarknessScore`.
    pub score: i32,
    /// `m_DeedCounters`, one per `m_Deeds` entry (grown on demand, as 0x00C4D2C0 does).
    #[serde(default)]
    pub counters: Vec<i32>,
}

/// Whether a tweak whose lineage (itself, then its fallback chain) is `lineage` is or inherits
/// from `tweak` (0x00C6F450 compares names; case is not significant in names).
pub fn inherits(lineage: &[String], tweak: &str) -> bool {
    lineage.iter().any(|t| t.eq_ignore_ascii_case(tweak))
}

impl Darkness {
    /// `Add` (0x00C41B50): the score never goes below 0.
    pub fn add(&mut self, n: i32) {
        self.score = self.score.saturating_add(n).max(0);
    }

    /// The level of the current score (0x00C487D0): -1 when below `levels[0]`.
    pub fn level(&self, cfg: &DarknessConfig) -> i32 {
        level_of(self.score, cfg)
    }

    /// A stat was incremented by `amount` (the engine passes `(int)` of its float amount) with
    /// an optional victim given by its tweak lineage (0x00C4D2C0). Returns the level before and
    /// after when a deed added to the score.
    pub fn notify_deed(&mut self, cfg: &DarknessConfig, stat: &str, victim: Option<&[String]>, amount: i32) -> Option<(i32, i32)> {
        if self.counters.len() < cfg.deeds.len() {
            self.counters.resize(cfg.deeds.len(), 0);
        }
        let mut changed = None;
        for (i, d) in cfg.deeds.iter().enumerate() {
            if !d.stat.eq_ignore_ascii_case(stat) {
                continue;
            }
            if let Some(lineage) = victim {
                // A victim is skipped when it is (or inherits from) an excluded tweak, or when
                // the deed names a tweak the victim is not (and does not inherit from).
                if d.excluded.iter().any(|x| inherits(lineage, x)) {
                    continue;
                }
                if let Some(t) = &d.tweak {
                    if !inherits(lineage, t) {
                        continue;
                    }
                }
            }
            self.counters[i] += amount;
            if self.counters[i] >= d.threshold {
                let before = self.level(cfg);
                self.add(d.value);
                let after = self.level(cfg);
                changed = Some((changed.map_or(before, |c: (i32, i32)| c.0), after));
                self.counters[i] = 0;
            }
        }
        changed
    }

    /// `DisSeqAct_NotifyMissionEnd` (0x00C4D500): the deed counters are emptied.
    pub fn mission_end(&mut self) {
        self.counters.clear();
    }
}

/// The level of a score (0x00C487D0).
pub fn level_of(score: i32, cfg: &DarknessConfig) -> i32 {
    (0..cfg.levels.len()).rev().find(|&i| score >= cfg.levels[i]).map_or(-1, |i| i as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lin(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn add_clamps_at_zero_and_levels_follow_the_table() {
        let cfg = DarknessConfig::default();
        let mut d = Darkness::default();
        d.add(-50);
        assert_eq!(d.score, 0);
        assert_eq!(d.level(&cfg), 0);
        for (n, lvl) in [(11, 0), (12, 1), (47, 1), (48, 2), (389, 9), (390, 10)] {
            let mut d = Darkness::default();
            d.add(n);
            assert_eq!(d.level(&cfg), lvl, "score {n}");
        }
    }

    #[test]
    fn civilian_kills_add_six_each_and_weepers_count_only_as_weepers() {
        let cfg = DarknessConfig::default();
        let civ = lin(&["Pwn_Aristo_MTall_1.Pwn_Aristo_MTall_1", "Pwn_Civ_Base.Pwn_Civ_Base"]);
        let mut d = Darkness::default();
        d.notify_deed(&cfg, stat::NUM_KILLS, Some(&civ), 1);
        assert_eq!(d.score, 6);
        d.notify_deed(&cfg, stat::NUM_KILLS, Some(&civ), 1);
        assert_eq!((d.score, d.level(&cfg)), (12, 1));
        // A weeper inherits from Pwn_Civ_Base but is excluded from the civilian deed: four
        // weeper kills make one +6.
        let weeper = lin(&["Pwn_WeeperElite_MTall_1.Pwn_WeeperElite_MTall_1", "Pwn_Weeper_MTBase.Pwn_Weeper_MTBase", "Pwn_Weeper_Base.Pwn_Weeper_Base", "Pwn_Civ_Base.Pwn_Civ_Base"]);
        let mut d = Darkness::default();
        for k in 1..=4 {
            d.notify_deed(&cfg, stat::NUM_KILLS, Some(&weeper), 1);
            assert_eq!(d.score, if k < 4 { 0 } else { 6 });
        }
    }

    #[test]
    fn guard_kills_count_by_four_and_alarms_and_corpses_by_five() {
        let cfg = DarknessConfig::default();
        let guard = lin(&["Twk_Pawn_Boyle_Party.Pwn_EliteGuard_MTall_2_Party", "Pwn_EliteGuard_Base.pwn_eliteguard_mtbase", "Pwn_Guard_Base.Pwn_Guard_Base"]);
        let mut d = Darkness::default();
        let scores: Vec<i32> = (0..8).map(|_| {
            d.notify_deed(&cfg, stat::NUM_KILLS, Some(&guard), 1);
            d.score
        }).collect();
        assert_eq!(scores, [0, 0, 0, 6, 6, 6, 6, 12]);
        let mut d = Darkness::default();
        for _ in 0..4 {
            d.notify_deed(&cfg, stat::NUM_ALARMS_TRIGGERED, None, 1);
            d.notify_deed(&cfg, stat::NUM_CORPSES_DISCOVERED, None, 1);
        }
        assert_eq!(d.score, 0);
        d.notify_deed(&cfg, stat::NUM_ALARMS_TRIGGERED, None, 1);
        assert_eq!(d.score, 6);
        d.notify_deed(&cfg, stat::NUM_CORPSES_DISCOVERED, None, 1);
        assert_eq!(d.score, 12);
        // Mission end clears the counters and keeps the score.
        d.notify_deed(&cfg, stat::NUM_ALARMS_TRIGGERED, None, 1);
        d.mission_end();
        assert_eq!(d.score, 12);
        assert!(d.counters.is_empty());
    }

    #[test]
    fn a_stat_without_a_victim_bypasses_the_filters() {
        let cfg = DarknessConfig::default();
        let mut d = Darkness::default();
        // Deed 0 (civilians, threshold 1) takes it; deeds 1..6 count one each.
        d.notify_deed(&cfg, stat::NUM_KILLS, None, 1);
        assert_eq!(d.score, 6);
        assert_eq!(&d.counters[..7], &[0, 1, 1, 1, 1, 1, 1]);
    }
}
