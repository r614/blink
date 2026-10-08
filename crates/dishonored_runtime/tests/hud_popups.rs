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
