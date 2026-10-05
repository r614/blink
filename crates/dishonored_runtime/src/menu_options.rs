//! The Options screen (`UI_OptionsMenu.OptionsMenu` in `Startup.upk`), shared by the
//! launcher and the pause menu: category tabs (General, Controls, Graphics, Audio), sub-tabs,
//! option lines with the movie's widgets (stepper, boolean stepper, slider, drop-down, link),
//! the keyboard mapping list, and the Video Settings / Brightness / controller scheme
//! secondary screens. Every original option is listed; the ones this port does not act on
//! yet are marked "not yet effective".
//!
//! Layout positions are this port's (the movie's `OptionsScreen`/`OptionsList` ActionScript
//! classes place the lines at runtime and are not executed); the art, widget geometry, state
//! labels, fonts, colours and texts are the movie's and the install's.

use crate::menu_art::{DisplayList, FontKind, MovieArt, States, TEXT, TEXT_DIM, px_matrix};
use crate::menu_audio::MenuSounds;
use crate::menu_ui::{MenuArt, MenuEvent, MenuNav, MenuTexts, SEC_BASE, SEC_MENU, SEC_TWK_MENU};
use crate::settings::GameSettings;
use bevy::prelude::*;
use dishonored_settings::{Category, Effect, Kind, OptionDef, Screen, Settings, SubCategory, Value};

const LIST_X: f32 = 170.0;
const LIST_TOP: f32 = 200.0;
const LINE_H: f32 = 44.0;
const WIDGET_X: f32 = 760.0;
const VISIBLE: usize = 10;

#[derive(Default, Clone)]
pub struct OptionsMenu {
    pub cat: usize,
    pub sub: usize,
    pub scroll: usize,
    pub secondary: Option<Screen>,
    /// Video settings being edited (applied on Accept).
    pub pending: Option<Settings>,
    /// Available resolutions (from the monitor).
    pub resolutions: Vec<(u32, u32)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// Redraw only.
    Redraw,
    /// Settings changed (saved).
    Changed,
    Close,
}

impl OptionsMenu {
    pub fn category(&self) -> Category {
        Category::ALL[self.cat.min(Category::ALL.len() - 1)]
    }
    pub fn sub_category(&self) -> SubCategory {
        let subs = self.category().subs();
        subs[self.sub.min(subs.len() - 1)]
    }

    /// Rows of the current list: option definitions or bindings.
    fn rows(&self, s: &Settings) -> Vec<Row> {
        match self.sub_category() {
            SubCategory::Keyboard => s.bindings.iter().map(|b| Row::Bind(b.action.clone())).collect(),
            sub => dishonored_settings::options_in(sub).map(|o| Row::Opt(o)).collect(),
        }
    }

