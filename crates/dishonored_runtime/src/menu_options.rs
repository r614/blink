//! The Options screen (`UI_OptionsMenu.OptionsMenu` in `Startup.upk`), shared by the
//! launcher and the pause menu, rebuilt from the movie's classes (`OptionsScreen` 244,
//! `OptionsList` 220, `TabsHandler` 222, `SubTabsHandler` 223, `OptionsLine` 226 and the
//! `_common.P_*` widgets 228-234; see `research/dishonored/MENUS.md`, "Options screen"):
//!
//! - layout: `options_mc` at the stage centre; title bar at (-545, -295) rotated -4 deg; main
//!   tabs (`lib_mTabs_tab_s`, 256 px on a 206 px step, centred) at (0, -210); sub tabs
//!   (`SubTab`, 50 px apart) at (0, -150); the list at (-450, -80) (-135 without sub tabs),
//!   lines every 48 px, 8 (9) visible, the widget at x 680; help bar `Options_main`;
//! - motion: every open / close / tab-swap / focus tween and stagger of those classes, the
//!   widgets' state timelines, the stepper's text slide, the slider's thumb tween;
//! - behaviour: steppers, booleans and sliders clamp at their ends (`Stop`), restore defaults
//!   asks `t_Q_RestoreSettings`, the video settings ask `t_Q_VideoSettings` after Accept.
//!
//! Secondary screens: video settings (`o_videoSettings`), brightness (`o_Options_gammaSetting`
//! over the `UI_Gamma` logos), controller scheme (`o_Options_padMapping`).

use crate::menu_anim::{Act, Anim, Ease, P};
use crate::menu_art::{DisplayList, FontKind, MovieArt, Props, States, TEXT, TEXT_DIM, px_matrix};
use crate::menu_audio::MenuSounds;
use crate::menu_ui::{MenuArt, MenuEvent, MenuNav, MenuTexts, SEC_BASE, SEC_MENU, SEC_TWK_MENU};
use crate::menu_widgets::{HelpBar, TitleBar, help_list, texts_any};
use crate::settings::GameSettings;
use bevy::prelude::*;
use dishonored_assets::swf::Matrix;
use dishonored_settings::{Category, Kind, OptionDef, Screen, Settings, SubCategory, Value};

/// Line pitch of `o_options_line` (48 px bkgd, `ItemsList.CreateElements`).
const LINE_H: f32 = 48.0;
/// `OptionsLine._widgetPosX`.
const WIDGET_X: f32 = 680.0;
/// Main tab width and step (`lib_mTabs_tab_s` 256 px, `_offsetTabX` -50).
const TAB_W: f32 = 256.0;
const TAB_STEP: f32 = 206.0;
/// Sub tab gap (`_offsetTabMax`, see MENUS.md).
const SUBTAB_GAP: f32 = 50.0;
/// `OptionsScreen._animCloseDuration` (the list's close duration, 0.5 s).
pub const CLOSE_SECS: f64 = 0.5;

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// Redraw only.
    Redraw,
    /// Settings changed (saved).
    Changed,
    /// The screen closes (its close animation keeps drawing for [`CLOSE_SECS`]).
    Close,
    /// Open a confirmation box: (purpose, question). The answer comes back through
    /// [`OptionsMenu::answer`].
    Ask(String, String),
}

/// The list currently shown or being hidden.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ListShow {
    cat: usize,
    sub: usize,
    /// Created at (the list's open tween starts here).
    t0: f64,
    /// Swap direction: +1 = 'r' (content enters from the right), -1 = 'l'.
    side: f32,
    /// Lines animate in (`Open`), or the list just fades in (`RefreshCategory`).
    lines: bool,
}

pub struct OptionsMenu {
    pub cat: usize,
    pub sub: usize,
    /// Pixel scroll of the list content (`_list_mc._y`).
    pub scroll: f32,
    pub secondary: Option<Screen>,
    /// Video settings being edited (applied on Accept).
    pub pending: Option<Settings>,
    /// Gamma when the brightness screen opened (B restores it).
    gamma_start: f32,
    /// Video settings before an Accept, while its question is open.
    video_restore: Option<dishonored_settings::Graphics>,
    /// Available resolutions (from the monitor).
    pub resolutions: Vec<(u32, u32)>,
    pub anim: Anim,
    opened: f64,
    /// `Close()` time: the screen keeps drawing its close for [`CLOSE_SECS`].
    pub closing: Option<f64>,
    list: Option<ListShow>,
    old_list: Option<(ListShow, f64)>,
    /// When the selection starts tracking the focus (`OnOpened` of the list).
    select_at: f64,
    focus: Option<String>,
    /// Stepper text slide: (option id, old text, side, time).
    slide: Option<(String, String, f32, f64)>,
    /// Sounds the scripts play later (`UpdateValue` after the text slide).
    sounds_at: Vec<(f64, &'static str)>,
    /// When the secondary screen opened.
    secondary_at: f64,
    /// Key capture of a binding row started at (the `bind_in` blink), and ended at (`bind_out`).
    bind_at: Option<(String, f64, Option<f64>)>,
    in_pause: bool,
}

impl Default for OptionsMenu {
    fn default() -> Self {
        Self {
            cat: 0,
            sub: 0,
            scroll: 0.0,
            secondary: None,
            pending: None,
            gamma_start: 2.2,
            video_restore: None,
            resolutions: Vec::new(),
            anim: Anim::new(7),
            opened: 0.0,
            closing: None,
            list: None,
            old_list: None,
            select_at: 0.0,
            focus: None,
            slide: None,
            sounds_at: Vec::new(),
            secondary_at: 0.0,
            bind_at: None,
            in_pause: false,
        }
    }
}

enum Row {
    Opt(&'static OptionDef),
    Bind(String),
}

impl Row {
    fn id(&self) -> String {
        match self {
            Row::Opt(o) => format!("opt:line:{}", o.id),
            Row::Bind(a) => format!("opt:bind:{a}"),
        }
    }
}

/// Stage matrix of the options screen origin (`options_mc` at the stage centre).
fn origin() -> Matrix {
    px_matrix(640.0, 360.0)
}

/// `x, y` then rotation `deg`.
fn place(x: f32, y: f32, deg: f32) -> Matrix {
    Props { x, y, rotation: deg, ..Props::default() }.matrix()
}

fn scaled(m: Matrix, s: f32) -> Matrix {
    m.mul(&Matrix { a: s, d: s, ..Matrix::default() })
}

impl OptionsMenu {
    pub fn category(&self) -> Category {
        Category::ALL[self.cat.min(Category::ALL.len() - 1)]
    }
    pub fn sub_category(&self) -> SubCategory {
        let subs = self.category().subs();
        subs[self.sub.min(subs.len() - 1)]
    }

    fn has_subs(cat: usize) -> bool {
        Category::ALL[cat.min(3)].subs().len() > 1
    }

    /// Visible lines (`_nbLinesView`): 8 with sub tabs, 9 without.
    fn view_lines(cat: usize) -> usize {
        if Self::has_subs(cat) { 8 } else { 9 }
    }

    fn list_y(cat: usize) -> f32 {
        if Self::has_subs(cat) { -80.0 } else { -135.0 }
    }

    /// Rows of a list: option definitions or bindings. The controller scheme is one link line
    /// (its value and `t_Modify_ControlsScheme`), as in the original.
    fn rows_of(cat: usize, sub: usize, s: &Settings) -> Vec<Row> {
        let subs = Category::ALL[cat.min(3)].subs();
        match subs[sub.min(subs.len() - 1)] {
            SubCategory::Keyboard => s.bindings.iter().map(|b| Row::Bind(b.action.clone())).collect(),
            sc => dishonored_settings::options_in(sc).filter(|o| o.id != "t_Modify_ControlsScheme").map(Row::Opt).collect(),
        }
    }

    fn rows(&self, s: &Settings) -> Vec<Row> {
        Self::rows_of(self.cat, self.sub, s)
    }

    /// `SetMenu()`: open the screen at `now` on its first tab.
    pub fn open(&mut self, art: &MenuArt, now: f64, in_pause: bool) {
        self.anim.clear();
        self.opened = now;
        self.closing = None;
        self.secondary = None;
        self.pending = None;
        self.cat = 0;
        self.sub = 0;
        self.scroll = 0.0;
        self.focus = None;
        self.slide = None;
        self.bind_at = None;
        self.in_pause = in_pause;
        self.old_list = None;
        let a = &mut self.anim;
        a.author("opt/bkgd", Props::default());
        a.now(now, "opt/bkgd", Act::Set(P::new().alpha(0.0)));
        a.now(now, "opt/bkgd", Act::To(P::new().alpha(100.0), 0.25, Ease::StrongOut));
        a.author("opt/screen", Props::default());
        if let Some(c) = art.get("common") {
            TitleBar::open(a, c, "opt/title", Props { x: -545.0, y: -295.0, rotation: -4.0, ..Props::default() }, now);
        }
        HelpBar::open(a, "opt/help", now);
        // Tabs: one every 50 ms from scale 200 (0.25 s Back.easeOut); then the shoulders.
        let n = Category::ALL.len();
        for i in 0..n {
            let k = format!("opt/tab/{i}");
            a.author(&k, Props::default());
            a.now(now, &k, Act::Set(P::new().visible(false)));
            let t = now + 0.05 * i as f64;
            a.at(t, &k, Act::Set(P::new().scale(200.0).visible(true)));
            a.at(t, &k, Act::To(if i == 0 { P::new().scale(100.0).y(-20.0) } else { P::new().scale(100.0).y(0.0) }, 0.25, Ease::BackOut));
            a.at(t, &k, Act::Play(if i == 0 { "selected" } else { "unselected" }.into()));
        }
        let tab_done = now + 0.05 * n as f64;
        a.author("opt/sh", Props::default());
        a.now(now, "opt/sh", Act::Set(P::new().alpha(0.0)));
        a.at(tab_done, "opt/sh", Act::Set(P::new().alpha(0.0).scale(230.0)));
        a.at(tab_done, "opt/sh", Act::To(P::new().alpha(100.0).scale(100.0), 0.25, Ease::BackOut));
        // `SetTabFocus(0)` -> the first list, created at once (`_bIsOpening`).
        self.open_subtabs(tab_done);
        self.show_list(tab_done, -1.0, true);
    }

