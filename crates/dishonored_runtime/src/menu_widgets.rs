//! Widgets the menu movies share, rebuilt from their classes:
//! - the title bar (`Common_assets.lib` `lib_titleMc`, class `_common.TitleBar`);
//! - the help bar (`lib_dynamicHelpBar`, class `_common.HelpBar`): the key hints at the bottom
//!   right, attached by `UIBase.InitHelpBar` at (1184, 651);
//! - the confirmation box (`UI_Global.Global` `gl_msgBox`, class `_common.MessageBox`), which
//!   the menus open through `MessageBoxInvoke.InvokeMessageBox` (`t_Q_*` questions, Yes / No).
//!
//! Timings are the classes' (see `research/dishonored/MENUS.md`, "Shared widgets"). Each widget
//! keeps its clips in the screen's [`Anim`] under a key prefix.

use crate::menu_anim::{Act, Anim, Ease, P};
use crate::menu_art::{DisplayList, FontKind, MovieArt, Props, States, TEXT, px_matrix};
use crate::menu_ui::{MenuArt, MenuTexts, SEC_BASE};
use bevy::prelude::*;
use dishonored_assets::swf::Matrix;

/// Register the authored properties of every named clip under `ch` (to `depth` levels) as
/// `prefix/<path>`.
pub fn author_tree(anim: &mut Anim, art: &MovieArt, prefix: &str, ch: u16, depth: u32) {
    if depth == 0 {
        return;
    }
    let Some(s) = art.sprite(ch) else { return };
    for p in art.movie.display_list(s, 0) {
        let (Some(name), Some(c)) = (p.name.clone(), p.character) else { continue };
        let alpha = p.color.map(|c| c.mul[3] * 100.0).unwrap_or(100.0);
        let key = format!("{prefix}/{name}");
        anim.author(&key, Props::from_matrix(&p.matrix.unwrap_or_default(), alpha));
        author_tree(anim, art, &key, c, depth - 1);
    }
}

/// `P` of a clip's authored properties (a `props` snapshot the classes tween back to).
pub fn props_of(anim: &Anim, key: &str) -> P {
    P::all(anim.authored(key))
}

/// The stage matrix of a clip chain: `base` then each named child along `path` (with the
/// screen's overrides). Returns the character at the end.
pub fn chain(art: &MovieArt, anim: &Anim, prefix: &str, now: f64, mut ch: u16, mut m: Matrix, path: &[&str]) -> Option<(u16, Matrix)> {
    let mut key = prefix.to_string();
    for name in path {
        let (c, cm) = art.child(ch, name)?;
        key = format!("{key}/{name}");
        let local = anim.value(&key, now).map(|p| p.matrix_like(&cm)).unwrap_or(cm);
        m = m.mul(&local);
        ch = c;
    }
    Some((ch, m))
}

/// Accumulated alpha (0..1) of a clip chain's overrides (authored alphas where untouched).
pub fn chain_alpha(anim: &Anim, prefix: &str, now: f64, path: &[&str]) -> f32 {
    let mut key = prefix.to_string();
    let mut a = anim.value(prefix, now).map(|p| p.alpha / 100.0).unwrap_or(1.0);
    for name in path {
        key = format!("{key}/{name}");
        a *= anim.value(&key, now).map(|p| p.alpha).unwrap_or(anim.authored(&key).alpha) / 100.0;
    }
    a
}

// ---------------------------------------------------------------------------------------------
// Title bar

/// `_common.TitleBar` (common 324) on `lib_titleMc` (262).
pub struct TitleBar;

impl TitleBar {
    const SHADOWS: usize = 3;

