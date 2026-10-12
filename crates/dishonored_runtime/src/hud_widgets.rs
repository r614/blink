//! The `UI_HUD_SF` "HUD" movie's ActionScript 2 classes, rewritten against [`gfx_stage`]: each
//! type below is one class (decoded with `swf_dump UI_HUD_SF HUD - --actions <class sprite>`,
//! see research/dishonored/HUD.md) and keeps its method names, constants, tweens (duration,
//! properties, easing) and intervals. The native side that calls them (`DisGFxMoviePlayerHUD`,
//! 0x00BB2280 and its helpers) is in `gfx_hud.rs` / `hud_popups.rs`.
//!
//! Timer and tween-completion callbacks come back through [`HudWidgets::on_event`] with a key
//! made of the widget, the callback and an argument ([`key`]).

use crate::gfx_stage::{AutoSize, Ease, Id, ImageRef, Prop, Stage};
use dishonored_assets::swf::ColorTransform;

// --- callback keys ---------------------------------------------------------------------------

pub const fn key(widget: u16, code: u16, arg: u32) -> u64 {
    ((widget as u64) << 48) | ((code as u64) << 32) | arg as u64
}
fn split(k: u64) -> (u16, u16, u32) {
    ((k >> 48) as u16, (k >> 32) as u16, k as u32)
}

const W_OBJ: u16 = 1;
const W_TGT: u16 = 2;
const W_PLOG: u16 = 3;
const W_GAUGE: u16 = 4;
const W_CHINFO: u16 = 5;
const W_MSG: u16 = 6; // + instance index
const W_SUB: u16 = 10;
const W_CHOICE: u16 = 11;
const W_ICON: u16 = 12;
const W_TUTO: u16 = 13;
const W_STRIPES: u16 = 14;
const W_LOC: u16 = 15;

/// `_utils.SaveMovieClipProperties.SaveProperties`: the clip's authored transform, tweened
/// back to by the open animations (`tweenTo(d, mc._props, ...)`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Props {
    pub alpha: f32,
    pub x: f32,
    pub y: f32,
    pub xscale: f32,
    pub yscale: f32,
    pub rotation: f32,
}

impl Props {
    pub fn save(st: &Stage, id: Id) -> Self {
        Self { alpha: st.get(id, Prop::Alpha), x: st.get(id, Prop::X), y: st.get(id, Prop::Y), xscale: st.get(id, Prop::XScale), yscale: st.get(id, Prop::YScale), rotation: st.get(id, Prop::Rotation) }
    }
    pub fn list(&self) -> [(Prop, f32); 6] {
        [(Prop::Alpha, self.alpha), (Prop::X, self.x), (Prop::Y, self.y), (Prop::XScale, self.xscale), (Prop::YScale, self.yscale), (Prop::Rotation, self.rotation)]
    }
}

fn tween_props(st: &mut Stage, id: Option<Id>, d: f32, p: &Props, e: Ease) {
    if let Some(id) = id {
        st.tween_to(id, d, &p.list(), e, None);
    }
}
fn set(st: &mut Stage, id: Option<Id>, p: Prop, v: f32) {
    if let Some(id) = id {
        st.set(id, p, v);
    }
}
fn get(st: &Stage, id: Option<Id>, p: Prop) -> f32 {
    id.map(|i| st.get(i, p)).unwrap_or(0.0)
}
fn tween(st: &mut Stage, id: Option<Id>, d: f32, props: &[(Prop, f32)], e: Ease, k: Option<u64>) {
    if let Some(id) = id {
        st.tween_to(id, d, props, e, k);
    }
}
fn tween_end(st: &mut Stage, id: Option<Id>, jump: bool) {
    if let Some(id) = id {
        st.tween_end(id, jump);
    }
}
fn goto_play(st: &mut Stage, id: Option<Id>, l: &str) {
    if let Some(id) = id {
        st.goto_and_play(id, l);
    }
}
fn goto_stop(st: &mut Stage, id: Option<Id>, l: &str) {
    if let Some(id) = id {
        st.goto_and_stop(id, l);
    }
}
fn visible(st: &mut Stage, id: Option<Id>, v: bool) {
    if let Some(id) = id {
        st.set_visible(id, v);
    }
}
fn clear(st: &mut Stage, iv: &mut Option<u32>) {
    if let Some(i) = iv.take() {
        st.clear_interval(i);
    }
}
fn interval(st: &mut Stage, iv: &mut Option<u32>, ms: f32, k: u64) {
    clear(st, iv);
    *iv = Some(st.set_interval(ms, k));
}

/// Sounds the classes ask `SoundHandler.PlaySound` for (theme names, see `hud_sounds.rs`).
#[derive(Default, Debug)]
pub struct SoundQueue(pub Vec<&'static str>);

// =============================================================================================
// ObjectivesNotification (hud_objW, `objectives_mc`)
// =============================================================================================

/// `ObjectivesState`: obj_Added 0, obj_Updated 1, obj_Completed 2, obj_Failed 3, obj_Error 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjState {
    Added = 0,
    Updated = 1,
    Completed = 2,
    Failed = 3,
}

impl ObjState {
    pub fn from_index(i: u8) -> Self {
        match i {
            1 => Self::Updated,
            2 => Self::Completed,
            3 => Self::Failed,
            _ => Self::Added,
        }
    }
    /// `GetIconStateLabel`.
    fn icon_label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Updated | Self::Added => "updated",
        }
    }
    /// `GetSoundID`.
    fn sound(self) -> &'static str {
        match self {
            Self::Added => "obj_taskAdded",
            Self::Updated => "obj_taskUpdated",
            Self::Completed => "obj_taskCompleted",
            Self::Failed => "obj_taskFailed",
        }
    }
    /// `GetStateColor` (glow colour offset of a task line).
    fn color(self) -> [f32; 3] {
        match self {
            Self::Added | Self::Updated => [227.0, 242.0, 214.0],
            Self::Completed => [74.0, 84.0, 15.0],
            Self::Failed => [130.0, 20.0, 21.0],
        }
    }
}

#[derive(Clone, Debug)]
pub struct TaskLine {
    pub state: ObjState,
    pub description: String,
}

pub struct ObjectivesNotification {
    pub mc: Id,
    window: Option<Id>,
    bkgd: Option<Id>,
    title: Option<Id>,
    name: Option<Id>,
    name_bkgd: Option<Id>,
    name_letters: Option<Id>,
    obj_state: Option<Id>,
    loz_ext: Option<Id>,
    loz_int: Option<Id>,
    line_u: Option<Id>,
    line_d: Option<Id>,
    p_bkgd: Props,
    p_title: Props,
    p_name: Props,
    p_name_letters: Props,
    p_obj_state: Props,
    p_loz_ext: Props,
    p_loz_int: Props,
    p_line_u: Props,
    p_line_d: Props,
    name_bkgd_init_w: f32,
    tasks: Vec<TaskLine>,
    state: ObjState,
    tasks_mc: Option<Id>,
    list_mc: Option<Id>,
    task_mcs: Vec<Id>,
    pub opened: bool,
    itvl_name: Option<u32>,
    itvl_open: Option<u32>,
    itvl_close: Option<u32>,
    itvl_ic: Option<u32>,
    itvl_loz: Option<u32>,
    itvl_task: Option<u32>,
    /// `_dAnimT` (`_utils.DelayedAnimation` over the task lines).
    d_anim: Option<u32>,
}

// Callback codes.
const OBJ_NAME_LETTERS: u16 = 1;
const OBJ_SHOW_TASKS: u16 = 2;
const OBJ_CLOSE: u16 = 3;
const OBJ_ON_CLOSED: u16 = 4;
const OBJ_STATE_ICON: u16 = 5;
const OBJ_LOZENGE: u16 = 6;
const OBJ_TASK_ANIM: u16 = 7;
const OBJ_GLOW: u16 = 8;

/// `_tasksApparitionDelay`, `_taskLifeDuration_const` (ms).
pub const TASKS_APPARITION_DELAY: f32 = 700.0;
pub const TASK_LIFE_DURATION: f32 = 3500.0;

impl ObjectivesNotification {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        let window = st.child(mc, "_window_mc");
        let f = |st: &Stage, n: &str| window.and_then(|w| st.find(w, n));
        let bkgd = f(st, "bkgd_mc");
        let title = f(st, "title_mc");
        let name = f(st, "name_mc");
        let name_bkgd = name.and_then(|n| st.child(n, "bkgd_mc"));
        let name_letters = name.and_then(|n| st.child(n, "txt_mc"));
        let obj_state = f(st, "objState_mc");
        let loz_ext = f(st, "lozengeExt_mc");
        let loz_int = f(st, "lozengeInt_mc");
        let line_u = f(st, "lineU_mc");
        let line_d = f(st, "lineD_mc");
        let p = |st: &Stage, id: Option<Id>| id.map(|i| Props::save(st, i)).unwrap_or_default();
        Self {
            mc,
            window,
            p_bkgd: p(st, bkgd),
            p_title: p(st, title),
            p_name: p(st, name),
            p_name_letters: p(st, name_letters),
            p_obj_state: p(st, obj_state),
            p_loz_ext: p(st, loz_ext),
            p_loz_int: p(st, loz_int),
            p_line_u: p(st, line_u),
            p_line_d: p(st, line_d),
            name_bkgd_init_w: name_bkgd.map(|b| st.width(b)).unwrap_or(0.0),
            bkgd,
            title,
            name,
            name_bkgd,
            name_letters,
            obj_state,
            loz_ext,
            loz_int,
            line_u,
            line_d,
            tasks: Vec::new(),
            state: ObjState::Added,
            tasks_mc: None,
            list_mc: None,
            task_mcs: Vec::new(),
            opened: false,
            itvl_name: None,
            itvl_open: None,
            itvl_close: None,
            itvl_ic: None,
            itvl_loz: None,
            itvl_task: None,
            d_anim: None,
        }
    }

    /// `SetObjectives(objectiveName, objectiveState, objectiveStateIdx, tasksList)`.
    pub fn set_objectives(&mut self, st: &mut Stage, snd: &mut SoundQueue, name: &str, title: &str, state: ObjState, tasks: Vec<TaskLine>) {
        self.tasks = tasks;
        self.state = state;
        self.set_name(st, name);
        self.set_title(st, title);
        clear(st, &mut self.d_anim);
        if let Some(t) = self.tasks_mc.take() {
            st.remove(t);
            self.list_mc = None;
            self.task_mcs.clear();
        }
        if !self.tasks.is_empty() {
            self.set_tasks(st);
        }
        self.open(st, snd);
    }

    fn set_title(&mut self, st: &mut Stage, title: &str) {
        let Some(t) = self.title else { return };
        let up = title.to_uppercase();
        for n in ["txt_mc", "txtShad0_mc", "txtShad1_mc", "txtShad2_mc"] {
            if let Some(tx) = st.find(t, &format!("{n}/txt")) {
                st.set_text(tx, &up);
                st.set_auto_size(tx, AutoSize::Right);
            }
        }
        // The backdrop strip grows to fit the title (it never shrinks back).
        let Some(bk) = st.child(t, "bkgd_mc") else { return };
        let bw = st.width(bk);
        let Some(tm) = st.child(t, "txt_mc") else { return };
        let need = st.get(tm, Prop::X).abs() + st.width(tm);
        if need > bw {
            st.set_width(bk, need + 60.0);
        }
    }

    fn set_name(&mut self, st: &mut Stage, name: &str) {
        let Some(nl) = self.name_letters else { return };
        if let Some(tx) = st.child(nl, "txt") {
            st.set_auto_size(tx, AutoSize::Right);
            st.set_text(tx, &name.to_uppercase());
        }
        let r2 = st.get(nl, Prop::X).abs();
        if r2 > self.name_bkgd_init_w {
            if let Some(b) = self.name_bkgd {
                st.set_width(b, r2 + 60.0);
            }
        }
    }

    fn set_tasks(&mut self, st: &mut Stage) {
        let Some(w) = self.window else { return };
        let d = st.next_depth(w);
        let Some(tasks_mc) = st.attach(w, "hud_objW_tasksList", "tasks_mc", d) else { return };
        st.set(tasks_mc, Prop::X, -110.0);
        st.set(tasks_mc, Prop::Y, 120.0);
        let d = st.next_depth(tasks_mc);
        let list = st.create_empty(tasks_mc, "list_mc", d);
        self.tasks_mc = Some(tasks_mc);
        self.list_mc = Some(list);
        self.task_mcs.clear();
        let mut prev: Option<Id> = None;
        for i in 0..self.tasks.len() {
            let d = st.next_depth(list);
            let Some(mc) = st.attach(list, "hud_objW_tasksList_item", &format!("t{i}"), d) else { continue };
            // The text first, so `_height` measures the wrapped lines.
            self.task_mcs.push(mc);
            self.set_task_mc(st, i);
            let y = match prev {
                Some(p) => st.get(p, Prop::Y) + st.height(p) + 15.0,
                None => 0.0,
            };
            st.set(mc, Prop::Y, y);
            st.set(mc, Prop::Alpha, 0.0);
            prev = Some(mc);
        }
        let mask = st.child(tasks_mc, "mask_mc");
        st.set_mask(list, mask);
    }

    fn set_task_mc(&mut self, st: &mut Stage, i: usize) {
        let Some(mc) = self.task_mcs.get(i).copied() else { return };
        let Some(t) = self.tasks.get(i).cloned() else { return };
        for n in ["txt_mc/txt", "txtGlow_mc/txt", "txtShadow_mc/txt"] {
            if let Some(tx) = st.find(mc, n) {
                st.set_text(tx, &t.description);
                st.set_auto_size(tx, AutoSize::Right);
                st.set_word_wrap(tx, true);
            }
        }
        // `SetColorTransform(txtGlow_mc).SetOffsetColor_Custom(1, r, g, b, 0)`.
        if let Some(g) = st.child(mc, "txtGlow_mc") {
            let c = t.state.color();
            st.set_color(g, ColorTransform { mul: [0.0, 0.0, 0.0, 1.0], add: [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, 0.0] });
        }
    }

    /// `Open()`.
    fn open(&mut self, st: &mut Stage, snd: &mut SoundQueue) {
        self.clear_intervals(st);
        self.opened = true;
        st.set_visible(self.mc, true);
        self.open_bkgd(st);
        self.open_name(st);
        self.open_title(st);
        self.open_lozenges(st);
        self.open_state_icon(st, snd);
        if !self.tasks.is_empty() {
            interval(st, &mut self.itvl_open, TASKS_APPARITION_DELAY, key(W_OBJ, OBJ_SHOW_TASKS, 0));
        } else {
            interval(st, &mut self.itvl_open, TASK_LIFE_DURATION, key(W_OBJ, OBJ_CLOSE, 0));
        }
    }

    fn open_name(&mut self, st: &mut Stage) {
        let n = self.name;
        set(st, n, Prop::Alpha, 0.0);
        tween_end(st, n, false);
        let x = get(st, n, Prop::X);
        set(st, n, Prop::X, x + 850.0);
        set(st, n, Prop::Alpha, 0.0);
        let p = self.p_name;
        tween_props(st, n, 0.25, &p, Ease::StrongOut);
        set(st, self.name_letters, Prop::Alpha, 0.0);
        interval(st, &mut self.itvl_name, 150.0, key(W_OBJ, OBJ_NAME_LETTERS, 0));
    }

    fn open_name_letters(&mut self, st: &mut Stage) {
        clear(st, &mut self.itvl_name);
        let p = self.p_name_letters;
        set(st, self.name_letters, Prop::X, p.x + 100.0);
        tween_props(st, self.name_letters, 0.25, &p, Ease::StrongOut);
    }

    fn open_title(&mut self, st: &mut Stage) {
        let t = self.title;
        tween_end(st, t, false);
        set(st, t, Prop::X, self.p_title.x - 150.0);
        set(st, t, Prop::Alpha, 0.0);
        let p = self.p_title;
        tween_props(st, t, 0.3, &p, Ease::StrongInOut);
        goto_play(st, t, "anim");
        set(st, self.line_u, Prop::X, self.p_line_u.x + 650.0);
        set(st, self.line_u, Prop::Alpha, 0.0);
        let p = self.p_line_u;
        tween_props(st, self.line_u, 0.3, &p, Ease::BackInOut);
        set(st, self.line_d, Prop::X, self.p_line_d.x - 250.0);
        set(st, self.line_d, Prop::Alpha, 0.0);
        let p = self.p_line_d;
        tween_props(st, self.line_d, 0.3, &p, Ease::BackOut);
    }

    fn open_bkgd(&mut self, st: &mut Stage) {
        let b = self.bkgd;
        tween_end(st, b, false);
        set(st, b, Prop::XScale, 150.0);
        set(st, b, Prop::YScale, 150.0);
        set(st, b, Prop::Alpha, 0.0);
        set(st, b, Prop::X, self.p_bkgd.x + 250.0);
        let p = self.p_bkgd;
        tween_props(st, b, 0.2, &p, Ease::StrongOut);
    }

    fn open_lozenges(&mut self, st: &mut Stage) {
        set(st, self.loz_ext, Prop::Alpha, 0.0);
        goto_stop(st, self.loz_ext, "1");
        set(st, self.loz_int, Prop::Alpha, 0.0);
        goto_stop(st, self.loz_int, "1");
        interval(st, &mut self.itvl_loz, 200.0, key(W_OBJ, OBJ_LOZENGE, 0));
    }

    fn play_lozenge_icon_anim(&mut self, st: &mut Stage) {
        clear(st, &mut self.itvl_loz);
        set(st, self.loz_ext, Prop::XScale, 150.0);
        set(st, self.loz_ext, Prop::YScale, 150.0);
        set(st, self.loz_ext, Prop::Rotation, 50.0);
        let p = self.p_loz_ext;
        tween_props(st, self.loz_ext, 0.25, &p, Ease::StrongOut);
        goto_play(st, self.loz_ext, "anim");
        set(st, self.loz_int, Prop::Alpha, 100.0);
        set(st, self.loz_int, Prop::XScale, 300.0);
        set(st, self.loz_int, Prop::YScale, 300.0);
        set(st, self.loz_int, Prop::Rotation, 50.0);
        let p = self.p_loz_int;
        tween_props(st, self.loz_int, 0.25, &p, Ease::StrongOut);
        goto_play(st, self.loz_int, self.state.icon_label());
    }

    fn open_state_icon(&mut self, st: &mut Stage, snd: &mut SoundQueue) {
        let o = self.obj_state;
        goto_stop(st, o, "1");
        tween_end(st, o, false);
        set(st, o, Prop::XScale, 250.0);
        set(st, o, Prop::YScale, 250.0);
        set(st, o, Prop::Alpha, 0.0);
        let p = self.p_obj_state;
        tween_props(st, o, 0.25, &p, Ease::StrongOut);
        interval(st, &mut self.itvl_ic, 100.0, key(W_OBJ, OBJ_STATE_ICON, 0));
        if self.tasks.is_empty() {
            snd.0.push(self.state.sound());
        }
    }

    fn show_tasks(&mut self, st: &mut Stage, snd: &mut SoundQueue) {
        clear(st, &mut self.itvl_open);
        // DelayedAnimation(this, 0, n - 1, 3500, PlayTaskAnim, OnTasksAnimEnd): task 0 now,
        // then one every 3.5 s, then the end callback 3.5 s after the last.
        self.anim_inc(st, snd, 0);
    }

    /// `_utils.DelayedAnimation.Anim_Inc(idx)`.
    fn anim_inc(&mut self, st: &mut Stage, snd: &mut SoundQueue, idx: usize) {
        clear(st, &mut self.d_anim);
        if idx + 1 > self.tasks.len() {
            // OnTasksAnimEnd.
            self.close(st);
            return;
        }
        self.play_task_anim(st, snd, idx);
        interval(st, &mut self.d_anim, TASK_LIFE_DURATION, key(W_OBJ, OBJ_TASK_ANIM, idx as u32 + 1));
    }

    fn play_task_anim(&mut self, st: &mut Stage, snd: &mut SoundQueue, i: usize) {
        let Some(mc) = self.task_mcs.get(i).copied() else { return };
        let d = 0.2;
        st.set(mc, Prop::Alpha, 0.0);
        st.tween_to(mc, d, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
        if let Some(g) = st.child(mc, "txtGlow_mc") {
            st.set(g, Prop::Alpha, 0.0);
        }
        interval(st, &mut self.itvl_task, d * 1000.0, key(W_OBJ, OBJ_GLOW, (i as u32) << 8));
        let list = self.list_mc;
        if i > 0 {
            if let Some(p) = self.task_mcs.get(i - 1).copied() {
                st.tween_to(p, 0.4, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
            }
        } else {
            let y = st.get(mc, Prop::Y) + st.height(mc) + 20.0;
            set(st, list, Prop::Y, y);
        }
        let target = -st.get(mc, Prop::Y);
        tween_end(st, list, true);
        tween(st, list, 0.25, &[(Prop::Y, target)], Ease::StrongOut, None);
        if let Some(t) = self.tasks.get(i) {
            snd.0.push(t.state.sound());
        }
    }

    /// `PlayGlowAnim(mc, count)`.
    fn play_glow_anim(&mut self, st: &mut Stage, i: usize, count: u32) {
        clear(st, &mut self.itvl_task);
        let Some(mc) = self.task_mcs.get(i).copied() else { return };
        let d = 0.25;
        if let Some(g) = st.child(mc, "txtGlow_mc") {
            st.set(g, Prop::Alpha, 100.0);
            st.tween_to(g, d, &[(Prop::Alpha, 0.0)], Ease::StrongInOut, None);
        }
        if count == 0 {
            if let (Some(ic), Some(t)) = (st.child(mc, "ic_mc"), self.tasks.get(i)) {
                st.goto_and_play(ic, t.state.icon_label());
            }
        }
        if count < 1 {
            interval(st, &mut self.itvl_task, d * 1000.0, key(W_OBJ, OBJ_GLOW, ((i as u32) << 8) | (count + 1)));
        }
    }

    /// `Close()`.
    pub fn close(&mut self, st: &mut Stage) {
        self.clear_intervals(st);
        interval(st, &mut self.itvl_close, 500.0, key(W_OBJ, OBJ_ON_CLOSED, 0));
        let bx = self.p_bkgd.x + 100.0;
        tween(st, self.bkgd, 0.2, &[(Prop::Alpha, 0.0), (Prop::X, bx)], Ease::StrongOut, None);
        let nx = self.p_name.x - 50.0;
        tween(st, self.name, 0.25, &[(Prop::Alpha, 0.0), (Prop::X, nx)], Ease::StrongOut, None);
        let tx = get(st, self.title, Prop::X) + 50.0;
        tween(st, self.title, 0.25, &[(Prop::Alpha, 0.0), (Prop::X, tx)], Ease::StrongOut, None);
        tween(st, self.obj_state, 0.25, &[(Prop::Alpha, 0.0), (Prop::XScale, 120.0), (Prop::YScale, 120.0)], Ease::StrongOut, None);
        tween(st, self.loz_ext, 0.25, &[(Prop::Alpha, 0.0), (Prop::XScale, 140.0), (Prop::YScale, 140.0)], Ease::StrongOut, None);
        if let Some(l) = self.loz_ext {
            st.stop(l);
        }
        tween(st, self.loz_int, 0.25, &[(Prop::Alpha, 0.0), (Prop::XScale, 120.0), (Prop::YScale, 120.0)], Ease::StrongOut, None);
        if let Some(l) = self.loz_int {
            st.stop(l);
        }
        let ux = self.p_line_u.x - 350.0;
        tween(st, self.line_u, 0.25, &[(Prop::Alpha, 0.0), (Prop::X, ux)], Ease::StrongOut, None);
        let dx = self.p_line_d.x + 250.0;
        tween(st, self.line_d, 0.3, &[(Prop::Alpha, 0.0), (Prop::X, dx)], Ease::StrongOut, None);
        if let Some(t) = self.tasks_mc {
            let y = st.get(t, Prop::Y) - 50.0;
            st.tween_to(t, 0.25, &[(Prop::Alpha, 0.0), (Prop::Y, y)], Ease::StrongOut, None);
        }
    }

    fn on_closed(&mut self, st: &mut Stage) {
        self.clear_intervals(st);
        st.set_visible(self.mc, false);
        self.opened = false;
        // `_nameLetters_mc.removeMovieClip()` is a no-op on a timeline clip.
        if let Some(t) = self.tasks_mc.take() {
            st.remove(t);
            self.list_mc = None;
            self.task_mcs.clear();
        }
    }

    fn clear_intervals(&mut self, st: &mut Stage) {
        clear(st, &mut self.itvl_name);
        clear(st, &mut self.itvl_open);
        clear(st, &mut self.itvl_close);
        clear(st, &mut self.itvl_ic);
        clear(st, &mut self.itvl_loz);
        clear(st, &mut self.itvl_task);
    }

    fn on_event(&mut self, st: &mut Stage, snd: &mut SoundQueue, code: u16, arg: u32) {
        match code {
            OBJ_NAME_LETTERS => self.open_name_letters(st),
            OBJ_SHOW_TASKS => self.show_tasks(st, snd),
            OBJ_CLOSE => self.close(st),
            OBJ_ON_CLOSED => self.on_closed(st),
            OBJ_STATE_ICON => {
                clear(st, &mut self.itvl_ic);
                goto_play(st, self.obj_state, self.state.icon_label());
            }
            OBJ_LOZENGE => self.play_lozenge_icon_anim(st),
            OBJ_TASK_ANIM => {
                self.d_anim = None;
                self.anim_inc(st, snd, arg as usize);
            }
            OBJ_GLOW => {
                self.itvl_task = None;
                self.play_glow_anim(st, (arg >> 8) as usize, arg & 0xff);
            }
            _ => {}
        }
    }
}

// =============================================================================================
// TargetNotification (hud_tgt, `targetNotification_mc`)
// =============================================================================================

/// `TargetNotificationEnum`: NType_Killed 1, NType_Neutralized 2, NType_Rescued 3, NType_Spared 4
/// (the native passes `EDisChapterTargetState + 1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Killed = 1,
    Neutralized = 2,
    Rescued = 3,
    Spared = 4,
}

