//! The journal's and the stats screen's data mapping (`journal.rs`, `mission_stats.rs`).

use dishonored_mission::TaskState;
use dishonored_mission::runtime::{MissionStatsRecord, ObjectiveView, TaskView};
use dishonored_runtime::journal::{self, Row, Tab};
use dishonored_runtime::mission_stats::{self, MissionStatsDef, StatRowDef, StatValue, StatsTexts, SummaryDef};

fn task(d: &str, state: TaskState, optional: bool, hidden: bool) -> TaskView {
    TaskView { description: d.into(), state, optional, hidden }
}

#[test]
fn objectives_list_orders_primary_first_and_hides_finished_tasks() {
    let objs = vec![
        ObjectiveView { description: "Find the key".into(), hidden: false, tasks: vec![task("Search the office", TaskState::Active, true, false)] },
        ObjectiveView { description: "Escape".into(), hidden: false, tasks: vec![task("Get out", TaskState::Active, false, false), task("Done bit", TaskState::Completed, false, false), task("Secret", TaskState::Active, false, true)] },
        ObjectiveView { description: "Hidden".into(), hidden: true, tasks: vec![task("x", TaskState::Active, false, false)] },
        ObjectiveView { description: "Finished".into(), hidden: false, tasks: vec![task("y", TaskState::Completed, false, false)] },
    ];
    let l = journal::objectives_list(&objs, false);
    // Primary objectives come first (0x00BD0A40), the optional one after (0x00BC9630); the
    // finished objective and the hidden one are not listed.
    assert_eq!(l.iter().map(|o| o.description.as_str()).collect::<Vec<_>>(), ["Escape", "Find the key"]);
    assert_eq!(l[0].tasks.iter().map(|t| t.description.as_str()).collect::<Vec<_>>(), ["Get out"]);
    assert!(l[1].optional && l[1].tasks[0].optional);
    assert_eq!(l[0].tasks[0].state, "not_done");
    assert!(l[0].tasks[0].hud_marker);
    // The history adds the finished tasks and objectives.
    let h = journal::objectives_list(&objs, true);
    assert_eq!(h.len(), 3);
    let esc = h.iter().find(|o| o.description == "Escape").unwrap();
    assert_eq!(esc.tasks.iter().map(|t| (t.description.as_str(), t.state)).collect::<Vec<_>>(), [("Get out", "not_done"), ("Done bit", "done")]);
    assert_eq!(h.iter().find(|o| o.description == "Finished").unwrap().state, "done");
    // Rows: a section whenever `bIsOptional` changes.
    let rows = journal::objective_rows(&l, "Primary tasks", "Optional Tasks");
    assert_eq!(rows[0], Row::Section("PRIMARY TASKS".into()));
    assert_eq!(rows[1], Row::Objective { text: "Escape".into(), state: "not_done", task: false });
    assert_eq!(rows[2], Row::Objective { text: "Get out".into(), state: "not_done", task: true });
    assert_eq!(rows[3], Row::Section("OPTIONAL TASKS".into()));
}

#[test]
fn tabs_and_start_tab_follow_the_movie() {
    let all = journal::tabs(true, true);
    assert_eq!(all, [Tab::Objectives, Tab::Logs, Tab::Powers, Tab::BoneCharms, Tab::Inventory]);
    let none = journal::tabs(false, false);
    assert_eq!(none, [Tab::Objectives, Tab::Logs, Tab::Inventory]);
    // `DJT_Powers` without powers opens the objectives (0x00BD11E0); `DJT_Sub_ChapterNotes`
    // the objectives' second sub tab.
    assert_eq!(journal::start_tab(&none, 2), (0, 0));
    assert_eq!(journal::start_tab(&none, 4), (2, 0));
    assert_eq!(journal::start_tab(&all, 3), (3, 0));
    assert_eq!(journal::start_tab(&all, 5), (0, 1));
    // Logs sub tabs list notes, books and audiographs (`EDisJournalItemSection` 8, 12, 7).
    let subs: Vec<_> = Tab::Logs.subs().iter().map(|s| s.1).collect();
    assert_eq!(subs, [journal::Sub::Items(&[8]), journal::Sub::Items(&[12]), journal::Sub::Items(&[7])]);
    assert_eq!(journal::journal_section("DJIS_Book"), Some(12));
    assert_eq!(journal::default_section("DisAbstractItemAudioLog"), 7);
    assert_eq!(journal::default_section("DisAbstractItem"), 1);
    assert_eq!(journal::unescape("a\\n\\nb"), "a\n\nb");
}