    /// `SetTitle` -> `Open()` at `now`. `base`: where the screen attaches the bar.
    pub fn open(anim: &mut Anim, art: &MovieArt, key: &str, base: Props, now: f64) {
        let Some(ch) = art.export("lib_titleMc") else { return };
        anim.author(key, base);
        author_tree(anim, art, key, ch, 2);
        anim.cancel(key, now);
        let k = |s: &str| format!("{key}/{s}");
        // Constructor: shadows, text and bkgd at alpha 0.
        for i in 0..Self::SHADOWS {
            anim.now(now, &k(&format!("_txtShadow{i}_mc")), Act::Set(P::new().alpha(0.0)));
        }
        anim.now(now, &k("_txt_mc"), Act::Set(P::new().alpha(0.0)));
        // Open: snap to props, then the bkgd swings in.
        anim.now(now, key, Act::Set(P::all(base)));
        let bk = k("_bkgd_mc");
        let bp = anim.authored(&bk);
        anim.now(now, &bk, Act::Set(P::new().alpha(0.0).x(bp.x - 120.0).scale(250.0).rot(-20.0)));
        anim.now(now, &bk, Act::To(P::all(bp), 0.3, Ease::StrongInOut));
        anim.now(now, &k("_bkgd_mc/cart_mc"), Act::To(P::new().alpha(80.0), 0.6, Ease::BackInOut));
        for l in ["lineUp_mc", "lineDown_mc"] {
            anim.now(now, &k(&format!("_bkgd_mc/{l}")), Act::Set(P::new().visible(false)));
        }
        // 150 ms: the lines, and the first shadow.
        let t1 = now + 0.15;
        anim.at(t1, &k("_bkgd_mc/lineUp_mc"), Act::From(P::new().alpha(0.0).x(-350.0).scale(150.0).visible(true), 0.3, Ease::StrongOut));
        anim.at(t1, &k("_bkgd_mc/lineDown_mc"), Act::From(P::new().alpha(0.0).x(1250.0).scale(150.0).visible(true), 0.35, Ease::StrongOut));
        // Shadows 0 and 1 every 100 ms, then the text (shadow 2 is skipped: off by one in
        // `OpenTitleShadow`).
        let mut t = t1;
        for i in 0..Self::SHADOWS - 1 {
            let sk = k(&format!("_txtShadow{i}_mc"));
            let sp = anim.authored(&sk);
            anim.at(t, &sk, Act::Set(P::new().alpha(0.0).x(sp.x - 20.0).scale(200.0).rot(-5.0)));
            anim.at(t, &sk, Act::To(P::all(sp), 0.25, Ease::BackInOut));
            t += 0.1;
        }
        let tk = k("_txt_mc");
        let tp = anim.authored(&tk);
        anim.at(t, &tk, Act::Set(P::new().alpha(0.0).x(tp.x - 20.0).scale(200.0).rot(-5.0)));
        anim.at(t, &tk, Act::To(P::all(tp), 0.25, Ease::StrongOut));
    }

    /// `Close()`: fade, grow and slide left.
    pub fn close(anim: &mut Anim, key: &str, now: f64) {
        anim.cancel_queued(key);
        let cur = anim.get(key, now);
        anim.now(now, key, Act::To(P::new().alpha(0.0).scale(110.0).x(cur.x - 150.0), 0.25, Ease::StrongOut));
    }