    pub fn build(&self, dl: &mut DisplayList, menu: &MenuArt, t: &MenuTexts, gs: &GameSettings, nav: &MenuNav, show_bg: bool) {
        let Some(art) = menu.get("options") else { return };
        let gamma_art = menu.get("gamma");
        let s = self.pending.as_ref().unwrap_or(&gs.s);
        let focus = nav.focus.as_deref().unwrap_or("");
        if show_bg {
            backdrop(dl, art);
        }
        match self.secondary {
            Some(Screen::Video) => return self.build_video(dl, art, t, s, nav),
            Some(Screen::Gamma) => return self.build_gamma(dl, art, gamma_art, t, s, nav),
            Some(Screen::GamepadScheme) => return self.build_gamepad(dl, menu, art, t, s, nav),
            None => {}
        }
        // Title and category tabs.
        dl.text(t.t(SEC_BASE, "t_Options").to_uppercase(), Rect::new(110.0, 28.0, 700.0, 70.0), 38.0, FontKind::Title, TEXT, Justify::Left);
        for (i, c) in Category::ALL.iter().enumerate() {
            let x = 330.0 + i as f32 * 210.0;
            let id = format!("opt:tab:{i}");
            let state = if i == self.cat { "selected" } else if focus == id { "over" } else { "unselected" };
            self.tab(dl, art, &id, x, 105.0, &t.t(SEC_TWK_MENU, c.label_key()).to_uppercase(), state, 1.0);
        }
        let subs = self.category().subs();
        if subs.len() > 1 {
            for (i, sc) in subs.iter().enumerate() {
                let x = 640.0 + (i as f32 - (subs.len() as f32 - 1.0) * 0.5) * 260.0;
                let id = format!("opt:sub:{i}");
                let state = if i == self.sub { "selected" } else if focus == id { "over" } else { "unselected" };
                self.tab(dl, art, &id, x, 150.0, &t.t(SEC_TWK_MENU, sc.label_key().unwrap_or("")), state, 0.85);
            }
        }
        // Lines.
        let rows = self.rows(s);
        let end = (self.scroll + VISIBLE).min(rows.len());
        for (vi, row) in rows[self.scroll.min(rows.len())..end].iter().enumerate() {
            let y = LIST_TOP + vi as f32 * LINE_H;
            match row {
                Row::Opt(o) => self.option_line(dl, art, t, s, o, y, focus),
                Row::Bind(a) => self.binding_line(dl, art, t, s, a, y, focus, nav.capturing.as_deref()),
            }
        }
        // Scroll bar (thin) when the list is longer than the view.
        if rows.len() > VISIBLE {
            let h = VISIBLE as f32 * LINE_H;
            let top = LIST_TOP - LINE_H * 0.5;
            dl.fill(Rect::new(1150.0, top, 1153.0, top + h), Color::srgba(0.89, 0.95, 0.84, 0.15));
            let th = h * VISIBLE as f32 / rows.len() as f32;
            let ty = top + (h - th) * self.scroll as f32 / (rows.len() - VISIBLE) as f32;
            dl.fill(Rect::new(1149.0, ty, 1154.0, ty + th), Color::srgba(0.89, 0.95, 0.84, 0.7));
            dl.item("opt:scroll:-1", Rect::new(1135.0, top - 30.0, 1170.0, top), false);
            dl.item("opt:scroll:+1", Rect::new(1135.0, top + h, 1170.0, top + h + 30.0), false);
        }
        self.footer(dl, t, true);
    }

    fn footer(&self, dl: &mut DisplayList, t: &MenuTexts, restore: bool) {
        let y = 660.0;
        dl.text(format!("Esc   {}", t.t(SEC_BASE, "t_Back")), Rect::new(900.0, y, 1180.0, y + 30.0), 22.0, FontKind::Normal, TEXT, Justify::Right);
        dl.item("opt:back", Rect::new(1040.0, y, 1180.0, y + 30.0), false);
        if restore {
            dl.text(format!("R   {}", t.t(SEC_MENU, "t_RestoreSettings")), Rect::new(110.0, y, 600.0, y + 30.0), 22.0, FontKind::Normal, TEXT, Justify::Left);
            dl.item("opt:restore", Rect::new(110.0, y, 420.0, y + 30.0), false);
        }
    }

    fn tab(&self, dl: &mut DisplayList, art: &MovieArt, id: &str, x: f32, y: f32, text: &str, state: &str, scale: f32) {
        let Some(ch) = art.export("SubTab") else { return };
        let m = dishonored_assets::swf::Matrix { a: scale, d: scale, tx: x * 20.0, ty: y * 20.0, ..Default::default() };
        let st = States::all(state);
        dl.movie(art, ch, m, &st, 1.0);
        let col = if state == "selected" || state == "over" { TEXT } else { TEXT_DIM };
        for f in DisplayList::fields(art, ch, m, &st) {
            dl.field_text(art, &f, text, Some(col));
        }
        let w = (text.chars().count() as f32 * 13.0 * scale).max(120.0 * scale);
        dl.item(id, Rect::from_center_size(Vec2::new(x, y), Vec2::new(w + 30.0, 40.0 * scale)), true);
    }

    fn line_frame(&self, dl: &mut DisplayList, art: &MovieArt, export: &str, y: f32, focused: bool, label: &str, label_col: Color) {
        let Some(ch) = art.export(export) else { return };
        let m = px_matrix(LIST_X, y);
        let st = States::all(if focused { "over" } else { "default" });
        dl.movie(art, ch, m, &st, 1.0);
        if let Some(f) = DisplayList::fields(art, ch, m, &st).first() {
            dl.field_text(art, f, label, Some(label_col));
        }
    }

