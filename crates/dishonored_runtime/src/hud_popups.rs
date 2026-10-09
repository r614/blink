//! The HUD's popup queue and what feeds it (`DisGFxMoviePlayerHUD.m_PopupQueue`,
//! `m_ActivePopup`, `m_fPopupTimer`; research/dishonored/HUD.md "Popups"):
//!
//! - Enqueueing (only while `m_bHandleObjectiveEvents`), one `DisObjectivePopupInfo` per popup
//!   (objective, event, task list):
//!   - objective added (0x00BB2480): a new popup {objective, Added};
//!   - task added (0x00BB27C0): merged into a queued popup of the same objective whose event is
//!     Added or Updated, else a new popup {objective, Updated, [task]};
//!   - task updated (0x00BB28E0): merged into a queued Updated popup, else new;
//!   - task completed (0x00BB2A00): merged into a queued Updated or Completed popup, else new;
//!   - task failed (0x00BB2B20): merged into a queued Updated or Failed popup; dropped when a
//!     Completed popup of the objective is queued; else new;
//!   - objective completed / failed (0x00BB2500 / 0x00BB2660): queued Updated popups of the
//!     objective are removed and their completed (failed) tasks move to the new
//!     {objective, Completed (Failed)} popup;
//!   - merging a task replaces an earlier entry of the same task (0x00B9EE60).
//! - Showing (0x00BAFE70, every HUD tick): `m_fPopupTimer` (starts at `m_fPopupDelayOnGameStart`
//!   2 s) counts down while the game runs; once below zero, and once the active popup's clip
//!   reports `_opened == false` (objectives_mc, or targetNotification_mc for a target
//!   notification), the next popup is taken from the front of the queue and shown
//!   (`SetObjectives` / `SetNotification`). With `PSI_HUD_bShowObjectivePopups` off the queue
//!   is emptied instead.
//!
//! The port's missions expose objectives as a list, so the events are found by comparing the
//! mission view with the previous one ([`PopupQueue::observe`]). An objective counts as
//! completed when all its visible required tasks are completed (the original's objectives
//! complete from their tasks the same way; LIKELY).

use crate::gfx_stage::{ImageRef, Stage};
use crate::hud_widgets::{HudWidgets, TargetKind};
use dishonored_mission::TaskState;
use dishonored_mission::runtime::MissionView;

/// `m_fPopupDelayOnGameStart`.
pub const POPUP_DELAY_ON_GAME_START: f32 = 2.0;