pub struct TargetNotification {
    pub mc: Id,
    portrait: Option<Id>,
    target_name: Option<Id>,
    notification: Option<Id>,
    letters: Option<Id>,
    bkgd_cont: Option<Id>,
    nb_char: usize,
    duration: f32,
    sound: &'static str,
    pub opened: bool,
    interval_id: Option<u32>,
    interval_open: Option<u32>,
}

const TGT_ON_OPENED: u16 = 1;
const TGT_CLOSE: u16 = 2;
const TGT_ON_CLOSED: u16 = 3;
const TGT_LETTER: u16 = 4;

/// Localized `texts` of the HUD (`t_targetAssassinated` ...), set by the native `initTexts`.
pub struct TargetTexts {
    pub assassinated: String,
    pub neutralized: String,
    pub rescued: String,
    pub spared: String,
}

impl TargetNotification {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        Self {
            mc,
            portrait: st.child(mc, "_portrait_mc"),
            target_name: st.child(mc, "_targetName_mc"),
            notification: st.child(mc, "_notification_mc"),
            letters: None,
            bkgd_cont: None,
            nb_char: 0,
            duration: 5.0,
            sound: "",
            opened: false,
            interval_id: None,
            interval_open: None,
        }
    }

    /// `SetNotification(targetName, portrait, notificationType, duration)`.
    pub fn set_notification(&mut self, st: &mut Stage, snd: &mut SoundQueue, texts: &TargetTexts, target_name: &str, portrait: Option<ImageRef>, kind: TargetKind, duration: f32) {
        clear(st, &mut self.interval_id);
        st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
        self.opened = true;
        self.duration = duration;
        self.destroy_dynamic_elements(st);
        if let Some(img) = self.portrait.and_then(|p| st.child(p, "img_mc")) {
            st.img_loader(img, portrait, false, true);
        }
        let frame = (kind as u8).to_string();
        for n in ["animPortrait_mc", "stroke0_mc", "stroke1_mc"] {
            if let Some(c) = self.portrait.and_then(|p| st.child(p, n)) {
                st.goto_and_stop(c, &frame);
            }
        }
        if let Some(t) = self.target_name.and_then(|t| st.find(t, "txt_mc/txt")) {
            st.set_text(t, "");
        }
        let (sound, text, color) = match kind {
            TargetKind::Killed => ("H_targetAssassinated", &texts.assassinated, 8524821u32),
            TargetKind::Neutralized => ("H_targetNeutralized", &texts.neutralized, 6575694),
            TargetKind::Rescued => ("H_targetRescued", &texts.rescued, 4936975),
            TargetKind::Spared => ("H_targetNeutralized", &texts.spared, 6575694),
        };
        self.sound = sound;
        let up = text.to_uppercase();
        for i in 0..3 {
            if let Some(t) = self.notification.and_then(|n| st.find(n, &format!("txtShadow{i}_mc/txt"))) {
                st.set_text(t, &up);
                st.set_text_color(t, color);
            }
        }
        if let Some(t) = self.notification.and_then(|n| st.find(n, "txt_mc/txt")) {
            st.set_text(t, &up);
        }
        self.create_target_name(st, target_name);
        self.open(st, snd);
    }

    fn create_target_name(&mut self, st: &mut Stage, name: &str) {
        let Some(tn) = self.target_name else { return };
        let Some(txt_mc) = st.child(tn, "txt_mc") else { return };
        if let Some(l) = self.letters.take() {
            st.remove(l);
        }
        let d = st.next_depth(txt_mc);
        let letters = st.create_empty(txt_mc, "_targetNameLetters_mc", d);
        self.letters = Some(letters);
        let chars: Vec<char> = name.chars().collect();
        self.nb_char = chars.len();
        let mut prev: Option<Id> = None;
        for (i, ch) in chars.iter().enumerate() {
            let d = st.next_depth(letters);
            let Some(l) = st.attach(letters, "hud_tgt_nameLetter", &format!("l{i}"), d) else { continue };
            if let Some(t) = st.child(l, "txt") {
                st.set_auto_size(t, AutoSize::Left);
                st.set_text(t, &ch.to_uppercase().collect::<String>());
            }
            st.set(l, Prop::Y, 0.0);
            st.set(l, Prop::Alpha, 0.0);
            if let Some(p) = prev {
                let pw = st.child(p, "txt").map(|t| st.text_width(t)).unwrap_or(0.0);
                let x = st.get(p, Prop::X) + pw + 1.0;
                st.set(l, Prop::X, x);
            }
            prev = Some(l);
        }
        let w = st.width(letters);
        st.set(letters, Prop::X, -w);
        // The name cartouche: left and right ends with 50 px middle parts between.
        let Some(bk) = st.child(tn, "bkgd_mc") else { return };
        if let Some(b) = self.bkgd_cont.take() {
            st.remove(b);
        }
        let d = st.next_depth(bk);
        let cont = st.create_empty(bk, "bkgd", d);
        self.bkgd_cont = Some(cont);
        let d = st.next_depth(cont);
        let left = st.attach(cont, "hud_tgt_nameCart_pL", "left_mc", d);
        let d = st.next_depth(cont);
        let right = st.attach(cont, "hud_tgt_nameCart_pR", "right_mc", d);
        let step = 50.0;
        let (lw, rw) = (left.map(|l| st.width(l)).unwrap_or(0.0), right.map(|r| st.width(r)).unwrap_or(0.0));
        let n = (((w - rw - lw + 150.0) / step).floor()).max(1.0) as usize;
        let mut last_x = 0.0;
        for i in 0..n {
            let d = st.next_depth(cont);
            if let Some(m) = st.attach(cont, "hud_tgt_nameCart_pM", &format!("part{i}"), d) {
                let x = left.map(|l| st.get(l, Prop::X) + st.width(l)).unwrap_or(0.0) + i as f32 * step;
                st.set(m, Prop::X, x);
                last_x = x;
            }
        }
        if let Some(r) = right {
            st.set(r, Prop::X, last_x + step);
        }
        let cw = st.width(cont);
        st.set(cont, Prop::X, -cw);
        let mask = st.child(bk, "mask_mc");
        st.set_mask(cont, mask);
    }

    fn open(&mut self, st: &mut Stage, snd: &mut SoundQueue) {
        st.set_visible(self.mc, true);
        if !self.sound.is_empty() {
            snd.0.push(self.sound);
        }
        interval(st, &mut self.interval_id, 1000.0, key(W_TGT, TGT_ON_OPENED, 0));
        if let Some(a) = self.portrait.and_then(|p| st.find(p, "animPortrait_mc/anim_mc")) {
            st.goto_and_play(a, "open");
        }
        goto_play(st, self.portrait, "open");
        goto_play(st, self.target_name, "open");
        if let Some(m) = self.target_name.and_then(|t| st.find(t, "bkgd_mc/mask_mc")) {
            st.goto_and_play(m, "open");
        }
        goto_play(st, self.notification, "open");
        st.goto_and_play(self.mc, "open");
        if self.nb_char > 0 {
            self.open_letter_name(st, self.nb_char - 1);
        }
    }

    /// `OpenLetterName(idx)`: from the last letter to the first, one every 25 ms.
    fn open_letter_name(&mut self, st: &mut Stage, idx: usize) {
        let Some(l) = self.letters.and_then(|ls| st.child(ls, &format!("l{idx}"))) else { return };
        st.set(l, Prop::Y, -20.0);
        st.set(l, Prop::Rotation, -25.0);
        st.set(l, Prop::XScale, 320.0);
        st.set(l, Prop::YScale, 320.0);
        st.set(l, Prop::Alpha, 100.0);
        st.tween_to(l, 0.45, &[(Prop::Alpha, 100.0), (Prop::Y, 0.0), (Prop::XScale, 100.0), (Prop::YScale, 100.0), (Prop::Rotation, 0.0)], Ease::BackInOut, None);
        // `"open" + (floor(random * 3) + 1)`: a deterministic pick per letter here.
        let variant = 1 + (idx * 7 + self.nb_char) % 3;
        if let Some(m) = st.child(l, "mask_mc") {
            st.goto_and_play(m, &format!("open{variant}"));
        }
        clear(st, &mut self.interval_open);
        if idx > 0 {
            interval(st, &mut self.interval_open, 25.0, key(W_TGT, TGT_LETTER, idx as u32 - 1));
        }
    }

    fn close(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_id);
        st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
        st.goto_and_play(self.mc, "close");
        interval(st, &mut self.interval_id, 1000.0, key(W_TGT, TGT_ON_CLOSED, 0));
    }

    fn destroy_dynamic_elements(&mut self, st: &mut Stage) {
        if let Some(img) = self.portrait.and_then(|p| st.child(p, "img_mc")) {
            st.img_loader(img, None, false, true);
        }
        if let Some(b) = self.bkgd_cont.take() {
            st.remove(b);
        }
        if let Some(l) = self.letters.take() {
            st.remove(l);
        }
    }

    fn on_event(&mut self, st: &mut Stage, code: u16, arg: u32) {
        match code {
            TGT_ON_OPENED => {
                clear(st, &mut self.interval_id);
                interval(st, &mut self.interval_id, self.duration * 1000.0, key(W_TGT, TGT_CLOSE, 0));
            }
            TGT_CLOSE => self.close(st),
            TGT_ON_CLOSED => {
                clear(st, &mut self.interval_id);
                self.destroy_dynamic_elements(st);
                st.set_visible(self.mc, false);
                self.opened = false;
            }
            TGT_LETTER => {
                self.interval_open = None;
                self.open_letter_name(st, arg as usize);
            }
            _ => {}
        }
    }
}

// =============================================================================================
// PickupLog (hud_pLog_, `pickupLog_mc`)
// =============================================================================================

pub struct PickupLog {
    pub mc: Id,
    slots: Vec<Id>,
    log_items: Vec<Id>,
    pooled: Vec<Id>,
    queued: Vec<(String, Option<ImageRef>)>,
    tweening: bool,
    interval_move: Option<u32>,
    intervals: Vec<Option<u32>>,
}

/// `_fadeInDuration`, `_fadeOutDuration` (s), `_itemLifeDuration` (s), `_nbSlots`, and the slot
/// layout `_itemProps` (y, alpha) for positions 0..3.
pub const PLOG_FADE_IN: f32 = 0.4;
pub const PLOG_FADE_OUT: f32 = 0.4;
pub const PLOG_LIFE: f32 = 2.0;
pub const PLOG_SLOTS: usize = 3;
pub const PLOG_ITEM_PROPS: [(f32, f32); 4] = [(0.0, 100.0), (-50.0, 60.0), (-100.0, 20.0), (-150.0, 0.0)];

const PLOG_MOVED: u16 = 1;
const PLOG_HIDE: u16 = 2;
const PLOG_HIDDEN: u16 = 3;

impl PickupLog {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        let mut s = Self { mc, slots: Vec::new(), log_items: Vec::new(), pooled: Vec::new(), queued: Vec::new(), tweening: false, interval_move: None, intervals: vec![None; PLOG_SLOTS] };
        for i in 0..PLOG_SLOTS {
            if let Some(m) = st.child(mc, &format!("_slot{i}_mc")) {
                s.slots.push(m);
                s.move_to_pool(st, m);
            }
        }
        s
    }

    fn slot_index(&self, mc: Id) -> usize {
        self.slots.iter().position(|s| *s == mc).unwrap_or(0)
    }

    /// `AddLogItem(itemName, itemIcon)`.
    pub fn add_log_item(&mut self, st: &mut Stage, name: &str, icon: Option<ImageRef>) {
        self.queued.push((name.to_string(), icon));
        if !self.tweening {
            if !self.log_items.is_empty() {
                self.tween_items(st);
            } else {
                self.on_item_moved(st);
            }
        }
    }

    fn tween_items(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_move);
        self.tweening = true;
        for (i, mc) in self.log_items.clone().into_iter().enumerate() {
            let (y, a) = PLOG_ITEM_PROPS[(i + 1).min(PLOG_ITEM_PROPS.len() - 1)];
            st.tween_to(mc, 0.25, &[(Prop::Y, y), (Prop::Alpha, a)], Ease::StrongOut, None);
        }
        interval(st, &mut self.interval_move, 250.0, key(W_PLOG, PLOG_MOVED, 0));
    }

    fn on_item_moved(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_move);
        self.tweening = false;
        if self.log_items.len() == PLOG_SLOTS {
            if let Some(last) = self.log_items.pop() {
                self.move_to_pool(st, last);
            }
        }
        if self.queued.is_empty() {
            return;
        }
        let (name, icon) = self.queued.remove(0);
        let Some(mc) = self.get_from_pool(st) else { return };
        self.set_slot(st, mc, &name, icon);
        self.log_items.insert(0, mc);
        st.set(mc, Prop::Y, PLOG_ITEM_PROPS[0].0);
        st.set(mc, Prop::Alpha, PLOG_ITEM_PROPS[0].1);
        self.show_slot(st, mc);
        if !self.queued.is_empty() {
            self.tween_items(st);
        }
    }

    fn move_to_pool(&mut self, st: &mut Stage, mc: Id) {
        let i = self.slot_index(mc);
        if let Some(iv) = self.intervals.get_mut(i) {
            clear(st, iv);
        }
        st.tween_end(mc, false);
        let slot = st.child(mc, "slot");
        tween_end(st, slot, false);
        st.set_visible(mc, false);
        set(st, slot, Prop::Alpha, 0.0);
        self.pooled.push(mc);
    }

    fn get_from_pool(&mut self, st: &mut Stage) -> Option<Id> {
        if self.pooled.is_empty() {
            return None;
        }
        let mc = self.pooled.remove(0);
        st.set_visible(mc, true);
        let slot = st.child(mc, "slot");
        set(st, slot, Prop::Alpha, 100.0);
        Some(mc)
    }

    fn set_slot(&mut self, st: &mut Stage, mc: Id, name: &str, icon: Option<ImageRef>) {
        let Some(slot) = st.child(mc, "slot") else { return };
        if let Some(t) = st.child(slot, "txt") {
            st.set_html(t, name);
        }
        if let Some(il) = st.child(slot, "imgLoader") {
            st.set(il, Prop::XScale, 100.0);
            st.set(il, Prop::YScale, 100.0);
            st.img_loader(il, icon, false, true);
        }
    }

    fn show_slot(&mut self, st: &mut Stage, mc: Id) {
        let Some(slot) = st.child(mc, "slot") else { return };
        st.set(slot, Prop::Alpha, 0.0);
        st.set(slot, Prop::X, 30.0);
        st.tween_to(slot, PLOG_FADE_IN, &[(Prop::Alpha, 100.0), (Prop::X, 0.0)], Ease::StrongOut, None);
        let i = self.slot_index(mc);
        let k = key(W_PLOG, PLOG_HIDE, mc as u32);
        if let Some(iv) = self.intervals.get_mut(i) {
            interval(st, iv, PLOG_LIFE * 1000.0, k);
        }
    }

    fn hide_slot(&mut self, st: &mut Stage, mc: Id) {
        let i = self.slot_index(mc);
        if let Some(iv) = self.intervals.get_mut(i) {
            clear(st, iv);
        }
        if let Some(slot) = st.child(mc, "slot") {
            st.tween_to(slot, PLOG_FADE_OUT, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
        }
        let k = key(W_PLOG, PLOG_HIDDEN, mc as u32);
        if let Some(iv) = self.intervals.get_mut(i) {
            interval(st, iv, PLOG_FADE_OUT * 1000.0, k);
        }
    }

    fn on_slot_hidden(&mut self, st: &mut Stage, mc: Id) {
        if let Some(p) = self.log_items.iter().position(|m| *m == mc) {
            self.log_items.remove(p);
        }
        self.move_to_pool(st, mc);
    }

    fn on_event(&mut self, st: &mut Stage, code: u16, arg: u32) {
        let mc = arg as Id;
        match code {
            PLOG_MOVED => {
                self.interval_move = None;
                self.on_item_moved(st);
            }
            PLOG_HIDE => {
                let i = self.slot_index(mc);
                if let Some(iv) = self.intervals.get_mut(i) {
                    *iv = None;
                }
                self.hide_slot(st, mc);
            }
            PLOG_HIDDEN => {
                let i = self.slot_index(mc);
                if let Some(iv) = self.intervals.get_mut(i) {
                    *iv = None;
                }
                self.on_slot_hidden(st, mc);
            }
            _ => {}
        }
    }
}