    fn option_line(&self, dl: &mut DisplayList, art: &MovieArt, t: &MenuTexts, s: &Settings, o: &OptionDef, y: f32, focus: &str) {
        let id = format!("opt:line:{}", o.id);
        let focused = focus == id;
        let saved_only = o.effect == Effect::SavedOnly;
        self.line_frame(dl, art, "o_options_line", y, focused, &t.option_label(o.id), TEXT);
        if saved_only {
            dl.text("not yet effective", Rect::new(LIST_X + 240.0, y - 12.0, WIDGET_X - 195.0, y + 12.0), 14.0, FontKind::Normal, TEXT_DIM, Justify::Right);
        }
        dl.item(&id, Rect::new(LIST_X, y - LINE_H * 0.5 + 2.0, 1130.0, y + LINE_H * 0.5 - 2.0), true);
        let st = States::all(if focused { "over" } else { "default" });
        let wm = px_matrix(WIDGET_X, y);
        match o.kind {
            Kind::Bool | Kind::Choice(_) | Kind::Resolution => {
                let value = value_text(t, o, s, &self.resolutions);
                let export = if o.kind == Kind::Resolution { "DropDownMenu_widget" } else { "OptionsStepper_widget" };
                if let Some(ch) = art.export(export) {
                    dl.movie(art, ch, wm, &st, 1.0);
                    if let Some(f) = DisplayList::fields(art, ch, wm, &st).first() {
                        dl.field_text(art, f, &value, Some(TEXT));
                    }
                }
                dl.item(format!("opt:arrow:{}:-1", o.id), Rect::new(WIDGET_X - 190.0, y - 18.0, WIDGET_X - 140.0, y + 18.0), false);
                dl.item(format!("opt:arrow:{}:+1", o.id), Rect::new(WIDGET_X + 140.0, y - 18.0, WIDGET_X + 190.0, y + 18.0), false);
            }
            Kind::Range { min, max, .. } => {
                let v = s.get(o.id).map(|v| v.as_f32()).unwrap_or(min);
                let frac = ((v - min) / (max - min)).clamp(0.0, 1.0);
                if let Some(ch) = art.export("Slider_widget") {
                    // Track and arrows at the widget origin; the thumb slides along the track.
                    let track = art.child(ch, "_bkgd_mc").and_then(|(bc, bm)| art.bounds(bc, wm.mul(&bm), &st)).unwrap_or(Rect::new(WIDGET_X - 130.0, y - 10.0, WIDGET_X + 130.0, y + 10.0));
                    for name in ["_rightArrow_mc", "_leftArrow_mc", "_bkgd_mc"] {
                        if let Some((c, m)) = art.child(ch, name) {
                            dl.movie(art, c, wm.mul(&m), &st, 1.0);
                        }
                    }
                    if let Some((tc, _)) = art.child(ch, "_thumb_mc") {
                        let half = art.bounds(tc, px_matrix(0.0, 0.0), &st).map(|r| r.width() * 0.5).unwrap_or(14.0);
                        let tx = track.min.x + half + (track.width() - 2.0 * half) * frac;
                        let tm = px_matrix(tx, y);
                        dl.movie(art, tc, tm, &st, 1.0);
                        if let Some(f) = DisplayList::fields(art, tc, tm, &st).first() {
                            dl.field_text(art, f, &number(v, o), None);
                        }
                    }
                    dl.item(format!("opt:slider:{}", o.id), Rect::new(track.min.x, y - 18.0, track.max.x, y + 18.0), false);
                }
                dl.item(format!("opt:arrow:{}:-1", o.id), Rect::new(WIDGET_X - 190.0, y - 18.0, WIDGET_X - 140.0, y + 18.0), false);
                dl.item(format!("opt:arrow:{}:+1", o.id), Rect::new(WIDGET_X + 140.0, y - 18.0, WIDGET_X + 190.0, y + 18.0), false);
            }
            Kind::Link(_) => {
                if let Some(ch) = art.export("Link_widget") {
                    dl.movie(art, ch, wm, &st, 1.0);
                    for f in DisplayList::fields(art, ch, wm, &st).iter().take(1) {
                        dl.field_text(art, f, &t.t(SEC_BASE, "t_Select"), Some(TEXT));
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn binding_line(&self, dl: &mut DisplayList, art: &MovieArt, t: &MenuTexts, s: &Settings, action: &str, y: f32, focus: &str, capturing: Option<&str>) {
        let id = format!("opt:bind:{action}");
        let focused = focus == id;
        let cap = capturing == Some(id.as_str());
        let applied = dishonored_settings::APPLIED_ACTIONS.contains(&action);
        let Some(ch) = art.export("o_MappingList_item") else { return };
        let m = px_matrix(LIST_X, y);
        let st = States::all(if focused { "over" } else { "default" }).with("widget_mc", if cap { "loop" } else { "default" });
        dl.movie(art, ch, m, &st, 1.0);
        let key = s.bindings.iter().find(|b| b.action == action).map(|b| b.key.as_str()).unwrap_or("None");
        let fields = DisplayList::fields(art, ch, m, &st);
        for f in &fields {
            if f.path.contains("widget") {
                let kl = if cap { "...".to_string() } else { t.key_label(key) };
                dl.field_text(art, f, &kl, Some(TEXT));
            } else {
                dl.field_text(art, f, &t.action_label(action), Some(TEXT));
            }
        }
        if !applied {
            dl.text("not yet effective", Rect::new(LIST_X + 280.0, y - 12.0, LIST_X + 520.0, y + 12.0), 14.0, FontKind::Normal, TEXT_DIM, Justify::Right);
        }
        dl.item(&id, Rect::new(LIST_X, y - LINE_H * 0.5 + 2.0, 1130.0, y + LINE_H * 0.5 - 2.0), true);
    }

    fn build_video(&self, dl: &mut DisplayList, art: &MovieArt, t: &MenuTexts, s: &Settings, nav: &MenuNav) {
        let focus = nav.focus.as_deref().unwrap_or("");
        let Some((ch, m)) = art.root_child("videoSettings_mc") else { return };
        // `_black_mc` backdrop (the movie fades it in), then the title and the lines.
        if let Some((b, bm)) = art.child(ch, "_black_mc") {
            dl.movie_solids(art, b, m.mul(&bm), &States::default(), 0.85);
        }
        dl.text(t.t(SEC_MENU, "t_Modify_VideoSettings").to_uppercase(), Rect::new(0.0, 225.0, 1280.0, 265.0), 30.0, FontKind::Title, TEXT, Justify::Center);
        let opts: Vec<&OptionDef> = dishonored_settings::video_options().collect();
        for (i, o) in opts.iter().enumerate() {
            self.option_line(dl, art, t, s, o, 320.0 + i as f32 * LINE_H, focus);
        }
        // Accept / Cancel buttons of the panel's button bar.
        let bar = art.child(ch, "_widget_mc").and_then(|(w, wm)| art.child(w, "buttonBar_mc").map(|(b, bm)| (b, m.mul(&wm).mul(&bm))));
        if let Some((b, bm)) = bar {
            if let Some(sp) = art.sprite(b) {
                for p in art.movie.display_list(sp, 0) {
                    if let (Some(c), false) = (p.character, matches!(p.name.as_deref(), Some("accept_mc") | Some("cancel_mc"))) {
                        dl.movie(art, c, bm.mul(&p.matrix.unwrap_or_default()), &States::default(), 1.0);
                    }
                }
            }
            for (name, id, label) in [("accept_mc", "video:accept", t.t(SEC_BASE, "t_Validate")), ("cancel_mc", "video:cancel", t.t(SEC_BASE, "t_Cancel"))] {
                if let Some((c, cm)) = art.child(b, name) {
                    let mm = bm.mul(&cm);
                    let st = States::all(if focus == id { "over" } else { "default" });
                    dl.movie(art, c, mm, &st, 1.0);
                    let r = art.bounds(c, mm, &st).unwrap_or(Rect::from_center_size(Vec2::new(mm.tx, mm.ty) / 20.0, Vec2::new(200.0, 40.0)));
                    for f in DisplayList::fields(art, c, mm, &st) {
                        dl.field_text(art, &f, &label.to_uppercase(), Some(TEXT));
                    }
                    dl.item(id, r, true);
                }
            }
        }
    }

    /// Brightness screen: `gammaSetting_mc` over a black backdrop and the `UI_Gamma`
    /// calibration logos (`gamma_logo`) at the five `m_BrightnessValues` (0.998 .. 0.80,
    /// `DefaultUI.ini`), each logo's visibility being `1 - value`.
    /// APPROXIMATION: the menu UI is not gamma-corrected here, so the logos preview the chosen
    /// gamma by raising their luminance to `2.2 / gamma`.
    fn build_gamma(&self, dl: &mut DisplayList, art: &MovieArt, gamma_art: Option<&MovieArt>, t: &MenuTexts, s: &Settings, nav: &MenuNav) {
        let focus = nav.focus.as_deref().unwrap_or("");
        dl.fill(Rect::new(-2000.0, -2000.0, 3280.0, 2720.0), Color::srgb(23.0 / 255.0, 25.0 / 255.0, 28.0 / 255.0));
        if let Some((c, g)) = gamma_art.and_then(|g| g.export("gamma_logo").map(|c| (c, g))) {
            let exp = 2.2 / s.graphics.gamma.max(0.1);
            for (i, v) in [0.998f32, 0.99, 0.95, 0.90, 0.80].iter().enumerate() {
                let x = 640.0 + (i as f32 - 2.0) * 150.0;
                let lum = (1.0 - v).powf(exp).clamp(0.0, 1.0);
                dl.movie(g, c, px_matrix(x, 220.0), &States::default(), lum);
            }
        }
        if let Some((ch, m)) = art.root_child("gammaSetting_mc") {
            dl.movie(art, ch, m, &States::default(), 1.0);
            for f in DisplayList::fields(art, ch, m, &States::default()) {
                let text = if f.path.contains("title") { t.option_label("PSI_Graphics_Gamma").to_uppercase() } else if f.path.contains("desc") { t.t(SEC_MENU, "t_GammaHint") } else { continue };
                dl.field_text(art, &f, &text, Some(TEXT));
            }
        }
        if let Some(o) = dishonored_settings::option("PSI_Graphics_Gamma") {
            self.option_line(dl, art, t, s, o, 560.0, focus);
        }
        self.footer(dl, t, false);
    }

    fn build_gamepad(&self, dl: &mut DisplayList, menu: &MenuArt, art: &MovieArt, t: &MenuTexts, s: &Settings, nav: &MenuNav) {
        let focus = nav.focus.as_deref().unwrap_or("");
        if let Some((ch, m)) = art.root_child("gamepadMapping_mc") {
            let st = States::all("PC");
            if let Some((b, bm)) = art.child(ch, "_black_mc") {
                dl.movie_solids(art, b, m.mul(&bm), &st, 0.9);
            }
            // The controller picture (`UI_GamepadMapping.Options_GamepadMapping_Xenon`) that the
            // movie's ImgLoader puts in `_img_mc`. APPROXIMATION: centred and fitted to the labels.
            if let Some((h, size)) = menu.images.get("gamepad") {
                let sc = (1000.0 / size.x).min(560.0 / size.y);
                let r = Rect::from_center_size(Vec2::new(640.0, 360.0), *size * sc);
                dl.prims.push(crate::menu_art::Prim::Quad { tex: h.clone(), src: [0.0, 0.0, size.x, size.y], dst: [(r.min.x, r.min.y), (r.max.x, r.min.y), (r.max.x, r.max.y), (r.min.x, r.max.y)], color: [1.0; 4] });
            }
            dl.movie(art, ch, m, &st, 1.0);
            // The scheme's action names (`m_GamepadMapping<N>[i]`) fill the picture's labels.
            let set = s.gamepad.binding_set + 1;
            let fields: Vec<_> = DisplayList::fields(art, ch, m, &st).into_iter().filter(|f| f.path.contains("_txt_mc")).collect();
            for (i, f) in fields.iter().enumerate() {
                let key = format!("m_GamepadMapping{set}[{i}]");
                if let Some(txt) = t.loc.get(SEC_TWK_MENU, &key) {
                    dl.field_text(art, f, txt, Some(TEXT));
                }
            }
        }
        if let Some(o) = dishonored_settings::option("PSI_Gamepad_BindingSet") {
            self.option_line(dl, art, t, s, o, 40.0, focus);
        }
        self.footer(dl, t, false);
    }

    /// Apply one menu event. `sounds` gets the original UI sound names.
    pub fn handle(&mut self, ev: &MenuEvent, gs: &mut GameSettings, nav: &mut MenuNav, sounds: &mut MenuSounds) -> Outcome {
        match ev {
            MenuEvent::Focus(_) => {
                sounds.play("Move");
                Outcome::Redraw
            }
            MenuEvent::Back => {
                if nav.capturing.is_some() {
                    return Outcome::None;
                }
                sounds.play("back");
                if self.secondary.take().is_some() {
                    self.pending = None;
                    nav.focus = None;
                    return Outcome::Redraw;
                }
                Outcome::Close
            }
            MenuEvent::Scroll(d) => {
                let rows = self.rows(&gs.s).len();
                let max = rows.saturating_sub(VISIBLE);
                let n = (self.scroll as i32 + d).clamp(0, max as i32) as usize;
                if n != self.scroll {
                    self.scroll = n;
                    return Outcome::Redraw;
                }
                Outcome::None
            }
            MenuEvent::Captured { id, key } => {
                if let (Some(action), Some(k)) = (id.strip_prefix("opt:bind:"), key) {
                    gs.s.rebind(action, k);
                    gs.commit();
                    sounds.play("validation");
                    return Outcome::Changed;
                }
                Outcome::Redraw
            }
            MenuEvent::TextCommit { .. } => Outcome::None,
            MenuEvent::Step { id, dir } => self.step(id, *dir, gs, sounds),
            MenuEvent::Activate { id, at } => self.activate(id, *at, gs, nav, sounds),
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

    fn step(&mut self, id: &str, dir: i32, gs: &mut GameSettings, sounds: &mut MenuSounds) -> Outcome {
        if let Some(i) = id.strip_prefix("opt:tab:").and_then(|x| x.parse::<usize>().ok()) {
            let _ = i;
            return self.change_tab((self.cat as i32 + dir).rem_euclid(4) as usize, sounds);
        }
        let Some(oid) = id.strip_prefix("opt:line:") else { return Outcome::None };
        let Some(def) = dishonored_settings::option(oid) else { return Outcome::None };
        if def.kind == Kind::Resolution {
            let res = self.resolutions.clone();
            let s = self.settings_mut(gs);
            let cur = s.graphics.resolution;
            let i = res.iter().position(|r| Some(*r) == cur).map(|i| i as i32).unwrap_or(-1);
            if !res.is_empty() {
                let n = (i + dir).rem_euclid(res.len() as i32) as usize;
                s.graphics.resolution = Some(res[n]);
            }
        } else if !matches!(def.kind, Kind::Link(_)) {
            self.settings_mut(gs).step(oid, dir);
        } else {
            return Outcome::None;
        }
        sounds.play("change");
        self.commit(gs)
    }

    fn change_tab(&mut self, cat: usize, sounds: &mut MenuSounds) -> Outcome {
        if cat != self.cat {
            self.cat = cat;
            self.sub = 0;
            self.scroll = 0;
            sounds.play("ChangeMainTab");
        }
        Outcome::Redraw
    }

    fn activate(&mut self, id: &str, at: Option<Vec2>, gs: &mut GameSettings, nav: &mut MenuNav, sounds: &mut MenuSounds) -> Outcome {
        if let Some(i) = id.strip_prefix("opt:tab:").and_then(|x| x.parse().ok()) {
            return self.change_tab(i, sounds);
        }
        if let Some(i) = id.strip_prefix("opt:sub:").and_then(|x| x.parse::<usize>().ok()) {
            if i != self.sub {
                self.sub = i;
                self.scroll = 0;
                sounds.play("ChangeSubTab");
            }
            return Outcome::Redraw;
        }
        if let Some(d) = id.strip_prefix("opt:scroll:").and_then(|x| x.parse::<i32>().ok()) {
            return self.handle(&MenuEvent::Scroll(d), gs, nav, sounds);
        }
        match id {
            "opt:back" => return self.handle(&MenuEvent::Back, gs, nav, sounds),
            "opt:restore" => {
                let sub = self.sub_category();
                gs.s.restore(sub);
                gs.commit();
                sounds.play("Reset");
                return Outcome::Changed;
            }
            "video:accept" => {
                if let Some(p) = self.pending.take() {
                    gs.s.graphics = p.graphics;
                    gs.commit();
                }
                self.secondary = None;
                nav.focus = None;
                sounds.play("validation");
                return Outcome::Changed;
            }
            "video:cancel" => {
                self.pending = None;
                self.secondary = None;
                nav.focus = None;
                sounds.play("back");
                return Outcome::Redraw;
            }
            _ => {}
        }
        if let Some(action) = id.strip_prefix("opt:bind:") {
            let _ = action;
            nav.capturing = Some(id.to_string());
            sounds.play("validation");
            return Outcome::Redraw;
        }
        if let Some(rest) = id.strip_prefix("opt:arrow:") {
            let (oid, d) = rest.rsplit_once(':').unwrap_or((rest, "+1"));
            let dir = if d.starts_with('-') { -1 } else { 1 };
            nav.focus = Some(format!("opt:line:{oid}"));
            return self.step(&format!("opt:line:{oid}"), dir, gs, sounds);
        }
        if let Some(oid) = id.strip_prefix("opt:slider:") {
            if let (Some(Kind::Range { min, max, .. }), Some(p)) = (dishonored_settings::option(oid).map(|o| o.kind), at) {
                let r = nav.items.iter().find(|i| i.id == id).map(|i| i.rect);
                if let Some(r) = r {
                    let f = ((p.x - r.min.x) / r.width().max(1.0)).clamp(0.0, 1.0);
                    self.settings_mut(gs).set(oid, Value::Float(min + (max - min) * f));
                    sounds.play("change");
                    return self.commit(gs);
                }
            }
            return Outcome::None;
        }
        if let Some(oid) = id.strip_prefix("opt:line:") {
            let Some(def) = dishonored_settings::option(oid) else { return Outcome::None };
            return match def.kind {
                Kind::Link(screen) => {
                    self.secondary = Some(screen);
                    if screen == Screen::Video {
                        self.pending = Some(gs.s.clone());
                    }
                    nav.focus = None;
                    sounds.play("OpenScreen");
                    Outcome::Redraw
                }
                Kind::Range { .. } => Outcome::None,
                _ => self.step(id, 1, gs, sounds),
            };
        }
        Outcome::None
    }
}

/// Options backdrop: the screen is dimmed (the movie's script fades the menu behind it) and
/// `options_mc/_bkgdTexture_mc` laid over it.
pub fn backdrop(dl: &mut DisplayList, art: &MovieArt) {
    dl.fill(Rect::new(-2000.0, -2000.0, 3280.0, 2720.0), Color::srgba(23.0 / 255.0, 25.0 / 255.0, 28.0 / 255.0, 0.72));
    if let Some((ch, m)) = art.root_child("options_mc") {
        dl.movie(art, ch, m, &States::default(), 1.0);
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

/// A line at `y` with the movie's line and widget art: `id` is the line item (nav),
/// arrows are `<id>:-1` / `<id>:+1`.
#[allow(clippy::too_many_arguments)]
pub fn generic_line(dl: &mut DisplayList, art: &MovieArt, y: f32, id: &str, label: &str, value: &str, widget: LineWidget, focused: bool) {
    let st = States::all(if focused { "over" } else { "default" });
    if let Some(ch) = art.export("o_options_line") {
        let m = px_matrix(LIST_X, y);
        dl.movie(art, ch, m, &st, 1.0);
        if let Some(f) = DisplayList::fields(art, ch, m, &st).first() {
            dl.field_text(art, f, label, Some(TEXT));
        }
    }
    if widget != LineWidget::Info {
        dl.item(id, Rect::new(LIST_X, y - LINE_H * 0.5 + 2.0, 1130.0, y + LINE_H * 0.5 - 2.0), true);
    }
    let wm = px_matrix(WIDGET_X, y);
    let export = match widget {
        LineWidget::Stepper => "OptionsStepper_widget",
        LineWidget::Text { .. } => "DropDownMenu_widget",
        LineWidget::Button => "Link_widget",
        LineWidget::Info => "",
    };
    if let Some(ch) = art.export(export) {
        dl.movie(art, ch, wm, &st, 1.0);
        if let Some(f) = DisplayList::fields(art, ch, wm, &st).first() {
            let v = match widget {
                LineWidget::Text { editing: true } => format!("{value}_"),
                _ => value.to_string(),
            };
            dl.field_text(art, f, &v, Some(TEXT));
        }
    } else if widget == LineWidget::Info {
        dl.text(value, Rect::new(WIDGET_X - 200.0, y - 15.0, 1130.0, y + 15.0), 20.0, FontKind::Normal, TEXT_DIM, Justify::Left);
    }
    if widget == LineWidget::Stepper {
        dl.item(format!("{id}:-1"), Rect::new(WIDGET_X - 190.0, y - 18.0, WIDGET_X - 140.0, y + 18.0), false);
        dl.item(format!("{id}:+1"), Rect::new(WIDGET_X + 140.0, y - 18.0, WIDGET_X + 190.0, y + 18.0), false);
    }
}

pub const GENERIC_LINE_H: f32 = LINE_H;

enum Row {
    Opt(&'static OptionDef),
    Bind(String),
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
