//! The mission's tracked player stats and the stats record (MissionView, MissionStatsRecord).

use dishonored_mission::runtime::{MissionStatsRecord, MissionView};

#[test]
fn tracked_stats_add_up_and_rows_read_them() {
    let mut v = MissionView::default();
    v.add_player_stat("ePlayerStat_HostileKill", 1.0);
    v.add_player_stat("ePlayerStat_HostileKill", 2.0);
    v.add_player_stat("ePlayerStat_CivilianKill", 1.0);
    assert_eq!(v.player_stat("ePlayerStat_HostileKill"), 3.0);
    assert_eq!(v.player_stat("ePlayerStat_GoldFound"), 0.0);
    let r = MissionStatsRecord { tweak: "Twk_MissionStats.Twk_M0_Prison".into(), stats: v.player_stats.clone(), chaos_high: true, flags: vec!["Prison_HermitRoom".into()] };
    // "Didn't Kill Anyone": HostileKill + CivilianKill (`m_bAddOtherStat`).
    assert_eq!(r.row_value("ePlayerStat_HostileKill", Some("ePlayerStat_CivilianKill")), 4.0);
    assert_eq!(r.row_value("ePlayerStat_ChaosLevel", None), 1.0);
    assert!(r.flag("prison_hermitroom"));
    assert!(!r.flag("Prison_Safe"));
}