// =============================================================================================
// PlayerGauges (hud_gauges, `masterHUD_mc`)
// =============================================================================================

pub struct PlayerGauges {
    pub mc: Id,
    bkgd: Option<Id>,
    health_meter: Option<Id>,
    mana_meter: Option<Id>,
    health_indic: Option<Id>,
    health_mask: Option<Id>,
    health_mask_regen: Option<Id>,
    health_glow: Option<Id>,
    health_refill: Option<Id>,
    mana_indic: Option<Id>,
    mana_mask: Option<Id>,
    mana_mask_regen: Option<Id>,
    mana_glow: Option<Id>,
    mana_refill: Option<Id>,
    health_mask_h: f32,
    mana_mask_h: f32,
    pub equipment: Option<Id>,
    ammo_count: Option<Id>,
    icon: Option<Id>,
    health_value: f32,
    mana_value: f32,
    low_health: bool,
    low_mana: bool,
    health_glow_duration: f32,
    health_glow_min: f32,
    health_glow_max: f32,
    mana_glow_duration: f32,
    mana_glow_min: f32,
    mana_glow_max: f32,
    icon_glow_playing: bool,
    interval_health: Option<u32>,
    interval_mana: Option<u32>,
    pub is_open: bool,
}

/// `_meterFillDuration`.
pub const METER_FILL_DURATION: f32 = 0.45;

const G_GLOW_HEALTH: u16 = 1;
const G_GLOW_MANA: u16 = 2;
const G_CLOSED: u16 = 3;

impl PlayerGauges {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        let hm = st.child(mc, "_healthMeter_mc");
        let mm = st.child(mc, "_manaMeter_mc");
        let c = |st: &Stage, p: Option<Id>, n: &str| p.and_then(|p| st.child(p, n));
        let health_mask = c(st, hm, "mask_mc");
        let mana_mask = c(st, mm, "mask_mc");
        let equipment = st.child(mc, "_equipment_mc");
        let mut g = Self {
            mc,
            bkgd: st.child(mc, "_bkgd_mc"),
            health_meter: hm,
            mana_meter: mm,
            health_indic: c(st, hm, "indic_mc"),
            health_mask,
            health_mask_regen: c(st, hm, "maskRegen_mc"),
            health_glow: c(st, hm, "glow_mc"),
            health_refill: c(st, hm, "refill_mc"),
            mana_indic: c(st, mm, "indic_mc"),
            mana_mask,
            mana_mask_regen: c(st, mm, "maskRegen_mc"),
            mana_glow: c(st, mm, "glow_mc"),
            mana_refill: c(st, mm, "refill_mc"),
            health_mask_h: 0.0,
            mana_mask_h: 0.0,
            equipment,
            ammo_count: c(st, equipment, "quantity"),
            icon: c(st, equipment, "ic"),
            health_value: 100.0,
            mana_value: 100.0,
            low_health: false,
            low_mana: false,
            health_glow_duration: 2.25,
            health_glow_min: 15.0,
            health_glow_max: 40.0,
            mana_glow_duration: 2.25,
            mana_glow_min: 15.0,
            mana_glow_max: 40.0,
            icon_glow_playing: false,
            interval_health: None,
            interval_mana: None,
            is_open: true,
        };
        set(st, g.health_mask, Prop::YScale, 100.0);
        g.health_mask_h = g.health_mask.map(|m| st.height(m)).unwrap_or(0.0);
        set(st, g.mana_mask, Prop::YScale, 100.0);
        g.mana_mask_h = g.mana_mask.map(|m| st.height(m)).unwrap_or(0.0);
        set(st, g.health_glow, Prop::Alpha, 100.0);
        set(st, g.mana_glow, Prop::Alpha, 100.0);
        g.set_empty_equipment(st);
        g.init_glow_anim(st);
        g
    }

    /// `Open()`.
    pub fn open(&mut self, st: &mut Stage) {
        self.is_open = true;
        st.set_visible(self.mc, true);
        st.tween_end(self.mc, false);
        st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
        goto_play(st, self.bkgd, "open");
    }
    /// `Close()`: fades out, hidden when the tween completes.
    pub fn close(&mut self, st: &mut Stage) {
        self.is_open = false;
        st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 0.0)], Ease::StrongOut, Some(key(W_GAUGE, G_CLOSED, 0)));
        goto_play(st, self.bkgd, "close");
    }

    fn upgrade_label(small: bool, large: bool) -> &'static str {
        match (small, large) {
            (true, true) => "upgd6",
            (true, false) => "upgd2",
            (false, true) => "upgd4",
            _ => "default",
        }
    }

    /// `FillHealthGauge(healthValue, hasSmallUpg, hasLargeUpg, lowHealth, regenValue)` (percent).
    pub fn fill_health(&mut self, st: &mut Stage, value: f32, small: bool, large: bool, low: bool, regen: f32) {
        self.health_value = value.max(0.1);
        if !self.low_health && low {
            (self.health_glow_duration, self.health_glow_min, self.health_glow_max) = (0.55, 35.0, 75.0);
            self.init_glow_anim(st);
        } else if self.low_health && !low {
            (self.health_glow_duration, self.health_glow_min, self.health_glow_max) = (2.25, 15.0, 40.0);
            self.init_glow_anim(st);
        }
        self.low_health = low;
        let l = Self::upgrade_label(small, large);
        goto_stop(st, self.health_meter, l);
        goto_stop(st, self.health_mask, l);
        goto_stop(st, self.health_mask_regen, l);
        if let Some(m) = self.health_refill.and_then(|r| st.child(r, "mask_mc")) {
            st.goto_and_stop(m, l);
        }
        st.goto_and_stop(self.mc, l);
        Self::fill(st, self.health_mask, self.health_mask_regen, self.health_indic, &mut self.health_mask_h, self.health_value, value, regen);
    }

    /// `FillManaGauge(manaValue, hasUpg, manaGaugeAvailable, manaGaugeHidden, lowMana, regenValue)`.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_mana(&mut self, st: &mut Stage, value: f32, upg: bool, available: bool, hidden: bool, low: bool, regen: f32) {
        self.mana_value = value.max(0.1);
        if available {
            visible(st, self.mana_glow, true);
            // `ResetColorTransform()`: the default transform (alpha multiplier 1 too).
            if let Some(m) = self.mana_meter {
                st.set_color(m, ColorTransform::default());
                st.set(m, Prop::Alpha, 100.0);
            }
        } else {
            visible(st, self.mana_glow, false);
            // `SetColor_Lock()`: multipliers 0.4 (alpha too).
            if let Some(m) = self.mana_meter {
                st.set_color(m, ColorTransform { mul: [0.4, 0.4, 0.4, 1.0], add: [0.0; 4] });
                st.set(m, Prop::Alpha, 40.0);
            }
        }
        if !self.low_mana && low {
            (self.mana_glow_duration, self.mana_glow_min, self.mana_glow_max) = (0.55, 35.0, 75.0);
            self.init_glow_anim(st);
        } else if self.low_mana && !low {
            (self.mana_glow_duration, self.mana_glow_min, self.mana_glow_max) = (2.25, 15.0, 40.0);
            self.init_glow_anim(st);
        }
        self.low_mana = low;
        let l = if upg { "upgd2" } else { "default" };
        visible(st, self.mana_meter, !hidden);
        goto_stop(st, self.mana_meter, l);
        goto_stop(st, self.mana_mask, l);
        goto_stop(st, self.mana_mask_regen, l);
        Self::fill(st, self.mana_mask, self.mana_mask_regen, self.mana_indic, &mut self.mana_mask_h, self.mana_value, value, regen);
    }

    /// The common part of the fills: the mask's `_yscale` tweens to the value, the indicator
    /// slides to the top of the fill (hidden at 100), the regeneration mask jumps.
    #[allow(clippy::too_many_arguments)]
    fn fill(st: &mut Stage, mask: Option<Id>, regen_mask: Option<Id>, indic: Option<Id>, mask_h: &mut f32, value: f32, raw: f32, regen: f32) {
        let Some(m) = mask else { return };
        let ys = st.get(m, Prop::YScale);
        st.set(m, Prop::YScale, 100.0);
        *mask_h = st.height(m);
        st.set(m, Prop::YScale, ys);
        let y = st.get(m, Prop::Y) - *mask_h * (value / 100.0);
        st.tween_end(m, false);
        st.tween_to(m, METER_FILL_DURATION, &[(Prop::YScale, value)], Ease::StrongOut, None);
        let a = if raw < 100.0 { 100.0 } else { 0.0 };
        tween_end(st, indic, false);
        tween(st, indic, METER_FILL_DURATION, &[(Prop::Y, y), (Prop::Alpha, a)], Ease::StrongOut, None);
        set(st, regen_mask, Prop::YScale, if regen < 0.0 { -1.0 } else { regen.max(0.1) });
    }

    pub fn refill_health(&mut self, st: &mut Stage) {
        goto_play(st, self.health_refill, "anim");
    }
    pub fn refill_mana(&mut self, st: &mut Stage) {
        goto_play(st, self.mana_refill, "anim");
    }

    pub fn set_empty_equipment(&mut self, st: &mut Stage) {
        visible(st, self.equipment, false);
    }

    /// `SetPower(powerIdx, isPowerActivated, hasEnoughMana)`.
    pub fn set_power(&mut self, st: &mut Stage, icon: Option<ImageRef>, active: bool, enough_mana: bool) {
        self.set_icon_image(st, icon);
        visible(st, self.equipment, true);
        visible(st, self.ammo_count, false);
        if active {
            self.play_icon_glow(st);
            goto_stop(st, self.equipment, "stop_pow_over");
        } else if enough_mana {
            self.stop_icon_glow(st);
            goto_stop(st, self.equipment, "stop_pow");
        } else {
            self.stop_icon_glow(st);
            goto_stop(st, self.equipment, "stop_pow_empty");
        }
    }

    /// `SetWeapon(weaponIdx, ammoTotal, ammoCount)`: `ammo_total` None hides the counter;
    /// `bullets` (the regular bullet's loaded count) selects `itemIcons_bulletCount`'s frame.
    pub fn set_weapon(&mut self, st: &mut Stage, icon: Option<ImageRef>, ammo_total: Option<i32>, bullets: Option<i32>) {
        self.stop_icon_glow(st);
        visible(st, self.equipment, true);
        match ammo_total {
            None => visible(st, self.ammo_count, false),
            Some(n) => {
                visible(st, self.ammo_count, true);
                if let Some(t) = self.ammo_count.and_then(|a| st.child(a, "txt")) {
                    st.set_text(t, &n.to_string());
                }
            }
        }
        match bullets {
            Some(loaded) => self.set_bullet_count(st, loaded),
            None => self.set_icon_image(st, icon),
        }
        let l = if ammo_total.is_none_or(|n| n > 0) { "default" } else { "stop_empty" };
        goto_stop(st, self.equipment, l);
    }

    /// `_common.EquipmentIcon.CreateContainer`: a fresh `ic._imgLoader_mc`.
    fn create_container(&mut self, st: &mut Stage) -> Option<Id> {
        let ic = self.icon?;
        if let Some(old) = st.child(ic, "_imgLoader_mc") {
            st.remove(old);
        }
        let d = st.next_depth(ic);
        Some(st.create_empty(ic, "_imgLoader_mc", d))
    }

    /// `_equipment_mc.SetIconMc("itemIcons_bulletCount", "bulletCount_mc")` and its frame
    /// `loaded + 1` (the pistol's loaded rounds).
    fn set_bullet_count(&mut self, st: &mut Stage, loaded: i32) {
        let Some(holder) = self.create_container(st) else { return };
        let d = st.next_depth(holder);
        if let Some(mc) = st.attach(holder, "itemIcons_bulletCount", "bulletCount_mc", d) {
            st.goto_and_stop(mc, &(loaded + 1).to_string());
        }
    }

    /// `SetIconImage(idx)`: the image `req_EquipmentIconImage(idx)` returns
    /// (`DisGlobalUIManager.m_EquipmentIcons`), loaded without auto-fit and centred.
    fn set_icon_image(&mut self, st: &mut Stage, icon: Option<ImageRef>) {
        let Some(holder) = self.create_container(st) else { return };
        st.img_loader(holder, icon, false, true);
    }

    fn play_icon_glow(&mut self, st: &mut Stage) {
        self.icon_glow_playing = true;
        if let Some(g) = self.icon.and_then(|i| st.child(i, "animGlow")) {
            st.goto_and_play(g, "over");
        }
    }
    fn stop_icon_glow(&mut self, st: &mut Stage) {
        if self.icon_glow_playing {
            self.icon_glow_playing = false;
            if let Some(g) = self.icon.and_then(|i| st.child(i, "animGlow")) {
                st.goto_and_play(g, "out");
            }
        }
    }

    fn init_glow_anim(&mut self, st: &mut Stage) {
        tween_end(st, self.health_glow, false);
        set(st, self.health_glow, Prop::Alpha, self.health_glow_min);
        tween_end(st, self.mana_glow, false);
        set(st, self.mana_glow, Prop::Alpha, self.mana_glow_min);
        self.loop_glow(st, true);
        self.loop_glow(st, false);
    }

    /// `loopGlowAnim_health` / `_mana`: alternate the glow between its min and max alpha.
    fn loop_glow(&mut self, st: &mut Stage, health: bool) {
        let (g, d, mn, mx) = if health { (self.health_glow, self.health_glow_duration, self.health_glow_min, self.health_glow_max) } else { (self.mana_glow, self.mana_glow_duration, self.mana_glow_min, self.mana_glow_max) };
        tween_end(st, g, false);
        let a = get(st, g, Prop::Alpha);
        let to = if a > mn { mn } else { mx };
        tween(st, g, d, &[(Prop::Alpha, to)], Ease::StrongOut, None);
        let (iv, code) = if health { (&mut self.interval_health, G_GLOW_HEALTH) } else { (&mut self.interval_mana, G_GLOW_MANA) };
        interval(st, iv, d * 1000.0, key(W_GAUGE, code, 0));
    }

    fn on_event(&mut self, st: &mut Stage, code: u16) {
        match code {
            G_GLOW_HEALTH => {
                self.interval_health = None;
                self.loop_glow(st, true);
            }
            G_GLOW_MANA => {
                self.interval_mana = None;
                self.loop_glow(st, false);
            }
            G_CLOSED => st.set_visible(self.mc, false),
            _ => {}
        }
    }
}

// =============================================================================================
// Simple fading texts: CrosshairInfosText, Subtitles, TutorialMessage
// =============================================================================================

/// `CrosshairInfosText` (`crosshairInfosTxt_mc`): `SetText(msg | null)`, 0.25 s fades.
pub struct CrosshairInfosText {
    pub mc: Id,
    txt: Option<Id>,
    interval_id: Option<u32>,
    pub text: Option<String>,
}

const CH_CLOSED: u16 = 1;

impl CrosshairInfosText {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        let txt = st.child(mc, "_txt");
        if let Some(t) = txt {
            st.set_text(t, "");
        }
        Self { mc, txt, interval_id: None, text: None }
    }
    pub fn set_text(&mut self, st: &mut Stage, msg: Option<&str>) {
        if self.text.as_deref() == msg {
            return;
        }
        self.text = msg.map(str::to_string);
        match msg {
            None => {
                clear(st, &mut self.interval_id);
                st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
                interval(st, &mut self.interval_id, 250.0, key(W_CHINFO, CH_CLOSED, 0));
            }
            Some(m) => {
                if let Some(t) = self.txt {
                    st.set_html(t, m);
                }
                clear(st, &mut self.interval_id);
                st.set_visible(self.mc, true);
                st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
            }
        }
    }
    fn on_event(&mut self, st: &mut Stage, code: u16) {
        if code == CH_CLOSED {
            self.interval_id = None;
            st.set_visible(self.mc, false);
            if let Some(t) = self.txt {
                st.set_text(t, "");
            }
        }
    }
}

/// `Subtitles` (`subtitles_mc`): `SetSubtitles(name, text | null)`, 0.25 s fades. The movie
/// has no `_NPC_Name_txt` field, so the speaker's name is not shown.
pub struct Subtitles {
    pub mc: Id,
    txt: Option<Id>,
    interval_id: Option<u32>,
    pub text: Option<String>,
    pub opened: bool,
}

const SUB_CLOSED: u16 = 1;

impl Subtitles {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        let txt = st.child(mc, "_txt");
        if let Some(t) = txt {
            st.set_text(t, "");
        }
        Self { mc, txt, interval_id: None, text: None, opened: false }
    }
    pub fn set_subtitles(&mut self, st: &mut Stage, text: Option<&str>) {
        if self.text.as_deref() == text {
            return;
        }
        self.text = text.map(str::to_string);
        clear(st, &mut self.interval_id);
        match text {
            None => {
                st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
                interval(st, &mut self.interval_id, 250.0, key(W_SUB, SUB_CLOSED, 0));
            }
            Some(s) => {
                if let Some(t) = self.txt {
                    st.set_html(t, s);
                }
                st.set_visible(self.mc, true);
                st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
            }
        }
    }
    fn on_event(&mut self, st: &mut Stage, code: u16) {
        if code == SUB_CLOSED {
            self.interval_id = None;
            st.set_visible(self.mc, false);
            if let Some(t) = self.txt {
                st.set_text(t, "");
            }
        }
    }
}

/// `TutorialMessage` (`tutorialMsg_mc`): `SetTutorialMessage(msg | null)`.
pub struct TutorialMessage {
    pub mc: Id,
    msg: Id,
    txt_mc: Option<Id>,
    txt: Option<Id>,
    opened: bool,
    pub text: Option<String>,
}

const TUTO_CLOSED: u16 = 1;

