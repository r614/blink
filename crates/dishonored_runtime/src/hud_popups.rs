//! When the top-right HUD panels show: the objectives banner (`objectives_mc`) and the target
//! paper (`targetNotification_mc`). Both are popups in the original, never permanent panels
//! (decoded from the `UI_HUD_SF` "HUD" ActionScript and `DisTweaks_GFxMoviePlayerHUD`; see
//! research/dishonored/MENUS.md "Objective popups"):
//!
//! - `objectives_mc` is class `ObjectivesNotification`. `Open()` slides the backdrop in (alpha
//!   0 -> 100, x + 250 -> x, 0.2 s `Strong.easeOut`), then shows the task lines after
//!   `_tasksApparitionDelay` 700 ms, each for `_taskLifeDuration_const` 3500 ms
//!   (`_utils.DelayedAnimation`), then `Close()`: alpha -> 0 with x + 100 over 0.2 s (backdrop)
//!   / 0.25 s (name strip), hidden by `OnClosed` 500 ms later. Without tasks it closes after
//!   3500 ms.
//! - `targetNotification_mc` is class `TargetNotification`. `Open()` plays the portrait's open
//!   timeline and after 1000 ms (`OnOpened`) waits the notification's duration
//!   (`m_fTargetNotificationDuration` 5 s in `Twk_InGameUI.Twk_GFxMoviePlayerHUD`), then
//!   `Close()`: alpha -> 0 over 0.25 s `Strong.easeOut`, hidden 1000 ms later.
//! - The first popup of a game waits `m_fPopupDelayOnGameStart` (2 s).
//!
//! The port's match banner (mode, clock, score) has no counterpart in the single-player HUD; it
//! follows the objectives popup: it opens at match start (after the game-start delay), when the
//! local score or the leader changes, when a mission objective or task changes and on the
//! journal key (`GBA_Journal`, J: the original's on-demand objectives view is the journal's
//! objectives tab). The Contracts target paper opens when the target changes and on the same
//! key. The scoreboard stays on Tab (`hud.rs`).

use crate::session::SessionView;
use bevy::prelude::*;

/// `Strong.easeOut` of `mx.transitions.easing` (quintic): 1 - (1 - t)^5.
pub fn strong_ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(5)
}

/// Timings of one popup class (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupTiming {
    /// Open tween (alpha in, slide from `open_dx`).
    pub open: f32,
    pub open_dx: f32,
    /// From `Open()` to `Close()`.
    pub hold: f32,
    /// Close tween (alpha out, slide to `close_dx`).
    pub close: f32,
    pub close_dx: f32,
    /// From `Close()` to hidden (`OnClosed`).
    pub hidden_after: f32,
}

/// `ObjectivesNotification` with `tasks` task lines.
pub fn objectives_timing(tasks: usize) -> PopupTiming {
    let hold = if tasks > 0 { 0.7 + 3.5 * tasks as f32 } else { 3.5 };
    PopupTiming { open: 0.2, open_dx: 250.0, hold, close: 0.25, close_dx: 100.0, hidden_after: 0.5 }
}

/// `TargetNotification` with a display duration (`m_fTargetNotificationDuration`).
pub fn target_timing(duration: f32) -> PopupTiming {
    // The open is the portrait's timeline (not reproduced): shown at full alpha.
    PopupTiming { open: 0.0, open_dx: 0.0, hold: 1.0 + duration, close: 0.25, close_dx: 0.0, hidden_after: 1.0 }
}

pub const POPUP_DELAY_ON_GAME_START: f32 = 2.0;
pub const TARGET_NOTIFICATION_DURATION: f32 = 5.0;

/// Alpha (0..1) and horizontal offset (stage pixels) of a popup.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Envelope {
    pub alpha: f32,
    pub dx: f32,
}

impl Envelope {
    pub fn shown(&self) -> bool {
        self.alpha > 0.001
    }
}

/// One popup: when it was opened and with which timings.
#[derive(Clone, Copy, Debug, Default)]
pub struct Popup {
    opened: Option<(f32, PopupTiming)>,
}

impl Popup {
    /// `Open()`: (re)start the popup at `now` (the movie clears its intervals and replays).
    pub fn open(&mut self, now: f32, t: PopupTiming) {
        self.opened = Some((now, t));
    }

    pub fn envelope(&self, now: f32) -> Envelope {
        let Some((t0, t)) = self.opened else { return Envelope::default() };
        let e = now - t0;
        if e < 0.0 {
            return Envelope::default();
        }
        if e < t.hold {
            let k = if t.open > 0.0 { strong_ease_out(e / t.open) } else { 1.0 };
            return Envelope { alpha: k, dx: t.open_dx * (1.0 - k) };
        }
        let c = e - t.hold;
        if c >= t.hidden_after {
            return Envelope::default();
        }
        let k = if t.close > 0.0 { strong_ease_out(c / t.close) } else { 1.0 };
        Envelope { alpha: 1.0 - k, dx: t.close_dx * k }
    }
}