    /// Draw the bar with `text` (upper-cased, as `SetTitle` does). `parent` is the screen's
    /// stage matrix the bar's props are relative to.
    pub fn draw(dl: &mut DisplayList, art: &MovieArt, metrics: &crate::menu_art::FontMetrics, anim: &Anim, key: &str, now: f64, parent: Matrix, text: &str) {
        let Some(ch) = art.export("lib_titleMc") else { return };
        let root = anim.get(key, now);
        if !root.visible || root.alpha <= 0.0 {
            return;
        }
        let m = parent.mul(&root.matrix());
        let up = text.to_uppercase();
        // `large` cart when the text is wider than the field (`_txtW`, about 692 px at 38 px).
        let large = metrics.width(FontKind::Title, &up, 38.0) > 692.0;
        let st = States::default().with("_bkgd_mc/cart_mc", if large { "#1" } else { "#0" });
        let ovr = anim.ovr(key, now);
        dl.movie_fx(art, ch, m, &st, root.alpha / 100.0, &ovr, false);
        for f in DisplayList::fields_fx(art, ch, m, &st, &ovr) {
            let mut f = f;
            f.color.mul[3] *= root.alpha / 100.0;
            dl.field_text(art, &f, &up, None);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Help bar

/// `_common.HelpBar` (common 330) on `lib_dynamicHelpBar` (309).
pub struct HelpBar;

/// Help lists of the root frame script `InitHelpBar` (button, text key in
/// `[DisGFxMoviePlayerBase_Texts]` / `[DisGFxMoviePlayerMenuBase_Texts]`), left to right.
pub fn help_list(id: &str) -> &'static [(&'static str, &'static str)] {
    match id {
        "Options_main" => &[("X", "t_RestoreSettings"), ("B", "t_Back")],
        "SecondaryScreen" => &[("A", "t_Validate"), ("B", "t_Back")],
        "SaveList" => &[("A", "t_Validate"), ("X", "t_DeleteSave"), ("B", "t_Back")],
        "MissionsScreen" => &[("A", "t_Validate"), ("X", "t_SeeMissionStats"), ("B", "t_Back")],
        "TutorialsScreen" => &[("A", "t_ReadTutorial"), ("B", "t_Back")],
        "exit" => &[("B", "t_Exit")],
        "close" => &[("B", "t_Close")],
        _ => &[("B", "t_Back")],
    }
}

/// Where `UIBase.InitHelpBar` attaches the bar.
pub const HELP_POS: Vec2 = Vec2::new(1184.0, 651.0);

impl HelpBar {
    /// `OpenHelp(id)`: from x + 20 and alpha 0 to its place in 0.3 s.
    pub fn open(anim: &mut Anim, key: &str, now: f64) {
        anim.author(key, Props { x: HELP_POS.x, y: HELP_POS.y, ..Props::default() });
        anim.cancel(key, now);
        anim.now(now, key, Act::Set(P::new().x(HELP_POS.x + 20.0).alpha(0.0).visible(true)));
        anim.now(now, key, Act::To(P::new().x(HELP_POS.x).alpha(100.0), 0.3, Ease::StrongOut));
    }

    /// `CloseHelp()`: fade out in 0.35 s.
    pub fn close(anim: &mut Anim, key: &str, now: f64) {
        anim.now(now, key, Act::To(P::new().alpha(0.0), 0.35, Ease::StrongOut));
    }

    /// Draw `entries` right-aligned (`setDisplay`); items `help:<btn>` act as the keys.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(dl: &mut DisplayList, art: &MovieArt, metrics: &crate::menu_art::FontMetrics, t: &MenuTexts, anim: &Anim, key: &str, now: f64, entries: &[(&str, &str)]) {
        let p = anim.get(key, now);
        if !p.visible || p.alpha <= 0.0 || entries.is_empty() {
            return;
        }
        let a = p.alpha / 100.0;
        let base = px_matrix(p.x, p.y);
        // Entry widths: icon (40 px at 90 %) then the text field (`help_txt`, 24 px, at x 2).
        let labels: Vec<String> = entries.iter().map(|(_, k)| texts_any(t, k).to_uppercase()).collect();
        let icon_w = 36.0;
        let widths: Vec<f32> = labels.iter().map(|l| icon_w + 2.0 + metrics.field_width(FontKind::Normal, l, 24.0)).collect();
        let mut xs = vec![0.0f32; entries.len()];
        let n = entries.len();
        xs[n - 1] = -widths[n - 1] - 10.0;
        for i in (0..n - 1).rev() {
            xs[i] = xs[i + 1] - widths[i] - 20.0;
        }
        // Background: R at x -100, the tiled middle, L left of it.
        let content = -xs[0];
        let mid = (content - 160.0).round().max(0.0);
        if let Some(r) = art.export("lib_help_bkgdR") {
            dl.movie(art, r, base.mul(&px_matrix(-100.0, 0.0)), &States::default(), a);
        }
        if mid > 0.0 {
            if let Some(tex) = art.textures.get("lib_help_bkgdExt -nopack").or_else(|| art.textures.get("lib_help_bkgdExt")) {
                let (x0, x1) = (p.x - 100.0 - mid, p.x - 100.0);
                let (y0, y1) = (p.y - 27.5, p.y + 27.5);
                dl.prims.push(crate::menu_art::Prim::Quad { tex: tex.clone(), src: [0.0, 0.0, mid.min(284.0), 55.0], dst: [(x0, y0), (x1, y0), (x1, y1), (x0, y1)], color: [1.0, 1.0, 1.0, a] });
            }
        }
        if let Some(l) = art.export("lib_help_bkgdL") {
            // L is drawn mirrored to the left of the middle (its art spans 0..100).
            let lm = Matrix { a: -1.0, ..px_matrix(p.x - 100.0 - mid, p.y) };
            dl.movie(art, l, lm, &States::default(), a);
        }
        for (i, ((btn, _), label)) in entries.iter().zip(&labels).enumerate() {
            let ex = p.x + xs[i];
            if let Some(ic) = art.export(&format!("lib_{btn}_Multi")) {
                let m = Matrix { a: 0.9, d: 0.9, ..px_matrix(ex + icon_w * 0.5, p.y) };
                dl.movie(art, ic, m, &States::all("PC"), a);
            }
            if let Some(ht) = art.export("help_txt") {
                let m = px_matrix(ex + icon_w, p.y);
                for f in DisplayList::fields(art, ht, m, &States::default()) {
                    dl.field_text(art, &f, label, Some(Color::WHITE.with_alpha(a)));
                }
            }
            dl.item(format!("help:{btn}"), Rect::new(ex, p.y - 25.0, ex + widths[i] + 5.0, p.y + 25.0), false);
        }
    }
}

/// A text key from whichever menu section holds it.
pub fn texts_any(t: &MenuTexts, key: &str) -> String {
    for sec in [SEC_BASE, crate::menu_ui::SEC_MENU, crate::menu_ui::SEC_MAIN, crate::journal::SEC_JOURNAL, "DisGFxMoviePlayerMissionStats_Texts"] {
        if let Some(s) = t.loc.get(sec, key) {
            return dishonored_assets::loc::plain(s);
        }
    }
    t.t(SEC_BASE, key)
}

// ---------------------------------------------------------------------------------------------
// Message box

/// An open confirmation box (`_common.MessageBox`).
#[derive(Clone, Debug)]
pub struct MsgBox {
    /// What the answer is for (the screen's own id).
    pub purpose: String,
    pub text: String,
    pub buttons: Vec<String>,
    pub sel: usize,
    pub opened: f64,
    /// `Close()` time and the confirmed index.
    pub closing: Option<(f64, i32)>,
}

pub const MSG_KEY: &str = "msg";

impl MsgBox {
    /// `ShowMessageBox(msg, b0, b1)` at `now`: the box opens (sound `G_MsgBoxOpen`).
    pub fn open(anim: &mut Anim, art: &MovieArt, purpose: &str, text: String, buttons: Vec<String>, now: f64) -> MsgBox {
        if let Some(ch) = art.export("gl_msgBox") {
            anim.author(MSG_KEY, Props { x: 640.0, y: 360.0, ..Props::default() });
            author_tree(anim, art, MSG_KEY, ch, 3);
        }
        anim.cancel(MSG_KEY, now);
        let k = |s: &str| format!("{MSG_KEY}/{s}");
        anim.now(now, MSG_KEY, Act::Set(P::new().alpha(100.0).scale(100.0)));
        anim.now(now, &k("_vignette_mc"), Act::Set(P::new().scale(120.0)));
        anim.now(now, &k("_vignette_mc"), Act::To(P::new().alpha(100.0).scale(100.0), 0.65, Ease::StrongOut));
        let mb = k("_msgBox_mc");
        anim.now(now, &k("_msgBox_mc/boxBkgd_mc"), Act::Set(P::new().alpha(0.0).scale(50.0)));
        anim.now(now, &k("_msgBox_mc/boxBkgd_mc"), Act::To(P::new().alpha(100.0).scale(100.0), 0.3, Ease::BackOut));
        anim.now(now, &k("_msgBox_mc/boxShape_mc"), Act::Set(P::new().alpha(0.0).scale(350.0)));
        anim.now(now, &k("_msgBox_mc/boxShape_mc"), Act::To(P::new().alpha(100.0).scale(100.0), 0.3, Ease::StrongInOut));
        anim.now(now, &mb, Act::Set(P::new().alpha(0.0)));
        anim.now(now, &mb, Act::To(P::new().alpha(100.0).rot(-2.0), 0.25, Ease::StrongInOut));
        anim.now(now, &k("_msgBox_mc/_txt_mc"), Act::Set(P::new().alpha(0.0).scale(300.0)));
        anim.now(now, &k("_msgBox_mc/_txt_mc"), Act::To(P::new().alpha(100.0).scale(100.0), 0.3, Ease::StrongInOut));
        for s in ["strokeUp_mc", "strokeDown_mc"] {
            let sk = k(&format!("_msgBox_mc/boxShape_mc/{s}"));
            anim.now(now, &sk, Act::Set(P::new().alpha(0.0).scale(300.0)));
            anim.now(now, &sk, Act::To(P::new().alpha(100.0).scale(100.0), 0.4, Ease::StrongInOut));
        }
        anim.now(now, &k("_msgBox_mc/btnBar_mc"), Act::Set(P::new().alpha(0.0).scale(250.0)));
        anim.now(now, &k("_msgBox_mc/btnBar_mc"), Act::To(P::new().alpha(100.0).scale(100.0), 0.3, Ease::StrongInOut));
        anim.now(now, &k("_bkgdWhite_mc"), Act::To(P::new().alpha(20.0), 0.25, Ease::StrongOut));
        anim.now(now, &k("_bkgdBlack_mc"), Act::To(P::new().alpha(60.0), 0.25, Ease::StrongOut));
        // Edge particles fly in from outside (`randRange` as written: floor(r*max - min + 1) + min).
        if let Some(pc) = art.child(art.export("gl_msgBox").unwrap_or(0), "_particles_mc").map(|(c, _)| c) {
            for (name, _, _) in art.children(pc) {
                let pk = k(&format!("_particles_mc/{name}"));
                let pp = anim.authored(&pk);
                let dx = anim.rand_range(0.0, 499.0) + 1.0;
                let dy = anim.rand_range(0.0, 499.0) + 1.0;
                let dr = anim.rand_range(-60.0, 60.0);
                let sx = if pp.x < 0.0 { -1.0 } else { 1.0 };
                let sy = if pp.y < 0.0 { -1.0 } else { 1.0 };
                anim.now(now, &pk, Act::Set(P::new().x(pp.x + sx * dx).y(pp.y + sy * dy).rot(pp.rotation + dr)));
                anim.now(now, &pk, Act::To(P::new().alpha(pp.alpha).x(pp.x).y(pp.y).rot(pp.rotation), 0.45, Ease::StrongOut));
            }
        }
        // Idle drift (`AnimBox`): the box grows to 105 % over 18 s, then 102.5 % over 8 s.
        anim.at(now + 0.25, &mb, Act::To(P::new().scale(105.0), 18.0, Ease::BackOut));
        anim.at(now + 0.25 + 12.5, &mb, Act::To(P::new().scale(102.5), 8.0, Ease::StrongOut));
        // `AnimBoxBkgd` / `_Loop`: the box art sways 20 px each way.
        let bk = k("_msgBox_mc/boxBkgd_mc/bkgd_mc");
        anim.now(now, &bk, Act::To(P::new().x(20.0).y(0.0), 8.0, Ease::StrongOut));
        anim.at(now + 2.5, &bk, Act::To(P::new().x(-20.0), 10.0, Ease::StrongOut));
        MsgBox { purpose: purpose.to_string(), text, buttons, sel: 0, opened: now, closing: None }
    }