impl TutorialMessage {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        let txt_mc = st.child(mc, "_txt_mc");
        let txt = txt_mc.and_then(|t| st.child(t, "txt"));
        if let Some(t) = txt {
            st.set_text(t, "");
            st.set_auto_size(t, AutoSize::Left);
            st.set_word_wrap(t, true);
        }
        st.set_visible(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        Self { mc, msg: mc, txt_mc, txt, opened: false, text: None }
    }
    pub fn set_message(&mut self, st: &mut Stage, msg: Option<&str>) {
        if self.text.as_deref() == msg {
            return;
        }
        self.text = msg.map(str::to_string);
        match msg {
            None => self.close(st),
            Some(m) => {
                if !self.opened {
                    self.open(st);
                }
                if let Some(t) = self.txt {
                    st.set_html(t, m);
                }
            }
        }
    }
    fn open(&mut self, st: &mut Stage) {
        st.set_visible(self.mc, true);
        self.opened = true;
        st.tween_end(self.msg, false);
        st.tween_to(self.msg, 0.35, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
        tween_end(st, self.txt_mc, false);
        set(st, self.txt_mc, Prop::XScale, 110.0);
        set(st, self.txt_mc, Prop::YScale, 110.0);
        tween(st, self.txt_mc, 0.15, &[(Prop::XScale, 100.0), (Prop::YScale, 100.0)], Ease::StrongOut, None);
    }
    fn close(&mut self, st: &mut Stage) {
        self.opened = false;
        st.tween_to(self.msg, 0.15, &[(Prop::Alpha, 0.0)], Ease::StrongOut, Some(key(W_TUTO, TUTO_CLOSED, 0)));
    }
    fn on_event(&mut self, st: &mut Stage, code: u16) {
        if code == TUTO_CLOSED {
            if let Some(t) = self.txt {
                st.set_text(t, "");
            }
        }
    }
}

// =============================================================================================
// TutorialWindow (hud_tutoWin_, `tutorialWindow_mc`)
// =============================================================================================

const W_TUTOWIN: u16 = 16;
const TW_OPEN_IMAGE: u16 = 1;
const TW_ON_OPENED: u16 = 2;
const TW_ANIM: u16 = 3;
const TW_ANIM_LOOP: u16 = 4;
const TW_ON_CLOSED: u16 = 5;

/// The window's pictures (`_img_mc` frames `tutorialImg + 1`): a gear (tutorial note), a note
/// (mission clues), a rune, a bone charm.
pub const TW_IMG_GEAR: u8 = 0;
pub const TW_IMG_NOTE: u8 = 1;
pub const TW_IMG_RUNE: u8 = 2;
pub const TW_IMG_CHARM: u8 = 3;

/// `TutorialWindow` (class sprite 812): `SetTutorialWindow(text, image)` opens it (from the left
/// at 250 %, 0.2 s `Back.easeOut`; the picture and strokes settle 150 ms later), then its parts
/// drift to and fro (`Anim` 5 s / `AnimLoop` 4.25 s, each re-armed 0.5 s before it ends) until
/// `Close()` (0.4 s). The native side keeps it up `m_fTutorialWindowDuration` (10 s) and
/// calls `OnClosed` once it is gone.
pub struct TutorialWindow {
    pub mc: Id,
    inner: Option<Id>,
    bkgd: Option<Id>,
    img: Option<Id>,
    txt_mc: Option<Id>,
    txt: Option<Id>,
    black: Option<Id>,
    white: Option<Id>,
    /// `_bkgd_mc.props`, `_img_mc.props`, `_txt_mc.props`, `_blackStroke_mc.props`,
    /// `_whiteStroke_mc.props` (saved by the constructor).
    props: [Props; 5],
    itvl_anim: Option<u32>,
    itvl_open: Option<u32>,
    itvl_closed: Option<u32>,
    pub opened: bool,
    /// Seconds up (the native side's `m_fTutorialWindowDuration` countdown).
    pub shown_for: f32,
}

impl TutorialWindow {
    pub const DURATION_ANIM: f32 = 5.0;
    pub const DURATION_ANIM_LOOP: f32 = 4.25;
    /// `m_fTutorialWindowDuration` (class default of `DisTweaks_GFxMoviePlayerHUD`).
    pub const DURATION: f32 = 10.0;

    pub fn new(st: &mut Stage, mc: Id) -> Self {
        let inner = st.child(mc, "_mc");
        let part = |st: &Stage, n: &str| inner.and_then(|i| st.child(i, n));
        let (bkgd, img, txt_mc, black, white) = (part(st, "_bkgd_mc"), part(st, "_img_mc"), part(st, "_txt_mc"), part(st, "_blackStroke_mc"), part(st, "_whiteStroke_mc"));
        let save = |st: &Stage, id: Option<Id>| id.map(|i| Props::save(st, i)).unwrap_or_default();
        let props = [save(st, bkgd), save(st, img), save(st, txt_mc), save(st, black), save(st, white)];
        let txt = txt_mc.and_then(|t| st.child(t, "txt"));
        st.set_visible(mc, false);
        Self { mc, inner, bkgd, img, txt_mc, txt, black, white, props, itvl_anim: None, itvl_open: None, itvl_closed: None, opened: false, shown_for: 0.0 }
    }

    /// `SetTutorialWindow(tutorialText, tutorialImg)`.
    pub fn set_tutorial_window(&mut self, st: &mut Stage, snd: &mut SoundQueue, text: &str, img: u8) {
        if let Some(t) = self.txt {
            st.set_html(t, text);
        }
        goto_stop(st, self.img, &(img as u32 + 1).to_string());
        self.open(st, snd);
    }

    fn open(&mut self, st: &mut Stage, snd: &mut SoundQueue) {
        clear(st, &mut self.itvl_anim);
        clear(st, &mut self.itvl_open);
        clear(st, &mut self.itvl_closed);
        self.clear_tweens(st, false);
        st.set_visible(self.mc, true);
        self.opened = true;
        self.shown_for = 0.0;
        snd.0.push("H_windowNotification");
        tween_end(st, self.inner, false);
        set(st, self.inner, Prop::Alpha, 0.0);
        set(st, self.inner, Prop::X, -250.0);
        set(st, self.inner, Prop::XScale, 250.0);
        set(st, self.inner, Prop::YScale, 250.0);
        tween(st, self.inner, 0.2, &[(Prop::X, 0.0), (Prop::Alpha, 100.0), (Prop::XScale, 100.0), (Prop::YScale, 100.0)], Ease::BackOut, None);
        tween_end(st, self.txt_mc, false);
        set(st, self.txt_mc, Prop::Alpha, 0.0);
        tween_end(st, self.img, false);
        set(st, self.img, Prop::Alpha, 0.0);
        set(st, self.img, Prop::X, self.props[1].x);
        tween_end(st, self.black, false);
        set(st, self.black, Prop::Alpha, 0.0);
        tween_end(st, self.white, false);
        set(st, self.white, Prop::Alpha, 0.0);
        interval(st, &mut self.itvl_open, 150.0, key(W_TUTOWIN, TW_OPEN_IMAGE, 0));
        goto_play(st, self.inner, "open");
    }

    fn open_image(&mut self, st: &mut Stage) {
        clear(st, &mut self.itvl_open);
        set(st, self.img, Prop::YScale, 200.0);
        set(st, self.img, Prop::XScale, 200.0);
        tween(st, self.img, 0.2, &[(Prop::Alpha, 100.0), (Prop::XScale, 100.0), (Prop::YScale, 100.0)], Ease::StrongOut, None);
        set(st, self.black, Prop::YScale, 200.0);
        set(st, self.black, Prop::XScale, 200.0);
        tween_props(st, self.black, 0.2, &self.props[3], Ease::BackOut);
        set(st, self.white, Prop::Rotation, -15.0);
        set(st, self.white, Prop::YScale, 200.0);
        set(st, self.white, Prop::XScale, 200.0);
        tween_props(st, self.white, 0.3, &self.props[4], Ease::StrongInOut);
        set(st, self.txt_mc, Prop::YScale, 150.0);
        set(st, self.txt_mc, Prop::XScale, 150.0);
        tween_props(st, self.txt_mc, 0.2, &self.props[2], Ease::StrongInOut);
        interval(st, &mut self.itvl_open, 0.3 * 1000.0, key(W_TUTOWIN, TW_ON_OPENED, 0));
    }

    fn anim(&mut self, st: &mut Stage) {
        clear(st, &mut self.itvl_anim);
        self.clear_tweens(st, false);
        let d = Self::DURATION_ANIM;
        let (b, i, t, w) = (self.props[0], self.props[1], self.props[2], self.props[4]);
        tween(st, self.bkgd, d, &[(Prop::X, b.x - 10.0), (Prop::XScale, 102.0), (Prop::YScale, 102.0), (Prop::Alpha, 90.0)], Ease::StrongOut, None);
        tween(st, self.img, d, &[(Prop::X, i.x + 10.0), (Prop::Rotation, 2.0)], Ease::StrongOut, None);
        tween(st, self.txt_mc, d, &[(Prop::X, t.x - 10.0)], Ease::StrongOut, None);
        tween(st, self.white, d, &[(Prop::X, w.x - 7.0), (Prop::Rotation, -5.0)], Ease::StrongOut, None);
        interval(st, &mut self.itvl_anim, d * 1000.0 - 500.0, key(W_TUTOWIN, TW_ANIM_LOOP, 0));
    }

    fn anim_loop(&mut self, st: &mut Stage) {
        clear(st, &mut self.itvl_anim);
        self.clear_tweens(st, false);
        let d = Self::DURATION_ANIM_LOOP;
        for (id, p) in [(self.bkgd, 0), (self.img, 1), (self.txt_mc, 2), (self.white, 4)] {
            tween_props(st, id, d, &self.props[p], Ease::StrongOut);
        }
        interval(st, &mut self.itvl_anim, d * 1000.0 - 500.0, key(W_TUTOWIN, TW_ANIM, 0));
    }

    fn clear_tweens(&mut self, st: &mut Stage, jump: bool) {
        for id in [self.inner, self.txt_mc, self.img, self.black, self.white] {
            tween_end(st, id, jump);
        }
    }

    /// `Close()`; `OnClosed` (hidden) follows once the 0.4 s slide is over.
    pub fn close(&mut self, st: &mut Stage) {
        clear(st, &mut self.itvl_open);
        clear(st, &mut self.itvl_anim);
        self.clear_tweens(st, false);
        self.opened = false;
        tween(st, self.inner, 0.4, &[(Prop::X, -30.0), (Prop::Alpha, 0.0)], Ease::StrongOut, None);
        tween(st, self.img, 0.2, &[(Prop::XScale, 120.0), (Prop::YScale, 120.0)], Ease::StrongOut, None);
        tween(st, self.white, 0.25, &[(Prop::Alpha, 0.0), (Prop::XScale, 130.0), (Prop::YScale, 130.0)], Ease::StrongOut, None);
        tween(st, self.black, 0.2, &[(Prop::Alpha, 0.0), (Prop::XScale, 120.0), (Prop::YScale, 120.0)], Ease::StrongOut, None);
        interval(st, &mut self.itvl_closed, 450.0, key(W_TUTOWIN, TW_ON_CLOSED, 0));
    }

    fn on_event(&mut self, st: &mut Stage, code: u16) {
        match code {
            TW_OPEN_IMAGE => self.open_image(st),
            TW_ON_OPENED => {
                clear(st, &mut self.itvl_open);
                self.anim(st);
            }
            TW_ANIM => self.anim(st),
            TW_ANIM_LOOP => self.anim_loop(st),
            TW_ON_CLOSED => {
                clear(st, &mut self.itvl_closed);
                st.set_visible(self.mc, false);
            }
            _ => {}
        }
    }
}

// =============================================================================================
// InteractionsMessage (hud_gameMessage / hud_interactionTxt / hud_runesTxt)
// =============================================================================================

pub struct InteractionsMessage {
    pub mc: Id,
    widget: u16,
    int_msg: Option<Id>,
    game_msg: Option<Id>,
    sep: Option<Id>,
    txt_int: Option<Id>,
    txt_game: Option<Id>,
    game_def_y: f32,
    sep_opened: bool,
    int_opened: bool,
    game_opened: bool,
    interval_msg: Option<u32>,
    pub interactions: Option<String>,
    pub game_message: Option<String>,
}

const IM_CLOSE_GAME: u16 = 1;
const IM_INT_CLOSED: u16 = 2;
const IM_GAME_CLOSED: u16 = 3;

impl InteractionsMessage {
    pub fn new(st: &mut Stage, mc: Id, index: u16) -> Self {
        let int_msg = st.child(mc, "_interactionsMsg_mc");
        let game_msg = st.child(mc, "_gameMsg_mc");
        let txt_int = int_msg.and_then(|m| st.child(m, "txt"));
        let txt_game = game_msg.and_then(|m| st.child(m, "txt"));
        for t in [txt_int, txt_game].into_iter().flatten() {
            st.set_text(t, "");
            st.set_auto_size(t, AutoSize::Left);
            st.set_word_wrap(t, true);
        }
        let sep = st.child(mc, "_sep_mc");
        set(st, sep, Prop::X, 0.0);
        set(st, sep, Prop::Alpha, 0.0);
        let game_def_y = get(st, game_msg, Prop::Y);
        // Closed state: the clips start at their authored alpha; the texts are empty.
        Self { mc, widget: W_MSG + index, int_msg, game_msg, sep, txt_int, txt_game, game_def_y, sep_opened: false, int_opened: false, game_opened: false, interval_msg: None, interactions: None, game_message: None }
    }

    /// `SetInteractions(msg | null)`.
    pub fn set_interactions(&mut self, st: &mut Stage, msg: Option<&str>) {
        if self.interactions.as_deref() == msg {
            return;
        }
        self.interactions = msg.map(str::to_string);
        match msg {
            None => self.close_interaction(st),
            Some(m) => {
                self.open_interaction(st);
                if let Some(t) = self.txt_int {
                    st.set_html(t, m);
                }
                self.set_game_msg_position(st);
            }
        }
    }

    /// `SetGameMessage(msg | null, duration)`.
    pub fn set_game_message(&mut self, st: &mut Stage, msg: Option<&str>, duration: f32) {
        self.game_message = msg.map(str::to_string);
        clear(st, &mut self.interval_msg);
        match msg {
            None => self.close_game(st),
            Some(m) => {
                self.open_game(st);
                interval(st, &mut self.interval_msg, duration * 1000.0, key(self.widget, IM_CLOSE_GAME, 0));
                if let Some(t) = self.txt_game {
                    st.set_html(t, m);
                }
            }
        }
    }

    fn set_game_msg_position(&mut self, st: &mut Stage) {
        let iy = get(st, self.int_msg, Prop::Y);
        set(st, self.game_msg, Prop::Y, iy);
        if self.int_opened {
            let h = self.int_msg.map(|m| st.height(m)).unwrap_or(0.0);
            let y = get(st, self.game_msg, Prop::Y) + h;
            set(st, self.game_msg, Prop::Y, y);
        }
    }

    fn open_separator(&mut self, st: &mut Stage) {
        self.sep_opened = true;
        tween_end(st, self.sep, false);
        set(st, self.sep, Prop::X, -35.0);
        tween(st, self.sep, 0.3, &[(Prop::X, 0.0), (Prop::Alpha, 100.0)], Ease::BackOut, None);
    }
    fn close_separator(&mut self, st: &mut Stage) {
        self.sep_opened = false;
        tween_end(st, self.sep, false);
        tween(st, self.sep, 0.2, &[(Prop::X, -10.0), (Prop::Alpha, 0.0)], Ease::StrongOut, None);
    }

    fn open_interaction(&mut self, st: &mut Stage) {
        if self.int_opened {
            return;
        }
        if !self.sep_opened {
            self.open_separator(st);
        }
        self.int_opened = true;
        tween_end(st, self.int_msg, false);
        set(st, self.int_msg, Prop::X, -25.0);
        tween(st, self.int_msg, 0.3, &[(Prop::X, 0.0), (Prop::Alpha, 100.0)], Ease::BackOut, None);
    }
    fn close_interaction(&mut self, st: &mut Stage) {
        if !self.int_opened {
            return;
        }
        if !self.game_opened {
            self.close_separator(st);
        }
        self.int_opened = false;
        tween_end(st, self.int_msg, false);
        tween(st, self.int_msg, 0.15, &[(Prop::X, -20.0), (Prop::Alpha, 0.0)], Ease::StrongOut, Some(key(self.widget, IM_INT_CLOSED, 0)));
    }
    fn open_game(&mut self, st: &mut Stage) {
        if self.game_opened {
            return;
        }
        if !self.sep_opened {
            self.open_separator(st);
        }
        self.game_opened = true;
        tween_end(st, self.game_msg, false);
        set(st, self.game_msg, Prop::Y, self.game_def_y + 15.0);
        tween(st, self.game_msg, 0.3, &[(Prop::Y, self.game_def_y), (Prop::Alpha, 100.0)], Ease::BackOut, None);
    }
    fn close_game(&mut self, st: &mut Stage) {
        if !self.game_opened {
            return;
        }
        if !self.int_opened {
            self.close_separator(st);
        }
        self.game_opened = false;
        tween_end(st, self.game_msg, false);
        tween(st, self.game_msg, 0.15, &[(Prop::Alpha, 0.0)], Ease::StrongOut, Some(key(self.widget, IM_GAME_CLOSED, 0)));
    }

    fn on_event(&mut self, st: &mut Stage, code: u16) {
        match code {
            IM_CLOSE_GAME => {
                self.interval_msg = None;
                self.game_message = None;
                self.close_game(st);
            }
            IM_INT_CLOSED => {
                if let Some(t) = self.txt_int {
                    st.set_text(t, "");
                }
                self.set_game_msg_position(st);
            }
            IM_GAME_CLOSED => {
                if let Some(t) = self.txt_game {
                    st.set_text(t, "");
                }
            }
            _ => {}
        }
    }
}

// =============================================================================================
// InteractionsWindow (hud_intWindow, `interactions_window_mc`): the crosshair interactions
// =============================================================================================

/// `SetInteractions(interactableName, interactions | null)`: shown and hidden at once (no
/// tween); the lines are the interaction texts joined with newlines (0x00BA6150).
pub struct InteractionsWindow {
    pub mc: Id,
    title: Option<Id>,
    lines: Option<Id>,
    pub opened: bool,
    current: Option<(String, String)>,
}

impl InteractionsWindow {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        let title = st.child(mc, "_title_txt");
        let lines = st.child(mc, "_interactions_txt");
        if let Some(t) = lines {
            st.set_text(t, "");
        }
        Self { mc, title, lines, opened: false, current: None }
    }
    pub fn set_interactions(&mut self, st: &mut Stage, name: &str, interactions: Option<&str>) {
        let want = interactions.map(|i| (name.to_string(), i.to_string()));
        if want == self.current {
            return;
        }
        self.current = want;
        match interactions {
            None => {
                st.set_visible(self.mc, false);
                self.opened = false;
            }
            Some(i) => {
                if !self.opened {
                    st.set_visible(self.mc, true);
                    self.opened = true;
                }
                if let Some(t) = self.title {
                    st.set_html(t, name);
                }
                if let Some(t) = self.lines {
                    st.set_html(t, i);
                }
            }
        }
    }
}

// =============================================================================================
// InteractionsIcons (hud_interactionIcons, `interactions_ic_mc`) and InteractionsIcons_QTE
// (hud_interactionIcons_QTE, `interactions_icQTE_mc`): the Special and SpecialQTE interaction
// groups (0x00BA6630 / 0x00BA6920: `SetInteractions([{iconIdx, txt}])`)
// =============================================================================================

const W_ICONS: u16 = 19;
const W_QTE: u16 = 22;
const IC_CLOSED: u16 = 1;
const QTE_PULSE: u16 = 2;

pub struct InteractionsIcons {
    pub mc: Id,
    widget: u16,
    qte: bool,
    icons: Option<Id>,
    ic: Option<Id>,
    nb_icons: u32,
    pub opened: bool,
    interval_anim: Option<u32>,
    pub list: Option<Vec<(u8, String)>>,
}

impl InteractionsIcons {
    /// The constructor (`_interactions_mc = this`); the clip starts hidden until `Open`.
    pub fn new(st: &mut Stage, mc: Id, qte: bool) -> Self {
        st.set_visible(mc, false);
        Self { mc, widget: if qte { W_QTE } else { W_ICONS }, qte, icons: None, ic: None, nb_icons: 0, opened: false, interval_anim: None, list: None }
    }

    /// `SetInteractions(list | null)`: null closes; else the icons are rebuilt and the clip opens.
    /// The native side sends a group only when it changed.
    pub fn set_interactions(&mut self, st: &mut Stage, list: Option<&[(u8, String)]>) {
        if self.list.as_deref() == list {
            return;
        }
        self.list = list.map(|l| l.to_vec());
        match list {
            None => self.close(st),
            Some(l) => {
                self.set_icons(st, l);
                if !self.opened {
                    self.open(st);
                }
            }
        }
    }

    /// `CreateContainer`: a fresh `_icons_mc`.
    fn create_container(&mut self, st: &mut Stage) {
        self.nb_icons = 0;
        if let Some(c) = self.icons.take() {
            st.remove(c);
        }
        let d = st.next_depth(self.mc);
        self.icons = Some(st.create_empty(self.mc, "_icons_mc", d));
    }

