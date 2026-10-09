//! Tests of `dishonored_runtime::hud_popups` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use dishonored_mission::TaskState;
use dishonored_mission::runtime::{MissionView, ObjectiveView, TaskView};
use dishonored_runtime::hud_popups::*;

fn shown(c: Option<PopupContent>) -> (String, u8, usize) {
    match c {
        Some(PopupContent::Objective { name, event, tasks }) => (name, event, tasks.len()),
        Some(PopupContent::Target { name, .. }) => (name, EV_TARGET, 0),
        None => (String::new(), 255, 0),
    }
}

#[test]
fn first_popup_waits_for_the_game_start_delay() {
    let mut q = PopupQueue::default();
    q.objective_added(0, "Eliminate Lady Boyle");
    // m_fPopupDelayOnGameStart = 2 s, strictly below zero to show.
    assert!(q.tick(1.0, false, true, false, false).is_none());
    assert!(q.tick(0.99, false, true, false, false).is_none());
    // Paused: the timer does not run.
    assert!(q.tick(5.0, true, true, false, false).is_none());
    assert_eq!(shown(q.tick(0.02, false, true, false, false)), ("Eliminate Lady Boyle".into(), EV_ADDED, 0));
}

#[test]
fn next_popup_waits_until_the_active_one_closed() {
    let mut q = PopupQueue::default();
    q.timer = -1.0;
    q.objective_added(0, "A");
    q.objective_added(1, "B");
    assert_eq!(shown(q.tick(0.016, false, true, false, false)).0, "A");
    // objectives_mc._opened is true while A shows (open, tasks, close + 500 ms).
    assert!(q.tick(0.016, false, true, true, false).is_none());
    assert!(q.tick(0.016, false, true, true, false).is_none());
    assert_eq!(shown(q.tick(0.016, false, true, false, false)).0, "B");
    assert!(q.tick(0.016, false, true, false, false).is_none());
}

#[test]
fn task_events_merge_into_queued_popups() {
    let mut q = PopupQueue::default();
    q.objective_added(0, "O");
    // Task added: merged into the queued Added popup.
    q.task_added(0, "O", 0, "Go to the party");
    assert_eq!(q.queue.len(), 1);
    assert_eq!(q.queue[0].tasks.len(), 1);
    // Task updated: only an Updated popup takes it, so a new popup.
    q.task_updated(0, "O", 0, "Go to the party (1/2)");
    assert_eq!(q.queue.len(), 2);
    assert_eq!(q.queue[1].event, EV_UPDATED);
    // Completed: merges into the Updated popup, replacing the task's earlier entry.
    q.task_completed(0, "O", 0, "Go to the party");
    assert_eq!(q.queue.len(), 2);
    assert_eq!(q.queue[1].tasks.len(), 1);
    assert_eq!(q.queue[1].tasks[0].event, EV_COMPLETED);
    // Objective completed: the Updated popup goes, its completed task moves to the new one.
    q.objective_ended(0, "O", false);
    assert_eq!(q.queue.len(), 2);
    assert_eq!(q.queue[1].event, EV_COMPLETED);
    assert_eq!(q.queue[1].tasks.len(), 1);
    // A failed task is dropped while a Completed popup of the objective is queued.
    q.task_failed(0, "O", 1, "Spare her");
    assert_eq!(q.queue.len(), 2);
}

#[test]
fn popups_off_empties_the_queue() {
    let mut q = PopupQueue::default();
    q.timer = -1.0;
    q.objective_added(0, "A");
    q.objective_added(1, "B");
    assert!(q.tick(0.016, false, false, false, false).is_none());
    assert!(q.queue.is_empty());
}

#[test]
fn mission_changes_become_popups() {
    let task = |d: &str, s: TaskState| TaskView { description: d.into(), state: s, optional: false, hidden: false };
    let mut mv = MissionView { objectives: vec![ObjectiveView { description: "Lady Boyle".into(), hidden: false, tasks: vec![task("Find her", TaskState::Active)] }], ..Default::default() };
    let mut q = PopupQueue::default();
    q.observe(&mv);
    // Objective added with its task merged in.
    assert_eq!(q.queue.len(), 1);
    assert_eq!((q.queue[0].event, q.queue[0].tasks.len()), (EV_ADDED, 1));
    q.queue.clear();
    // Unchanged: nothing.
    q.observe(&mv);
    assert!(q.queue.is_empty());
    // The task completes (the only required task): an Updated popup, then the objective's
    // Completed popup takes its task.
    mv.objectives[0].tasks[0].state = TaskState::Completed;
    q.observe(&mv);
    assert_eq!(q.queue.len(), 1);
    assert_eq!(q.queue[0].event, EV_COMPLETED);
    assert_eq!(q.queue[0].tasks[0].event, EV_COMPLETED);
    // A target notification queues behind.
    mv.notification = Some((7, "TargetEliminated".into(), "Twk_ChapterTarget_LadyBoyle".into()));
    q.observe(&mv);
    assert_eq!(q.queue.len(), 2);
    assert_eq!(q.queue[1].event, EV_TARGET);
}

// --- interactions (the crosshair's focus) ------------------------------------------------------

/// `m_InteractionTexts` stand-ins by `EDisUIInteraction` (the install's values are read at run time).
fn texts() -> Vec<String> {
    let mut t = vec![String::new(); 40];
    t[DUI_SPECIAL_CHOKE] = "`GBA_Block` CHOKE".into();
    t[DUI_CROSSHAIR_PICKUP] = "`GBA_Use` PICKUP".into();
    t[DUI_CROSSHAIR_CARRY] = "`GBA_Use` CARRY".into();
    t
}