    /// `OptionsSubTabsContainer.Open` for the current category (or `Close` without sub tabs).
    fn open_subtabs(&mut self, now: f64) {
        let a = &mut self.anim;
        a.author("opt/subc", Props::default());
        if !Self::has_subs(self.cat) {
            a.now(now, "opt/subc", Act::To(P::new().alpha(0.0), 0.2, Ease::StrongOut));
            return;
        }
        a.now(now, "opt/subc", Act::End);
        a.now(now, "opt/subc", Act::Set(P::new().alpha(100.0).visible(true)));
        a.author("opt/subc/_tabs_mc", Props::default());
        a.now(now, "opt/subc/_tabs_mc", Act::Set(P::new().alpha(0.0).x(250.0)));
        a.now(now, "opt/subc/_tabs_mc", Act::To(P::new().alpha(100.0).x(0.0), 0.3, Ease::StrongOut));
        a.author("opt/subc/_bkgd_mc", Props { x: -159.95, y: 5.25, alpha: 75.0, ..Props::default() });
        a.now(now, "opt/subc/_bkgd_mc", Act::Set(P::new().alpha(0.0).x(-159.95 - 250.0)));
        a.now(now, "opt/subc/_bkgd_mc", Act::To(P::new().alpha(75.0).x(-159.95), 0.35, Ease::StrongOut));
        for i in 0..Category::ALL[self.cat].subs().len() {
            let k = format!("opt/subtab/{i}");
            a.author(&k, Props::default());
            a.now(now, &k, Act::Play(if i == self.sub { "selected" } else { "unselected" }.into()));
        }
    }

    /// `OptionsList.Open(animSide)` (or the plain fade of `RefreshCategory`) at `t0`.
    fn show_list(&mut self, t0: f64, side: f32, lines: bool) {
        let show = ListShow { cat: self.cat, sub: self.sub, t0, side, lines };
        self.list = Some(show);
        self.scroll = 0.0;
        self.anim.cancel("opt/row/", t0);
        self.anim.author("opt/scroll", Props::default());
        self.anim.now(t0, "opt/scroll", Act::Set(P::new().y(0.0)));
        let n = Self::view_lines(self.cat);
        self.select_at = if lines { t0 + 0.015 * n as f64 } else { f64::INFINITY };
        self.focus = None;
    }

    /// `OptionsScreen.HideList(side)`: the shown list hides, the new one opens 150 ms later.
    fn swap_list(&mut self, now: f64, side: f32) {
        if let Some(l) = self.list.take() {
            self.old_list = Some((l, now));
        }
        self.show_list(now + 0.15, side, true);
    }