fn prison() -> MissionStatsDef {
    let row = |d: &str, stat: &str| StatRowDef { description: d.into(), stat: stat.into(), additional: "ePlayerStat_NumKills".into(), ..Default::default() };
    MissionStatsDef {
        path: "Twk_MissionStats.Twk_M0_Prison".into(),
        mission_number: 0,
        summary: vec![SummaryDef { description: "Opened Jelly's Safe".into(), flag: "Prison_JellySafe".into(), value: true }],
        rows: vec![
            row("Hostiles Killed", "ePlayerStat_HostileKill"),
            row("Overall Chaos (Current Total)", "ePlayerStat_ChaosLevel"),
            StatRowDef { description: "Didn't Kill Anyone".into(), stat: "ePlayerStat_HostileKill".into(), add_other: true, additional: "ePlayerStat_CivilianKill".into(), checkbox: true, ..Default::default() },
            StatRowDef { description: "Ghost (Never Detected)".into(), stat: "ePlayerStat_NPCsAlerted".into(), additional: "ePlayerStat_NumKills".into(), checkbox: true, ..Default::default() },
            StatRowDef { max: 3, ..row("Runes Found", "ePlayerStat_RuneFound") },
            StatRowDef { max: 515, ..row("Coins Found", "ePlayerStat_GoldFound") },
        ],
        background: "UI_MissionStatsBg_Prison.MissionStats_Prison".into(),
    }
}

#[test]
fn stats_rows_map_the_record_like_set_mission_stats() {
    let rec = MissionStatsRecord {
        tweak: "Twk_MissionStats.Twk_M0_Prison".into(),
        stats: vec![("ePlayerStat_CivilianKill".into(), 1.0), ("ePlayerStat_RuneFound".into(), 2.0), ("ePlayerStat_GoldFound".into(), 129.0)],
        chaos_high: false,
        flags: Vec::new(),
    };
    let (list, pickups) = mission_stats::screen_rows(&prison(), &rec, &StatsTexts::default());
    let v: Vec<(&str, &StatValue)> = list.iter().map(|r| (r.name.as_str(), &r.value)).collect();
    assert_eq!(v[0], ("Hostiles Killed", &StatValue::Number(0)));
    assert_eq!(v[1], ("Overall Chaos (Current Total)", &StatValue::Text("Low".into())));
    // HostileKill + CivilianKill = 1: not checked.
    assert_eq!(v[2], ("Didn't Kill Anyone", &StatValue::Check(false)));
    assert_eq!(v[3], ("Ghost (Never Detected)", &StatValue::Check(true)));
    // No summary item: the empty line.
    assert_eq!(v[4], ("No special actions accomplished", &StatValue::None));
    assert_eq!(list[0].section, "Mission Stats");
    assert_eq!(list[4].section, "Special Actions");
    assert_eq!(pickups.iter().map(|p| (p.icon, p.value, p.max)).collect::<Vec<_>>(), [("runes", 2, 3), ("coins", 129, 515)]);
    // A set summary flag lists the item instead; the chaos row reads High.
    let rec = MissionStatsRecord { chaos_high: true, flags: vec!["prison_jellysafe".into()], ..rec };
    let (list, _) = mission_stats::screen_rows(&prison(), &rec, &StatsTexts::default());
    assert_eq!(list[1].value, StatValue::Text("High".into()));
    assert_eq!(list.last().unwrap().name, "Opened Jelly's Safe");
    assert_eq!(list.len(), 5);
}

#[test]
fn stat_rounding_and_background_paths() {
    // `ROUND(v * 2 + 0.5) >> 1`.
    assert_eq!(mission_stats::round_stat(0.0), 0);
    assert_eq!(mission_stats::round_stat(1.0), 1);
    assert_eq!(mission_stats::round_stat(1.4), 1);
    assert_eq!(mission_stats::round_stat(1.6), 2);
    assert_eq!(mission_stats::round_stat(129.0), 129);
    assert_eq!(mission_stats::background_texture("UI_MissionStatsBg_Prison.MissionStats_Prison"), Some(("UI_MissionStatsBg_Prison_SF".into(), "MissionStats_Prison".into())));
    let defs = vec![prison(), MissionStatsDef { path: "Twk_MissionStats.Twk_M1_Overseers".into(), mission_number: 1, ..Default::default() }];
    assert_eq!(mission_stats::def_for(&defs, "twk_missionstats.twk_m1_overseers").unwrap().mission_number, 1);
    assert_eq!(mission_stats::def_for(&defs, "").unwrap().mission_number, 0);
}

/// With the install: every shipped `Twk_M*` tweak reads (rows, max values, background, story
/// flags of its summary items).
#[test]
fn shipped_mission_stats_tweaks_load() {
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    let defs = mission_stats::load_defs(&content);
    if defs.is_empty() {
        return;
    }
    let m0 = defs.iter().find(|d| d.path.ends_with("Twk_M0_Prison")).expect("Twk_M0_Prison");
    assert_eq!(m0.rows.len(), 12);
    assert_eq!(m0.rows[4].stat, "ePlayerStat_ChaosLevel");
    assert!(m0.rows[5].checkbox && m0.rows[5].add_other);
    assert!(m0.rows.iter().any(|r| r.stat == "ePlayerStat_GoldFound" && r.max > 0));
    assert!(m0.summary.iter().all(|s| !s.flag.is_empty()));
    assert_eq!(m0.background, "UI_MissionStatsBg_Prison.MissionStats_Prison");
    for n in 1..=8 {
        assert!(defs.iter().any(|d| d.mission_number == n), "mission {n}");
    }
}