    /// Inputs are taken 250 ms after opening (`OnOpened` -> `EnableInputs`).
    pub fn input_ready(&self, now: f64) -> bool {
        self.closing.is_none() && now - self.opened >= 0.25
    }

    /// `APressed`: close with the focused button (sound `Confirmation`).
    pub fn confirm(&mut self, anim: &mut Anim, now: f64) {
        if self.closing.is_some() {
            return;
        }
        self.closing = Some((now, self.sel as i32));
        anim.cancel_queued(MSG_KEY);
        anim.now(now, MSG_KEY, Act::To(P::new().alpha(0.0).scale(105.0), 0.45, Ease::StrongOut));
        anim.now(now, &format!("{MSG_KEY}/_msgBox_mc/boxBkgd_mc"), Act::To(P::new().alpha(0.0), 0.15, Ease::StrongOut));
    }

    /// The box was removed (250 ms after `Close`): the confirmed index.
    pub fn finished(&self, now: f64) -> Option<i32> {
        self.closing.filter(|(t, _)| now - t >= 0.25).map(|(_, i)| i)
    }

    /// Left / right: move the focus, `Stop` at the ends (no wrap).
    pub fn step(&mut self, dir: i32) -> &'static str {
        let n = self.buttons.len() as i32;
        let s = self.sel as i32 + dir;
        if s < 0 || s >= n {
            return "Stop";
        }
        self.sel = s as usize;
        "Move"
    }