    /// `Close()` at `now`.
    pub fn close(&mut self, now: f64) {
        self.closing = Some(now);
        TitleBar::close(&mut self.anim, "opt/title", now);
        HelpBar::close(&mut self.anim, "opt/help", now);
        let a = &mut self.anim;
        a.now(now, "opt/bkgd", Act::To(P::new().x(-150.0).alpha(0.0), 0.25, Ease::StrongOut));
        a.now(now, "opt/subc", Act::To(P::new().alpha(0.0), 0.2, Ease::StrongOut));
        let n = Category::ALL.len();
        for i in 0..n {
            let k = format!("opt/tab/{i}");
            let t = now + 0.02 * (n - 1 - i) as f64;
            a.at(t, &k, Act::To(P::new().alpha(0.0).scale(120.0), 0.25, Ease::StrongOut));
        }
        a.now(now, "opt/sh", Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
    }

    /// The close animation is over (the screen can be dropped).
    pub fn closed(&self, now: f64) -> bool {
        self.closing.is_some_and(|t| now - t >= CLOSE_SECS)
    }

    /// Advance the screen's clock: queued script steps, delayed sounds, and focus changes
    /// (the line / widget `over` and `out` timelines). Returns whether anything still moves.
    pub fn tick(&mut self, nav: &MenuNav, sounds: &mut MenuSounds, now: f64) -> bool {
        self.anim.advance(now);
        self.sounds_at.retain(|(t, s)| {
            if *t <= now {
                sounds.play(s);
                false
            } else {
                true
            }
        });
        if self.closing.is_none() && now >= self.select_at {
            let f = nav.focus.clone().filter(|f| f.starts_with("opt:line:") || f.starts_with("opt:bind:") || f.starts_with("video:") || f.starts_with("gamma:"));
            if f != self.focus {
                if let Some(old) = self.focus.take() {
                    self.focus_out(&old, now);
                }
                if let Some(new) = &f {
                    self.focus_in(new, now);
                }
                self.focus = f;
            }
        }
        if let Some((_, t)) = self.old_list {
            if now - t > 0.35 {
                self.old_list = None;
            }
        }
        self.anim.busy(now, 1.0) || self.closing.is_some() || self.slide.is_some() || self.bind_at.is_some() || self.focus.is_some()
    }

    /// `playSelectionOn`: the line plays `over` and slides 20 px right; its widget `over`.
    fn focus_in(&mut self, id: &str, now: f64) {
        let k = format!("opt/row/{id}");
        self.anim.author(&k, Props::default());
        self.anim.now(now, &k, Act::Play("over".into()));
        self.anim.now(now, &k, Act::To(P::new().x(20.0).alpha(100.0), if id.starts_with("opt:bind:") { 0.2 } else { 0.25 }, Ease::StrongOut));
        let w = format!("opt/w/{id}");
        self.anim.now(now, &w, Act::Play("over".into()));
    }

    fn focus_out(&mut self, id: &str, now: f64) {
        let k = format!("opt/row/{id}");
        self.anim.now(now, &k, Act::Play("out".into()));
        self.anim.now(now, &k, Act::To(P::new().x(0.0), if id.starts_with("opt:bind:") { 0.2 } else { 0.25 }, Ease::StrongOut));
        let w = format!("opt/w/{id}");
        self.anim.now(now, &w, Act::Play("out".into()));
    }

    // -----------------------------------------------------------------------------------------
    // Drawing

    pub fn build(&self, dl: &mut DisplayList, menu: &MenuArt, t: &MenuTexts, gs: &GameSettings, nav: &MenuNav, now: f64) {
        let Some(art) = menu.get("options") else { return };
        let s = self.pending.as_ref().unwrap_or(&gs.s);
        let screen_a = self.anim.get("opt/screen", now).alpha / 100.0;
        // Backdrop: `_bkgdTexture_mc` slides in.
        if let Some((ch, m)) = art.root_child("options_mc") {
            let ovr = |p: &str| if p == "_bkgdTexture_mc" { self.anim.value("opt/bkgd", now).map(|v| Props { x: -641.6 + v.x, y: -360.65, ..v }) } else { None };
            dl.movie_fx(art, ch, m, &States::default(), screen_a, &ovr, false);
        }
        if let Some(c) = menu.get("common") {
            let title = t.t(SEC_BASE, "t_Options");
            let mut sub = DisplayList::default();
            TitleBar::draw(&mut sub, c, &menu.metrics, &self.anim, "opt/title", now, origin(), &title);
            fade(dl, sub, screen_a);
        }
        self.draw_tabs(dl, menu, t, nav, now, screen_a);
        // Lists: the hiding one, then the shown one.
        if let Some((old, t_hide)) = self.old_list {
            self.draw_list(dl, menu, t, s, nav, old, Some(t_hide), now, screen_a);
        }
        if let Some(l) = self.list {
            if now >= l.t0 {
                self.draw_list(dl, menu, t, s, nav, l, self.closing, now, screen_a);
            }
        }
        if let Some(c) = menu.get("common") {
            if self.secondary.is_none() {
                let entries: Vec<(&str, &str)> = help_list("Options_main").to_vec();
                HelpBar::draw(dl, c, &menu.metrics, t, &self.anim, "opt/help", now, &entries);
            }
        }
        match self.secondary {
            Some(Screen::Video) => self.build_video(dl, menu, art, t, s, nav, now),
            Some(Screen::Gamma) => self.build_gamma(dl, menu, art, t, s, nav, now),
            Some(Screen::GamepadScheme) => self.build_gamepad(dl, menu, art, t, s, nav, now),
            None => {}
        }
    }

    fn draw_tabs(&self, dl: &mut DisplayList, menu: &MenuArt, t: &MenuTexts, nav: &MenuNav, now: f64, screen_a: f32) {
        let (Some(art), Some(common)) = (menu.get("options"), menu.get("common")) else { return };
        let focus = nav.focus.as_deref().unwrap_or("");
        let tabs_m = origin().mul(&place(0.0, -210.0, -4.0));
        let n = Category::ALL.len();
        let w_all = TAB_W * n as f32 - (TAB_W - TAB_STEP) * (n as f32 - 1.0);
        if let Some(tab) = common.export("lib_mTabs_tab_s") {
            for (i, c) in Category::ALL.iter().enumerate() {
                let k = format!("opt/tab/{i}");
                let p = self.anim.get(&k, now);
                if !p.visible || p.alpha <= 0.0 {
                    continue;
                }
                let cx = -w_all * 0.5 + TAB_W * 0.5 + TAB_STEP * i as f32;
                let id = format!("opt:tab:{i}");
                let m = tabs_m.mul(&px_matrix(cx, 0.0)).mul(&Props { x: 0.0, ..p }.matrix());
                let (label, secs) = self.anim.timeline(&k, now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("default".into(), 0.0));
                // Hover plays `over` / `out` on tabs that are not selected.
                let label = if i != self.cat && focus == id && label == "unselected" { "over".to_string() } else { label };
                let st = States::default().with_timed("tab_mc", &label, secs).with_timed("tab_mc/glow_mc", "loop", (now - self.opened) as f32);
                let a = p.alpha / 100.0 * screen_a;
                dl.movie(common, tab, m, &st, a);
                let text = t.t(SEC_TWK_MENU, c.label_key()).to_uppercase();
                for f in DisplayList::fields(common, tab, m, &st) {
                    let mut f = f;
                    f.color.mul[3] *= a;
                    dl.field_text(common, &f, &text, None);
                }
                dl.item(&id, Rect::from_center_size(Vec2::from(tab_center(&m)), Vec2::new(200.0, 50.0)), false);
            }
        }
        // Shoulders (LB / RB): `lib_L/RShoulder_Multi` at -/+ (W/2 + 15).
        let sh = self.anim.get("opt/sh", now);
        for (name, x, btn) in [("lib_LShoulder_Multi", -(w_all * 0.5 + 15.0), "LB"), ("lib_RShoulder_Multi", w_all * 0.5 + 15.0, "RB")] {
            if let Some(ic) = common.export(name) {
                let m = scaled(tabs_m.mul(&px_matrix(x, 0.0)), sh.xscale / 100.0);
                dl.movie(common, ic, m, &States::all("PC"), sh.alpha / 100.0 * screen_a);
                dl.item(format!("opt:btn:{btn}"), Rect::from_center_size(Vec2::from(tab_center(&m)), Vec2::new(44.0, 44.0)), false);
            }
        }
        // Sub tabs.
        let subs = self.category().subs();
        let sc = self.anim.get("opt/subc", now);
        if subs.len() > 1 && sc.visible && sc.alpha > 0.0 {
            if let Some(cont) = art.export("o_OptionsScreen_SubTabs_") {
                let cm = origin().mul(&place(0.0, -150.0, -4.0));
                let a = sc.alpha / 100.0 * screen_a;
                let ovr = |p: &str| match p {
                    "_bkgd_mc" => self.anim.value("opt/subc/_bkgd_mc", now),
                    "_tabs_mc" => Some(self.anim.value("opt/subc/_tabs_mc", now).unwrap_or_default()),
                    "_tabs_mc/_LTrigger_mc" | "_tabs_mc/_RTrigger_mc" => None,
                    _ => None,
                };
                // Widths of the sub tabs (their text, `$TitleFont` 21 px upper-cased).
                let labels: Vec<String> = subs.iter().map(|s| t.t(SEC_TWK_MENU, s.label_key().unwrap_or("")).to_uppercase()).collect();
                let widths: Vec<f32> = labels.iter().map(|l| menu.metrics.field_width(FontKind::Title, l, 21.0)).collect();
                let total: f32 = widths.iter().sum::<f32>() + SUBTAB_GAP * (widths.len() as f32 - 1.0);
                let trig = total * 0.5 + 50.0;
                let ovr2 = |p: &str| match p {
                    "_tabs_mc/_LTrigger_mc" => Some(Props { x: -trig, xscale: 85.0, yscale: 85.0, ..Props::default() }),
                    "_tabs_mc/_RTrigger_mc" => Some(Props { x: trig, xscale: 85.0, yscale: 85.0, ..Props::default() }),
                    other => ovr(other),
                };
                dl.movie_fx(art, cont, cm, &States::all("PC"), a, &ovr2, false);
                let tabs_p = self.anim.get("opt/subc/_tabs_mc", now);
                let tm = cm.mul(&px_matrix(tabs_p.x, 0.0));
                let ta = a * tabs_p.alpha / 100.0;
                if let Some(st_ch) = art.export("SubTab") {
                    let mut x = -total * 0.5;
                    for (i, label) in labels.iter().enumerate() {
                        let cx = x + widths[i] * 0.5;
                        x += widths[i] + SUBTAB_GAP;
                        let k = format!("opt/subtab/{i}");
                        let id = format!("opt:sub:{i}");
                        let (lab, secs) = self.anim.timeline(&k, now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("default".into(), 0.0));
                        let lab = if i != self.sub && focus == id && lab == "unselected" { "over".to_string() } else { lab };
                        let m = tm.mul(&px_matrix(cx, 0.0));
                        let st = States::default().with_timed("tab_mc", &lab, secs);
                        dl.movie(art, st_ch, m, &st, ta);
                        for f in DisplayList::fields(art, st_ch, m, &st) {
                            let mut f = f;
                            f.color.mul[3] *= ta;
                            dl.field_text(art, &f, label, None);
                        }
                        dl.item(&id, Rect::from_center_size(Vec2::from(tab_center(&m)), Vec2::new(widths[i] + 10.0, 36.0)), false);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_list(&self, dl: &mut DisplayList, menu: &MenuArt, t: &MenuTexts, s: &Settings, nav: &MenuNav, l: ListShow, hide: Option<f64>, now: f64, screen_a: f32) {
        let Some(art) = menu.get("options") else { return };
        let n_view = Self::view_lines(l.cat);
        let rows = Self::rows_of(l.cat, l.sub, s);
        let keyboard = matches!(Category::ALL[l.cat].subs()[l.sub.min(Category::ALL[l.cat].subs().len() - 1)], SubCategory::Keyboard);
        // List clip alpha (`ShowList` 0.3 s) and the inner `_list_mc` (x from +-35, 0.25 s).
        let dt = (now - l.t0).max(0.0) as f32;
        let mut clip_a = Ease::StrongOut.at(dt / 0.3);
        let (mut inner_x, mut inner_a) = if l.lines { (l.side * 35.0 * (1.0 - Ease::StrongOut.at(dt / 0.25)), Ease::StrongOut.at(dt / 0.25)) } else { (0.0, 1.0) };
        let mut row_close: Option<f64> = None;
        if let Some(th) = hide {
            let ht = (now - th).max(0.0) as f32;
            let k = Ease::StrongOut.at(ht / 0.25);
            clip_a *= 1.0 - k;
            if self.closing == Some(th) {
                // Screen close: the inner list fades over 0.5 s without moving, lines fade
                // bottom first every 20 ms.
                inner_a *= 1.0 - Ease::StrongOut.at(ht / 0.5);
                row_close = Some(th);
            } else {
                // Tab swap: content moves away from the side the new list comes from.
                inner_x += -l.side * 25.0 * k;
                inner_a *= 1.0 - k;
            }
        }
        let a = clip_a * inner_a * screen_a;
        if a <= 0.002 {
            return;
        }
        let lm = origin().mul(&place(-450.0, Self::list_y(l.cat), -4.0)).mul(&px_matrix(inner_x, 0.0));
        let scroll = self.anim.get("opt/scroll", now).y;
        let mask_h = LINE_H * n_view as f32 + 1.0;
        let focus = nav.focus.as_deref().unwrap_or("");
        let line_export = if keyboard { "o_MappingList_item" } else { "o_options_line" };
        let Some(line_ch) = art.export(line_export) else { return };
        let count = rows.len().max(n_view);
        for i in 0..count {
            let y = 24.0 + LINE_H * i as f32 - scroll;
            let visible = y - 24.0 >= -0.5 && y + 24.0 <= mask_h + 0.5;
            let row = rows.get(i);
            let id = row.map(|r| r.id()).unwrap_or_else(|| format!("opt:ghost:{i}"));
            // Per-line open: x + 20 -> 0 and alpha 0 -> 100 in 0.25 s, 15 ms apart.
            let vi = ((y - 24.0) / LINE_H).round().max(0.0) as f32;
            let mut lx = 0.0;
            let mut la = 1.0;
            if l.lines {
                let st = l.t0 + 0.015 * vi as f64;
                let k = Ease::StrongOut.at(((now - st) as f32 / 0.25).max(0.0));
                if now < st {
                    la = 0.0;
                } else {
                    lx = 20.0 * (1.0 - k);
                    la = k;
                }
            }
            if let Some(v) = self.anim.value(&format!("opt/row/{id}"), now) {
                if now >= self.select_at {
                    lx = v.x;
                }
            }
            if let Some(th) = row_close {
                let st = th + 0.02 * (n_view as f64 - 1.0 - vi as f64).max(0.0);
                la *= 1.0 - Ease::StrongOut.at(((now - st) as f32 / 0.2).max(0.0));
            }
            if row.is_none() {
                // Ghost lines (`ghostItemN`): frame `disabled`, alpha 0.
                la = 0.0;
            }
            if !visible || la * a <= 0.002 {
                if let Some(r) = row {
                    if hide.is_none() {
                        dl.item(r.id(), Rect::new(-10000.0, -10000.0, -9999.0, -9999.0), true);
                    }
                }
                continue;
            }
            let rm = lm.mul(&px_matrix(lx, y));
            let (label, secs) = self.anim.timeline(&format!("opt/row/{id}"), now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("default".into(), 0.0));
            let mut st = States::default().with_timed("*", &label, secs).with("_bkgd_mc/mc", &format!("#{}", i % 3)).with("bkgd_mc/mc", &format!("#{}", i % 3));
            let capturing = nav.capturing.as_deref() == Some(id.as_str());
            if let (true, Some((bid, t0, end))) = (keyboard, &self.bind_at) {
                if *bid == id {
                    let (lab, since) = match end {
                        Some(e) => ("bind_out", (now - e) as f32),
                        None => ("bind_in", (now - t0) as f32),
                    };
                    st = st.with_timed("*", lab, since);
                }
            }
            let ra = la * a;
            dl.movie(art, line_ch, rm, &st, ra);
            match row {
                Some(Row::Opt(o)) => {
                    let fields = DisplayList::fields(art, line_ch, rm, &st);
                    if let Some(f) = fields.iter().find(|f| f.path.contains("_txt_mc")) {
                        let mut f = f.clone();
                        f.color.mul[3] *= ra;
                        dl.field_text(art, &f, &line_label(t, o), Some(TEXT));
                    }
                    self.draw_widget(dl, menu, art, t, s, o, &id, rm.mul(&px_matrix(WIDGET_X, 0.0)), ra, now, focus == id);
                }
                Some(Row::Bind(action)) => {
                    let key = s.bindings.iter().find(|b| &b.action == action).map(|b| b.key.as_str()).unwrap_or("None");
                    for f in DisplayList::fields(art, line_ch, rm, &st) {
                        let mut f = f;
                        f.color.mul[3] *= ra;
                        let text = if f.path.contains("widget") { t.key_label(key) } else { t.action_label(action) };
                        dl.field_text(art, &f, &text, None);
                    }
                    let _ = capturing;
                }
                None => {}
            }
            if hide.is_none() {
                if let Some(r) = row {
                    let (c0x, c0y) = rm.apply(0.0, -22.0 * 20.0);
                    let (c1x, c1y) = rm.apply(930.0 * 20.0, 22.0 * 20.0);
                    dl.item(r.id(), Rect::from_corners(Vec2::new(c0x, c0y) / 20.0, Vec2::new(c1x, c1y) / 20.0), true);
                }
            }
        }
        // Scroll bar (`lib_scrollBar`), left of the list when the content is taller than the mask.
        if hide.is_none() && rows.len() > n_view {
            if let Some(common) = menu.get("common") {
                if let Some(sb) = common.export("lib_scrollBar") {
                    let total = LINE_H * rows.len() as f32;
                    let track_h = mask_h - 19.35;
                    let lift_h = (track_h * mask_h / total).clamp(20.0, track_h);
                    let ratio = (scroll / (total - mask_h).max(1.0)).clamp(0.0, 1.0);
                    let sm = origin().mul(&place(-450.0, Self::list_y(l.cat), -4.0)).mul(&px_matrix(-25.0, 0.0));
                    let ovr = |p: &str| match p {
                        "_track_mc" => Some(Props { x: 0.0, y: 19.35, yscale: 100.0 * track_h / 268.35, ..Props::default() }),
                        "_arrowDown_mc" => Some(Props { x: 10.0, y: 19.35 + track_h, rotation: 90.0, ..Props::default() }),
                        _ => None,
                    };
                    dl.movie_fx(common, sb, sm, &States::default(), a, &ovr, true);
                    if let Some(lift) = common.export("lib_LS_scroll_Multi") {
                        let y = 19.35 + lift_h * 0.5 + (track_h - lift_h) * ratio;
                        dl.movie(common, lift, sm.mul(&px_matrix(10.0, y)), &States::all("PC"), a);
                    }
                }
            }
        }
    }

    /// One option's widget at `m`.
    #[allow(clippy::too_many_arguments)]
    fn draw_widget(&self, dl: &mut DisplayList, menu: &MenuArt, art: &MovieArt, t: &MenuTexts, s: &Settings, o: &OptionDef, id: &str, m: Matrix, a: f32, now: f64, focused: bool) {
        let wk = format!("opt/w/{id}");
        let (label, secs) = self.anim.timeline(&wk, now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("default".into(), 0.0));
        let mut st = States::default().with_timed("*", &label, secs);
        // Arrows: locked at the ends or without focus (`UpdateArrows`); focused ones loop.
        let at_min = |s: &Settings| matches!(position(s, o, &self.resolutions), Some((i, _)) if i == 0);
        let at_max = |s: &Settings| matches!(position(s, o, &self.resolutions), Some((i, n)) if i + 1 >= n);
        let arrows = |st: States| -> States {
            let l = if !focused || at_min(s) { "locked" } else { "over" };
            let r = if !focused || at_max(s) { "locked" } else { "over" };
            st.with_timed("_leftArrow_mc", l, secs).with_timed("_rightArrow_mc", r, secs)
        };
        let field_a = |f: &dishonored_assets::swf::TextPlacement| {
            let mut f = f.clone();
            f.color.mul[3] *= a;
            f
        };
        match o.kind {
            Kind::Bool => {
                let Some(ch) = art.export("OptionsStepperB_widget") else { return };
                let on = matches!(s.get(o.id), Some(Value::Bool(true)));
                st = arrows(st).with("btn0", if on { "unselected" } else { "selected" }).with("btn1", if on { "selected" } else { "unselected" }).with("bkgd0_mc", if on { "unselected" } else { "selected" }).with("bkgd1_mc", if on { "selected" } else { "unselected" });
                dl.movie(art, ch, m, &st, a);
                for f in DisplayList::fields(art, ch, m, &st) {
                    let text = if f.path.contains("btn0") { t.value_label("BP_False") } else { t.value_label("BP_True") };
                    dl.field_text(art, &field_a(&f), &text.to_uppercase(), None);
                }
                dl.item(format!("opt:half:{}:0", o.id), half_rect(&m, -141.0), false);
                dl.item(format!("opt:half:{}:1", o.id), half_rect(&m, 0.0), false);
            }
            Kind::Choice(_) => {
                let Some(ch) = art.export("OptionsStepper_widget") else { return };
                st = arrows(st);
                // `UpdateText`: the old text leaves (0.15 s), the new one enters 150 ms later (0.1 s).
                let mut txt = value_text(t, o, s, &self.resolutions);
                let mut tp: Option<Props> = None;
                if let Some((sid, old, side, t0)) = &self.slide {
                    if sid == o.id {
                        let dt = (now - t0) as f32;
                        if dt < 0.15 {
                            let k = Ease::StrongInOut.at(dt / 0.15);
                            txt = old.clone();
                            tp = Some(Props { x: 0.45 - side * 20.0 * k, alpha: 100.0 * (1.0 - k), ..Props::default() });
                        } else {
                            let k = Ease::BackOut.at((dt - 0.15) / 0.1);
                            tp = Some(Props { x: 0.45 + side * 20.0 * (1.0 - k), alpha: 100.0 * k.min(1.0), ..Props::default() });
                        }
                    }
                }
                let ovr = move |p: &str| if p == "_txt_mc" { tp } else { None };
                dl.movie_fx(art, ch, m, &st, a, &ovr, false);
                for f in DisplayList::fields_fx(art, ch, m, &st, &ovr) {
                    dl.field_text(art, &field_a(&f), &txt, Some(TEXT));
                }
                arrow_items(dl, &m, o.id);
            }
            Kind::Range { min, max, step } => {
                let Some(ch) = art.export("Slider_widget") else { return };
                st = arrows(st);
                let v = s.get(o.id).map(|v| v.as_f32()).unwrap_or(min);
                let ratio = ((v - min) / (max - min)).clamp(0.0, 1.0);
                let target = -138.0 + ratio * 276.0;
                let tx = self.anim.value(&format!("opt/thumb/{}", o.id), now).map(|p| p.x).unwrap_or(target);
                let ovr = move |p: &str| if p == "_thumb_mc" { Some(Props { x: tx, ..Props::default() }) } else { None };
                dl.movie_fx(art, ch, m, &st, a, &ovr, false);
                // The thumb shows `String(value)` only for integer ranges.
                let ints = min.fract() == 0.0 && max.fract() == 0.0 && step.fract() == 0.0;
                if ints {
                    for f in DisplayList::fields_fx(art, ch, m, &st, &ovr) {
                        dl.field_text(art, &field_a(&f), &format!("{}", v.round() as i64), None);
                    }
                }
                let (x0, y0) = m.apply(-138.0 * 20.0 - 520.0, 0.0);
                let (x1, y1) = m.apply(138.0 * 20.0 + 520.0, 0.0);
                dl.item(format!("opt:slider:{}", o.id), Rect::new(x0.min(x1) / 20.0, y0.min(y1) / 20.0 - 16.0, x0.max(x1) / 20.0, y0.max(y1) / 20.0 + 16.0), false);
                arrow_items(dl, &m, o.id);
            }
            Kind::Resolution => {
                let Some(ch) = art.export("DropDownMenu_widget") else { return };
                st = st.with_timed("_btn_mc", &label, secs);
                dl.movie(art, ch, m, &st, a);
                let txt = value_text(t, o, s, &self.resolutions);
                for f in DisplayList::fields(art, ch, m, &st) {
                    dl.field_text(art, &field_a(&f), &txt, Some(TEXT));
                }
                arrow_items(dl, &m, o.id);
            }
            Kind::Link(_) => {
                let Some(ch) = art.export("Link_widget") else { return };
                // Focus swaps the value for the action text (0.25 s each way).
                let k = Ease::StrongOut.at((secs / 0.25).min(1.0));
                let on = if label == "over" { k } else if label == "out" { 1.0 - k } else { 0.0 };
                let tp = Props { x: -130.9, alpha: 100.0 * on, ..Props::default() };
                let vp = Props { x: -0.05, alpha: 100.0 * (1.0 - on), ..Props::default() };
                let ovr = move |p: &str| match p {
                    "_txt_mc" => Some(tp),
                    "_txtValue_mc" => Some(vp),
                    _ => None,
                };
                let lst = if label == "over" { States::default().with_timed("*", "over", secs) } else { st.clone() };
                dl.movie_fx(art, ch, m, &lst, a, &ovr, false);
                let action = texts_any(t, o.id);
                let value = link_value(t, o, s);
                for f in DisplayList::fields_fx(art, ch, m, &lst, &ovr) {
                    let text = if f.path.contains("_txtValue_mc") { value.clone() } else { action.clone() };
                    dl.field_text(art, &field_a(&f), &text, None);
                }
            }
        }
        let _ = menu;
    }

    #[allow(clippy::too_many_arguments)]
    fn build_video(&self, dl: &mut DisplayList, menu: &MenuArt, art: &MovieArt, t: &MenuTexts, s: &Settings, nav: &MenuNav, now: f64) {
        let focus = nav.focus.as_deref().unwrap_or("");
        let Some((ch, m)) = art.root_child("videoSettings_mc") else { return };
        let p = self.anim.get("opt/video", now);
        let veil = self.anim.get("opt/veil", now).alpha / 100.0;
        // `_black_mc` veil (0 -> 90 %), then the panel (alpha 0, scale 120 -> 100).
        if let Some((b, bm)) = art.child(ch, "_black_mc") {
            dl.movie_solids(art, b, m.mul(&bm), &States::default(), veil);
        }
        let pm = m.mul(&Props { x: 0.0, y: 0.0, xscale: p.xscale, yscale: p.yscale, ..Props::default() }.matrix());
        let a = p.alpha / 100.0;
        // `InitWidget`: the bar below the 3 lines, the panel centred and turned -2.5 deg.
        let list_y = 57.9;
        let bar_y = list_y + 3.0 * LINE_H + 20.0;
        let wid_y = -(bar_y + 50.0 + 20.0) * 0.5;
        let ovr = move |q: &str| match q {
            "_black_mc" => Some(Props { visible: false, ..Props::default() }),
            "_widget_mc" => Some(Props { y: wid_y, rotation: -2.5, ..Props::default() }),
            "_widget_mc/buttonBar_mc" => Some(Props { y: bar_y, ..Props::default() }),
            _ => None,
        };
        dl.movie_fx(art, ch, pm, &States::default(), a, &ovr, false);
        let wm = pm.mul(&Props { y: wid_y, rotation: -2.5, ..Props::default() }.matrix());
        // Title: the link's name upper-cased.
        if let Some((tc, tm)) = art.child(art.child(ch, "_widget_mc").map(|c| c.0).unwrap_or(0), "title_mc") {
            for f in DisplayList::fields(art, tc, wm.mul(&tm), &States::default()) {
                let mut f = f;
                f.color.mul[3] *= a;
                dl.field_text(art, &f, &t.t(SEC_TWK_MENU, "m_SettingsSubMenu_VideoSettings").to_uppercase(), None);
            }
        }
        let lm = wm.mul(&px_matrix(-330.55, list_y));
        if let Some(line) = art.export("o_options_line_s") {
            for (i, o) in dishonored_settings::video_options().enumerate() {
                let id = format!("opt:line:{}", o.id);
                let k = format!("opt/row/{id}");
                let (label, secs) = self.anim.timeline(&k, now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("default".into(), 0.0));
                let rx = self.anim.value(&k, now).map(|v| v.x).unwrap_or(0.0);
                let rm = lm.mul(&px_matrix(rx, 24.0 + LINE_H * i as f32));
                let st = States::default().with_timed("*", &label, secs).with("_bkgd_mc/mc", &format!("#{}", i % 3));
                dl.movie(art, line, rm, &st, a);
                if let Some(f) = DisplayList::fields(art, line, rm, &st).iter().find(|f| f.path.contains("_txt_mc")) {
                    let mut f = f.clone();
                    f.color.mul[3] *= a;
                    dl.field_text(art, &f, &line_label(t, o), Some(TEXT));
                }
                self.draw_widget(dl, menu, art, t, s, o, &id, rm.mul(&px_matrix(400.0, 0.0)), a, now, focus == id);
                let (c0x, c0y) = rm.apply(0.0, -22.0 * 20.0);
                let (c1x, c1y) = rm.apply(620.0 * 20.0, 22.0 * 20.0);
                dl.item(&id, Rect::from_corners(Vec2::new(c0x, c0y) / 20.0, Vec2::new(c1x, c1y) / 20.0), true);
            }
        }
        // Accept / Cancel (`lib_A_Multi` + `t_Apply`, `lib_B_Multi` + `t_Cancel`).
        let bar = wm.mul(&px_matrix(0.0, bar_y));
        if let Some(btn) = art.child(art.export("o_videoSettings_buttonsBar").unwrap_or(0), "accept_mc").map(|c| c.0) {
            for (x, id, key, icon) in [(-154.0, "video:accept", "t_Apply", "lib_A_Multi"), (154.0, "video:cancel", "t_Cancel", "lib_B_Multi")] {
                let bm = bar.mul(&px_matrix(x, 24.0));
                let k = format!("opt/w/{id}");
                let (label, secs) = self.anim.timeline(&k, now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("default".into(), 0.0));
                let st = States::all_timed(&label, secs);
                dl.movie(art, btn, bm, &st, a);
                let text = texts_any(t, key).to_uppercase();
                for f in DisplayList::fields(art, btn, bm, &st) {
                    let mut f = f;
                    f.color.mul[3] *= a;
                    dl.field_text(art, &f, &text, None);
                }
                if let Some(common) = menu.get("common") {
                    if let Some(ic) = common.export(icon) {
                        let w = menu.metrics.width(FontKind::Normal, &text, 25.0);
                        dl.movie(common, ic, scaled(bm.mul(&px_matrix(-w * 0.5 - 26.0, 0.0)), 0.9), &States::all("PC"), a);
                    }
                }
                let (cx, cy) = bm.apply(0.0, 0.0);
                dl.item(id, Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(280.0, 44.0)), true);
            }
        }
        let _ = focus;
    }

    /// Brightness: the `UI_Gamma` logos at the five `m_BrightnessValues` behind the movie's
    /// `gammaSetting_mc` (no veil) with its slider line.
    /// APPROXIMATION: the menu UI is not gamma-corrected, so the logos preview the chosen gamma
    /// by raising their luminance to `2.2 / gamma`.
    #[allow(clippy::too_many_arguments)]
    fn build_gamma(&self, dl: &mut DisplayList, menu: &MenuArt, art: &MovieArt, t: &MenuTexts, s: &Settings, nav: &MenuNav, now: f64) {
        let focus = nav.focus.as_deref().unwrap_or("");
        let p = self.anim.get("opt/gamma", now);
        let a = p.alpha / 100.0;
        dl.fill(Rect::new(-640.0, -360.0, 1920.0, 1080.0), Color::srgba(23.0 / 255.0, 25.0 / 255.0, 28.0 / 255.0, a));
        if let Some((c, g)) = menu.get("gamma").and_then(|g| g.export("gamma_logo").map(|c| (c, g))) {
            let exp = 2.2 / s.graphics.gamma.max(0.1);
            for (i, v) in [0.998f32, 0.99, 0.95, 0.90, 0.80].iter().enumerate() {
                let x = 640.0 + (i as f32 - 2.0) * 150.0;
                let lum = (1.0 - v).powf(exp).clamp(0.0, 1.0);
                dl.movie(g, c, px_matrix(x, 220.0), &States::default(), lum * a);
            }
        }
        if let Some((ch, m)) = art.root_child("gammaSetting_mc") {
            let m = m.mul(&px_matrix(0.0, p.y));
            dl.movie(art, ch, m, &States::default(), a);
            for f in DisplayList::fields(art, ch, m, &States::default()) {
                let text = if f.path.contains("title") { t.option_label("PSI_Graphics_Gamma").to_uppercase() } else if f.path.contains("desc") { t.t(SEC_MENU, "t_GammaHint") } else { continue };
                let mut f = f;
                f.color.mul[3] *= a;
                dl.field_text(art, &f, &text, None);
            }
            if let (Some(o), Some((_, wm))) = (dishonored_settings::option("PSI_Graphics_Gamma"), art.child(ch, "_widget_mc")) {
                let id = format!("opt:line:{}", o.id);
                let lm = m.mul(&wm);
                self.draw_widget(dl, menu, art, t, s, o, &id, lm, a, now, focus == id);
                let (cx, cy) = lm.apply(0.0, 0.0);
                dl.item(&id, Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(420.0, 44.0)), true);
            }
        }
        if let Some(c) = menu.get("common") {
            HelpBar::draw(dl, c, &menu.metrics, t, &self.anim, "opt/help2", now, help_list("SecondaryScreen"));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_gamepad(&self, dl: &mut DisplayList, menu: &MenuArt, art: &MovieArt, t: &MenuTexts, s: &Settings, nav: &MenuNav, now: f64) {
        let focus = nav.focus.as_deref().unwrap_or("");
        let Some((ch, m)) = art.root_child("gamepadMapping_mc") else { return };
        let veil = self.anim.get("opt/veil", now).alpha / 100.0;
        let st = States::all("PC");
        if let Some((b, bm)) = art.child(ch, "_black_mc") {
            dl.movie_solids(art, b, m.mul(&bm), &st, veil);
        }
        // The controller picture the movie's ImgLoader puts in `_img_mc`.
        // APPROXIMATION: centred and fitted to the labels.
        if let Some((h, size)) = menu.images.get("gamepad") {
            let sc = (1000.0 / size.x).min(560.0 / size.y);
            let r = Rect::from_center_size(Vec2::new(640.0, 360.0), *size * sc);
            dl.prims.push(crate::menu_art::Prim::Quad { tex: h.clone(), src: [0.0, 0.0, size.x, size.y], dst: [(r.min.x, r.min.y), (r.max.x, r.min.y), (r.max.x, r.max.y), (r.min.x, r.max.y)], color: [1.0, 1.0, 1.0, veil.min(1.0)] });
        }
        let ovr = |q: &str| if q == "_black_mc" { Some(Props { visible: false, ..Props::default() }) } else { None };
        dl.movie_fx(art, ch, m, &st, 1.0, &ovr, false);
        // The scheme's action names (`m_GamepadMapping<N>[i]`) fill the picture's labels.
        let set = s.gamepad.binding_set + 1;
        let fields: Vec<_> = DisplayList::fields(art, ch, m, &st).into_iter().filter(|f| f.path.contains("_txt_mc")).collect();
        for (i, f) in fields.iter().enumerate() {
            let key = if i == 0 { format!("m_GamepadMapping{set}") } else { format!("m_GamepadMapping{set}[{i}]") };
            if let Some(txt) = t.loc.get(SEC_TWK_MENU, &key) {
                dl.field_text(art, f, txt, Some(TEXT));
            }
        }
        if let (Some(o), Some((_, wm))) = (dishonored_settings::option("PSI_Gamepad_BindingSet"), art.child(ch, "_widget_mc")) {
            let id = format!("opt:line:{}", o.id);
            let lm = m.mul(&wm);
            self.draw_widget(dl, menu, art, t, s, o, &id, lm, 1.0, now, focus == id);
            let (cx, cy) = lm.apply(0.0, 0.0);
            dl.item(&id, Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(420.0, 44.0)), true);
        }
        if let Some(c) = menu.get("common") {
            HelpBar::draw(dl, c, &menu.metrics, t, &self.anim, "opt/help2", now, help_list("SecondaryScreen"));
        }
    }

    // -----------------------------------------------------------------------------------------
    // Input

    /// Apply one menu event at `now`. `sounds` gets the original UI sound names.
    pub fn handle(&mut self, ev: &MenuEvent, gs: &mut GameSettings, nav: &mut MenuNav, sounds: &mut MenuSounds, now: f64) -> Outcome {
        if self.closing.is_some() {
            return Outcome::None;
        }
        match ev {
            MenuEvent::Focus(_) => {
                sounds.play("Move");
                // A move selects even when the list opened without a selection (after a
                // restore, `_bDelayedSelection`).
                self.select_at = self.select_at.min(now);
                self.keep_visible(nav, gs, now);
                Outcome::Redraw
            }
            MenuEvent::Back => {
                if nav.capturing.is_some() {
                    return Outcome::None;
                }
                match self.secondary {
                    Some(Screen::Gamma) => {
                        // B restores the value the screen opened with.
                        let g = self.gamma_start;
                        gs.s.set("PSI_Graphics_Gamma", Value::Float(g));
                        gs.commit();
                        sounds.play("Back");
                        self.close_secondary(nav, now);
                        Outcome::Changed
                    }
                    Some(Screen::Video) => {
                        sounds.play("back");
                        self.pending = None;
                        self.close_secondary(nav, now);
                        Outcome::Redraw
                    }
                    Some(Screen::GamepadScheme) => {
                        sounds.play("Back");
                        self.close_secondary(nav, now);
                        Outcome::Redraw
                    }
                    None => {
                        sounds.play("Back");
                        self.close(now);
                        Outcome::Close
                    }
                }
            }
            MenuEvent::Scroll(d) => {
                if self.secondary.is_some() {
                    return Outcome::None;
                }
                // The wheel moves the content 60 px.
                let rows = self.rows(&gs.s).len();
                let max = (LINE_H * rows as f32 - LINE_H * Self::view_lines(self.cat) as f32).max(0.0);
                let n = (self.scroll + 60.0 * *d as f32).clamp(0.0, max);
                if n != self.scroll {
                    self.scroll = n;
                    self.anim.now(now, "opt/scroll", Act::To(P::new().y(n), 0.15, Ease::StrongOut));
                    return Outcome::Redraw;
                }
                Outcome::None
            }
            MenuEvent::Captured { id, key } => {
                if let Some((bid, _, end)) = self.bind_at.as_mut() {
                    if bid == id {
                        *end = Some(now);
                    }
                }
                if let (Some(action), Some(k)) = (id.strip_prefix("opt:bind:"), key) {
                    gs.s.rebind(action, k);
                    gs.commit();
                    return Outcome::Changed;
                }
                Outcome::Redraw
            }
            MenuEvent::TextCommit { .. } => Outcome::None,
            MenuEvent::Button(b) => match *b {
                "Stop" => {
                    sounds.play("Stop");
                    Outcome::None
                }
                "X" if self.secondary.is_none() => {
                    sounds.play("Reset");
                    let sub = self.sub_category();
                    Outcome::Ask("restore".into(), format!("{}|{}", self.category().label_key(), sub.label_key().unwrap_or("")))
                }
                "LB" | "RB" if self.secondary.is_none() => {
                    let n = Category::ALL.len();
                    let to = if *b == "LB" { (self.cat + n - 1) % n } else { (self.cat + 1) % n };
                    self.change_tab(to, sounds, now)
                }
                "LT" | "RT" if self.secondary.is_none() && Self::has_subs(self.cat) => {
                    let n = self.category().subs().len();
                    let to = if *b == "LT" { (self.sub + n - 1) % n } else { (self.sub + 1) % n };
                    self.change_sub(to, sounds, now)
                }
                _ => Outcome::None,
            },
            MenuEvent::Step { id, dir } => self.step(id, *dir, gs, sounds, now),
            MenuEvent::Activate { id, at } => self.activate(id, *at, gs, nav, sounds, now),
        }
    }

    /// The answer of a confirmation box this screen asked for.
    pub fn answer(&mut self, purpose: &str, idx: i32, gs: &mut GameSettings, now: f64) -> Outcome {
        match (purpose, idx) {
            ("restore", 0) => {
                let sub = self.sub_category();
                gs.s.restore(sub);
                gs.commit();
                // `RefreshCategory` -> `FillList`: the list rebuilds and fades in, no line motion,
                // nothing selected until the player moves.
                self.show_list(now, 0.0, false);
                Outcome::Changed
            }
            ("video", 0) => {
                self.pending = None;
                self.video_restore = None;
                self.close_secondary_no_nav(now);
                Outcome::Changed
            }
            ("video", _) => {
                // `RestoreInitSettings`: back to the values the screen opened with; the screen
                // stays open.
                if let Some(g) = self.video_restore.take() {
                    gs.s.graphics = g;
                    gs.commit();
                }
                self.pending = Some(gs.s.clone());
                Outcome::Changed
            }
            _ => Outcome::None,
        }
    }

    /// Keep the focused row inside the mask (`ScrollToCurSelection`, 0.15 s).
    fn keep_visible(&mut self, nav: &MenuNav, gs: &GameSettings, now: f64) {
        if self.secondary.is_some() {
            return;
        }
        let Some(f) = nav.focus.as_deref() else { return };
        let rows = self.rows(&gs.s);
        let Some(i) = rows.iter().position(|r| r.id() == f) else { return };
        let n = Self::view_lines(self.cat) as f32;
        let top = LINE_H * i as f32;
        let mut s = self.scroll;
        if top < s {
            s = top;
        } else if top + LINE_H > s + LINE_H * n {
            s = top + LINE_H - LINE_H * n;
        }
        if s != self.scroll {
            self.scroll = s;
            self.anim.now(now, "opt/scroll", Act::To(P::new().y(s), 0.15, Ease::StrongOut));
        }
    }

    fn settings_mut<'a>(&'a mut self, gs: &'a mut GameSettings) -> &'a mut Settings {
        match self.pending.as_mut() {
            Some(p) => p,
            None => &mut gs.s,
        }
    }

    fn commit(&self, gs: &mut GameSettings) -> Outcome {
        if self.pending.is_none() {
            gs.commit();
        }
        Outcome::Changed
    }

    fn step(&mut self, id: &str, dir: i32, gs: &mut GameSettings, sounds: &mut MenuSounds, now: f64) -> Outcome {
        let Some(oid) = id.strip_prefix("opt:line:") else {
            if id.starts_with("video:") {
                // Left / right on the button bar move between Accept and Cancel.
                return Outcome::None;
            }
            return Outcome::None;
        };
        let Some(def) = dishonored_settings::option(oid) else { return Outcome::None };
        let res = self.resolutions.clone();
        let before = value_text_plain(def, self.settings_mut(gs), &res);
        let pos_before = position(self.settings_mut(gs), def, &res);
        let changed = match def.kind {
            Kind::Resolution => {
                let s = self.settings_mut(gs);
                let i = res.iter().position(|r| Some(*r) == s.graphics.resolution).map(|i| i as i32).unwrap_or(-1);
                let n = i + dir;
                if !res.is_empty() && n >= 0 && (n as usize) < res.len() {
                    s.graphics.resolution = Some(res[n as usize]);
                    true
                } else {
                    false
                }
            }
            Kind::Link(_) => return Outcome::None,
            _ if oid == "PSI_Gamepad_BindingSet" && self.secondary.is_none() => return Outcome::None,
            _ => self.settings_mut(gs).step(oid, dir),
        };
        if !changed {
            sounds.play("Stop");
            return Outcome::None;
        }
        match def.kind {
            Kind::Choice(_) | Kind::Resolution => {
                // `UpdateText(side)`: the value slides; `Change` plays when it lands (150 ms).
                self.slide = Some((oid.to_string(), before, if dir < 0 { -1.0 } else { 1.0 }, now));
                self.sounds_at.push((now + 0.15, "change"));
            }
            Kind::Range { min, max, .. } => {
                // `Change` from the key handler and again from `UpdateValue`; the thumb moves 0.1 s.
                sounds.play("change");
                sounds.play("change");
                let k = format!("opt/thumb/{oid}");
                let _ = pos_before;
                let v_old = before.parse::<f32>().ok();
                let v = self.settings_mut(gs).get(oid).map(|v| v.as_f32()).unwrap_or(min);
                let to = -138.0 + ((v - min) / (max - min)).clamp(0.0, 1.0) * 276.0;
                if self.anim.value(&k, now).is_none() {
                    let from = v_old.map(|vo| -138.0 + ((vo - min) / (max - min)).clamp(0.0, 1.0) * 276.0).unwrap_or(to);
                    self.anim.now(now, &k, Act::Set(P::new().x(from)));
                }
                self.anim.now(now, &k, Act::To(P::new().x(to), 0.1, Ease::StrongOut));
            }
            _ => sounds.play("change"),
        }
        self.commit(gs)
    }

    fn change_tab(&mut self, cat: usize, sounds: &mut MenuSounds, now: f64) -> Outcome {
        if cat == self.cat {
            return Outcome::None;
        }
        let prev = self.cat;
        let last = Category::ALL.len() - 1;
        // animSide: forward (and the wrap from last to first) is 'r'.
        let side = if (cat > prev && !(prev == 0 && cat == last)) || (prev == last && cat == 0) { 1.0 } else { -1.0 };
        sounds.play("ChangeMainTab");
        let a = &mut self.anim;
        let ko = format!("opt/tab/{prev}");
        a.now(now, &ko, Act::To(P::new().scale(100.0).y(0.0), 0.25, Ease::BackOut));
        a.now(now, &ko, Act::Play("unselected".into()));
        let kn = format!("opt/tab/{cat}");
        a.now(now, &kn, Act::To(P::new().scale(100.0).y(-20.0), 0.25, Ease::BackOut));
        a.now(now, &kn, Act::Play("selected".into()));
        self.cat = cat;
        self.sub = 0;
        self.open_subtabs(now);
        self.swap_list(now, side);
        Outcome::Redraw
    }

    fn change_sub(&mut self, sub: usize, sounds: &mut MenuSounds, now: f64) -> Outcome {
        if sub == self.sub {
            return Outcome::None;
        }
        let prev = self.sub;
        let last = self.category().subs().len() - 1;
        let side = if (sub > prev && !(prev == 0 && sub == last)) || (prev == last && sub == 0) { 1.0 } else { -1.0 };
        sounds.play("ChangeSubTab");
        self.anim.now(now, &format!("opt/subtab/{prev}"), Act::Play("unselected".into()));
        self.anim.now(now, &format!("opt/subtab/{sub}"), Act::Play("selected".into()));
        self.sub = sub;
        self.swap_list(now, side);
        Outcome::Redraw
    }

    fn open_secondary(&mut self, screen: Screen, gs: &GameSettings, nav: &mut MenuNav, sounds: &mut MenuSounds, now: f64) {
        sounds.play("change");
        self.secondary = Some(screen);
        self.secondary_at = now;
        nav.focus = None;
        let a = &mut self.anim;
        a.author("opt/veil", Props::default());
        a.now(now, "opt/veil", Act::Set(P::new().alpha(0.0)));
        a.now(now, "opt/veil", Act::To(P::new().alpha(90.0), 0.25, Ease::StrongOut));
        HelpBar::open(a, "opt/help2", now);
        match screen {
            Screen::Video => {
                self.pending = Some(gs.s.clone());
                a.now(now, "opt/screen", Act::To(P::new().alpha(50.0), 0.25, Ease::StrongOut));
                a.author("opt/video", Props::default());
                a.now(now, "opt/video", Act::Set(P::new().alpha(0.0).scale(120.0)));
                a.now(now, "opt/video", Act::To(P::new().alpha(100.0).scale(100.0), 0.25, Ease::StrongOut));
                // The video settings screen closes the help bar (`SetHelp` -> `CloseHelp`).
                HelpBar::close(a, "opt/help2", now);
                HelpBar::close(a, "opt/help", now);
            }
            Screen::Gamma => {
                self.gamma_start = gs.s.graphics.gamma;
                a.now(now, "opt/screen", Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
                a.author("opt/gamma", Props::default());
                a.now(now, "opt/gamma", Act::Set(P::new().alpha(0.0).y(150.0)));
                a.now(now, "opt/gamma", Act::To(P::new().alpha(100.0).y(0.0), 0.25, Ease::StrongOut));
            }
            Screen::GamepadScheme => {}
        }
    }

    /// Secondary screens have no close animation (`setInterval(OnClosed, 0.5)` is 0.5 ms):
    /// they hide, and the options screen fades back to 100 % in 0.2 s.
    fn close_secondary(&mut self, nav: &mut MenuNav, now: f64) {
        self.close_secondary_no_nav(now);
        nav.focus = None;
    }

    fn close_secondary_no_nav(&mut self, now: f64) {
        self.secondary = None;
        self.anim.now(now, "opt/screen", Act::To(P::new().alpha(100.0), 0.2, Ease::StrongOut));
        HelpBar::open(&mut self.anim, "opt/help", now);
    }

    fn activate(&mut self, id: &str, at: Option<Vec2>, gs: &mut GameSettings, nav: &mut MenuNav, sounds: &mut MenuSounds, now: f64) -> Outcome {
        if let Some(i) = id.strip_prefix("opt:tab:").and_then(|x| x.parse().ok()) {
            return self.change_tab(i, sounds, now);
        }
        if let Some(i) = id.strip_prefix("opt:sub:").and_then(|x| x.parse::<usize>().ok()) {
            return self.change_sub(i, sounds, now);
        }
        if let Some(b) = id.strip_prefix("opt:btn:").or_else(|| id.strip_prefix("help:")) {
            let ev = match b {
                "B" => MenuEvent::Back,
                "X" => MenuEvent::Button("X"),
                "LB" => MenuEvent::Button("LB"),
                "RB" => MenuEvent::Button("RB"),
                "A" => match nav.focus.clone() {
                    Some(f) => MenuEvent::Activate { id: f, at: None },
                    None => return Outcome::None,
                },
                _ => return Outcome::None,
            };
            return self.handle(&ev, gs, nav, sounds, now);
        }
        match id {
            "opt:back" => return self.handle(&MenuEvent::Back, gs, nav, sounds, now),
            "opt:restore" => return self.handle(&MenuEvent::Button("X"), gs, nav, sounds, now),
            "video:accept" => {
                sounds.play("validation");
                let modified = self.pending.as_ref().is_some_and(|p| p.graphics != gs.s.graphics);
                if let Some(p) = self.pending.clone() {
                    if modified {
                        // `OnApplyVideoSettings`, then the question; No restores the values the
                        // screen opened with (`RestoreInitSettings`).
                        self.video_restore = Some(gs.s.graphics.clone());
                        gs.s.graphics = p.graphics;
                        gs.commit();
                        return Outcome::Ask("video".into(), String::new());
                    }
                }
                self.pending = None;
                self.close_secondary(nav, now);
                return Outcome::Changed;
            }
            "video:cancel" => return self.handle(&MenuEvent::Back, gs, nav, sounds, now),
            _ => {}
        }
        if let Some(action) = id.strip_prefix("opt:bind:") {
            let _ = action;
            nav.capturing = Some(id.to_string());
            self.bind_at = Some((id.to_string(), now, None));
            sounds.play("validation");
            return Outcome::Redraw;
        }
        if let Some(rest) = id.strip_prefix("opt:arrow:") {
            let (oid, d) = rest.rsplit_once(':').unwrap_or((rest, "+1"));
            let dir = if d.starts_with('-') { -1 } else { 1 };
            nav.focus = Some(format!("opt:line:{oid}"));
            return self.step(&format!("opt:line:{oid}"), dir, gs, sounds, now);
        }
        if let Some(rest) = id.strip_prefix("opt:half:") {
            // Click on a boolean half selects it.
            let (oid, i) = rest.rsplit_once(':').unwrap_or((rest, "1"));
            let want = i == "1";
            let cur = matches!(self.settings_mut(gs).get(oid), Some(Value::Bool(true)));
            if want != cur {
                self.settings_mut(gs).set(oid, Value::Bool(want));
                sounds.play("change");
                return self.commit(gs);
            }
            return Outcome::None;
        }
        if let Some(oid) = id.strip_prefix("opt:slider:") {
            if let (Some(Kind::Range { min, max, step }), Some(p)) = (dishonored_settings::option(oid).map(|o| o.kind), at) {
                let r = nav.items.iter().find(|i| i.id == id).map(|i| i.rect);
                if let Some(r) = r {
                    // `GetIndexFromPosition`: snap to the step grid along the track.
                    let tpos = ((p.x - r.min.x - 26.0) / (r.width() - 52.0).max(1.0)).clamp(0.0, 1.0);
                    let steps = ((max - min) / step).max(1.0);
                    let v = min + if tpos < 1.0 { (tpos * steps).round() / steps } else { 1.0 } * (max - min);
                    self.settings_mut(gs).set(oid, Value::Float(v));
                    let k = format!("opt/thumb/{oid}");
                    let to = -138.0 + ((v - min) / (max - min)).clamp(0.0, 1.0) * 276.0;
                    self.anim.now(now, &k, Act::To(P::new().x(to), 0.1, Ease::StrongOut));
                    sounds.play("change");
                    return self.commit(gs);
                }
            }
            return Outcome::None;
        }
        if let Some(oid) = id.strip_prefix("opt:line:") {
            let Some(def) = dishonored_settings::option(oid) else { return Outcome::None };
            let link = match def.kind {
                Kind::Link(screen) => Some(screen),
                _ if oid == "PSI_Gamepad_BindingSet" && self.secondary.is_none() => Some(Screen::GamepadScheme),
                _ => None,
            };
            if let Some(screen) = link {
                self.open_secondary(screen, gs, nav, sounds, now);
                return Outcome::Redraw;
            }
            // Gamma screen: A validates.
            if self.secondary == Some(Screen::Gamma) && oid == "PSI_Graphics_Gamma" {
                sounds.play("Validation");
                self.close_secondary(nav, now);
                return Outcome::Redraw;
            }
            if self.secondary == Some(Screen::GamepadScheme) {
                sounds.play("Validation");
                self.close_secondary(nav, now);
                return Outcome::Redraw;
            }
            // Steppers, booleans and sliders take no A.
            return Outcome::None;
        }
        Outcome::None
    }

    /// Key capture ended (the binding row plays `bind_out`).
    pub fn capture_done(&mut self, now: f64) {
        if let Some((_, _, end)) = self.bind_at.as_mut() {
            if end.is_none() {
                *end = Some(now);
            }
        }
    }
}

/// Multiply the alpha of everything in `sub` by `a` and append it.
fn fade(dl: &mut DisplayList, sub: DisplayList, a: f32) {
    use crate::menu_art::Prim;
    for p in sub.prims {
        dl.prims.push(match p {
            Prim::Quad { tex, src, dst, mut color } => {
                color[3] *= a;
                Prim::Quad { tex, src, dst, color }
            }
            Prim::Text { text, rect, size, font, color, align, top, rot } => Prim::Text { text, rect, size, font, color: color.with_alpha(color.alpha() * a), align, top, rot },
            Prim::Fill { rect, color } => Prim::Fill { rect, color: color.with_alpha(color.alpha() * a) },
        });
    }
    dl.items.extend(sub.items);
}

fn tab_center(m: &Matrix) -> (f32, f32) {
    (m.tx / 20.0, m.ty / 20.0)
}

fn half_rect(m: &Matrix, x: f32) -> Rect {
    let (ax, ay) = m.apply(x * 20.0, 0.0);
    let (bx, by) = m.apply((x + 141.0) * 20.0, 0.0);
    Rect::new(ax.min(bx) / 20.0, ay.min(by) / 20.0 - 16.0, ax.max(bx) / 20.0, ay.max(by) / 20.0 + 16.0)
}

fn arrow_items(dl: &mut DisplayList, m: &Matrix, oid: &str) {
    for (x, d) in [(-166.0, "-1"), (166.0, "+1")] {
        let (cx, cy) = m.apply(x * 20.0, 0.0);
        dl.item(format!("opt:arrow:{oid}:{d}"), Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(44.0, 44.0)), false);
    }
}

/// The question of an [`Outcome::Ask`]: `t_Q_RestoreSettings` with `§CATEGORY§` replaced by
/// "Category" or "Category -> Sub-category" (`OptionsScreen.XPressed`), or
/// `t_Q_VideoSettings`. Buttons: `t_Yes`, `t_No`.
pub fn question(t: &MenuTexts, purpose: &str, payload: &str) -> String {
    match purpose {
        "restore" => {
            let (c, s) = payload.split_once('|').unwrap_or((payload, ""));
            let mut name = t.t(SEC_TWK_MENU, c);
            if !s.is_empty() {
                name = format!("{name} -> {}", t.t(SEC_TWK_MENU, s));
            }
            texts_any(t, "t_Q_RestoreSettings").replace("§CATEGORY§", &name)
        }
        "video" => texts_any(t, "t_Q_VideoSettings"),
        _ => String::new(),
    }
}

/// Index of the current value among the widget's values, and the count (arrows, clamping).
fn position(s: &Settings, o: &OptionDef, res: &[(u32, u32)]) -> Option<(usize, usize)> {
    match (o.kind, s.get(o.id)?) {
        (Kind::Bool, Value::Bool(b)) => Some((b as usize, 2)),
        (Kind::Choice(c), Value::Int(i)) => Some((i.max(0) as usize, c.len())),
        (Kind::Range { min, max, step }, v) => {
            let n = ((max - min) / step).round() as usize + 1;
            Some((((v.as_f32() - min) / step).round().max(0.0) as usize, n))
        }
        (Kind::Resolution, Value::Resolution(w, h)) => Some((res.iter().position(|r| *r == (w, h)).unwrap_or(0), res.len().max(1))),
        _ => None,
    }
}

/// The line's label: the option's `PSI_*` name; links show what they open (the gamma value's
/// name, the video settings sub-menu).
fn line_label(t: &MenuTexts, o: &OptionDef) -> String {
    match o.id {
        "t_Modify_Gamma" => t.option_label("PSI_Graphics_Gamma"),
        "t_Modify_VideoSettings" => t.t(SEC_TWK_MENU, "m_SettingsSubMenu_VideoSettings"),
        id => t.option_label(id),
    }
}

/// The value a link line shows when not focused.
fn link_value(t: &MenuTexts, o: &OptionDef, s: &Settings) -> String {
    match o.id {
        "PSI_Gamepad_BindingSet" => value_text(t, o, s, &[]),
        "t_Modify_Gamma" => format!("{:.1}", s.graphics.gamma),
        _ => String::new(),
    }
}

fn number(v: f32, o: &OptionDef) -> String {
    match o.kind {
        Kind::Range { step, .. } if step < 1.0 => format!("{v:.1}"),
        _ => format!("{v:.0}"),
    }
}

/// Displayed value of a stepper / drop-down line.
pub fn value_text(t: &MenuTexts, o: &OptionDef, s: &Settings, _res: &[(u32, u32)]) -> String {
    match (o.kind, s.get(o.id)) {
        (Kind::Bool, Some(Value::Bool(b))) => t.value_label(if b { "BP_True" } else { "BP_False" }),
        (Kind::Choice(c), Some(Value::Int(i))) => c.get(i.max(0) as usize).map(|k| t.value_label(k)).unwrap_or_default(),
        (Kind::Resolution, Some(Value::Resolution(w, h))) if w > 0 => format!("{w} x {h}"),
        // Unset: the desktop mode (what the original picks on first run).
        (Kind::Resolution, _) => "Desktop".to_string(),
        (_, Some(v)) => number(v.as_f32(), o),
        _ => String::new(),
    }
}

/// [`value_text`] without localisation (to remember the text a stepper slides out).
fn value_text_plain(o: &OptionDef, s: &Settings, _res: &[(u32, u32)]) -> String {
    match (o.kind, s.get(o.id)) {
        (Kind::Choice(c), Some(Value::Int(i))) => c.get(i.max(0) as usize).map(|k| k.to_string()).unwrap_or_default(),
        (Kind::Resolution, Some(Value::Resolution(w, h))) if w > 0 => format!("{w} x {h}"),
        (Kind::Resolution, _) => "Desktop".to_string(),
        (_, Some(v)) => format!("{}", v.as_f32()),
        _ => String::new(),
    }
}

/// Widget of a free-form line (launcher screens built in the options style).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LineWidget {
    /// Stepper with arrows (`opt:arrow`-style items `<id>:-1` / `<id>:+1`).
    Stepper,
    /// Text entry shown in the drop-down frame; `editing` draws a caret.
    Text { editing: bool },
    /// Link-style action.
    Button,
    /// Label only (status text).
    Info,
}

pub const GENERIC_LINE_H: f32 = LINE_H;
/// List origin of the launcher's own pages drawn in the options style (stage px).
pub const LIST_X: f32 = 190.0;

/// A line at `y` with the movie's line and widget art: `id` is the line item (nav),
/// arrows are `<id>:-1` / `<id>:+1`. `state` is the line's (label, seconds) timeline and
/// `idx` its strip variant (`(i mod 3)`).
#[allow(clippy::too_many_arguments)]
pub fn generic_line(dl: &mut DisplayList, art: &MovieArt, y: f32, id: &str, label: &str, value: &str, widget: LineWidget, focused: bool) {
    generic_line_at(dl, art, LIST_X, y, 0, id, label, value, widget, if focused { ("over", 1.0) } else { ("default", 0.0) }, 1.0);
}

#[allow(clippy::too_many_arguments)]
pub fn generic_line_at(dl: &mut DisplayList, art: &MovieArt, x: f32, y: f32, idx: usize, id: &str, label: &str, value: &str, widget: LineWidget, state: (&str, f32), a: f32) {
    let st = States::default().with_timed("*", state.0, state.1).with("_bkgd_mc/mc", &format!("#{}", idx % 3));
    let m = px_matrix(x, y);
    if let Some(ch) = art.export("o_options_line") {
        dl.movie(art, ch, m, &st, a);
        if let Some(f) = DisplayList::fields(art, ch, m, &st).iter().find(|f| f.path.contains("_txt_mc")) {
            let mut f = f.clone();
            f.color.mul[3] *= a;
            dl.field_text(art, &f, label, Some(TEXT));
        }
    }
    if widget != LineWidget::Info {
        dl.item(id, Rect::new(x, y - LINE_H * 0.5 + 2.0, x + 930.0, y + LINE_H * 0.5 - 2.0), true);
    }
    let wm = px_matrix(x + WIDGET_X, y);
    let export = match widget {
        LineWidget::Stepper => "OptionsStepper_widget",
        LineWidget::Text { .. } => "DropDownMenu_widget",
        LineWidget::Button => "Link_widget",
        LineWidget::Info => "",
    };
    if let Some(ch) = art.export(export) {
        let wst = States::default().with_timed("*", state.0, state.1);
        // Links show their value text (`_txtValue_mc`); the action text stays hidden.
        let ovr = |p: &str| match (widget, p) {
            (LineWidget::Button, "_txtValue_mc") => Some(Props { x: -0.05, ..Props::default() }),
            (LineWidget::Button, "_txt_mc") => Some(Props { x: -130.9, alpha: 0.0, ..Props::default() }),
            _ => None,
        };
        dl.movie_fx(art, ch, wm, &wst, a, &ovr, false);
        let fields = DisplayList::fields_fx(art, ch, wm, &wst, &ovr);
        let f = match widget {
            LineWidget::Button => fields.iter().find(|f| f.path.contains("_txtValue_mc")),
            _ => fields.first(),
        };
        if let Some(f) = f {
            let v = match widget {
                LineWidget::Text { editing: true } => format!("{value}_"),
                _ => value.to_string(),
            };
            let mut f = f.clone();
            f.color.mul[3] = a;
            dl.field_text(art, &f, &v, Some(TEXT));
        }
    } else if widget == LineWidget::Info {
        dl.text(value, Rect::new(x + WIDGET_X - 200.0, y - 15.0, x + 960.0, y + 15.0), 20.0, FontKind::Normal, TEXT_DIM.with_alpha(a), Justify::Left);
    }
    if widget == LineWidget::Stepper {
        dl.item(format!("{id}:-1"), Rect::new(x + WIDGET_X - 190.0, y - 18.0, x + WIDGET_X - 140.0, y + 18.0), false);
        dl.item(format!("{id}:+1"), Rect::new(x + WIDGET_X + 140.0, y - 18.0, x + WIDGET_X + 190.0, y + 18.0), false);
    }
}

/// Options backdrop: `options_mc/_bkgdTexture_mc` over a dimmed screen.
pub fn backdrop(dl: &mut DisplayList, art: &MovieArt) {
    dl.fill(Rect::new(-640.0, -360.0, 1920.0, 1080.0), Color::srgba(23.0 / 255.0, 25.0 / 255.0, 28.0 / 255.0, 0.72));
    if let Some((ch, m)) = art.root_child("options_mc") {
        dl.movie(art, ch, m, &States::default(), 1.0);
    }
}