    /// `SetIconMovieClip(idx, e)`: an `hud_interactionIc` with the text and the icon frame of
    /// `iconIdx` (1 mantle, 2 pickpocket, 3 choke, 4 assassination, 5 drop_assassination,
    /// 6 adrenaline_kill).
    fn set_icon_movie_clip(&mut self, st: &mut Stage, idx: usize, e: &(u8, String)) -> Option<Id> {
        let c = self.icons?;
        let d = st.next_depth(c);
        let ic = st.attach(c, "hud_interactionIc", &format!("ic{idx}"), d)?;
        if let Some(t) = st.child(ic, "txt") {
            st.set_html(t, &e.1);
        }
        let label = match e.0 {
            1 => "mantle",
            2 => "pickpocket",
            3 => "choke",
            4 => "assassination",
            5 => "drop_assassination",
            6 => "adrenaline_kill",
            _ => "",
        };
        if let Some(i) = st.child(ic, "ic").filter(|_| !label.is_empty()) {
            st.goto_and_stop(i, label);
        }
        Some(ic)
    }

    /// `SetIcons(list)`: the icons stacked upward from the middle, 5 apart, at x -368; the QTE
    /// variant shows the first entry only, beside its icon, with the pulsing stroke.
    fn set_icons(&mut self, st: &mut Stage, list: &[(u8, String)]) {
        if self.qte {
            clear(st, &mut self.interval_anim);
            self.create_container(st);
            self.ic = list.first().and_then(|e| self.set_icon_movie_clip(st, 0, e));
            if let Some(ic) = self.ic {
                let w = st.child(ic, "ic").map(|i| st.width(i)).unwrap_or(0.0);
                st.set(ic, Prop::X, 0.5 * w + 20.0);
                if let Some(s) = st.child(ic, "_stroke_mc") {
                    st.goto_and_stop(s, "2");
                }
                self.nb_icons += 1;
            }
            self.anim_pulse(st);
            return;
        }
        self.create_container(st);
        let mut prev: Option<Id> = None;
        for (i, e) in list.iter().enumerate() {
            let Some(ic) = self.set_icon_movie_clip(st, i, e) else { continue };
            st.set(ic, Prop::X, -368.0);
            let y = match prev {
                None => (0.0 - st.height(ic)) * 0.5,
                Some(p) => st.get(p, Prop::Y) - st.height(p) - 5.0,
            };
            st.set(ic, Prop::Y, y);
            self.nb_icons += 1;
            prev = Some(ic);
        }
    }

    /// `Open` / `PlayOpenAnimation`: fade in (0.35 s); the QTE variant pops in from 110 %
    /// (0.2 s, `Back.easeOut`).
    fn open(&mut self, st: &mut Stage) {
        st.set_visible(self.mc, true);
        self.opened = true;
        st.tween_end(self.mc, false);
        st.set(self.mc, Prop::Alpha, 0.0);
        if self.qte {
            st.set(self.mc, Prop::XScale, 110.0);
            st.set(self.mc, Prop::YScale, 110.0);
            st.tween_to(self.mc, 0.2, &[(Prop::YScale, 100.0), (Prop::XScale, 100.0), (Prop::Alpha, 100.0)], Ease::BackOut, None);
        } else {
            st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
        }
    }

    /// `Close`: fade out (0.25 s), then `onClosed` hides the clip and drops the icons.
    fn close(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_anim);
        self.opened = false;
        st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, Some(key(self.widget, IC_CLOSED, 0)));
    }

    /// `AnimPulse`: the stroke from 90 % opaque to 120 % transparent (0.2 s), every 500 ms.
    fn anim_pulse(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_anim);
        if let Some(s) = self.ic.and_then(|ic| st.child(ic, "_stroke_mc")) {
            st.tween_end(s, false);
            st.set(s, Prop::XScale, 90.0);
            st.set(s, Prop::YScale, 90.0);
            st.set(s, Prop::Alpha, 100.0);
            st.tween_to(s, 0.2, &[(Prop::YScale, 120.0), (Prop::XScale, 120.0), (Prop::Alpha, 0.0)], Ease::StrongOut, None);
        }
        interval(st, &mut self.interval_anim, 0.2 * 1000.0 + 300.0, key(self.widget, QTE_PULSE, 0));
    }

    fn on_event(&mut self, st: &mut Stage, code: u16) {
        match code {
            IC_CLOSED if !self.opened => {
                st.set_visible(self.mc, false);
                if let Some(c) = self.icons.take() {
                    st.remove(c);
                }
                self.ic = None;
            }
            QTE_PULSE => self.anim_pulse(st),
            _ => {}
        }
    }
}

// =============================================================================================
// PlayerChoice (hud_playerChoice, `playerChoice_mc`; a `_common.GenericMenu`)
// =============================================================================================

pub struct PlayerChoice {
    pub mc: Id,
    title: Option<Id>,
    indic: Option<Id>,
    title_props: Props,
    container: Option<Id>,
    buttons: Vec<Id>,
    pub selection: usize,
    pub options: Option<Vec<String>>,
    interval_open: Option<u32>,
    interval_close: Option<u32>,
    /// Confirmed choice waiting for `OnPlayerChoiceConfirm` (150 ms after the press).
    pub confirmed: Option<usize>,
    /// `_global.bUsingGamepad`: the native copies `PlayerInput.bUsingGamepad` (the last input
    /// device) into the movie whenever it changes (0x00B941F0, HUD vtable +0x1E8, the getter
    /// 0x00B8C6E0). The port reads no gamepad: false.
    pub using_gamepad: bool,
    /// `_root.inputs.InputsEnabled` with the selection handler's `_handlerEnabledState`: on from
    /// `InitInputs` (`SetMenu`), off from `RemoveInputs` (`APressed`, `Confirm`, `Close`).
    inputs: bool,
    /// The button the mouse is over: a button's `btn.onRollOver` fires as the mouse enters it.
    hover: Option<usize>,
    /// The button the mouse went down on: `btn.onRelease` fires for a release over it.
    press: Option<usize>,
}

/// `PlayerChoice.playSelectionOn`: on PC (`_global.PlatformName == "PC"`) the selected
/// button's `_a_mc` is forced to the Xenon glyph and shown only while `_global.bUsingGamepad`;
/// on the consoles it is always shown. The PC movie has no keyboard glyph for the choices.
pub fn choice_a_icon_visible(platform_pc: bool, using_gamepad: bool) -> bool {
    !platform_pc || using_gamepad
}

/// What the HUD movie's `_common.InputsManager` makes of a key (`InputManager`, Flash key codes
/// as the native forwards them while the movie has focus): Up (38) / Down (40) are the "up" /
/// "down" inputs, held keys repeating (`holdEnabled`); Enter (13) is "A", once per press. The
/// number keys (48-57) are "DockBar", which `PlayerChoice` never takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceKey {
    Up,
    Down,
    A,
}

pub fn choice_key(key: bevy::input::keyboard::KeyCode, repeat: bool) -> Option<ChoiceKey> {
    use bevy::input::keyboard::KeyCode;
    match key {
        KeyCode::ArrowUp => Some(ChoiceKey::Up),
        KeyCode::ArrowDown => Some(ChoiceKey::Down),
        // Windows reports both Enter keys as VK_RETURN, the engine's `Enter`.
        KeyCode::Enter | KeyCode::NumpadEnter if !repeat => Some(ChoiceKey::A),
        _ => None,
    }
}

/// `_btnOutProps` / `_btnOverProps` (alpha, x, xscale, yscale; the 3D `_z` / `_yrotation` of
/// GFx are not reproduced).
const BTN_OUT: [(Prop, f32); 4] = [(Prop::Alpha, 85.0), (Prop::X, 0.0), (Prop::XScale, 100.0), (Prop::YScale, 100.0)];
const BTN_OVER: [(Prop, f32); 4] = [(Prop::Alpha, 100.0), (Prop::X, 15.0), (Prop::XScale, 110.0), (Prop::YScale, 110.0)];
/// `_offsetBtnY`.
const BTN_OFFSET_Y: f32 = 2.5;

const PC_OPEN_BTN: u16 = 1;
const PC_CLOSE_BTN: u16 = 2;
const PC_CONFIRM: u16 = 3;

impl PlayerChoice {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        let title = st.child(mc, "_title_mc");
        let indic = title.and_then(|t| st.child(t, "_indic_mc"));
        let mut title_props = title.map(|t| Props::save(st, t)).unwrap_or_default();
        title_props.alpha = 100.0;
        // `_xscale = _yscale = 95`.
        st.set(mc, Prop::XScale, 95.0);
        st.set(mc, Prop::YScale, 95.0);
        Self { mc, title, indic, title_props, container: None, buttons: Vec::new(), selection: 0, options: None, interval_open: None, interval_close: None, confirmed: None, using_gamepad: false, inputs: false, hover: None, press: None }
    }

    /// `SetMenu(menuContent | null, menuTitle)`.
    pub fn set_menu(&mut self, st: &mut Stage, snd: &mut SoundQueue, options: Option<&[String]>, title: Option<&str>) {
        let new = options.map(|o| o.to_vec());
        if new == self.options {
            return;
        }
        self.options = new;
        clear(st, &mut self.interval_open);
        clear(st, &mut self.interval_close);
        let Some(opts) = options else {
            self.close(st);
            return;
        };
        let _ = snd;
        if let Some(c) = self.container.take() {
            st.remove(c);
        }
        let d = st.next_depth(self.mc);
        let cont = st.create_empty(self.mc, "_btnContainer_mc", d);
        self.container = Some(cont);
        self.buttons.clear();
        for (i, o) in opts.iter().enumerate() {
            let d = st.next_depth(cont);
            let Some(b) = st.attach(cont, "hud_plC_button", &format!("btn{i}"), d) else { continue };
            for n in ["_txt_mc/txt", "_txtGlow_mc/txt"] {
                if let Some(t) = st.find(b, n) {
                    st.set_html(t, o);
                }
            }
            st.goto_and_stop(b, "default");
            let h = st.height(b);
            st.set(b, Prop::X, 0.0);
            st.set(b, Prop::Y, i as f32 * (h + BTN_OFFSET_Y));
            st.set(b, Prop::Alpha, 0.0);
            self.buttons.push(b);
        }
        // Bottom / left alignment.
        let ch = st.height(cont);
        st.set(cont, Prop::Y, -ch);
        st.set(cont, Prop::X, 0.0);
        // PlayerChoice.SetMenu: place the title against the buttons.
        let th = self.title.map(|t| st.height(t)).unwrap_or(0.0);
        let r3 = if th > ch { 0.5 * th - 10.0 } else { 0.5 * th - 25.0 };
        let cy = st.get(cont, Prop::Y) - r3;
        st.set(cont, Prop::Y, cy);
        set(st, self.title, Prop::Y, 0.5 * (cy - r3));
        match title {
            None => {
                if let Some(t) = self.title.and_then(|t| st.child(t, "txt")) {
                    st.set_text(t, "");
                }
                if let Some(tt) = self.title.and_then(|t| st.child(t, "ttip")) {
                    st.set_visible(tt, false);
                }
            }
            Some(s) => {
                if let Some(tt) = self.title.and_then(|t| st.child(t, "ttip")) {
                    st.set_visible(tt, true);
                }
                if let Some(t) = self.title.and_then(|t| st.child(t, "txt")) {
                    st.set_html(t, s);
                    let x = -50.0 - st.text_width(t);
                    st.set(t, Prop::X, x);
                    if let Some(tt) = self.title.and_then(|tm| st.child(tm, "ttip")) {
                        st.set(tt, Prop::X, x);
                    }
                }
            }
        }
        let ix = self.indic.map(|i| st.width(i) + st.get(i, Prop::X)).unwrap_or(0.0);
        st.set(cont, Prop::X, ix + 20.0);
        self.selection = 0;
        self.open(st);
        self.play_selection_on(st, 0);
        // `InitInputs`: "A" and the menu's `_inputsList` ("up", "down") go to this menu.
        self.inputs = true;
        self.hover = None;
        self.press = None;
        self.confirmed = None;
    }

    fn open(&mut self, st: &mut Stage) {
        tween_end(st, self.title, false);
        set(st, self.title, Prop::X, -50.0);
        set(st, self.title, Prop::Alpha, 0.0);
        let p = self.title_props;
        tween(st, self.title, 0.35, &[(Prop::X, p.x), (Prop::Alpha, 100.0)], Ease::BackInOut, None);
        clear(st, &mut self.interval_open);
        clear(st, &mut self.interval_close);
        st.set_visible(self.mc, true);
        self.open_button(st, 0);
    }

    fn open_button(&mut self, st: &mut Stage, idx: usize) {
        clear(st, &mut self.interval_open);
        if idx < self.buttons.len() {
            if let Some(b) = self.buttons.get(idx).copied() {
                st.tween_to(b, 0.35, &BTN_OUT, Ease::BackOut, None);
            }
            interval(st, &mut self.interval_open, 50.0, key(W_CHOICE, PC_OPEN_BTN, idx as u32 + 1));
        }
    }

    /// Move the highlight (mouse over / arrows).
    pub fn select(&mut self, st: &mut Stage, idx: usize) {
        if idx == self.selection || idx >= self.buttons.len() {
            return;
        }
        let old = self.selection;
        self.play_selection_out(st, old);
        self.selection = idx;
        self.play_selection_on(st, idx);
    }

    /// `UpPressed` -> `SelectionHandler.UpPressed`: the button above with `Move`; at the first
    /// one `Stop` (`_bSelectionLoop` is never set for the choices).
    pub fn up_pressed(&mut self, st: &mut Stage, snd: &mut SoundQueue) {
        if !self.inputs {
            return;
        }
        if self.selection > 0 {
            self.select(st, self.selection - 1);
            snd.0.push("Move");
        } else {
            snd.0.push("Stop");
        }
    }

    /// `DownPressed` -> `SelectionHandler.DownPressed`: the button below with `Move`; at the
    /// last one `Stop`.
    pub fn down_pressed(&mut self, st: &mut Stage, snd: &mut SoundQueue) {
        if !self.inputs {
            return;
        }
        if self.selection + 1 < self.buttons.len() {
            self.select(st, self.selection + 1);
            snd.0.push("Move");
        } else {
            snd.0.push("Stop");
        }
    }

    /// The "A" input (`InputsManager` -> `APressed`): confirm the selection.
    pub fn a_pressed(&mut self, st: &mut Stage, snd: &mut SoundQueue) {
        if self.inputs {
            self.confirm(st, snd, self.selection);
        }
    }

    pub fn key(&mut self, st: &mut Stage, snd: &mut SoundQueue, k: ChoiceKey) {
        match k {
            ChoiceKey::Up => self.up_pressed(st, snd),
            ChoiceKey::Down => self.down_pressed(st, snd),
            ChoiceKey::A => self.a_pressed(st, snd),
        }
    }

    /// The button whose `btn` hit area holds `p` (stage pixels). `btn` is a DefineButton2 (91:
    /// its hit state the 62 x 62 shape 90, placed at x 5.53, y 0.65 from (-40.9, -17.85) in
    /// `hud_plC_button`); the hit test is the hit records' bounds through the stage transforms.
    pub fn button_at(&self, st: &Stage, p: bevy::math::Vec2) -> Option<usize> {
        self.buttons.iter().position(|&b| {
            let Some(hit) = st.child(b, "btn") else { return false };
            if !st.visible(b) || !st.visible(hit) {
                return false;
            }
            let Some(n) = st.node(hit) else { return false };
            let Some(mv) = st.movies.get(n.src).map(|m| &m.movie) else { return false };
            let Some(recs) = mv.buttons.get(&n.character) else { return false };
            let world = st.world_matrix(hit);
            recs.iter().filter(|r| r.states & 8 != 0).any(|r| {
                let Some(sh) = mv.shapes.get(&r.character) else { return false };
                let m = world.mul(&r.matrix);
                let c = [(sh.bounds.xmin, sh.bounds.ymin), (sh.bounds.xmax, sh.bounds.ymin), (sh.bounds.xmax, sh.bounds.ymax), (sh.bounds.xmin, sh.bounds.ymax)].map(|(x, y)| m.apply(x, y));
                let (x0, x1) = c.iter().fold((f32::MAX, f32::MIN), |a, q| (a.0.min(q.0), a.1.max(q.0)));
                let (y0, y1) = c.iter().fold((f32::MAX, f32::MIN), |a, q| (a.0.min(q.1), a.1.max(q.1)));
                (x0..=x1).contains(&(p.x * 20.0)) && (y0..=y1).contains(&(p.y * 20.0))
            })
        })
    }

    /// The mouse (stage pixels; None off the stage): the movie gets a move when the position
    /// changed (the GFx engine's mouse event, 0x00974F80) and the left button's press / release.
    /// Entering a button runs `SelectionHandler.RollOver` (the selection with `Move`), a release
    /// over the button it went down on runs `Click` (the selection, then the "A" input's
    /// `APressed`).
    pub fn mouse(&mut self, st: &mut Stage, snd: &mut SoundQueue, p: Option<bevy::math::Vec2>, moved: bool, down: bool, up: bool) {
        let at = p.and_then(|p| self.button_at(st, p));
        if moved && at != self.hover {
            self.hover = at;
            if let Some(i) = at {
                self.roll_over(st, snd, i);
            }
        }
        if down {
            self.press = at;
        }
        if up {
            if let Some(i) = self.press.take().filter(|&i| at == Some(i)) {
                self.click(st, snd, i);
            }
        }
    }

    /// `SelectionHandler.RollOver`.
    pub fn roll_over(&mut self, st: &mut Stage, snd: &mut SoundQueue, idx: usize) {
        if idx != self.selection && idx < self.buttons.len() && self.inputs {
            self.select(st, idx);
            snd.0.push("Move");
        }
    }

    /// `SelectionHandler.Click`: select (no sound), then the "A" input's target `APressed`.
    pub fn click(&mut self, st: &mut Stage, snd: &mut SoundQueue, idx: usize) {
        if !self.inputs || idx >= self.buttons.len() {
            return;
        }
        self.select(st, idx);
        self.a_pressed(st, snd);
    }

    fn play_selection_on(&mut self, st: &mut Stage, idx: usize) {
        let Some(b) = self.buttons.get(idx).copied() else { return };
        if let Some(a) = st.child(b, "_a_mc") {
            st.set_visible(a, choice_a_icon_visible(true, self.using_gamepad));
        }
        st.tween_end(b, false);
        st.tween_to(b, 0.25, &BTN_OVER, Ease::StrongOut, None);
        st.goto_and_play(b, "over");
        if let Some(c) = st.child(b, "circle_mc") {
            st.goto_and_play(c, "2");
        }
        if let Some(a) = self.indic.and_then(|i| st.child(i, "arche_mc")) {
            st.goto_and_play(a, "2");
        }
    }

    fn play_selection_out(&mut self, st: &mut Stage, idx: usize) {
        let Some(b) = self.buttons.get(idx).copied() else { return };
        st.tween_end(b, false);
        st.tween_to(b, 0.15, &BTN_OUT, Ease::StrongOut, None);
        if let Some(c) = st.child(b, "circle_mc") {
            st.goto_and_stop(c, "1");
        }
        st.goto_and_play(b, "out");
    }

    /// `APressed` -> `Confirm`: the inputs go (`RemoveInputs`), the button plays `valid`,
    /// `Validation` sounds, and 150 ms later `OnPlayerChoiceConfirm` reports the choice.
    pub fn confirm(&mut self, st: &mut Stage, snd: &mut SoundQueue, idx: usize) {
        if idx >= self.buttons.len() {
            return;
        }
        self.inputs = false;
        if idx != self.selection {
            self.select(st, idx);
        }
        if let Some(b) = self.buttons.get(idx).copied() {
            st.goto_and_play(b, "valid");
        }
        snd.0.push("Validation");
        interval(st, &mut self.interval_close, 150.0, key(W_CHOICE, PC_CONFIRM, idx as u32));
    }

    pub fn close(&mut self, st: &mut Stage) {
        self.inputs = false;
        clear(st, &mut self.interval_open);
        clear(st, &mut self.interval_close);
        if self.buttons.is_empty() {
            st.set_visible(self.mc, false);
            return;
        }
        self.close_button(st, 0);
    }

    fn close_button(&mut self, st: &mut Stage, idx: usize) {
        clear(st, &mut self.interval_close);
        if let Some(b) = self.buttons.get(idx).copied() {
            let x = st.get(b, Prop::X) - 30.0;
            st.tween_to(b, 0.2, &[(Prop::Alpha, 0.0), (Prop::YScale, 80.0), (Prop::XScale, 80.0), (Prop::X, x)], Ease::StrongOut, None);
        }
        if idx + 1 < self.buttons.len() {
            interval(st, &mut self.interval_close, 50.0, key(W_CHOICE, PC_CLOSE_BTN, idx as u32 + 1));
        } else {
            // OnClosed.
            st.set_visible(self.mc, false);
            if let Some(c) = self.container.take() {
                st.remove(c);
            }
            self.buttons.clear();
        }
    }

    fn on_event(&mut self, st: &mut Stage, code: u16, arg: u32) {
        match code {
            PC_OPEN_BTN => {
                self.interval_open = None;
                self.open_button(st, arg as usize);
            }
            PC_CLOSE_BTN => {
                self.interval_close = None;
                self.close_button(st, arg as usize);
            }
            PC_CONFIRM => {
                self.interval_close = None;
                self.confirmed = Some(arg as usize);
            }
            _ => {}
        }
    }
}