    pub fn draw(&self, dl: &mut DisplayList, menu: &MenuArt, anim: &Anim, now: f64, focus_since: &dyn Fn(usize) -> (bool, f32)) {
        let Some(art) = menu.get("global") else { return };
        let Some(ch) = art.export("gl_msgBox") else { return };
        if self.finished(now).is_some() {
            return;
        }
        let root = anim.get(MSG_KEY, now);
        let m = root.matrix();
        let ovr = anim.ovr(MSG_KEY, now);
        let t = (now - self.opened) as f32;
        let st = States::default().with_timed("_msgBox_mc/boxShape_mc", "@0", t).clock(t);
        dl.movie_fx(art, ch, m, &st, root.alpha / 100.0, &ovr, true);
        // The question, centred on the box (`txt._y = -textHeight / 2`).
        if let Some((tc, tm)) = chain(art, anim, MSG_KEY, now, ch, m, &["_msgBox_mc", "_txt_mc"]) {
            let a = chain_alpha(anim, MSG_KEY, now, &["_msgBox_mc", "_txt_mc"]) * root.alpha / 100.0;
            if !DisplayList::fields(art, tc, tm, &States::default()).is_empty() {
                let scale = (tm.a * tm.a + tm.b * tm.b).sqrt();
                let lines = (menu.metrics.width(FontKind::Normal, &self.text, 23.0) / 912.0).ceil().max(1.0) + self.text.matches('\n').count() as f32;
                let h = lines * 23.0 * 1.25;
                let (cx, cy) = tm.apply(0.0, 0.0);
                let c = Vec2::new(cx, cy) / 20.0;
                dl.paragraph(&self.text, Rect::from_center_size(c, Vec2::new(912.0 * scale, h * scale)), 23.0 * scale, FontKind::Normal, TEXT.with_alpha(a), Justify::Center);
            }
        }
        // Buttons side by side in the bar, centred (`MovieClipCreator`, offsetX 0 on PC).
        let Some(bt) = art.export("gl_msgBox_btn") else { return };
        let Some((_, bar)) = chain(art, anim, MSG_KEY, now, ch, m, &["_msgBox_mc", "btnBar_mc"]) else { return };
        let bar_a = chain_alpha(anim, MSG_KEY, now, &["_msgBox_mc", "btnBar_mc"]) * root.alpha / 100.0;
        let w = 285.1f32;
        let n = self.buttons.len() as f32;
        for (i, label) in self.buttons.iter().enumerate() {
            let x = (w * 0.5 + i as f32 * w).round() - 0.5 * w * n;
            let bm = bar.mul(&px_matrix(x, 6.0));
            let (on, secs) = focus_since(i);
            let st = if on { States::all_timed("over", secs) } else if secs < 5.0 { States::all_timed("out", secs) } else { States::all("default") };
            dl.movie(art, bt, bm, &st, bar_a);
            let fields = DisplayList::fields(art, bt, bm, &st);
            if let Some(f) = DisplayList::label_field(&fields) {
                dl.field_text(art, f, &label.to_uppercase(), Some(TEXT.with_alpha(bar_a)));
            }
            let (cx, cy) = bm.apply(12.0 * 20.0, 1.5 * 20.0);
            dl.item(format!("msg:btn:{i}"), Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(249.0, 49.0)), true);
        }
    }
}