/// What opens the popups, compared frame to frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Triggers {
    /// Match identity (mode, match number): a new one is a game start.
    pub game: Option<(String, u32)>,
    /// Local score and the leader (multiplayer banner).
    pub score: Option<(i32, Option<u32>)>,
    /// Mission objectives: (objective, task, state, hidden) and the newest notification.
    pub objectives: Vec<(String, bool, Vec<(String, u8, bool)>)>,
    pub notification: Option<u32>,
    /// Contracts target.
    pub target: Option<u32>,
}

impl Triggers {
    pub fn of(view: &SessionView) -> Self {
        let m = &view.matchv;
        if m.mode.is_empty() {
            return Self::default();
        }
        let me = view.me.as_ref().map(|p| p.id);
        let mine = m.scores.iter().find(|e| Some(e.id) == me).map(|e| e.score);
        let leader = m.scores.iter().max_by_key(|e| e.score).map(|e| e.id);
        let mission = m.mission.as_ref();
        Self {
            game: Some((m.mode.clone(), m.match_number)),
            score: if mission.is_none() { mine.map(|s| (s, leader)) } else { None },
            objectives: mission
                .map(|mv| mv.objectives.iter().map(|o| (o.description.clone(), o.hidden, o.tasks.iter().map(|t| (t.description.clone(), t.state as u8, t.hidden)).collect())).collect())
                .unwrap_or_default(),
            notification: mission.and_then(|mv| mv.notification.as_ref().map(|n| n.0)),
            target: view.target_info.as_ref().and_then(|t| t.target),
        }
    }
}

/// Popup state of the HUD, read by `gfx_hud`.
#[derive(Resource, Default)]
pub struct HudPopups {
    pub banner: Popup,
    pub target: Popup,
    last: Triggers,
    /// Lines the banner shows (its "tasks"): the multiplayer score line, or the mission's
    /// visible active tasks.
    tasks: usize,
}

impl HudPopups {
    pub fn banner_env(&self, now: f32) -> Envelope {
        self.banner.envelope(now)
    }
    pub fn target_env(&self, now: f32) -> Envelope {
        self.target.envelope(now)
    }

    /// Compare the triggers with the last frame's and open what changed. `auto` false (the
    /// `PSI_HUD_bShowObjectivePopups` option off) leaves only the key.
    pub fn step(&mut self, now: f32, trig: Triggers, tasks: usize, key: bool, auto: bool) {
        self.tasks = tasks;
        let banner_t = objectives_timing(self.tasks);
        let target_t = target_timing(TARGET_NOTIFICATION_DURATION);
        let prev = std::mem::take(&mut self.last);
        if trig.game.is_none() {
            *self = Self::default();
            return;
        }
        let new_game = trig.game != prev.game;
        if auto {
            if new_game {
                self.banner.open(now + POPUP_DELAY_ON_GAME_START, banner_t);
                if trig.target.is_some() {
                    self.target.open(now + POPUP_DELAY_ON_GAME_START, target_t);
                }
            } else {
                let changed = (trig.score != prev.score && prev.score.is_some() && trig.score.is_some()) || trig.objectives != prev.objectives;
                if changed {
                    self.banner.open(now, banner_t);
                }
                // `TargetNotification` opens for a target notification only: the Contracts
                // target, or a mission's Notify action (never for the task list).
                let notified = trig.notification != prev.notification && trig.notification.is_some();
                if (trig.target != prev.target && trig.target.is_some()) || notified {
                    self.target.open(now, target_t);
                }
            }
        }
        if key {
            self.banner.open(now, banner_t);
            if trig.target.is_some() || trig.notification.is_some() {
                self.target.open(now, target_t);
            }
        }
        self.last = trig;
    }
}

/// Track the triggers each frame (before `gfx_hud` draws).
pub fn track(
    view: Res<SessionView>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Option<Res<crate::settings::Bindings>>,
    settings: Option<Res<crate::settings::GameSettings>>,
    local: Res<crate::input_map::LocalInput>,
    mut popups: ResMut<HudPopups>,
) {
    let trig = Triggers::of(&view);
    let tasks = match view.matchv.mission.as_ref() {
        Some(mv) => mv.objectives.iter().filter(|o| !o.hidden).flat_map(|o| o.tasks.iter()).filter(|t| !t.hidden && t.state == dishonored_mission::TaskState::Active).count(),
        None => 1,
    };
    let key = local.enabled && bindings.is_some_and(|b| b.just_pressed("GBA_Journal", &keys, &mouse, 0.0));
    let auto = settings.is_none_or(|s| s.s.hud.objective_popups);
    popups.step(time.elapsed_secs(), trig, tasks, key, auto);
}