/// `EDisUIObjectiveEvent` / `EDisUITaskEvent`.
pub const EV_ADDED: u8 = 0;
pub const EV_UPDATED: u8 = 1;
pub const EV_COMPLETED: u8 = 2;
pub const EV_FAILED: u8 = 3;
pub const EV_TARGET: u8 = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct TaskInfo {
    /// Task key (objective index, task index).
    pub task: (usize, usize),
    pub event: u8,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PopupInfo {
    /// Objective index (None for a target notification).
    pub objective: Option<usize>,
    pub name: String,
    pub event: u8,
    pub tasks: Vec<TaskInfo>,
    pub target: Option<(String, Option<String>, TargetKind)>,
}

/// What to show for a popup taken from the queue.
#[derive(Clone, Debug, PartialEq)]
pub enum PopupContent {
    /// `SetObjectives(name, title of event, event, [(description, task event)])`.
    Objective { name: String, event: u8, tasks: Vec<(String, u8)> },
    /// `SetNotification(name, portrait, kind, duration)`.
    Target { name: String, portrait: Option<String>, kind: TargetKind },
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Snapshot {
    /// Per objective: (description, hidden, per task (description, state, hidden, optional)).
    objectives: Vec<(String, bool, Vec<(String, TaskState, bool, bool)>)>,
    notification: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct PopupQueue {
    pub timer: f32,
    pub active: Option<PopupInfo>,
    pub queue: Vec<PopupInfo>,
    /// `m_bHandleObjectiveEvents`.
    pub handle_events: bool,
    last: Option<Snapshot>,
}

impl Default for PopupQueue {
    fn default() -> Self {
        Self { timer: POPUP_DELAY_ON_GAME_START, active: None, queue: Vec::new(), handle_events: true, last: None }
    }
}

fn merge_task(p: &mut PopupInfo, t: TaskInfo) {
    p.tasks.retain(|x| x.task != t.task);
    p.tasks.push(t);
}

impl PopupQueue {
    pub fn objective_added(&mut self, obj: usize, name: &str) {
        if self.handle_events {
            self.queue.push(PopupInfo { objective: Some(obj), name: name.to_string(), event: EV_ADDED, tasks: Vec::new(), target: None });
        }
    }

    fn task_event(&mut self, obj: usize, name: &str, t: TaskInfo, merge_into: &[u8], drop_if: &[u8]) {
        if !self.handle_events {
            return;
        }
        for p in self.queue.iter_mut().filter(|p| p.objective == Some(obj)) {
            if merge_into.contains(&p.event) {
                merge_task(p, t);
                return;
            }
            if drop_if.contains(&p.event) {
                return;
            }
        }
        let mut p = PopupInfo { objective: Some(obj), name: name.to_string(), event: EV_UPDATED, tasks: Vec::new(), target: None };
        merge_task(&mut p, t);
        self.queue.push(p);
    }

    pub fn task_added(&mut self, obj: usize, name: &str, task: usize, desc: &str) {
        self.task_event(obj, name, TaskInfo { task: (obj, task), event: EV_ADDED, description: desc.to_string() }, &[EV_ADDED, EV_UPDATED], &[]);
    }
    pub fn task_updated(&mut self, obj: usize, name: &str, task: usize, desc: &str) {
        self.task_event(obj, name, TaskInfo { task: (obj, task), event: EV_UPDATED, description: desc.to_string() }, &[EV_UPDATED], &[]);
    }
    pub fn task_completed(&mut self, obj: usize, name: &str, task: usize, desc: &str) {
        self.task_event(obj, name, TaskInfo { task: (obj, task), event: EV_COMPLETED, description: desc.to_string() }, &[EV_UPDATED, EV_COMPLETED], &[]);
    }
    pub fn task_failed(&mut self, obj: usize, name: &str, task: usize, desc: &str) {
        self.task_event(obj, name, TaskInfo { task: (obj, task), event: EV_FAILED, description: desc.to_string() }, &[EV_UPDATED, EV_FAILED], &[EV_COMPLETED]);
    }

    /// Objective completed / failed: queued Updated popups of it go, their tasks with the same
    /// end state move to the new popup.
    pub fn objective_ended(&mut self, obj: usize, name: &str, failed: bool) {
        if !self.handle_events {
            return;
        }
        let ev = if failed { EV_FAILED } else { EV_COMPLETED };
        let mut p = PopupInfo { objective: Some(obj), name: name.to_string(), event: ev, tasks: Vec::new(), target: None };
        let mut i = self.queue.len();
        while i > 0 {
            i -= 1;
            if self.queue[i].objective == Some(obj) && self.queue[i].event == EV_UPDATED {
                let q = self.queue.remove(i);
                for t in q.tasks.into_iter().filter(|t| t.event == ev) {
                    merge_task(&mut p, t);
                }
            }
        }
        self.queue.push(p);
    }

    /// `DisSeqAct_ShowTargetNotification` (event 4).
    pub fn target_notification(&mut self, name: &str, portrait: Option<String>, kind: TargetKind) {
        self.queue.push(PopupInfo { objective: None, name: name.to_string(), event: EV_TARGET, tasks: Vec::new(), target: Some((name.to_string(), portrait, kind)) });
    }

    /// One HUD tick (0x00BAFE70). `obj_open` / `target_open`: the clips' `_opened`.
    pub fn tick(&mut self, dt: f32, paused: bool, popups_on: bool, obj_open: bool, target_open: bool) -> Option<PopupContent> {
        if !paused {
            self.timer -= dt;
        }
        let mut ready = self.timer < 0.0;
        if let Some(a) = &self.active {
            let open = if a.event == EV_TARGET { target_open } else { obj_open };
            ready &= !open;
        }
        if !ready {
            return None;
        }
        self.active = None;
        if !popups_on {
            self.queue.clear();
            return None;
        }
        if self.queue.is_empty() {
            return None;
        }
        let p = self.queue.remove(0);
        self.active = Some(p.clone());
        Some(match p.target {
            Some((name, portrait, kind)) => PopupContent::Target { name, portrait, kind },
            None => PopupContent::Objective { name: p.name, event: p.event, tasks: p.tasks.into_iter().map(|t| (t.description, t.event)).collect() },
        })
    }

    /// Compare the mission's objectives with the last view and enqueue what changed.
    pub fn observe(&mut self, mv: &MissionView) {
        let snap = Snapshot {
            objectives: mv.objectives.iter().map(|o| (o.description.clone(), o.hidden, o.tasks.iter().map(|t| (t.description.clone(), t.state, t.hidden, t.optional)).collect())).collect(),
            notification: mv.notification.as_ref().map(|n| n.0),
        };
        let prev = self.last.take().unwrap_or_default();
        for (oi, (name, hidden, tasks)) in snap.objectives.iter().enumerate() {
            if *hidden {
                continue;
            }
            let po = prev.objectives.get(oi);
            let was_visible = po.is_some_and(|p| !p.1);
            if !was_visible {
                self.objective_added(oi, name);
            }
            for (ti, (desc, state, thidden, _)) in tasks.iter().enumerate() {
                if *thidden || *state == TaskState::Inactive {
                    continue;
                }
                let pt = po.filter(|_| was_visible).and_then(|p| p.2.get(ti)).filter(|p| !p.2);
                let pstate = pt.map(|p| p.1).unwrap_or(TaskState::Inactive);
                match (pstate, *state) {
                    (TaskState::Inactive, TaskState::Active) => self.task_added(oi, name, ti, desc),
                    (TaskState::Inactive, TaskState::Completed) | (TaskState::Active, TaskState::Completed) => self.task_completed(oi, name, ti, desc),
                    (TaskState::Inactive, TaskState::Failed) | (TaskState::Active, TaskState::Failed) => self.task_failed(oi, name, ti, desc),
                    (TaskState::Active, TaskState::Active) if pt.is_some_and(|p| p.0 != *desc) => self.task_updated(oi, name, ti, desc),
                    _ => {}
                }
            }
            let done = |ts: &Vec<(String, TaskState, bool, bool)>| {
                let req: Vec<_> = ts.iter().filter(|t| !t.2 && !t.3).collect();
                !req.is_empty() && req.iter().all(|t| t.1 == TaskState::Completed)
            };
            if done(tasks) && !(was_visible && po.is_some_and(|p| done(&p.2))) {
                self.objective_ended(oi, name, false);
            }
        }
        // Mission target notifications (`DisSeqAct_ShowTargetNotification`: state, target).
        if snap.notification != prev.notification {
            if let Some((_, state, target)) = &mv.notification {
                let kind = match state.as_str() {
                    s if s.contains("Neutralized") => TargetKind::Neutralized,
                    s if s.contains("Rescued") => TargetKind::Rescued,
                    s if s.contains("Spared") => TargetKind::Spared,
                    _ => TargetKind::Killed,
                };
                if !state.contains("Active") {
                    self.target_notification(target, None, kind);
                }
            }
        }
        self.last = Some(snap);
    }
}

// --- interactions -----------------------------------------------------------------------------

/// The interactable the player looks at (the original's interaction focus).
#[derive(Clone, Debug, PartialEq)]
pub struct Focus {
    /// Interaction text with its key tag (`` `GBA_Use` Talk ``).
    pub text: String,
    /// Where the crosshair moves to (world, Dishonored units).
    pub point: Option<[f32; 3]>,
    /// What it is (`m_InteractableName`, the window's title): a talker's name.
    pub name: String,
}

/// `EDisUIInteraction` (DisGFxMoviePlayerHUD), the index of `m_InteractionTexts`.
pub const DUI_SPECIAL_CHOKE: usize = 3;
pub const DUI_CROSSHAIR_PICKUP: usize = 31;
pub const DUI_CROSSHAIR_CARRY: usize = 32;

/// The interactions window for the local player (the Crosshair interaction group, 0x00BA6150):
/// exactly what the host's sim chose as the crosshair target (`MatchView::focus`,
/// `dishonored_sim::crosshair`) with its prompt. `texts` is
/// `Twk_GFxMoviePlayerHUD.m_InteractionTexts`, by `EDisUIInteraction`. The choke and assassinate
/// prompts are the Special group's icons, the chain's lines the Context group
/// ([`interaction_groups`]).
pub fn focused_interaction(mv: &MissionView, matchv: &dishonored_multiplayer::MatchView, me: &dishonored_sim::state::PlayerState, texts: &[String]) -> Option<Focus> {
    use dishonored_sim::crosshair::CrosshairStatus;
    if !me.alive || mv.outcome.is_some() {
        return None;
    }
    // An open dialog choice replaces the interactions.
    if mv.choices.iter().any(|c| c.0 == me.id) {
        return None;
    }
    // On a chain the climb state's builder returns early with its Context lines (0x00A67830).
    if me.mode == dishonored_sim::state::MoveMode::Climbing {
        return None;
    }
    let f = matchv.focus.iter().find(|f| f.player == me.id)?;
    let text = |i: usize| texts.get(i).filter(|s| !s.is_empty()).cloned();
    let mut lines: Vec<String> = Vec::new();
    let use_line = if !f.text.is_empty() {
        // Prompts without a key tag are Use interactions (`m_InteractText` "`GBA_Use` ...").
        Some(if f.text.contains('`') { f.text.clone() } else { format!("`GBA_Use` {}", f.text) })
    } else {
        // A target with no text of its own: the HUD's text for its kind.
        match f.crosshair.map(|c| c.status) {
            Some(CrosshairStatus::Pickup) => text(DUI_CROSSHAIR_PICKUP),
            Some(CrosshairStatus::MovableCorpse) => text(DUI_CROSSHAIR_CARRY),
            _ => None,
        }
    };
    lines.extend(use_line);
    if lines.is_empty() {
        return None;
    }
    Some(Focus { text: lines.join("\n"), point: f.point, name: f.name.clone() })
}

/// The Context, Special and SpecialQTE interaction groups (`EDisUIInteraction` ids, ascending;
/// 0x00B87D20 picks the group of an id).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InteractionGroups {
    pub context: Vec<usize>,
    pub special: Vec<usize>,
    pub qte: Vec<usize>,
}

/// `EDisUIInteraction` ids of the groups the port builds.
pub const DUI_SPECIAL_ASSASSINATE: usize = 4;
pub const DUI_QTE_DROP_ASSASSINATE: usize = 5;
pub const DUI_CONTEXT_DROP_MOVABLE: usize = 8;
pub const DUI_CONTEXT_THROW_MOVABLE: usize = 9;
pub const DUI_CONTEXT_DROP_CORPSE: usize = 10;
pub const DUI_CONTEXT_THROW_CORPSE: usize = 11;
pub const DUI_CONTEXT_CHAIN_DROP: usize = 12;
pub const DUI_CONTEXT_CHAIN_JUMP: usize = 13;
pub const DUI_CONTEXT_EXIT_KEYHOLE: usize = 14;

/// The interaction builder (0x00AA9130) for the groups the window does not show: a state's own
/// builder first, returning early (the chain 0x00A67830: Context 12 and 13; the keyhole peek
/// 0x00A5FF60: Context 14); else the focus's special flags (0x00B8CC60: a drop assassination,
/// QTE 5, wins over the assassination, Special 4; the choke, Special 3) and the context of what
/// the hands hold (a prop: 8 and 9; a body: 10 and 11). Mantle, steal, adrenaline kill and the
/// power-cancel lines are not built (their sim state is missing).
pub fn interaction_groups(mv: &MissionView, matchv: &dishonored_multiplayer::MatchView, me: &dishonored_sim::state::PlayerState, peeking: bool) -> InteractionGroups {
    let mut g = InteractionGroups::default();
    if !me.alive || mv.outcome.is_some() || mv.choices.iter().any(|c| c.0 == me.id) {
        return g;
    }
    if me.mode == dishonored_sim::state::MoveMode::Climbing {
        g.context = vec![DUI_CONTEXT_CHAIN_DROP, DUI_CONTEXT_CHAIN_JUMP];
        return g;
    }
    if peeking {
        g.context = vec![DUI_CONTEXT_EXIT_KEYHOLE];
        return g;
    }
    if let Some(f) = matchv.focus.iter().find(|f| f.player == me.id) {
        if f.drop_assassinate {
            g.qte.push(DUI_QTE_DROP_ASSASSINATE);
        } else if f.assassinate {
            g.special.push(DUI_SPECIAL_ASSASSINATE);
        }
        if f.choke {
            g.special.push(DUI_SPECIAL_CHOKE);
        }
        g.special.sort_unstable();
    }
    if me.holding.is_some() {
        g.context = vec![DUI_CONTEXT_DROP_MOVABLE, DUI_CONTEXT_THROW_MOVABLE];
    } else if me.carrying.is_some() {
        g.context = vec![DUI_CONTEXT_DROP_CORPSE, DUI_CONTEXT_THROW_CORPSE];
    }
    g
}

/// `m_AsleepCrosshairText` (the pawn tweaks' value): the crosshair's info line over someone
/// asleep or knocked out. Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
pub const ASLEEP_CROSSHAIR_TEXT: &str = "(Unconscious)";

/// How far the crosshair probe reaches a body (uu).
const SLEEPER_REACH: f32 = 300.0;

/// The unconscious pawn under the crosshair, if any (`DisUIInteractionProbe`, approximated by the
/// view ray passing within 45 uu of the pawn's chest).
pub fn focused_sleeper<'a>(view: &'a crate::session::SessionView, me: &dishonored_sim::state::PlayerState, yaw: f32, pitch: f32) -> Option<&'a dishonored_sim::state::PlayerState> {
    if !me.alive {
        return None;
    }
    let eye = me.position + glam::Vec3::Z * 64.0;
    let fwd = dishonored_sim::movement::view_vector(yaw, pitch);
    view.others
        .iter()
        .filter(|o| o.alive && o.condition == dishonored_sim::state::Condition::Unconscious)
        .filter_map(|o| {
            let d = o.position + glam::Vec3::Z * 40.0 - eye;
            let t = d.dot(fwd);
            (t > 0.0 && t < SLEEPER_REACH && (d - fwd * t).length() < 45.0).then_some((t, o))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|x| x.1)
}

// --- pickup log -------------------------------------------------------------------------------

/// What a change of the inventory adds to the pickup log: (`"name"` or `"name (count)"`,
/// small icon `Package.Texture`) per item (`AddLogItem`, 0x00B98630).
pub fn pickup_items(prev: &dishonored_sim::state::Inventory, now: &dishonored_sim::state::Inventory, t: &crate::gfx_hud::HudTexts) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let fmt = |name: &str, n: i64| if n == 1 { name.to_string() } else { format!("{name} ({n})") };
    let coins = now.coins as i64 - prev.coins as i64;
    if coins > 0 {
        out.push((fmt(if coins == 1 { &t.coin.0 } else { &t.coin.1 }, coins), Some("UI_ItemIcons_Small.Money_Small".to_string())));
    }
    let runes = now.runes as i64 - prev.runes as i64;
    if runes > 0 {
        out.push((fmt(if runes == 1 { &t.rune.0 } else { &t.rune.1 }, runes), Some("UI_ItemIcons_Small.Rune_Small".to_string())));
    }
    let charms = (now.charms_owned & !prev.charms_owned).count_ones() as i64;
    if charms > 0 {
        out.push((fmt(&t.charm, charms), Some("UI_ItemIcons_Small.BoneCharms_Small".to_string())));
    }
    for (i, icon) in ["UI_ItemIcons_Small.HealthElixir_Small", "UI_ItemIcons_Small.ManaElixir_Small"].iter().enumerate() {
        let d = now.elixirs[i] as i64 - prev.elixirs[i] as i64;
        if d > 0 {
            out.push((fmt(&t.elixirs[i], d), Some(icon.to_string())));
        }
    }
    for i in 0..8 {
        let d = now.ammo[i] as i64 - prev.ammo[i] as i64;
        if d > 0 {
            out.push((fmt(&t.ammo[i], d), t.ammo_icons[i].clone()));
        }
    }
    out
}

