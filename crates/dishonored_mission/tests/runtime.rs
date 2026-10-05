//! Interpreter behaviour on the sample mission, with synthetic positions and events (no
//! install needed).

use dishonored_mission::runtime::{Command, MissionEvent, MissionState, StepInput};
use dishonored_mission::{MissionDef, NpcKind, TaskState};
use glam::Vec3;
use std::collections::BTreeMap;

const SAMPLE: &str = include_str!("../../../missions/boyle_masked_host.ron");

fn mission() -> MissionState {
    let def = MissionDef::from_ron(SAMPLE).expect("sample parses");
    assert!(def.validate().is_empty(), "{:?}", def.validate());
    // Synthetic layout: markers 2000 uu apart along X.
    let markers = def.markers.iter().enumerate().map(|(i, m)| (m.id.clone(), (Vec3::new(i as f32 * 2000.0, 0.0, 0.0), m.radius))).collect();
    MissionState::new(def, markers)
}

fn pos(m: &MissionState, marker: &str) -> Vec3 {
    m.markers[marker].0
}

fn step(m: &mut MissionState, player: Vec3, events: &[MissionEvent]) -> Vec<Command> {
    let npcs: BTreeMap<String, Vec3> = BTreeMap::from([("host".to_string(), Vec3::new(500.0, 500.0, 0.0))]);
    m.step(&StepInput { dt: 1.0 / 60.0, map: "", players: &[(1, player, true)], npc_positions: &npcs, events, total_npcs: 24 })
}

fn kill(npc: &str) -> MissionEvent {
    MissionEvent::NpcKilled { npc: Some(npc.into()), by_player: true, kind: NpcKind::Civilian }
}

#[test]
fn clean_run_completes_with_optional_goals() {
    let mut m = mission();
    let start = pos(&m, "start");
    step(&mut m, start, &[]);
    assert_eq!(m.view.messages.first().map(|x| x.1.as_str()), Some("The Masked Host"));
    assert!(m.view.notification.is_some());
    assert_eq!(m.task("host.kill"), Some(TaskState::Inactive));
    // Walking past the informant is not enough; talking to him reveals the host.
    let p = pos(&m, "informant_spot");
    step(&mut m, p, &[]);
    assert!(!m.flag("know_host"));
    assert!(m.view.talkers.iter().any(|t| t.0 == "informant"));
    step(&mut m, p, &[MissionEvent::Talked("informant".into())]);
    assert!(m.flag("know_host"));
    assert_eq!(m.task("host.kill"), Some(TaskState::Active));
    assert_eq!(m.view.target, Some([500.0, 500.0, 0.0]), "marker follows the host");
    step(&mut m, start + Vec3::new(0.0, 5000.0, 0.0), &[kill("host")]);
    assert!(m.flag("host_dead"));
    assert_eq!(m.task("host.escape"), Some(TaskState::Active));
    assert_eq!(m.task("style.precise"), Some(TaskState::Completed));
    let p = pos(&m, "exfil");
    step(&mut m, p, &[]);
    let o = m.outcome().expect("mission over");
    assert!(o.success, "{o:?}");
    assert_eq!(m.task("style.ghost"), Some(TaskState::Completed));
    assert_eq!(m.view.stats.civilians_killed, 1);
    assert!(m.view.stats.ghost());
}

#[test]
fn wrong_guest_detection_and_alarm() {
    let mut m = mission();
    let start = pos(&m, "start");
    step(&mut m, start, &[]);
    step(&mut m, start, &[kill("decoy_a")]);
    assert_eq!(m.task("style.precise"), Some(TaskState::Failed));
    assert!(m.view.messages.iter().any(|x| x.1 == "That was not the host."));
    step(&mut m, start, &[MissionEvent::PlayerDetected, MissionEvent::AlarmRaised]);
    assert_eq!(m.task("style.ghost"), Some(TaskState::Failed));
    // Reinforcements arrive 4 s after the alarm (SeqAct_Delay equivalent).
    let mut spawned = Vec::new();
    for _ in 0..(5 * 60) {
        for c in step(&mut m, start, &[]) {
            if let Command::Spawn(n) = c {
                spawned.push(n);
            }
        }
    }
    assert_eq!(spawned, vec!["reinforcement_a".to_string(), "reinforcement_b".to_string()]);
    assert!(m.outcome().is_none());
}

#[test]
fn exfil_without_the_kill_does_nothing_and_time_limit_fails() {
    let mut m = mission();
    let p = pos(&m, "exfil");
    step(&mut m, p, &[]);
    assert!(m.outcome().is_none());
    // Fast-forward past the 900 s limit.
    let far = Vec3::new(-50000.0, 0.0, 0.0);
    for _ in 0..(901 * 60) {
        step(&mut m, far, &[]);
        if m.outcome().is_some() {
            break;
        }
    }
    let o = m.outcome().expect("timed out");
    assert!(!o.success);
}

#[test]
fn death_without_respawn_fails() {
    let mut m = mission();
    let npcs = BTreeMap::new();
    m.step(&StepInput { dt: 1.0 / 60.0, map: "", players: &[(1, Vec3::ZERO, false)], npc_positions: &npcs, events: &[MissionEvent::PlayerKilled(1)], total_npcs: 0 });
    assert!(matches!(m.outcome(), Some(o) if !o.success));
}

#[test]
fn ron_round_trip() {
    let def = MissionDef::from_ron(SAMPLE).unwrap();
    let again = MissionDef::from_ron(&def.to_ron()).unwrap();
    assert_eq!(def, again);
}