// =============================================================================================
// PlayerStatesIcons (hud_icPlayerStates, `playerStatesIcon_mc`)
// =============================================================================================

pub struct PlayerStatesIcons {
    pub mc: Id,
    bkgd: Option<Id>,
    ic: Option<Id>,
    interval_close: Option<u32>,
    pub icon: Option<&'static str>,
}

const PS_CLOSED: u16 = 1;

impl PlayerStatesIcons {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        Self { mc, bkgd: st.child(mc, "_bkgd_mc"), ic: None, interval_close: None, icon: None }
    }
    /// `SetIcon("crouch" | "slide" | "sneak" | null)`.
    pub fn set_icon(&mut self, st: &mut Stage, icon: Option<&'static str>) {
        if self.icon == icon {
            return;
        }
        self.icon = icon;
        clear(st, &mut self.interval_close);
        match icon {
            None => self.close(st),
            Some(name) => {
                if let Some(old) = self.ic.take() {
                    st.remove(old);
                }
                let d = st.next_depth(self.mc);
                self.ic = st.attach(self.mc, name, "_ic", d);
                set(st, self.ic, Prop::XScale, 85.0);
                set(st, self.ic, Prop::YScale, 85.0);
                self.open(st);
            }
        }
    }
    fn open(&mut self, st: &mut Stage) {
        st.set_visible(self.mc, true);
        tween_end(st, self.bkgd, false);
        set(st, self.bkgd, Prop::X, -10.0);
        set(st, self.bkgd, Prop::Rotation, 15.0);
        tween(st, self.bkgd, 0.35, &[(Prop::X, 0.0), (Prop::Y, 0.0), (Prop::Rotation, 0.0)], Ease::StrongOut, None);
        st.tween_end(self.mc, false);
        st.set(self.mc, Prop::Alpha, 0.0);
        st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
    }
    fn close(&mut self, st: &mut Stage) {
        goto_play(st, self.ic, "close");
        st.tween_end(self.mc, false);
        st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
        interval(st, &mut self.interval_close, 250.0, key(W_ICON, PS_CLOSED, 0));
    }
    fn on_event(&mut self, st: &mut Stage, code: u16) {
        if code == PS_CLOSED {
            self.interval_close = None;
            if let Some(i) = self.ic.take() {
                st.remove(i);
            }
            st.set_visible(self.mc, false);
        }
    }
}

// =============================================================================================
// BlackStripes (hud_blackStripes, `blackStripes_mc`): the cinematic bars
// =============================================================================================

pub struct BlackStripes {
    pub mc: Id,
    pub opened: bool,
    interval_id: Option<u32>,
}

const BS_CLOSED: u16 = 1;

/// `BlackStripes._stripeH`.
const STRIPE_H: f32 = 70.0;

impl BlackStripes {
    /// The constructor: hidden, transparent, placed (`SetStripesPosition`).
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        Self::set_stripes_position(st, mc);
        Self { mc, opened: false, interval_id: None }
    }
    /// `SetStripesPosition`: both bars as wide as the stage and `_stripeH + emptySpaceY` high,
    /// the upper one at `GetMovieSpaceCoord(1).coordMin._y`, the lower one at
    /// `_sceneH - _stripeH` (the 1280x720 stage at 16:9: no empty space).
    fn set_stripes_position(st: &mut Stage, mc: Id) {
        let (min, _) = crate::hud_wheel::movie_space(1.0);
        let stage = crate::gfx_stage::STAGE;
        if let Some(up) = st.child(mc, "_stripeUp_mc") {
            st.set(up, Prop::Y, min.y);
            st.set_height(up, STRIPE_H);
            st.set_width(up, stage.x);
        }
        if let Some(down) = st.child(mc, "_stripeDown_mc") {
            st.set(down, Prop::Y, stage.y - STRIPE_H);
            st.set_height(down, STRIPE_H);
            st.set_width(down, stage.x);
        }
    }
    /// `SetBlackStripes(displayState, useTween)` (`_stripeH` 70; at 16:9 the stripes are not
    /// widened by `emptySpaceY`).
    pub fn set(&mut self, st: &mut Stage, show: bool, use_tween: bool) {
        if show == self.opened {
            return;
        }
        clear(st, &mut self.interval_id);
        self.opened = show;
        match (show, use_tween) {
            (true, true) => {
                // `Open`: placed again, shown, faded in.
                Self::set_stripes_position(st, self.mc);
                st.set_visible(self.mc, true);
                st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
            }
            (true, false) => {
                st.set(self.mc, Prop::Alpha, 100.0);
                st.set_visible(self.mc, true);
            }
            (false, true) => {
                st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
                interval(st, &mut self.interval_id, 250.0, key(W_STRIPES, BS_CLOSED, 0));
            }
            (false, false) => {
                st.set(self.mc, Prop::Alpha, 0.0);
                st.set_visible(self.mc, false);
            }
        }
    }
    fn on_event(&mut self, st: &mut Stage, code: u16) {
        if code == BS_CLOSED {
            self.interval_id = None;
            st.set_visible(self.mc, false);
        }
    }
}

// =============================================================================================
// OxygenGauge (hud_oMeter, `oxygenGauge_mc`): the air left under water
// =============================================================================================

const W_OXYGEN: u16 = 17;
const OX_CLOSED: u16 = 1;
const OX_GLOW: u16 = 2;

pub struct OxygenGauge {
    pub mc: Id,
    bkgd: Option<Id>,
    gauge: Option<Id>,
    mask: Option<Id>,
    indic: Option<Id>,
    glow: Option<Id>,
    bkgd_def: glam::Vec2,
    gauge_def: (glam::Vec2, f32),
    pub value: f32,
    interval_close: Option<u32>,
    interval_glow: Option<u32>,
}

impl OxygenGauge {
    /// `_glowMinAlpha`, `_glowMaxAlpha`, `_glowDuration`, `_meterFillDuration`.
    const GLOW_MIN: f32 = 15.0;
    const GLOW_MAX: f32 = 30.0;
    const GLOW_DURATION: f32 = 2.25;
    const FILL_DURATION: f32 = 0.45;

    /// The constructor: hidden, transparent, the parts' placed positions kept.
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        let bkgd = st.child(mc, "_bkgd_mc");
        let gauge = st.child(mc, "_gauge_mc");
        let sub = |st: &Stage, n: &str| gauge.and_then(|g| st.child(g, n));
        let (mask, indic, glow) = (sub(st, "_mask_mc"), sub(st, "_indic_mc"), sub(st, "_glow_mc"));
        let bkgd_def = bkgd.map_or(glam::Vec2::ZERO, |b| glam::Vec2::new(st.get(b, Prop::X), st.get(b, Prop::Y)));
        let gauge_def = gauge.map_or((glam::Vec2::ZERO, 0.0), |g| (glam::Vec2::new(st.get(g, Prop::X), st.get(g, Prop::Y)), st.get(g, Prop::Rotation)));
        Self { mc, bkgd, gauge, mask, indic, glow, bkgd_def, gauge_def, value: 100.0, interval_close: None, interval_glow: None }
    }

    /// `FillOxygenGauge(oxygenValue)` (percent): the mask scaled to the value, the indicator at
    /// its top, shown while the air is not full.
    pub fn fill(&mut self, st: &mut Stage, oxygen: f32) {
        self.value = oxygen.max(0.1);
        let Some(mask) = self.mask else { return };
        let old = st.get(mask, Prop::YScale);
        st.set(mask, Prop::YScale, 100.0);
        let mask_h = st.bounds_in_parent(mask).map_or(0.0, |r| r.size().y);
        st.set(mask, Prop::YScale, old);
        let y = st.get(mask, Prop::Y) - mask_h * self.value / 100.0;
        st.set(mask, Prop::YScale, self.value);
        let alpha = if oxygen < 100.0 { 100.0 } else { 0.0 };
        if let Some(i) = self.indic {
            st.tween_end(i, false);
            st.set(i, Prop::Y, y);
            st.tween_to(i, Self::FILL_DURATION, &[(Prop::Alpha, alpha)], Ease::StrongOut, None);
        }
    }

    /// `Open`: the background and the gauge swing in (1.35 s), the clip fades in, the glow loops.
    pub fn open(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_close);
        st.set_visible(self.mc, true);
        if let Some(b) = self.bkgd {
            st.tween_end(b, false);
            st.set(b, Prop::X, 10.0);
            st.set(b, Prop::Rotation, 15.0);
            st.tween_to(b, 1.35, &[(Prop::Rotation, 0.0), (Prop::Y, self.bkgd_def.y), (Prop::X, self.bkgd_def.x)], Ease::StrongOut, None);
            st.goto_and_play(b, "loop");
        }
        if let Some(g) = self.gauge {
            st.set(g, Prop::Rotation, 25.0);
            st.set(g, Prop::X, 15.0);
            st.tween_to(g, 1.35, &[(Prop::Rotation, self.gauge_def.1), (Prop::Y, self.gauge_def.0.y), (Prop::X, self.gauge_def.0.x)], Ease::StrongOut, None);
        }
        st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
        self.glow_step(st);
    }

    /// `Close`: the glow stops, the clip fades out and hides after 250 ms (`OnClosed`).
    pub fn close(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_glow);
        st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
        interval(st, &mut self.interval_close, 250.0, key(W_OXYGEN, OX_CLOSED, 0));
    }

    /// `initGlowAnim`: the glow tweens towards the other end of 15..30 % every 2.25 s.
    fn glow_step(&mut self, st: &mut Stage) {
        if let Some(g) = self.glow {
            st.tween_end(g, false);
            let to = if st.get(g, Prop::Alpha) > Self::GLOW_MIN { Self::GLOW_MIN } else { Self::GLOW_MAX };
            st.tween_to(g, Self::GLOW_DURATION, &[(Prop::Alpha, to)], Ease::StrongOut, None);
        }
        interval(st, &mut self.interval_glow, Self::GLOW_DURATION * 1000.0, key(W_OXYGEN, OX_GLOW, 0));
    }

    fn on_event(&mut self, st: &mut Stage, code: u16) {
        match code {
            OX_CLOSED => {
                clear(st, &mut self.interval_close);
                if let Some(b) = self.bkgd {
                    st.goto_and_stop(b, "1");
                }
                st.set_visible(self.mc, false);
            }
            OX_GLOW => self.glow_step(st),
            _ => {}
        }
    }
}

// =============================================================================================
// InteractionGauge (hud_chGauge_, `interactionGauge_mc`; base InteractionGaugeBase): the hold
// gauges of a usable (kind 1) and of the choke (kind 2), driven by the HUD's 0x00BA5B90 (open),
// 0x00B94D40 (`SetGauge(value x 100)`) and 0x00B94C10 (`Close`)
// =============================================================================================

const W_IGAUGE: u16 = 18;
const IG_CLOSED: u16 = 1;
const IG_GLOW: u16 = 2;

pub struct InteractionGauge {
    pub mc: Id,
    gauge: Option<Id>,
    indic: Option<Id>,
    mask: Option<Id>,
    txt: Option<Id>,
    bkgd_txt: Option<Id>,
    glow: Option<Id>,
    gauge_def_x: f32,
    actionable: bool,
    pub open: bool,
    pub value: i32,
    interval_close: Option<u32>,
    interval_glow: Option<u32>,
}

impl InteractionGauge {
    /// `_minGlow_alpha`, `_maxGlow_alpha`, `_glowDuration`.
    const GLOW_MIN: f32 = 35.0;
    const GLOW_MAX: f32 = 85.0;
    const GLOW_DURATION: f32 = 0.3;

    pub fn new(st: &mut Stage, mc: Id) -> Self {
        let gauge = st.child(mc, "_gauge_mc");
        let sub = |st: &Stage, n: &str| gauge.and_then(|g| st.child(g, n));
        let s = Self {
            mc,
            gauge,
            indic: sub(st, "_indic_mc"),
            mask: sub(st, "_mask_mc"),
            txt: sub(st, "_txt_mc"),
            bkgd_txt: sub(st, "_bkgdTxt_mc"),
            glow: sub(st, "_glow_mc"),
            gauge_def_x: gauge.map_or(0.0, |g| st.get(g, Prop::X)),
            actionable: false,
            open: false,
            value: 0,
            interval_close: None,
            interval_glow: None,
        };
        st.set_visible(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        s
    }

    /// `SetGauge(gaugeValue)`: the gears (actionable gauges), the mask and the indicator go to
    /// frame `value + 1`.
    pub fn set_gauge(&mut self, st: &mut Stage, value: i32) {
        self.value = value;
        let frame = (value + 1).to_string();
        if self.actionable {
            for n in ["_gear0_mc", "_gear1_mc", "_gear2_mc"] {
                if let Some(g) = self.gauge.and_then(|g| st.child(g, n)) {
                    st.goto_and_stop(g, &frame);
                }
            }
        }
        for m in [self.mask, self.indic].into_iter().flatten() {
            st.goto_and_stop(m, &frame);
        }
    }

    /// `Open(btnTag, isActionnableGauge)`: shown at 115 % and settling (0.5 s), the gauge
    /// sliding in from 20 px left, the text background from 60 px right, the "HOLD <tag>" text,
    /// blue (actionable) or red colours, the glow pulse. `text` is the converted "HOLD <key>".
    pub fn open(&mut self, st: &mut Stage, text: &str, actionable: bool) {
        st.set_visible(self.mc, true);
        clear(st, &mut self.interval_close);
        clear(st, &mut self.interval_glow);
        self.actionable = actionable;
        self.open = true;
        self.set_gauge(st, 0);
        st.set(self.mc, Prop::XScale, 115.0);
        st.set(self.mc, Prop::YScale, 115.0);
        st.tween_end(self.mc, false);
        st.tween_to(self.mc, 0.5, &[(Prop::YScale, 100.0), (Prop::XScale, 100.0), (Prop::Alpha, 100.0)], Ease::StrongOut, None);
        if let Some(g) = self.gauge {
            st.tween_end(g, false);
            st.set(g, Prop::Alpha, 100.0);
            st.set(g, Prop::X, self.gauge_def_x - 20.0);
            st.set(g, Prop::XScale, 100.0);
            st.set(g, Prop::YScale, 100.0);
            st.tween_to(g, 0.25, &[(Prop::X, self.gauge_def_x)], Ease::BackOut, None);
        }
        if let Some(b) = self.bkgd_txt {
            st.set(b, Prop::X, 60.0);
            st.tween_to(b, 0.35, &[(Prop::X, 0.0)], Ease::BackOut, None);
        }
        // `SetText` (PC: "HOLD <tag>"), on the text and its glow copy.
        for path in ["txt_mc.txt", "txtGlow_mc.txt"] {
            if let Some(t) = self.txt.and_then(|m| st.find(m, path)) {
                st.set_html(t, text);
            }
        }
        if let Some(g) = self.gauge {
            st.goto_and_stop(g, if actionable { "blue" } else { "red" });
        }
        // `SetOffsetColor_Custom(alphaMul, r, g, b, 0)` on the glow, the text background and
        // the text glow.
        let (glow, bkgd, txt) = if actionable { ((1.0, [73.0, 166.0, 255.0]), (0.2, [23.0, 62.0, 229.0]), (1.0, [73.0, 166.0, 255.0])) } else { ((1.0, [229.0, 23.0, 43.0]), (0.2, [150.0, 9.0, 9.0]), (1.0, [229.0, 23.0, 43.0])) };
        let offset = |st: &mut Stage, id: Option<Id>, (a, c): (f32, [f32; 3])| {
            if let Some(id) = id {
                st.set_color(id, ColorTransform { mul: [0.0, 0.0, 0.0, a], add: [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, 0.0] });
            }
        };
        offset(st, self.glow, glow);
        offset(st, self.bkgd_txt, bkgd);
        offset(st, self.txt.and_then(|m| st.child(m, "txtGlow_mc")), txt);
        self.update_glow(st);
    }

    /// `Close`: grows to 130 % while fading (0.25 s), hidden 250 ms later (`OnClosed`).
    pub fn close(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_glow);
        clear(st, &mut self.interval_close);
        self.open = false;
        st.tween_to(self.mc, 0.25, &[(Prop::YScale, 130.0), (Prop::XScale, 130.0), (Prop::Alpha, 0.0)], Ease::StrongOut, None);
        interval(st, &mut self.interval_close, 250.0, key(W_IGAUGE, IG_CLOSED, 0));
    }

    /// `UpdateGlow`: the glow and the text glow tween to the other end of 35..85 % every 0.3 s.
    fn update_glow(&mut self, st: &mut Stage) {
        let txt_glow = self.txt.and_then(|m| st.child(m, "txtGlow_mc"));
        for g in [self.glow, txt_glow].into_iter().flatten() {
            st.tween_end(g, true);
        }
        let to = if self.glow.is_some_and(|g| st.get(g, Prop::Alpha) > Self::GLOW_MIN) { Self::GLOW_MIN } else { Self::GLOW_MAX };
        for g in [self.glow, txt_glow].into_iter().flatten() {
            st.tween_to(g, Self::GLOW_DURATION, &[(Prop::Alpha, to)], Ease::StrongOut, None);
        }
        interval(st, &mut self.interval_glow, Self::GLOW_DURATION * 1000.0, key(W_IGAUGE, IG_GLOW, 0));
    }

    fn on_event(&mut self, st: &mut Stage, code: u16) {
        match code {
            IG_CLOSED => {
                clear(st, &mut self.interval_close);
                st.set_visible(self.mc, false);
            }
            IG_GLOW => self.update_glow(st),
            _ => {}
        }
    }
}

// =============================================================================================
// Crosshair (`_dot_mc`, `_aspect_mc`: Crosshair / Crosshair_Gun / Crosshair_Crossbow)
// =============================================================================================

pub struct Crosshairs {
    pub dot: Option<Id>,
    pub aspect: Option<Id>,
    pub aspect_name: Option<String>,
    dot_state: Option<u8>,
    aspect_state: Option<u8>,
    sent_dispersion: Option<f32>,
}

impl Crosshairs {
    pub fn new(st: &mut Stage) -> Self {
        let dot = st.child(st.root, "_dot_mc");
        Self { dot, aspect: None, aspect_name: None, dot_state: None, aspect_state: None, sent_dispersion: None }
    }

    /// `SetCrosshairState(state)` on the dot (`gotoAndStop(state + 1)`; 0 default, 1 empty,
    /// 2 over).
    pub fn set_dot_state(&mut self, st: &mut Stage, state: u8) {
        if self.dot_state == Some(state) {
            return;
        }
        self.dot_state = Some(state);
        goto_stop(st, self.dot, &(state as u32 + 1).to_string());
    }

    /// The native's crosshair update (0x00B9FF30): when the crosshair name changes, the old
    /// `_aspect_mc` is removed and the new symbol attached to `_root` at the next depth.
    pub fn set_aspect(&mut self, st: &mut Stage, name: Option<&str>) {
        if self.aspect_name.as_deref() == name {
            return;
        }
        if let Some(a) = self.aspect.take() {
            st.remove(a);
        }
        self.aspect_name = name.map(str::to_string);
        self.aspect_state = None;
        self.sent_dispersion = None;
        if let Some(n) = name {
            let root = st.root;
            let d = st.next_depth(root);
            self.aspect = st.attach(root, n, "_aspect_mc", d);
        }
    }

    pub fn set_aspect_state(&mut self, st: &mut Stage, state: u8) {
        if self.aspect_state == Some(state) {
            return;
        }
        self.aspect_state = Some(state);
        goto_stop(st, self.aspect, &(state as u32 + 1).to_string());
    }

    /// Place the dot and the aspect (stage pixels) with the crosshair opacity and visibility.
    pub fn place(&mut self, st: &mut Stage, pos: [f32; 2], dot_visible: bool, aspect_visible: bool, alpha: f32) {
        for (id, v) in [(self.dot, dot_visible), (self.aspect, aspect_visible)] {
            if let Some(id) = id {
                st.set(id, Prop::X, pos[0]);
                st.set(id, Prop::Y, pos[1]);
                st.set(id, Prop::Alpha, alpha);
                st.set_visible(id, v);
            }
        }
    }