// --- harness ----------------------------------------------------------------------------------

/// A HUD request from a script step (`{"hud": {...}}`), to show each popup and state on
/// demand (fixtures/scripts/hud_*.json).
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HudTest {
    /// Queue an objective popup: event 0 added, 1 updated, 2 completed, 3 failed.
    Objective {
        name: String,
        event: u8,
        #[serde(default)]
        tasks: Vec<(String, u8)>,
    },
    /// Queue a target notification (state 1 killed, 2 neutralized, 3 rescued, 4 spared;
    /// portrait `Package.Texture`).
    Target {
        name: String,
        state: u8,
        #[serde(default)]
        portrait: Option<String>,
    },
    /// Add a pickup log line (`icon` `Package.Texture`).
    Pickup {
        name: String,
        #[serde(default)]
        icon: Option<String>,
    },
    /// Show a game message for `duration` seconds.
    Message {
        text: String,
        #[serde(default = "five")]
        duration: f32,
    },
    /// Show / clear the subtitle.
    Subtitle {
        #[serde(default)]
        text: Option<String>,
    },
    /// Show / close a choice menu; `select` moves the highlight.
    Choice {
        #[serde(default)]
        options: Option<Vec<String>>,
        #[serde(default)]
        select: Option<usize>,
    },
    /// Show / clear the crosshair interactions window.
    Interaction {
        #[serde(default)]
        name: String,
        #[serde(default)]
        text: Option<String>,
    },
    /// Show / close the Special (or SpecialQTE) interaction icons: `EDisUIInteraction` ids
    /// (1 mantle .. 6 adrenaline kill) with the HUD's texts; empty closes.
    Icons {
        ids: Vec<u8>,
        #[serde(default)]
        qte: bool,
    },
    /// Show / clear the Context interactions text (`interactions_txt_mc`).
    ContextText {
        #[serde(default)]
        text: Option<String>,
    },
    /// Show / hide the cinematic stripes.
    Stripes { show: bool },
    /// Show the location discovery banner (`SetLocation`, 5 s).
    Location { name: String },
    /// Open / close the quick-access wheel (`hud_wheel.rs`).
    Wheel { open: bool },
    /// Point the open wheel at an angle (degrees clockwise from the top: `SetSelectionAngle`).
    WheelAngle { angle: f32 },
    /// Press (or release) a number key outside the wheel: the dock shows that slot.
    Shortcut {
        slot: usize,
        #[serde(default)]
        release: bool,
    },
    /// A step of the mouse wheel outside the quick-access wheel (`GBA_NextItem` +1,
    /// `GBA_PrevItem` -1): the dock cycles through the filled slots and uses the one it shows
    /// when the wheel has been still for `m_fShortcutsInputTimer`.
    ShortcutStep { dir: i32 },
    /// A number key while the wheel is open: the pointed-at item goes to the slot.
    Assign { slot: usize },
    /// The tutorial line (`SetTutorialMessage`), up `seconds` (10 by default).
    Tutorial {
        text: String,
        #[serde(default)]
        seconds: Option<f32>,
    },
    /// A tutorial window (`SetTutorialWindow`): picture 0 gear, 1 note, 2 rune, 3 bone charm; the
    /// HUD tweak's text for the picture when none is given.
    TutorialWindow {
        image: u8,
        #[serde(default)]
        text: Option<String>,
    },
    /// Add task markers on the spot: (metres ahead of the player, sideways, up, optional, name).
    TaskMarkers { markers: Vec<(f32, f32, f32, bool, String)> },
    /// The game over screen (the pause menu movie with `ShowGameOverMenu`), as if the player died.
    GameOver,
    /// A game save: the saving notification (`hud_global.rs`).
    Saving,
}