fn focus(crosshair: Option<(dishonored_sim::crosshair::TargetRef, dishonored_sim::crosshair::CrosshairStatus)>, text: &str, name: &str, choke: bool) -> dishonored_multiplayer::MatchView {
    let crosshair = crosshair.map(|(target, status)| dishonored_sim::crosshair::Crosshair { target, usable: true, status });
    dishonored_multiplayer::MatchView {
        focus: vec![dishonored_multiplayer::FocusView { player: 1, crosshair, text: text.into(), name: name.into(), point: Some([1.0, 2.0, 3.0]), choke, ..Default::default() }],
        ..Default::default()
    }
}

fn me() -> dishonored_sim::state::PlayerState {
    dishonored_sim::state::PlayerState::new(1, glam::Vec3::ZERO, 0.0)
}

#[test]
fn the_prompt_is_the_hosts_crosshair_target() {
    use dishonored_sim::crosshair::{CrosshairStatus as St, TargetRef as T};
    let mv = MissionView::default();
    // A host target's prompt and name, as the host sent them; the crosshair moves to its point.
    let f = focused_interaction(&mv, &focus(Some((T::Host(0), St::Usable)), "`GBA_Use` Talk", "Granny Rags", false), &me(), &texts()).unwrap();
    assert_eq!((f.text.as_str(), f.name.as_str(), f.point), ("`GBA_Use` Talk", "Granny Rags", Some([1.0, 2.0, 3.0])));
    // A prompt without a key tag is a Use interaction.
    let f = focused_interaction(&mv, &focus(Some((T::Host(0), St::Usable)), "Open", "", false), &me(), &texts()).unwrap();
    assert_eq!(f.text, "`GBA_Use` Open");
    // Targets without a text of their own: the HUD's text for their kind.
    assert_eq!(focused_interaction(&mv, &focus(Some((T::Pickup(4), St::Pickup)), "", "", false), &me(), &texts()).unwrap().text, "`GBA_Use` PICKUP");
    assert_eq!(focused_interaction(&mv, &focus(Some((T::Body(7), St::MovableCorpse)), "", "", false), &me(), &texts()).unwrap().text, "`GBA_Use` CARRY");
    // Nothing in focus: no window, whatever else is near.
    assert_eq!(focused_interaction(&mv, &focus(None, "", "", false), &me(), &texts()), None);
    // Another player's focus is not ours.
    let mut other = me();
    other.id = 2;
    assert_eq!(focused_interaction(&mv, &focus(Some((T::Host(0), St::Usable)), "`GBA_Use` Talk", "", false), &other, &texts()), None);
}

#[test]
fn the_choke_is_a_special_icon_not_a_window_line() {
    use dishonored_runtime::hud_popups::interaction_groups;
    use dishonored_sim::crosshair::{CrosshairStatus as St, TargetRef as T};
    let mv = MissionView::default();
    // A pawn that can be choked: no window, the choke icon (Special group, 0x00B87D20).
    assert_eq!(focused_interaction(&mv, &focus(Some((T::Pawn(9), St::Usable)), "", "", true), &me(), &texts()), None);
    assert_eq!(interaction_groups(&mv, &focus(Some((T::Pawn(9), St::Usable)), "", "", true), &me(), false).special, vec![DUI_SPECIAL_CHOKE]);
    // A talker that can be choked: Talk in the window, the choke as an icon.
    let f = focused_interaction(&mv, &focus(Some((T::Host(1), St::Usable)), "`GBA_Use` Talk", "Slackjaw", true), &me(), &texts()).unwrap();
    assert_eq!(f.text, "`GBA_Use` Talk");
    // A drop assassination wins over the assassination (0x00B8CC60).
    let mut m = focus(Some((T::Pawn(9), St::Usable)), "", "", false);
    m.focus[0].assassinate = true;
    m.focus[0].drop_assassinate = true;
    let g = interaction_groups(&mv, &m, &me(), false);
    assert_eq!((g.special.clone(), g.qte.clone()), (vec![], vec![5]));
}

#[test]
fn climbing_and_choices_replace_the_focus() {
    use dishonored_sim::crosshair::{CrosshairStatus as St, TargetRef as T};
    let mut mv = MissionView::default();
    let mut p = me();
    p.mode = dishonored_sim::state::MoveMode::Climbing;
    // On a chain: no window, the Context group's two lines (0x00A67830).
    assert_eq!(focused_interaction(&mv, &focus(Some((T::Host(0), St::Usable)), "`GBA_Use` Talk", "", false), &p, &texts()), None);
    assert_eq!(dishonored_runtime::hud_popups::interaction_groups(&mv, &focus(None, "", "", false), &p, false).context, vec![12, 13]);
    mv.choices.push((1, vec!["Yes".into()]));
    assert_eq!(focused_interaction(&mv, &focus(Some((T::Host(0), St::Usable)), "`GBA_Use` Talk", "", false), &me(), &texts()), None);
}

/// The install's `m_InteractionTexts`: the choke's line is a Block one (skipped without the install).
#[test]
#[ignore]
fn retail_interaction_texts_decode() {
    let Ok(c) = dishonored_assets::Content::locate() else { return };
    let loc = dishonored_assets::loc::Localization::load(&c.root, "INT", &["Twk_InGameUI"]);
    let t = dishonored_runtime::gfx_hud::interaction_texts(&c, &loc);
    assert!(t.get(DUI_SPECIAL_CHOKE).is_some_and(|s| s.starts_with("`GBA_Block`")), "{t:?}");
    assert!(t.get(DUI_CROSSHAIR_CARRY).is_some_and(|s| s.contains("`GBA_Use`")), "{t:?}");
}