    /// `SetDispersion(d, p)` of `Crosshair_Gun` / `Crosshair_Crossbow`, sent when `d` moved by
    /// 0.1 or more (0x00B9FF30). Gun: `_parts_int_mc._rotation = -5p/100`,
    /// `_parts_ext_mc._rotation = -20p/100`, ext alpha `100 - 1.5 max(0, p - 50)`, parts 0..3 to
    /// (d,-d), (d,d), (-d,d), (-d,-d) in both clips. Crossbow: `off = -15p/100`, ext alpha
    /// `100 - 1.5 max(0, p - 40)`, ext part0 y = -d-off, part1 x = d+off, part2 y = d+off,
    /// part3 x = -d-off, part4 x = d+off+5, part5 x = -d-off-5; int part0 y = -d, part1
    /// (d, -d), part2 y = d, part3 (-d, -d).
    pub fn set_dispersion(&mut self, st: &mut Stage, d: f32, p: f32) {
        if self.sent_dispersion.is_some_and(|s| (s - d).abs() < 0.1) {
            return;
        }
        self.sent_dispersion = Some(d);
        let Some(a) = self.aspect else { return };
        let gun = self.aspect_name.as_deref() == Some("crosshair_gun");
        let crossbow = self.aspect_name.as_deref() == Some("crosshair_crossbow");
        let ext = st.child(a, "_parts_ext_mc");
        let int = st.child(a, "_parts_int_mc");
        let part = |st: &Stage, c: Option<Id>, k: usize| c.and_then(|c| st.child(c, &format!("part{k}")));
        if gun {
            set(st, int, Prop::Rotation, -5.0 * p / 100.0);
            set(st, ext, Prop::Rotation, -20.0 * p / 100.0);
            set(st, ext, Prop::Alpha, 100.0 - 1.5 * (p - 50.0).max(0.0));
            let corners = [(d, -d), (d, d), (-d, d), (-d, -d)];
            for c in [ext, int] {
                for (k, (x, y)) in corners.iter().enumerate() {
                    let pt = part(st, c, k);
                    set(st, pt, Prop::X, *x);
                    set(st, pt, Prop::Y, *y);
                }
            }
        } else if crossbow {
            let off = -15.0 * p / 100.0;
            set(st, ext, Prop::Alpha, 100.0 - 1.5 * (p - 40.0).max(0.0));
            let e = |k| part(st, ext, k);
            let (e0, e1, e2, e3, e4, e5) = (e(0), e(1), e(2), e(3), e(4), e(5));
            set(st, e0, Prop::Y, -d - off);
            set(st, e1, Prop::X, d + off);
            set(st, e2, Prop::Y, d + off);
            set(st, e3, Prop::X, -d - off);
            set(st, e4, Prop::X, d + off + 5.0);
            set(st, e5, Prop::X, -d - off - 5.0);
            let i = |k| part(st, int, k);
            let (i0, i1, i2, i3) = (i(0), i(1), i(2), i(3));
            set(st, i0, Prop::Y, -d);
            set(st, i1, Prop::X, d);
            set(st, i1, Prop::Y, -d);
            set(st, i2, Prop::Y, d);
            set(st, i3, Prop::X, -d);
            set(st, i3, Prop::Y, -d);
        }
    }
}

// =============================================================================================
// Location (hud_location, `location_mc`): location discovery
// =============================================================================================

/// `SetLocation(name, duration)` (`DisSeqAct_ShowLocationDiscovery`, native 0x00B95160 with
/// `m_fLocationDiscoveryDuration` 5 s): the name as a big first letter, the middle part and a
/// big last letter (`hud_location_BigLetter` / `hud_location_letter`, 3.5 px overlap),
/// revealed by a mask widening from 10 px over 0.95 s; separators, tags and backdrop open
/// with it (0.65 s / 0.4 s `tweenFrom`), all fading out over 0.6 s at the end.
pub struct Location {
    pub mc: Id,
    sep_up: Option<Id>,
    sep_down: Option<Id>,
    tag_left: Option<Id>,
    tag_right: Option<Id>,
    bkgd: Option<Id>,
    container: Option<Id>,
    mask: Option<Id>,
    mask_w: f32,
    def_y: f32,
    duration: f32,
    nb_char: usize,
    pub opened: bool,
    interval_open: Option<u32>,
    interval_elements: Option<u32>,
    interval_location: Option<u32>,
    interval_close: Option<u32>,
}

const LOC_ELEMENTS: u16 = 1;
const LOC_LETTER: u16 = 2;
const LOC_CLOSE: u16 = 3;
const LOC_CLOSED: u16 = 4;

impl Location {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        let c = |st: &Stage, n: &str| st.child(mc, n);
        Self {
            mc,
            sep_up: c(st, "_sepUp_mc"),
            sep_down: c(st, "_sepDown_mc"),
            tag_left: c(st, "_tagLeft_mc"),
            tag_right: c(st, "_tagRight_mc"),
            bkgd: c(st, "_bkgd_mc"),
            container: None,
            mask: None,
            mask_w: 0.0,
            def_y: st.get(mc, Prop::Y),
            duration: 5.0,
            nb_char: 0,
            opened: false,
            interval_open: None,
            interval_elements: None,
            interval_location: None,
            interval_close: None,
        }
    }

    fn clear_container(&mut self, st: &mut Stage) {
        if let Some(c) = self.container.take() {
            st.remove(c);
        }
        if let Some(m) = self.mask.take() {
            st.remove(m);
        }
    }

    pub fn set_location(&mut self, st: &mut Stage, name: &str, duration: f32) {
        for iv in [&mut self.interval_open, &mut self.interval_elements, &mut self.interval_location, &mut self.interval_close] {
            clear(st, iv);
        }
        st.tween_end(self.mc, false);
        st.set(self.mc, Prop::Alpha, 100.0);
        self.opened = true;
        let chars: Vec<char> = name.chars().collect();
        self.nb_char = chars.len().min(3);
        self.duration = duration;
        // CreateContainer.
        self.clear_container(st);
        let d = st.next_depth(self.mc);
        let cont = st.create_empty(self.mc, "_location_mc", d);
        let d = st.next_depth(self.mc);
        self.mask = st.attach(self.mc, "hud_location_Mask", "_mask_mc", d);
        self.container = Some(cont);
        let mut prev: Option<Id> = None;
        let mut first: Option<Id> = None;
        for i in 0..self.nb_char {
            let middle = i > 0 && i + 1 < self.nb_char;
            let (export, text): (&str, String) = if middle {
                ("hud_location_letter", chars[1..chars.len() - 1].iter().collect())
            } else {
                let ch = if i == 0 { chars[0] } else { chars[chars.len() - 1] };
                ("hud_location_BigLetter", ch.to_string())
            };
            let d = st.next_depth(cont);
            let Some(l) = st.attach(cont, export, &format!("l{i}"), d) else { continue };
            if let Some(t) = st.child(l, "txt") {
                st.set_auto_size(t, AutoSize::Left);
                st.set_text(t, &text.to_uppercase());
            }
            st.set(l, Prop::Y, 0.0);
            if let Some(p) = prev {
                let pw = st.child(p, "txt").map(|t| st.width(t)).unwrap_or(0.0);
                let x = st.get(p, Prop::X) + pw - 3.5;
                st.set(l, Prop::X, x);
            }
            if first.is_none() {
                first = Some(l);
            }
            prev = Some(l);
        }
        let half_w = st.width(cont) / 2.0;
        self.mask_w = 2.0 * half_w + 20.0;
        st.set_mask(cont, self.mask);
        st.set(cont, Prop::Alpha, 0.0);
        for l in st.children(cont) {
            let x = st.get(l, Prop::X) - half_w;
            st.set(l, Prop::X, x);
        }
        let last_w = prev.map(|p| st.width(p)).unwrap_or(0.0);
        let first_w = first.map(|f| st.width(f)).unwrap_or(0.0);
        if let Some(s) = self.sep_up {
            st.tween_end(s, false);
            st.set(s, Prop::Alpha, 100.0);
            st.set(s, Prop::X, half_w - last_w);
            st.set_visible(s, false);
        }
        if let Some(s) = self.sep_down {
            st.tween_end(s, false);
            st.set(s, Prop::Alpha, 100.0);
            st.set(s, Prop::X, -half_w + first_w);
            st.set_visible(s, false);
        }
        for (t, x) in [(self.tag_left, -half_w - 2.5), (self.tag_right, half_w + 2.5)] {
            if let Some(t) = t {
                st.tween_end(t, false);
                st.set(t, Prop::X, x);
                st.set(t, Prop::Alpha, 100.0);
                st.set_visible(t, false);
            }
        }
        st.set_visible(self.mc, true);
        // SetBkgd.
        if let Some(b) = self.bkgd {
            let w = st.width(cont) + 2.0 * self.tag_left.map(|t| st.width(t)).unwrap_or(0.0);
            st.tween_end(b, false);
            st.set(b, Prop::Alpha, 100.0);
            st.set_width(b, w + 50.0);
            st.set_visible(b, false);
        }
        // Open.
        st.set(self.mc, Prop::Y, self.def_y);
        st.tween_end(self.mc, false);
        st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
        interval(st, &mut self.interval_elements, self.nb_char as f32 * 15.0, key(W_LOC, LOC_ELEMENTS, 0));
        interval(st, &mut self.interval_location, self.duration * 1000.0, key(W_LOC, LOC_CLOSE, 0));
    }

    fn open_elements(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_elements);
        visible(st, self.sep_up, true);
        if let Some(s) = self.sep_up {
            st.tween_from(s, 0.65, &[(Prop::X, -50.0), (Prop::Alpha, 0.0)], Ease::StrongOut, None);
        }
        visible(st, self.sep_down, true);
        if let Some(s) = self.sep_down {
            st.tween_from(s, 0.65, &[(Prop::X, 50.0), (Prop::Alpha, 0.0)], Ease::StrongOut, None);
        }
        for t in [self.tag_left, self.tag_right].into_iter().flatten() {
            st.tween_from(t, 0.65, &[(Prop::X, 0.0), (Prop::Alpha, 0.0)], Ease::StrongOut, None);
            st.set_visible(t, true);
            st.goto_and_play(t, "open");
        }
        if let Some(b) = self.bkgd {
            st.set_visible(b, true);
            st.tween_from(b, 0.4, &[(Prop::XScale, 30.0), (Prop::Alpha, 0.0)], Ease::StrongOut, None);
        }
        self.open_letter(st);
    }

    fn open_letter(&mut self, st: &mut Stage) {
        clear(st, &mut self.interval_open);
        if let Some(m) = self.mask {
            // `_width = 10`, then `tweenTo(0.95, {_width: _maskW})` (as an x scale).
            st.tween_end(m, false);
            let lw = st.local_bounds(m).map(|r| r.width()).unwrap_or(1.0).max(1e-3);
            st.set(m, Prop::XScale, 10.0 / lw * 100.0);
            st.tween_to(m, 0.95, &[(Prop::XScale, self.mask_w / lw * 100.0)], Ease::StrongOut, None);
        }
        if let Some(c) = self.container {
            st.tween_to(c, 0.35, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
        }
    }

    pub fn close(&mut self, st: &mut Stage) {
        self.opened = false;
        clear(st, &mut self.interval_location);
        clear(st, &mut self.interval_close);
        st.tween_end(self.mc, false);
        st.tween_to(self.mc, 0.6, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
        interval(st, &mut self.interval_close, 600.0, key(W_LOC, LOC_CLOSED, 0));
        for s in [self.sep_up, self.sep_down].into_iter().flatten() {
            st.tween_end(s, false);
            st.tween_to(s, 0.45, &[(Prop::X, 0.0)], Ease::BackOut, None);
        }
    }

    fn on_event(&mut self, st: &mut Stage, code: u16) {
        match code {
            LOC_ELEMENTS => {
                self.interval_elements = None;
                self.open_elements(st);
            }
            LOC_LETTER => {
                self.interval_open = None;
                self.open_letter(st);
            }
            LOC_CLOSE => {
                self.interval_location = None;
                self.close(st);
            }
            LOC_CLOSED => {
                self.interval_close = None;
                st.set_visible(self.mc, false);
                self.clear_container(st);
            }
            _ => {}
        }
    }
}

// =============================================================================================
// Awareness markers (`awarenessIndicator` in `markers_mc`)
// =============================================================================================

/// `Twk_GFxMoviePlayerHUD.m_AwarenessMarkerSpecificSettings` (`DisAwarenessMarkerSettings`) and
/// `m_AwarenessMarkerSettings` (research/dishonored/HUD.md, "Awareness markers").
pub mod awareness {
    /// `m_fDisplay_PlayerProxyMaxDist`: the NPC's idea of where the player is must be this close.
    pub const PLAYER_PROXY_MAX_DIST: f32 = 200.0;
    /// `m_fDisplay_NPCMaxDist_Vision` / `_NoVision`.
    pub const NPC_MAX_DIST: f32 = 10_000.0;
    /// `m_fFadeOutDelay_InitiallyHidden` / `_NowHidden`, `m_fFadeOutAnimDuration` (seconds).
    pub const FADE_DELAY_INITIAL: f32 = 0.0;
    pub const FADE_DELAY_NOW: f32 = 1.0;
    pub const FADE_ANIM: f32 = 3.0;
    /// `m_fMinScaleOutsideFOV` / `m_fMaxScaleOutsideFOV`.
    pub const MIN_SCALE_OUTSIDE: f32 = 50.0;
    pub const MAX_SCALE_OUTSIDE: f32 = 150.0;
    /// `m_ScaleVariation.m_EndValue` (start and end distance are both 0: constant).
    pub const SCALE_VALUE: f32 = 75.0;
    /// `m_AlphaVariation`: 100 from 200 uu away, down to 0 at 150 uu.
    pub const ALPHA_START_DIST: f32 = 200.0;
    pub const ALPHA_END_DIST: f32 = 150.0;
    /// `m_AllowedAIAwareness` (the `EAIAwareness` values of the tweak's six names: Unaware,
    /// AwareOfPlayer, Surprised, Suspicious, InCombat, Choked; Fearful and Begging are left out).
    pub const ALLOWED: [u8; 6] = [0, 1, 2, 3, 5, 7];
}

/// What the native side hands a marker each frame (the arguments of the update 0x00BBD630).
#[derive(Clone, Copy, Debug)]
pub struct MarkerInput {
    /// The NPC was rendered within the last 0.2 s (or passes a second visibility test).
    pub seen: bool,
    /// `EDisAttentionLevel` toward the player (0 when the NPC has none).
    pub level: u8,
    /// Where to draw it: None when it cannot be projected.
    pub place: Option<MarkerPlace>,
}

#[derive(Clone, Copy, Debug)]
pub struct MarkerPlace {
    /// Stage pixels (clamped to the marker area when off screen).
    pub pos: [f32; 2],
    pub offscreen: bool,
    /// Camera to the head, Dishonored units.
    pub distance: f32,
    /// `tan(fov / 2)` of the camera.
    pub tan_half_fov: f32,
}

/// One `DisAwarenessMarker` (vtable 0xD61FC0: init 0x00BC5B40, release 0x00BC5C10, tick
/// 0x00BB9FF0, update 0x00BBD630, chevrons 0x00BBA030): two halves `left_mc` / `right_mc`
/// (labels fadeIn, visible, quickFadeOut), each with chevrons `mc1..mc3` (fillIn, emptyOut,
/// full, empty, busted_1..3); frame 2 of the symbol is the off-screen look.
pub struct AwarenessMarker {
    pub mc: Option<Id>,
    left: Option<Id>,
    right: Option<Id>,
    chevrons: [[Option<Id>; 3]; 2],
    /// +0x30: the level last shown.
    pub level: u8,
    /// +0x31: faded in (not yet fading out).
    pub shown: bool,
    /// +0x32 / +0x34: the display timer and the fade-out timer.
    pub display_timer: f32,
    pub fade_timer: f32,
    /// +0x35: showing the off-screen look.
    offscreen: bool,
}

impl AwarenessMarker {
    pub fn new() -> Self {
        Self { mc: None, left: None, right: None, chevrons: [[None; 3]; 2], level: 0, shown: false, display_timer: 0.0, fade_timer: 0.0, offscreen: false }
    }

    /// The clip is attached on the first time it is wanted (vslot +4).
    fn init(&mut self, st: &mut Stage, layer: Id, name: &str) {
        let d = st.next_depth(layer);
        self.mc = st.attach(layer, "awarenessIndicator", name, d);
        let Some(mc) = self.mc else { return };
        self.left = st.child(mc, "left_mc");
        self.right = st.child(mc, "right_mc");
        for (h, half) in [self.left, self.right].into_iter().enumerate() {
            for k in 0..3 {
                self.chevrons[h][k] = half.and_then(|c| st.child(c, &format!("mc{}", k + 1)));
            }
        }
        // The halves wait on their first frame until "fadeIn".
        st.set_visible(mc, false);
    }

    /// `FUN_00bba030(level, old, animate)`: the chevrons of both halves.
    fn set_chevrons(&mut self, st: &mut Stage, level: u8, old: u8, animate: bool) {
        if level > 3 {
            if level == 4 {
                for half in 0..2 {
                    for k in 0..3 {
                        goto_stop(st, self.chevrons[half][k], &format!("busted_{}", k + 1));
                    }
                }
            }
            return;
        }
        let animate = animate && old <= 3;
        for k in 0..3usize {
            let full = (k as u8) < level;
            let was = (k as u8) < old && old < 4;
            for half in 0..2 {
                let c = self.chevrons[half][k];
                if animate {
                    if full != was {
                        goto_play(st, c, if full { "fillIn" } else { "emptyOut" });
                    }
                } else {
                    goto_stop(st, c, if full { "full" } else { "empty" });
                }
            }
        }
    }

    /// One frame (tick, then update). Returns true when the marker releases itself.
    pub fn update(&mut self, st: &mut Stage, layer: Id, name: &str, dt: f32, inp: &MarkerInput) -> bool {
        self.display_timer -= dt;
        self.fade_timer -= dt;
        let level = inp.level;
        if !inp.seen {
            if self.level < level {
                self.display_timer = awareness::FADE_DELAY_INITIAL;
            }
        } else {
            self.display_timer = awareness::FADE_DELAY_NOW;
        }
        let show = (1..=3).contains(&level) && self.display_timer > 0.0;
        let busted = level == 4 && self.level != 4;
        if self.mc.is_none() && (show || busted) {
            self.init(st, layer, name);
        }
        let mut release = false;
        if !show {
            if busted {
                goto_stop(st, self.left, "visible");
                goto_stop(st, self.right, "visible");
                self.set_chevrons(st, 4, self.level, true);
                self.shown = false;
                self.fade_timer = awareness::FADE_ANIM;
            } else if !self.shown {
                if self.mc.is_some() && self.fade_timer < 0.0 {
                    release = true;
                }
            } else {
                goto_play(st, self.left, "quickFadeOut");
                goto_play(st, self.right, "quickFadeOut");
                self.shown = false;
                self.fade_timer = awareness::FADE_ANIM;
            }
            self.level = level;
        } else if !self.shown {
            goto_play(st, self.left, "fadeIn");
            goto_play(st, self.right, "fadeIn");
            self.set_chevrons(st, level, self.level, false);
            self.shown = true;
            self.level = level;
        } else if level != self.level {
            self.set_chevrons(st, level, self.level, true);
            self.level = level;
        }
        if release {
            self.remove(st);
            return true;
        }
        let Some(mc) = self.mc else { return false };
        let Some(pl) = inp.place else {
            st.set_visible(mc, false);
            return false;
        };
        st.set_visible(mc, true);
        if pl.offscreen != self.offscreen {
            st.goto_and_stop(mc, &(pl.offscreen as u32 + 1).to_string());
            self.offscreen = pl.offscreen;
        }
        // Scale: 500 / max(tan(fov / 2) x distance, 1) x the scale variation; alpha from the
        // alpha variation of the distance.
        let var = awareness::SCALE_VALUE;
        let mut scale = 500.0 / (pl.tan_half_fov * pl.distance).max(1.0) * var;
        let left = (pl.pos[0] - crate::gfx_stage::STAGE.x * 0.5) <= 0.0;
        let (rot, yscale) = if pl.offscreen {
            let (dx, dy) = (pl.pos[0] - crate::gfx_stage::STAGE.x * 0.5, pl.pos[1] - crate::gfx_stage::STAGE.y * 0.5);
            let len = (dx * dx + dy * dy).sqrt().max(1e-4);
            let mut a = (dy / len).clamp(-1.0, 1.0).acos().to_degrees();
            if !left {
                a = 360.0 - a;
            }
            scale = scale.clamp(awareness::MIN_SCALE_OUTSIDE, awareness::MAX_SCALE_OUTSIDE);
            (a + 90.0, if left { -scale } else { scale })
        } else {
            (0.0, scale)
        };
        let k = ((pl.distance - awareness::ALPHA_START_DIST) / (awareness::ALPHA_END_DIST - awareness::ALPHA_START_DIST)).clamp(0.0, 1.0);
        st.set(mc, Prop::X, pl.pos[0]);
        st.set(mc, Prop::Y, pl.pos[1]);
        st.set(mc, Prop::Rotation, rot);
        st.set(mc, Prop::XScale, scale);
        st.set(mc, Prop::YScale, yscale);
        st.set(mc, Prop::Alpha, 100.0 * (1.0 - k));
        false
    }

    pub fn remove(&mut self, st: &mut Stage) {
        if let Some(mc) = self.mc.take() {
            st.remove(mc);
        }
    }
}

impl Default for AwarenessMarker {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================================
// HUDFX movie (UI_HUDFX.HUDFX, `DisGFxMoviePlayerHUDFX`): BlackShroud, DirectionalDamages
// =============================================================================================

const W_SHROUD: u16 = 20;
const W_DAMAGE: u16 = 21;

/// `BlackShroud` (`blackShroud_mc`): the stealth shroud, open while the player sneaks
/// (0x00B96050 -> 0x00B8D490); 0.35 s fades (`Strong.easeOut`), hidden when closed.
pub struct BlackShroud {
    pub mc: Id,
    pub opened: bool,
    interval_close: Option<u32>,
    /// Driven by a keyhole peek (its black fade), not the stealth shroud.
    peeking: bool,
}

impl BlackShroud {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        // The HUD layout (0x00B96250, on every viewport change) sizes the clip to the stage's
        // extent on screen (`_width` / `_height`): its 512 x 512 vignette (`HUDFX_I29`, black with
        // the shroud in its alpha) covers the whole view; the stage shows all of 1280 x 720.
        st.set_width(mc, crate::gfx_stage::STAGE.x);
        st.set_height(mc, crate::gfx_stage::STAGE.y);
        Self { mc, opened: false, interval_close: None, peeking: false }
    }
    /// A keyhole peek's black fade (0..1) over the screen (StatePlayerMasterHolePeeking,
    /// 0x00A72D50 / 0x00A63400: the camera fades to black, the view switches, the fade clears);
    /// None gives the shroud back to the stealth state.
    pub fn peek(&mut self, st: &mut Stage, fade: Option<f32>) {
        match fade {
            Some(a) => {
                clear(st, &mut self.interval_close);
                self.peeking = true;
                self.opened = true;
                st.set_visible(self.mc, a > 0.0);
                st.set(self.mc, Prop::Alpha, a.clamp(0.0, 1.0) * 100.0);
            }
            None if self.peeking => {
                self.peeking = false;
                self.opened = false;
                st.set_visible(self.mc, false);
                st.set(self.mc, Prop::Alpha, 0.0);
            }
            None => {}
        }
    }
    pub fn set(&mut self, st: &mut Stage, open: bool) {
        if self.peeking {
            return;
        }
        if open == self.opened {
            return;
        }
        self.opened = open;
        clear(st, &mut self.interval_close);
        if open {
            st.set_visible(self.mc, true);
            st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 100.0)], Ease::StrongOut, None);
        } else {
            st.tween_to(self.mc, 0.35, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
            interval(st, &mut self.interval_close, 350.0, key(W_SHROUD, 1, 0));
        }
    }
    fn on_event(&mut self, st: &mut Stage) {
        self.interval_close = None;
        st.set_visible(self.mc, false);
    }
}