fn five() -> f32 {
    5.0
}

/// Apply a harness request.
pub fn apply_test(t: HudTest, q: &mut PopupQueue, w: &mut HudWidgets, st: &mut Stage, _texts: &crate::gfx_hud::HudTexts, img: &mut dyn FnMut(&str) -> Option<ImageRef>) {
    match t {
        HudTest::Objective { name, event, tasks } => {
            let obj = 1000 + q.queue.len();
            q.queue.push(PopupInfo {
                objective: Some(obj),
                name,
                event,
                tasks: tasks.into_iter().enumerate().map(|(i, (d, e))| TaskInfo { task: (obj, i), event: e, description: d }).collect(),
                target: None,
            });
        }
        HudTest::Target { name, state, portrait } => {
            let kind = match state {
                2 => TargetKind::Neutralized,
                3 => TargetKind::Rescued,
                4 => TargetKind::Spared,
                _ => TargetKind::Killed,
            };
            q.target_notification(&name, portrait, kind);
        }
        HudTest::Pickup { name, icon } => {
            let r = icon.as_deref().and_then(|p| img(p));
            if let Some(l) = w.pickup_log.as_mut() {
                l.add_log_item(st, &name, r);
            }
        }
        HudTest::Message { text, duration } => {
            if let Some(g) = w.game_msg.as_mut() {
                g.set_game_message(st, Some(&text), duration);
            }
        }
        HudTest::Subtitle { text } => {
            if let Some(s) = w.subtitles.as_mut() {
                s.set_subtitles(st, text.as_deref());
            }
        }
        HudTest::Choice { options, select } => {
            if let Some(c) = w.choice.as_mut() {
                if let Some(o) = &options {
                    c.set_menu(st, &mut w.sounds, Some(o.as_slice()), None);
                } else if select.is_none() {
                    c.set_menu(st, &mut w.sounds, None, None);
                }
                if let Some(i) = select {
                    c.select(st, i);
                }
            }
        }
        HudTest::Interaction { name, text } => {
            if let Some(iw) = w.interaction_window.as_mut() {
                iw.set_interactions(st, &name, text.as_deref());
            }
        }
        HudTest::Icons { ids, qte } => {
            let list: Vec<(u8, String)> = ids.iter().map(|&i| (i, _texts.interactions.get(i as usize).cloned().unwrap_or_default())).collect();
            let list = (!list.is_empty()).then_some(list);
            let w = if qte { w.interaction_qte.as_mut() } else { w.interaction_icons.as_mut() };
            if let Some(ic) = w {
                ic.set_interactions(st, list.as_deref());
            }
        }
        HudTest::ContextText { text } => {
            if let Some(im) = w.interactions.as_mut() {
                im.set_interactions(st, text.as_deref());
            }
        }
        HudTest::Stripes { show } => {
            if let Some(s) = w.stripes.as_mut() {
                s.set(st, show, true);
            }
        }
        // Handled by the wheel (`hud_wheel::update_power_wheel`).
        HudTest::Wheel { .. } | HudTest::WheelAngle { .. } | HudTest::Shortcut { .. } | HudTest::ShortcutStep { .. } | HudTest::Assign { .. } | HudTest::Saving => {}
        // Handled by the HUD itself (`gfx_hud::update_gfx_hud`).
        HudTest::Tutorial { .. } | HudTest::TutorialWindow { .. } | HudTest::TaskMarkers { .. } | HudTest::GameOver => {}
        HudTest::Location { name } => {
            if let Some(l) = w.location.as_mut() {
                l.set_location(st, &name, crate::gfx_hud::LOCATION_DISCOVERY_DURATION);
            }
        }
    }
}