/// `DirectionalDamages` (`directionalDamages_mc`): `TakeDamage(hitAngle, damageIntensity,
/// screenW, screenH)` places a pooled `dirHitSprite` (10) and the `_indic_mc` arrow on the screen
/// edge towards the hit (`GetClippedPosition`, ratios 1 and 0.7), plays the hit (crack and
/// particles by intensity) and raises the red vignette (`_vignette_mc` alpha +
/// `0.01 i^2 + 0.4 i - 1`, 0.15 s, held `max(40 i, 2500)` ms then 3.5 s out). The hit fades
/// out over 1.2 s after `max(35 i, 1500)` ms.
pub struct DirectionalDamages {
    pub mc: Id,
    vignette: Option<Id>,
    indic: Option<Id>,
    vignette_alpha: f32,
    hits: Vec<Id>,
    all_hits: Vec<Id>,
    interval_indic: Option<u32>,
    interval_vignette: Option<u32>,
    interval_h: Vec<Option<u32>>,
}

/// `_maxHitSprites`.
pub const MAX_HIT_SPRITES: usize = 10;

const DD_HIDE_INDIC: u16 = 1;
const DD_HIDE_HIT: u16 = 2;
const DD_HIT_HIDDEN: u16 = 3;
const DD_HIDE_VIGNETTE: u16 = 4;

/// `GetClippedPosition(hitAngle, ratio)`: the edge point (x right, y up, in half-screen units)
/// for a hit from `hitAngle` degrees (0 ahead, 90 right).
pub fn clipped_position(angle: f32, ratio: f32) -> [f32; 2] {
    let (s, c) = angle.to_radians().sin_cos();
    let r4 = c.abs();
    let r5 = if r4 > 0.0 { s / r4 } else { 1.0 };
    if r5.abs() < 1.0 {
        [r5 * ratio, if c < 0.0 { -ratio } else { ratio }]
    } else {
        let r6 = s.abs();
        let r9 = if r6 > 0.0 { c / r6 } else { 1.0 };
        [if s < 0.0 { -ratio } else { ratio }, r9 * ratio]
    }
}

impl DirectionalDamages {
    pub fn new(st: &mut Stage, mc: Id) -> Self {
        let vignette = st.child(mc, "_vignette_mc");
        if let Some(v) = vignette {
            st.set(v, Prop::Alpha, 0.0);
        }
        if let Some(l) = st.child(mc, "_line_mc") {
            st.set_visible(l, false);
        }
        let mut hits = Vec::new();
        for i in 0..MAX_HIT_SPRITES {
            let d = st.next_depth(mc);
            if let Some(h) = st.attach(mc, "dirHitSprite", &format!("h{i}"), d) {
                st.set_visible(h, false);
                hits.push(h);
            }
        }
        Self { mc, vignette, indic: st.child(mc, "_indic_mc"), vignette_alpha: 0.0, all_hits: hits.clone(), hits, interval_indic: None, interval_vignette: None, interval_h: vec![None; MAX_HIT_SPRITES] }
    }

    fn idx(&self, h: Id) -> usize {
        self.all_hits.iter().position(|x| *x == h).unwrap_or(0)
    }

    pub fn take_damage(&mut self, st: &mut Stage, angle: f32, intensity: f32, w: f32, h: f32) {
        if self.hits.is_empty() {
            return;
        }
        clear(st, &mut self.interval_indic);
        let p = clipped_position(angle, 1.0);
        let pi = clipped_position(angle, 0.7);
        let dur = (intensity * 35.0).max(1500.0);
        if let Some(i) = self.indic {
            st.set(i, Prop::X, pi[0] * w * 0.5);
            st.set(i, Prop::Y, pi[1] * h * -0.5);
            st.set(i, Prop::Rotation, angle);
            st.goto_and_play(i, "hit");
        }
        interval(st, &mut self.interval_indic, dur + 150.0, key(W_DAMAGE, DD_HIDE_INDIC, 0));
        let Some(mc) = self.hits.pop() else { return };
        for n in ["big_glassBreak_mc", "hit0_mc", "hit1_mc"] {
            if let Some(c) = st.child(mc, n) {
                st.set_visible(c, false);
            }
        }
        st.set(mc, Prop::X, p[0] * w * 0.5);
        st.set(mc, Prop::Y, p[1] * h * -0.5);
        st.set(mc, Prop::Alpha, 100.0);
        st.set(mc, Prop::Rotation, angle);
        let i = self.idx(mc);
        let k = key(W_DAMAGE, DD_HIDE_HIT, mc as u32);
        if let Some(iv) = self.interval_h.get_mut(i) {
            interval(st, iv, dur, k);
        }
        self.show_vignette(st, intensity);
        self.play_hit_anim(st, mc, intensity);
    }

    fn play_hit_anim(&mut self, st: &mut Stage, mc: Id, i: f32) {
        st.set_visible(mc, true);
        let x = st.get(mc, Prop::X);
        let y = st.get(mc, Prop::Y);
        let fx = if x < 0.0 { x + 100.0 } else { x - 100.0 };
        let fy = if y < 0.0 { y + 100.0 } else { y - 100.0 };
        st.tween_from(mc, 0.15, &[(Prop::Alpha, 0.0), (Prop::XScale, 150.0), (Prop::YScale, 140.0), (Prop::X, fx), (Prop::Y, fy)], Ease::StrongIn, None);
        if let Some(s) = st.child(mc, "small_glassBreak_mc") {
            st.goto_and_play(s, "shake");
        }
        let particles = st.child(mc, "particles_mc");
        let big = st.child(mc, "big_glassBreak_mc");
        if i < 20.0 {
            if let Some(h0) = st.child(mc, "hit0_mc") {
                st.set_visible(h0, true);
            }
            goto_play(st, particles, "hit0");
        } else if let Some(h1) = st.child(mc, "hit1_mc") {
            st.set_visible(h1, true);
        }
        if i > 20.0 {
            if let Some(b) = big {
                st.set_visible(b, true);
                st.set(b, Prop::Alpha, 0.0);
                st.tween_to(b, 0.15, &[(Prop::Alpha, 60.0), (Prop::XScale, 90.0), (Prop::YScale, 90.0)], Ease::StrongIn, None);
            }
            goto_play(st, particles, "hit1");
        }
        if i > 50.0 {
            goto_play(st, particles, "hit2");
        }
        if i > 70.0 {
            goto_play(st, big, "shine");
            goto_play(st, particles, "hit3");
        }
        if i > 90.0 {
            goto_play(st, big, "shine2");
            goto_play(st, particles, "hit3");
        }
    }

    fn show_vignette(&mut self, st: &mut Stage, i: f32) {
        clear(st, &mut self.interval_vignette);
        let Some(v) = self.vignette else { return };
        st.tween_end(v, false);
        let add = 0.01 * i * i + 0.4 * i - 1.0;
        self.vignette_alpha = (st.get(v, Prop::Alpha) + add).min(100.0);
        st.tween_to(v, 0.15, &[(Prop::Alpha, self.vignette_alpha)], Ease::StrongInOut, None);
        interval(st, &mut self.interval_vignette, (i * 40.0).max(2500.0), key(W_DAMAGE, DD_HIDE_VIGNETTE, 0));
    }

    fn on_event(&mut self, st: &mut Stage, code: u16, arg: u32) {
        let mc = arg as Id;
        match code {
            DD_HIDE_INDIC => {
                self.interval_indic = None;
                goto_play(st, self.indic, "close");
            }
            DD_HIDE_HIT => {
                let i = self.idx(mc);
                if let Some(iv) = self.interval_h.get_mut(i) {
                    *iv = None;
                }
                st.tween_to(mc, 1.2, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
                let k = key(W_DAMAGE, DD_HIT_HIDDEN, mc as u32);
                if let Some(iv) = self.interval_h.get_mut(i) {
                    interval(st, iv, 1200.0, k);
                }
            }
            DD_HIT_HIDDEN => {
                let i = self.idx(mc);
                if let Some(iv) = self.interval_h.get_mut(i) {
                    *iv = None;
                }
                st.set_visible(mc, false);
                self.hits.push(mc);
            }
            DD_HIDE_VIGNETTE => {
                self.interval_vignette = None;
                self.vignette_alpha = 0.0;
                if let Some(v) = self.vignette {
                    st.tween_to(v, 3.5, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
                }
            }
            _ => {}
        }
    }
}

/// The HUDFX movie's widgets.
pub struct HudFxWidgets {
    pub shroud: Option<BlackShroud>,
    pub damages: Option<DirectionalDamages>,
    /// The keyhole's frame: keyHole_mc and the two black rectangles beside it.
    keyhole: Vec<Id>,
    keyhole_up: bool,
}

impl HudFxWidgets {
    pub fn new(st: &mut Stage) -> Self {
        let root = st.root;
        let shroud = st.child(root, "blackShroud_mc").map(|m| BlackShroud::new(st, m));
        let damages = st.child(root, "directionalDamages_mc").map(|m| DirectionalDamages::new(st, m));
        // `KeyHole` (keyhole spying): hidden as its constructor leaves it, shown while the eye is
        // at a keyhole (`show_keyhole`).
        let mut keyhole = Vec::new();
        for n in ["keyHole_mc", "_black_rectangle0_mc", "_black_rectangle1_mc"] {
            if let Some(m) = st.child(root, n) {
                st.set_visible(m, false);
                keyhole.push(m);
            }
        }
        Self { shroud, damages, keyhole, keyhole_up: false }
    }
    /// The keyhole frame (the keyhole shape with a black rectangle on each side) over the view
    /// while the player peeks, and the peek's black fade over it (`fade` 0..1; None: not
    /// peeking). The frame comes up once the screen has gone black (the view switches there,
    /// 0x00A63400) and goes with the peek.
    pub fn show_keyhole(&mut self, st: &mut Stage, fade: Option<f32>) {
        let up = fade.is_some() && (self.keyhole_up || fade.is_some_and(|f| f >= 1.0));
        if up != self.keyhole_up {
            self.keyhole_up = up;
            for m in &self.keyhole {
                st.set_visible(*m, up);
            }
        }
        if let Some(s) = self.shroud.as_mut() {
            s.peek(st, fade);
        }
    }
    pub fn on_event(&mut self, st: &mut Stage, k: u64) {
        let (w, code, arg) = split(k);
        match w {
            W_SHROUD => {
                if let Some(s) = self.shroud.as_mut() {
                    s.on_event(st)
                }
            }
            W_DAMAGE => {
                if let Some(d) = self.damages.as_mut() {
                    d.on_event(st, code, arg)
                }
            }
            _ => {}
        }
    }
}

// =============================================================================================
// All widgets of the HUD movie
// =============================================================================================

pub struct HudWidgets {
    pub objectives: Option<ObjectivesNotification>,
    pub target: Option<TargetNotification>,
    pub pickup_log: Option<PickupLog>,
    pub gauges: Option<PlayerGauges>,
    pub crosshair_info: Option<CrosshairInfosText>,
    /// `gameMsg_mc` (bottom centre) and `interactions_txt_mc` (left).
    pub game_msg: Option<InteractionsMessage>,
    pub interactions: Option<InteractionsMessage>,
    pub interaction_window: Option<InteractionsWindow>,
    /// `interactions_ic_mc` / `interactions_icQTE_mc` (the Special / SpecialQTE groups).
    pub interaction_icons: Option<InteractionsIcons>,
    pub interaction_qte: Option<InteractionsIcons>,
    pub subtitles: Option<Subtitles>,
    pub choice: Option<PlayerChoice>,
    pub state_icon: Option<PlayerStatesIcons>,
    pub tutorial: Option<TutorialMessage>,
    pub tutorial_window: Option<TutorialWindow>,
    pub stripes: Option<BlackStripes>,
    pub location: Option<Location>,
    pub oxygen: Option<OxygenGauge>,
    pub interaction_gauge: Option<InteractionGauge>,
    pub crosshairs: Crosshairs,
    pub sounds: SoundQueue,
}

impl HudWidgets {
    /// Construct the classes on their root clips (as `registerClass` does on placement).
    pub fn new(st: &mut Stage) -> Self {
        let root = st.root;
        let c = |st: &Stage, n: &str| st.child(root, n);
        let objectives = c(st, "objectives_mc").map(|m| ObjectivesNotification::new(st, m));
        let target = c(st, "targetNotification_mc").map(|m| TargetNotification::new(st, m));
        let pickup_log = c(st, "pickupLog_mc").map(|m| PickupLog::new(st, m));
        let gauges = c(st, "masterHUD_mc").map(|m| PlayerGauges::new(st, m));
        let crosshair_info = c(st, "crosshairInfosTxt_mc").map(|m| CrosshairInfosText::new(st, m));
        let game_msg = c(st, "gameMsg_mc").map(|m| InteractionsMessage::new(st, m, 0));
        let interactions = c(st, "interactions_txt_mc").map(|m| InteractionsMessage::new(st, m, 1));
        let interaction_window = c(st, "interactions_window_mc").map(|m| InteractionsWindow::new(st, m));
        let interaction_icons = c(st, "interactions_ic_mc").map(|m| InteractionsIcons::new(st, m, false));
        let interaction_qte = c(st, "interactions_icQTE_mc").map(|m| InteractionsIcons::new(st, m, true));
        let subtitles = c(st, "subtitles_mc").map(|m| Subtitles::new(st, m));
        let choice = c(st, "playerChoice_mc").map(|m| PlayerChoice::new(st, m));
        let state_icon = c(st, "playerStatesIcon_mc").map(|m| PlayerStatesIcons::new(st, m));
        let tutorial = c(st, "tutorialMsg_mc").map(|m| TutorialMessage::new(st, m));
        let tutorial_window = c(st, "tutorialWindow_mc").map(|m| TutorialWindow::new(st, m));
        let stripes = c(st, "blackStripes_mc").map(|m| BlackStripes::new(st, m));
        let location = c(st, "location_mc").map(|m| Location::new(st, m));
        let oxygen = c(st, "oxygenGauge_mc").map(|m| OxygenGauge::new(st, m));
        let interaction_gauge = c(st, "interactionGauge_mc").map(|m| InteractionGauge::new(st, m));
        // Widgets the port does not drive yet start hidden, as their classes do.
        for n in ["skipCutScene_mc", "grenadeCooking_mc"] {
            if let Some(m) = c(st, n) {
                st.set_visible(m, false);
            }
        }
        let crosshairs = Crosshairs::new(st);
        Self { objectives, target, pickup_log, gauges, crosshair_info, game_msg, interactions, interaction_window, interaction_icons, interaction_qte, subtitles, choice, state_icon, tutorial, tutorial_window, stripes, location, oxygen, interaction_gauge, crosshairs, sounds: SoundQueue::default() }
    }

    /// Route a stage event to its widget.
    pub fn on_event(&mut self, st: &mut Stage, k: u64) {
        let (w, code, arg) = split(k);
        let snd = &mut self.sounds;
        match w {
            W_OBJ => {
                if let Some(o) = self.objectives.as_mut() {
                    o.on_event(st, snd, code, arg)
                }
            }
            W_TGT => {
                if let Some(o) = self.target.as_mut() {
                    o.on_event(st, code, arg)
                }
            }
            W_PLOG => {
                if let Some(o) = self.pickup_log.as_mut() {
                    o.on_event(st, code, arg)
                }
            }
            W_GAUGE => {
                if let Some(o) = self.gauges.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_CHINFO => {
                if let Some(o) = self.crosshair_info.as_mut() {
                    o.on_event(st, code)
                }
            }
            x if x == W_MSG => {
                if let Some(o) = self.game_msg.as_mut() {
                    o.on_event(st, code)
                }
            }
            x if x == W_MSG + 1 => {
                if let Some(o) = self.interactions.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_SUB => {
                if let Some(o) = self.subtitles.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_CHOICE => {
                if let Some(o) = self.choice.as_mut() {
                    o.on_event(st, code, arg)
                }
            }
            W_ICON => {
                if let Some(o) = self.state_icon.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_TUTO => {
                if let Some(o) = self.tutorial.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_TUTOWIN => {
                if let Some(o) = self.tutorial_window.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_STRIPES => {
                if let Some(o) = self.stripes.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_LOC => {
                if let Some(o) = self.location.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_OXYGEN => {
                if let Some(o) = self.oxygen.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_IGAUGE => {
                if let Some(o) = self.interaction_gauge.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_ICONS => {
                if let Some(o) = self.interaction_icons.as_mut() {
                    o.on_event(st, code)
                }
            }
            W_QTE => {
                if let Some(o) = self.interaction_qte.as_mut() {
                    o.on_event(st, code)
                }
            }
            _ => {}
        }
    }
}
