//! In-game pause menu (Esc), rebuilt from the original `UI_PauseMenu.PauseMenu` movie
//! (`UI_PauseMenu_SF`; classes `PauseScreen` 149, `PauseMenu` 148, `PauseMenuButton` 137,
//! `PauseScreenTitle` 144, `_common.AnimatedBackground` 140, `_common.Vignette` 157):
//!
//! - entries (`SetPauseMenu`): Resume (`t_ResumeGame`), Save Game / Load Game (`t_SaveGame`,
//!   `t_LoadGamePauseMenu`; struck through when not allowed), Options, Help (`t_Help`, struck
//!   through: the port has no tutorial notes), Back to Main Menu (`t_BackToMainMenu`, asks
//!   `t_Q_BackToMainMenu`), Back to Windows (`t_BackToWindows`, asks `t_Q_BackToWindows`);
//! - layout: a vertical list on a 53 px pitch, bottom-aligned at `_menu_mc` (48.15, 229.2),
//!   90 %, turned -10 deg; the vertical "PAUSE" title; the strip variant of item i is frame
//!   `i mod 4`;
//! - motion: `PauseScreen.Open` (lines, compass, texture at once; title 30 ms; lines 80 ms;
//!   input 150 ms; splatters 205 ms; Corvo 220 ms; blades 260 ms; the list at 275 ms, one
//!   item every 50 ms from 200 % and 520 px left), `Close` (0.2-0.4 s fades; hidden at 500 ms),
//!   focus (`x` 30, Back.easeOut / back), the selection parallax, the 150 ms confirm delay;
//! - sounds: `OpenScreen`, `Move`, `Validation`, `Failure`, `back`, `CloseScreen`.
//!
//! An offline match is paused while the menu is up, until its close is over (`OnMenuClosed`,
//! 500 ms after Resume); an online session keeps running, as the server does not pause.

use crate::menu_anim::{Act, Anim, Ease, P};
use crate::menu_art::{self, DisplayList, LayerCache, MovieArt, Props, States, TEXT};
use crate::menu_audio::MenuSounds;
use crate::menu_options::{OptionsMenu, Outcome};
use crate::menu_ui::{self, MenuArt, MenuEvent, MenuNav, MenuTexts, SEC_BASE, SEC_MENU};
use crate::menu_widgets::{HelpBar, MsgBox, author_tree, texts_any};
use crate::settings::GameSettings;
use bevy::prelude::*;
use dishonored_assets::swf::Matrix;

#[derive(Resource, Default)]
pub struct PauseMenu {
    /// The menu is up (also while it closes: the match resumes at `OnMenuClosed`).
    pub open: bool,
    pub options: Option<OptionsMenu>,
    root: Option<Entity>,
    cache: LayerCache,
    dirty: bool,
    /// The game was started by the launcher (`--from-launcher`): quitting returns to it.
    pub from_launcher: bool,
    /// Open on the next frame (harness control `pause`).
    pub request: bool,
    /// Save / load list open (`saveGame_mc` / `loadGame_mc`): (saving, entries (id, text)).
    list: Option<(bool, Vec<(String, String)>)>,
    anim: Anim,
    opened_at: f64,
    /// `Close()` time and whether it goes back to the game.
    closing: Option<(f64, bool)>,
    /// The confirmed entry runs at this time (`OnPlayerChoiceConfirm`, 150 ms).
    pending: Option<(f64, String)>,
    input_from: f64,
    sel: usize,
    ids: Vec<&'static str>,
    msg: Option<MsgBox>,
    msg_focus: (usize, f64),
    list_sel: usize,
    now: f64,
    /// The game over screen (`PauseScreen.ShowGameOverMenu(reason)`): the player died; the
    /// message is `m_GameOverMessage`, else the pause tweak's `m_PlayerDiedMessage`.
    pub game_over: Option<String>,
    /// Seconds the local player has been dead (starts the game over screen).
    dead_for: f32,
    /// Other full-screen movies up ([`OVERLAY_JOURNAL`], [`OVERLAY_STATS`]): they pause an
    /// offline match too (`bPauseGameWhileActive`), free the cursor and keep this menu closed.
    pub overlay: u8,
}

/// [`PauseMenu::overlay`] bits.
pub const OVERLAY_JOURNAL: u8 = 1;
pub const OVERLAY_STATS: u8 = 2;

impl PauseMenu {
    /// The entries of the screen up: the pause list or the game over list.
    fn items(&self) -> &'static [(&'static str, &'static str, &'static str)] {
        if self.game_over.is_some() { GAME_OVER_ITEMS } else { ITEMS }
    }
}

/// `DisTweaks_GFxMoviePlayerPauseMenu.m_PlayerDiedMessage` (the tweak's value).
pub const PLAYER_DIED_MESSAGE: &str = "You have met your demise.";
/// Seconds between the death and the game over screen (APPROXIMATION, U-889: the native trigger
/// is not traced).
pub const GAME_OVER_DELAY: f32 = 2.0;

/// Entries: (id, text section, key, locked when).
const ITEMS: &[(&str, &str, &str)] = &[
    ("pause:resume", SEC_MENU, "t_ResumeGame"),
    ("pause:save", SEC_MENU, "t_SaveGame"),
    ("pause:load", SEC_MENU, "t_LoadGamePauseMenu"),
    ("pause:options", SEC_BASE, "t_Options"),
    ("pause:help", SEC_MENU, "t_Help"),
    ("pause:quit", SEC_MENU, "t_BackToMainMenu"),
    ("pause:windows", SEC_MENU, "t_BackToWindows"),
];

/// `PauseScreen.SetGameOverMenu`: Resume From Last Save (`bSkipCloseMenu`), Load game, Back to
/// Main Menu, Back to Windows.
const GAME_OVER_ITEMS: &[(&str, &str, &str)] = &[
    ("go:continue", SEC_MENU, "t_ContinueFromLastSave"),
    ("go:load", SEC_MENU, "t_LoadASpecificSave"),
    ("pause:quit", SEC_MENU, "t_BackToMainMenu"),
    ("pause:windows", SEC_MENU, "t_BackToWindows"),
];

/// Item height and spacing (`p_pauseMenu_item` 48 px, `_offsetBtnY` 5).
const ITEM_H: f32 = 48.0;
const ITEM_GAP: f32 = 5.0;

pub fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let root = menu_art::spawn_layer(&mut commands, 50);
    commands.entity(root).insert((Visibility::Hidden, menu_art::DrawsBlackStripes));
    commands.insert_resource(menu_ui::load_menu_art(&["pause", "options", "gamma", "common", "global", "load", "journal", "stats"], &mut images));
    commands.insert_resource(MenuTexts::load());
    commands.insert_resource(PauseMenu { root: Some(root), from_launcher: std::env::args().any(|a| a == "--from-launcher"), anim: Anim::new(0xa5e), ..default() });
}

/// Fixed-tick run condition: an offline match stops while paused.
pub fn sim_running(p: Option<Res<PauseMenu>>, loading: Option<Res<crate::loading_screen::LoadingScreen>>, session: Res<crate::session::Session>) -> bool {
    // The loading screen also stops an offline match (the original loads levels synchronously).
    let stopped = p.is_some_and(|p| p.open || p.overlay != 0) || loading.is_some_and(|l| l.active);
    !(stopped && matches!(session.kind, crate::session::SessionKind::Offline { .. }))
}

fn locked(id: &str, saves: Option<&crate::saves::Saves>) -> bool {
    match id {
        "pause:save" | "pause:load" | "go:load" => saves.is_none(),
        // (`lockState: !_isContinueGame`: a save to continue from)
        "go:continue" => saves.is_none_or(|s| s.list().is_empty()),
        // No tutorial notes in the port (`ShowPauseMenu(showHelpButton)` false).
        "pause:help" => true,
        _ => false,
    }
}

/// `PauseScreen.Open` at `now` (also `BackToPauseMenu`).
fn open_screen(p: &mut PauseMenu, art: &MenuArt, sounds: &mut MenuSounds, now: f64, first: bool) {
    let Some(pm) = art.get("pause") else {
        p.input_from = now;
        return;
    };
    p.opened_at = now;
    p.closing = None;
    p.input_from = now + 0.15;
    p.pending = None;
    // (the game over screen plays `GO_OpenGameOverMenu` instead: `ShowGameOverMenu`)
    sounds.play(if p.game_over.is_some() { "GO_OpenGameOverMenu" } else { "OpenScreen" });
    let game_over = p.game_over.is_some();
    let n_items = p.items().len();
    let a = &mut p.anim;
    if let Some((ch, m)) = pm.root_child("pauseMenu_mc") {
        a.author("pm", Props::from_matrix(&m, 100.0));
        author_tree(a, pm, "pm", ch, 3);
    }
    a.now(now, "pm", Act::Set(P::new().visible(true).alpha(100.0)));
    // Background and vignette open with the movie (first open) or stay.
    if first {
        open_background(a, pm, now);
    }
    let k = |s: &str| format!("pm/{s}");
    let ah = |a: &Anim, s: &str| a.authored(&format!("pm/{s}"));
    let lh = ah(a, "_lineH_mc");
    a.now(now, &k("_lineH_mc"), Act::Set(P::new().alpha(0.0).x(lh.x + 200.0)));
    a.now(now, &k("_lineH_mc"), Act::To(P::all(lh), 0.35, Ease::BackInOut));
    let lv = ah(a, "_lineV_mc");
    a.now(now, &k("_lineV_mc"), Act::Set(P::new().alpha(0.0).y(lv.y + 200.0)));
    a.now(now, &k("_lineV_mc"), Act::To(P::all(lv), 0.35, Ease::BackInOut));
    let mk = ah(a, "_maskMenu_mc");
    a.now(now, &k("_maskMenu_mc"), Act::To(P::all(mk), 0.2, Ease::StrongOut));
    a.now(now, &k("_menu_mc"), Act::Set(P::new().alpha(0.0).scale(90.0).rot(-10.0)));
    let cp = ah(a, "_compass_mc");
    a.now(now, &k("_compass_mc"), Act::Set(P::new().scale(250.0).alpha(0.0)));
    a.now(now, &k("_compass_mc"), Act::To(P::all(cp), 0.25, Ease::StrongOut));
    a.now(now, &k("_compass_mc"), Act::Play("loop".into()));
    let tp = ah(a, "_texture_mc");
    a.now(now, &k("_texture_mc"), Act::Set(P::new().alpha(0.0).x(tp.x - 200.0)));
    a.now(now, &k("_texture_mc"), Act::To(P::all(tp), 0.25, Ease::StrongOut));
    for s in ["_splatters_mc/splatter0_mc", "_splatters_mc/splatter1_mc", "_splatters_mc/splatter2_mc", "_line0_mc", "_line1_mc", "_line2_mc"] {
        a.now(now, &k(s), Act::End);
        a.now(now, &k(s), Act::Set(P::new().alpha(0.0)));
    }
    a.now(now, &k("_blades_mc"), Act::Set(P::new().visible(false)));
    a.now(now, &k("_corvo_mc"), Act::Set(P::new().visible(false)));
    // 30 ms: the title (`PauseScreenTitle`: bkgd from 320 px left, lines at +150 ms, the first
    // shadow at +150 ms, the text at +250 ms; the second shadow never fades in).
    let t = now + 0.03;
    for s in ["_txtShadow0_mc", "_txtShadow1_mc", "_txt_mc", "_bkgd_mc"] {
        a.now(now, &k(&format!("_title_mc/{s}")), Act::Set(P::new().alpha(0.0)));
    }
    let tb = ah(a, "_title_mc/_bkgd_mc");
    a.at(t, &k("_title_mc/_bkgd_mc"), Act::Set(P::new().alpha(0.0).x(tb.x - 320.0)));
    a.at(t, &k("_title_mc/_bkgd_mc"), Act::To(P::all(tb), 0.3, Ease::StrongInOut));
    a.at(t + 0.15, &k("_title_mc/_bkgd_mc/lineUp_mc"), Act::From(P::new().alpha(0.0).x(-350.0).scale(150.0), 0.3, Ease::StrongOut));
    a.at(t + 0.15, &k("_title_mc/_bkgd_mc/lineDown_mc"), Act::From(P::new().alpha(0.0).x(1250.0).scale(150.0), 0.35, Ease::StrongOut));
    let s0 = ah(a, "_title_mc/_txtShadow0_mc");
    a.at(t + 0.15, &k("_title_mc/_txtShadow0_mc"), Act::Set(P::new().alpha(0.0).x(s0.x - 20.0).scale(200.0).rot(-5.0)));
    a.at(t + 0.15, &k("_title_mc/_txtShadow0_mc"), Act::To(P::all(s0), 0.25, Ease::BackInOut));
    let tx = ah(a, "_title_mc/_txt_mc");
    a.at(t + 0.25, &k("_title_mc/_txt_mc"), Act::Set(P::new().alpha(0.0).x(tx.x - 20.0).scale(200.0).rot(-5.0)));
    a.at(t + 0.25, &k("_title_mc/_txt_mc"), Act::To(P::all(tx), 0.25, Ease::StrongOut));
    // `GameOverTitle` (`_gameOver_mc`, `OpenTitle` at 30 ms): the text in from 150 px right and
    // the backing from 350 px left, 0.25 s `Strong.easeOut`.
    if game_over {
        let gt = ah(a, "_gameOver_mc/_txt_mc");
        a.now(now, &k("_gameOver_mc"), Act::Set(P::new().visible(false)));
        a.at(t, &k("_gameOver_mc"), Act::Set(P::new().visible(true)));
        a.at(t, &k("_gameOver_mc/_txt_mc"), Act::Set(P::new().alpha(0.0).x(gt.x + 150.0)));
        a.at(t, &k("_gameOver_mc/_txt_mc"), Act::To(P::all(gt), 0.25, Ease::StrongOut));
        let gb = ah(a, "_gameOver_mc/_bkgd_mc");
        a.at(t, &k("_gameOver_mc/_bkgd_mc"), Act::Set(P::new().alpha(0.0).x(gb.x - 350.0)));
        a.at(t, &k("_gameOver_mc/_bkgd_mc"), Act::To(P::all(gb), 0.25, Ease::StrongOut));
    }
    // 80 ms: the strokes.
    for (s, dy, d, e) in [("_line0_mc", -150.0, 0.25, Ease::StrongInOut), ("_line1_mc", 250.0, 0.45, Ease::StrongInOut), ("_line2_mc", 250.0, 0.25, Ease::StrongOut)] {
        let lp = ah(a, s);
        a.at(now + 0.08, &k(s), Act::Set(P::new().y(lp.y + dy)));
        a.at(now + 0.08, &k(s), Act::To(P::all(lp), d, e));
    }
    // 205 ms: splatters.
    for (i, (s, d)) in [(120.0, 0.25), (130.0, 0.35), (180.0, 0.4)].iter().enumerate() {
        let kk = format!("_splatters_mc/splatter{i}_mc");
        let sp = ah(a, &kk);
        a.at(now + 0.205, &k(&kk), Act::Set(P::new().scale(*s)));
        a.at(now + 0.205, &k(&kk), Act::To(P::all(sp), *d, Ease::StrongOut));
    }
    // 220 ms: the mask portrait.
    let co = ah(a, "_corvo_mc");
    a.at(now + 0.22, &k("_corvo_mc"), Act::Set(P::new().visible(true).scale(280.0).alpha(0.0).x(co.x - 10.0).y(co.y + 200.0)));
    a.at(now + 0.22, &k("_corvo_mc"), Act::To(P::all(co), 0.25, Ease::StrongOut));
    if game_over {
        a.at(now + 0.22, &k("_corvo_mc"), Act::Play("anim_slice".into()));
    }
    // 260 ms: the blades fly in (`AnimatedBlades.Open`).
    a.at(now + 0.26, &k("_blades_mc"), Act::Set(P::new().visible(true)));
    if let Some(bl) = pm.root_child("pauseMenu_mc").and_then(|(ch, _)| pm.child(ch, "_blades_mc")).map(|c| c.0) {
        let d = 0.01 * a.rand_range(55.0, 105.0);
        for (name, _, _) in pm.children(bl) {
            let kk = format!("pm/_blades_mc/{name}");
            let bp = a.authored(&kk);
            let sx = if bp.x < 0.0 { -1.0 } else { 1.0 };
            let sy = if bp.y < 0.0 { -1.0 } else { 1.0 };
            let x = bp.x + sx * a.rand_range(850.0, 1050.0);
            let y = bp.y + sy * a.rand_range(20.0, 40.0);
            let r = a.rand_range(30.0, 80.0);
            a.at(now + 0.26, &kk, Act::From(P::new().alpha(50.0).x(x).y(y).rot(r), d, Ease::StrongOut));
        }
    }
    // 275 ms: the list (`OpenMenu`: alpha 100 in 0.25 s; items every 50 ms).
    a.at(now + 0.275, &k("_menu_mc"), Act::To(P::new().alpha(100.0).scale(90.0).rot(-10.0), 0.25, Ease::StrongOut));
    for i in 0..n_items {
        let kk = format!("pm/item/{i}");
        a.forget(&kk);
        a.author(&kk, Props::default());
        a.now(now, &kk, Act::Set(P::new().alpha(0.0)));
        let ti = now + 0.275 + 0.05 * i as f64;
        a.at(ti, &kk, Act::Set(P::new().scale(200.0).x(-520.0)));
        a.at(ti, &kk, Act::To(P::new().scale(100.0).x(0.0).alpha(100.0), 0.25, Ease::BackOut));
    }
    // The current entry's timeline is on `over` (its x tween is overridden by the open tween).
    let sel = p.sel;
    p.anim.now(now, &format!("pm/item/{sel}"), Act::Play("over".into()));
    let n = p.items().len();
    parallax(&mut p.anim, sel, n, now);
}

/// `AnimatedBackground.Open` (`bkgd_mc`, when the movie starts) and `Vignette.Open`.
fn open_background(a: &mut Anim, pm: &MovieArt, now: f64) {
    if let Some((ch, m)) = pm.root_child("bkgd_mc") {
        a.author("bg", Props::from_matrix(&m, 100.0));
        author_tree(a, pm, "bg", ch, 2);
        let d = 0.01 * a.rand_range(100.0, 200.0);
        for group in ["_blades_mc", "_particles_mc"] {
            if let Some((gc, _)) = pm.child(ch, group) {
                for (name, _, _) in pm.children(gc) {
                    let k = format!("bg/{group}/{name}");
                    let p = a.authored(&k);
                    let sx = if p.x < 0.0 { -1.0 } else { 1.0 };
                    let sy = if p.y < 0.0 { -1.0 } else { 1.0 };
                    let x = p.x + sx * a.rand_range(300.0, 700.0);
                    let y = p.y + sy * a.rand_range(200.0, 400.0);
                    let r = a.rand_range(30.0, 80.0);
                    a.now(now, &k, Act::From(P::new().alpha(50.0).x(x).y(y).rot(r), d, Ease::StrongOut));
                }
            }
        }
        a.now(now, "bg/_white_mc", Act::Set(P::new().alpha(0.0)));
        a.now(now, "bg/_white_mc", Act::To(P::new().alpha(15.0), 0.25, Ease::StrongOut));
    }
    a.author("vig", Props { x: 640.0, y: 360.0, ..Props::default() });
    a.now(now, "vig", Act::Set(P::new().alpha(0.0).scale(110.0)));
    a.now(now, "vig", Act::To(P::new().alpha(100.0).scale(100.0), 0.25, Ease::StrongOut));
    // After 0.5 s the inner edges flicker between 95 % and 100 % every 100 ms.
    a.author("vig/_vignette_mc", Props { x: 0.7, y: 0.7, ..Props::default() });
    let mut t = now + 0.5;
    let mut lo = true;
    while t < now + 120.0 {
        a.at(t, "vig/_vignette_mc", Act::To(P::new().alpha(if lo { 95.0 } else { 100.0 }), 0.1, Ease::StrongOut));
        t += 0.1;
        lo = !lo;
    }
}

/// `PauseMenu.onSelectionUpdated`: the compass turns and the panel parts shift.
fn parallax(a: &mut Anim, idx: usize, n: usize, now: f64) {
    let p = idx as f32 * 100.0 / n.saturating_sub(1).max(1) as f32;
    a.now(now, "pm/_compass_mc/compass0_mc", Act::To(P::new().rot(-10.25 * idx as f32), 0.25, Ease::StrongOut));
    a.now(now, "pm/_compass_mc/compass1_mc", Act::To(P::new().rot(7.5 * idx as f32), 0.25, Ease::StrongOut));
    let tp = a.authored("pm/_texture_mc");
    a.now(now, "pm/_texture_mc", Act::To(P::new().x(tp.x - 0.3 * p).y(tp.y - 0.085 * p), 0.25, Ease::StrongOut));
    a.now(now, "pm", Act::To(P::new().x(640.0 + 0.3 * p).y(360.0 - 0.085 * p), 0.25, Ease::StrongOut));
    let cp = a.authored("pm/_corvo_mc");
    a.now(now, "pm/_corvo_mc", Act::To(P::new().x(cp.x - 0.3 * p).y(cp.y - 0.085 * p).scale(100.0 - 0.1 * p), 0.25, Ease::StrongOut));
    let bp = a.authored("pm/_blades_mc");
    a.now(now, "pm/_blades_mc", Act::To(P::new().x(bp.x - 0.08 * p).y(bp.y - 0.085 * p).scale(100.0 - 0.1 * p), 0.25, Ease::StrongOut));
}

/// `PauseScreen.Close(bBackToGame)`.
fn close_screen(p: &mut PauseMenu, sounds: &mut MenuSounds, now: f64, back_to_game: bool) {
    p.closing = Some((now, back_to_game));
    p.input_from = f64::INFINITY;
    let a = &mut p.anim;
    let k = |s: &str| format!("pm/{s}");
    let ah = |a: &Anim, s: &str| a.authored(&format!("pm/{s}"));
    if back_to_game {
        sounds.play("CloseScreen");
        a.now(now, "bg/_white_mc", Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
        a.now(now, "vig", Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
    }
    let lh = ah(a, "_lineH_mc");
    a.now(now, &k("_lineH_mc"), Act::To(P::new().alpha(0.0).x(lh.x - 250.0), 0.35, Ease::StrongOut));
    // (`_lineV_mc` uses `_lineH_mc`'s y, as the original does.)
    a.now(now, &k("_lineV_mc"), Act::To(P::new().alpha(0.0).y(lh.y - 250.0), 0.35, Ease::StrongOut));
    let mp = ah(a, "_menu_mc");
    a.now(now, &k("_menu_mc"), Act::To(P::new().alpha(0.0).x(mp.x - 50.0), 0.2, Ease::StrongOut));
    let mk = ah(a, "_maskMenu_mc");
    a.now(now, &k("_maskMenu_mc"), Act::To(P::new().x(mk.x - 90.0), 0.2, Ease::StrongOut));
    for s in ["_compass_mc", "_texture_mc"] {
        let x = a.get(&k(s), now).x;
        a.now(now, &k(s), Act::To(P::new().x(x - 150.0).alpha(0.0), 0.25, Ease::StrongOut));
    }
    a.now(now, &k("_corvo_mc"), Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
    for (i, d) in [0.25, 0.35, 0.4].iter().enumerate() {
        a.now(now, &k(&format!("_splatters_mc/splatter{i}_mc")), Act::To(P::new().alpha(0.0), *d, Ease::StrongOut));
    }
    let l0 = ah(a, "_line0_mc");
    a.now(now, &k("_line0_mc"), Act::To(P::new().alpha(0.0).y(l0.y + 250.0), 0.25, Ease::StrongOut));
    let l1 = ah(a, "_line1_mc");
    a.now(now, &k("_line1_mc"), Act::To(P::new().alpha(0.0).y(l1.y - 250.0), 0.3, Ease::StrongOut));
    // (`_line2_mc` uses `_line1_mc`'s y.)
    a.now(now, &k("_line2_mc"), Act::To(P::new().alpha(0.0).y(l1.y + 150.0), 0.27, Ease::StrongInOut));
    a.now(now, &k("_blades_mc"), Act::To(P::new().alpha(0.0), 0.4, Ease::StrongOut));
    let tx = a.get(&k("_title_mc"), now).x;
    a.now(now, &k("_title_mc"), Act::To(P::new().alpha(0.0).scale(110.0 * 0.83).x(tx - 150.0), 0.25, Ease::StrongOut));
    a.cancel_queued("pm/item/");
}

#[allow(clippy::too_many_arguments)]
pub fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mut p: ResMut<PauseMenu>,
    mut nav: ResMut<MenuNav>,
    mut local: ResMut<crate::input_map::LocalInput>,
    mut sounds: ResMut<MenuSounds>,
    art: Option<Res<MenuArt>>,
    time: Res<Time<Real>>,
    mut vis: Query<&mut Visibility, With<menu_art::MenuLayer>>,
    (view, session): (Res<crate::session::SessionView>, Res<crate::session::Session>),
    hud: Option<ResMut<crate::gfx_hud::GfxHud>>,
) {
    let now = time.elapsed_secs_f64();
    p.now = now;
    // Harness: `{"hud": {"kind": "game_over"}}`.
    if let Some(mut h) = hud {
        let before = h.tests.len();
        h.tests.retain(|t| !matches!(t, crate::hud_popups::HudTest::GameOver));
        if h.tests.len() != before && !p.open {
            p.game_over = Some(PLAYER_DIED_MESSAGE.to_string());
            p.request = true;
        }
    }
    // The player is dead in an offline mission that does not respawn him: the game over screen
    // (`PauseScreen.ShowGameOverMenu`) after a moment.
    let dead = view.me.as_ref().is_some_and(|m| !m.alive) && view.respawn_in.is_none() && view.matchv.mission.is_some() && matches!(session.kind, crate::session::SessionKind::Offline { .. });
    if !dead {
        p.dead_for = 0.0;
    } else if !p.open {
        p.dead_for += time.delta_secs();
        if p.dead_for >= GAME_OVER_DELAY && p.game_over.is_none() {
            p.game_over = Some(PLAYER_DIED_MESSAGE.to_string());
            p.request = true;
        }
    }
    if !p.open && ((keys.just_pressed(KeyCode::Escape) && local.override_input.is_none() && !dead && p.overlay == 0) || p.request) {
        p.request = false;
        p.open = true;
        p.dirty = true;
        p.options = None;
        p.list = None;
        p.msg = None;
        p.sel = 0;
        nav.enabled = false;
        nav.settle = 1;
        nav.focus = None;
        nav.wrap = true;
        nav.horizontal = false;
        local.enabled = false;
        p.anim.clear();
        if let Some(a) = art.as_deref() {
            open_screen(&mut p, a, &mut sounds, now, true);
        } else {
            p.input_from = now;
        }
    }
    if p.open {
        p.anim.advance(now);
        let ready = p.msg.as_ref().map(|m| m.input_ready(now)).unwrap_or(p.pending.is_none() && now >= p.input_from);
        nav.enabled = ready;
        // The pause list loops (`_bSelectionLoop`); the save list and the options do not.
        nav.wrap = p.msg.is_none() && p.options.is_none() && p.list.is_none();
        nav.horizontal = p.msg.is_some();
    }
    if let Some(root) = p.root {
        if let Ok(mut v) = vis.get_mut(root) {
            let want = if p.open { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
        }
    }
}

fn finish_close(p: &mut PauseMenu, nav: &mut MenuNav, local: &mut crate::input_map::LocalInput) {
    p.open = false;
    p.game_over = None;
    p.options = None;
    p.list = None;
    p.closing = None;
    p.msg = None;
    nav.enabled = false;
    nav.items.clear();
    local.enabled = true;
}

#[allow(clippy::too_many_arguments)]
pub fn handle(
    mut p: ResMut<PauseMenu>,
    mut nav: ResMut<MenuNav>,
    mut gs: ResMut<GameSettings>,
    mut sounds: ResMut<MenuSounds>,
    mut local: ResMut<crate::input_map::LocalInput>,
    mut exit: MessageWriter<AppExit>,
    mut saves: Option<ResMut<crate::saves::Saves>>,
    texts: Option<Res<MenuTexts>>,
    art: Option<Res<MenuArt>>,
    mut back: Option<ResMut<crate::handoff::ReturnToMenu>>,
) {
    if !p.open {
        nav.events.clear();
        return;
    }
    let now = p.now;
    // The menu's close is over: `OnMenuClosed` (the match resumes).
    if let Some((t, true)) = p.closing {
        if now - t >= 0.5 {
            finish_close(&mut p, &mut nav, &mut local);
            return;
        }
    }
    // The confirm delay.
    if let Some((at, id)) = p.pending.clone() {
        if now >= at {
            p.pending = None;
            if id == "go:continue" {
                // `OnLoadLastSaveClicked`: `Dis_Load 0`, the newest save.
                if let Some(s) = saves.as_mut() {
                    s.load_slot(0);
                }
                close_screen(&mut p, &mut sounds, now, true);
            } else {
                run_entry(&mut p, &id, art.as_deref(), texts.as_deref(), &mut sounds, saves.as_deref(), now);
            }
        }
    }
    // The question was answered.
    if let Some(idx) = p.msg.as_ref().and_then(|m| m.finished(now)) {
        let purpose = p.msg.take().map(|m| m.purpose).unwrap_or_default();
        match (purpose.as_str(), idx) {
            ("quit", 0) => match (p.from_launcher, back.as_mut()) {
                // The launcher shows its menu in this window's place, then this exits.
                (true, Some(r)) => r.request = true,
                (from_launcher, _) => {
                    if !from_launcher {
                        // Started from the command line: open a launcher for the player.
                        if let Ok(exe) = std::env::current_exe() {
                            let _ = std::process::Command::new(exe).spawn();
                        }
                    }
                    exit.write(AppExit::Success);
                }
            },
            ("windows", 0) => crate::handoff::quit_to_desktop(&mut exit),
            ("save", 0) | ("load", 0) => {
                if let (Some(s), Some(id)) = (saves.as_mut(), p.list.as_ref().and_then(|(_, e)| e.get(p.list_sel)).map(|e| e.0.clone())) {
                    if let Some(rest) = id.strip_prefix("save:") {
                        if purpose == "save" {
                            s.save(rest.parse().unwrap_or(dishonored_save::slot::MANUAL0));
                        } else {
                            s.load_file(std::path::PathBuf::from(rest));
                        }
                    }
                }
                p.list = None;
                close_screen(&mut p, &mut sounds, now, true);
            }
            (opt, i) if opt.starts_with("opt:") => {
                if let Some(o) = p.options.as_mut() {
                    o.answer(&opt[4..], i, &mut gs, now);
                }
            }
            ("save", _) | ("load", _) => {}
            _ => {
                // No: back to the pause entries (`BackToPauseMenu`).
                if let Some(a) = art.as_deref() {
                    open_screen(&mut p, a, &mut sounds, now, false);
                }
            }
        }
        nav.focus = None;
        p.dirty = true;
    }
    let events = std::mem::take(&mut nav.events);
    for ev in events {
        p.dirty = true;
        if p.msg.is_some() {
            let pp = &mut *p;
            let Some(m) = pp.msg.as_mut() else { continue };
            match &ev {
                MenuEvent::Focus(id) => {
                    if let Some(i) = id.strip_prefix("msg:btn:").and_then(|x| x.parse::<usize>().ok()) {
                        m.sel = i;
                        pp.msg_focus = (i, now);
                        sounds.play("Move");
                    }
                }
                MenuEvent::Step { dir, .. } => {
                    let s = m.step(*dir);
                    sounds.play(s);
                    pp.msg_focus = (m.sel, now);
                }
                MenuEvent::Activate { id, .. } if id.starts_with("msg:btn:") => {
                    if let Some(i) = id.strip_prefix("msg:btn:").and_then(|x| x.parse::<usize>().ok()) {
                        m.sel = i.min(m.buttons.len().saturating_sub(1));
                    }
                    sounds.play("Confirmation");
                    m.confirm(&mut pp.anim, now);
                }
                _ => {}
            }
            continue;
        }
        if p.options.is_some() {
            let out = match p.options.as_mut() {
                Some(o) => o.handle(&ev, &mut gs, &mut nav, &mut sounds, now),
                None => Outcome::None,
            };
            match out {
                Outcome::Close => {
                    // `OptionsScreen.Close` -> `BackToPauseMenu` at once (the options keep
                    // drawing their close).
                    if let Some(a) = art.as_deref() {
                        open_screen(&mut p, a, &mut sounds, now, false);
                    }
                    nav.focus = Some(p.items()[p.sel].0.into());
                }
                Outcome::Ask(purpose, payload) => {
                    if let (Some(a), Some(t)) = (art.as_deref(), texts.as_deref()) {
                        let text = crate::menu_options::question(t, &purpose, &payload);
                        ask(&mut p, a, t, &mut sounds, &format!("opt:{purpose}"), text, now);
                    }
                }
                _ => {}
            }
            if let MenuEvent::Captured { .. } = ev {
                if let Some(o) = p.options.as_mut() {
                    o.capture_done(now);
                }
            }
            continue;
        }
        // Save / load list.
        if let Some((saving, entries)) = p.list.clone() {
            match ev {
                MenuEvent::Focus(id) => {
                    if let Some(i) = entries.iter().position(|e| e.0 == id) {
                        p.list_sel = i;
                    }
                    sounds.play("Move");
                }
                MenuEvent::Button("Stop") => sounds.play("Stop"),
                MenuEvent::Back => {
                    // `CloseList`: back to the pause entries (full reopen).
                    sounds.play("back");
                    p.list = None;
                    if let Some(a) = art.as_deref() {
                        open_screen(&mut p, a, &mut sounds, now, false);
                    }
                    nav.focus = Some(if saving { "pause:save" } else { "pause:load" }.into());
                }
                MenuEvent::Activate { id, .. } if id.starts_with("save:") => {
                    let Some(i) = entries.iter().position(|e| e.0 == id) else { continue };
                    p.list_sel = i;
                    sounds.play("validation");
                    let (Some(a), Some(t)) = (art.as_deref(), texts.as_deref()) else { continue };
                    if saving {
                        // A new slot saves at once; an existing save asks `t_Q_SaveGame`.
                        if i == 0 {
                            if let Some(s) = saves.as_mut() {
                                let slot = id.strip_prefix("save:").and_then(|x| x.parse().ok()).unwrap_or(dishonored_save::slot::MANUAL0);
                                s.save(slot);
                            }
                            p.list = None;
                            close_screen(&mut p, &mut sounds, now, true);
                        } else {
                            ask(&mut p, a, t, &mut sounds, "save", texts_any(t, "t_Q_SaveGame"), now);
                        }
                    } else {
                        ask(&mut p, a, t, &mut sounds, "load", texts_any(t, "t_Q_LoadGame"), now);
                    }
                }
                _ => {}
            }
            continue;
        }
        match ev {
            MenuEvent::Focus(id) => {
                sounds.play("Move");
                if let Some(i) = p.items().iter().position(|x| x.0 == id) {
                    if i != p.sel {
                        let lk_old = locked(p.items()[p.sel].0, saves.as_deref());
                        let lk_new = locked(p.items()[i].0, saves.as_deref());
                        let ko = format!("pm/item/{}", p.sel);
                        let a = &mut p.anim;
                        a.now(now, &ko, Act::To(P::new().scale(100.0).x(0.0).alpha(100.0), 0.25, Ease::StrongOut));
                        a.now(now, &ko, Act::Play(if lk_old { "out_lock" } else { "out" }.into()));
                        let kn = format!("pm/item/{i}");
                        a.now(now, &kn, Act::To(P::new().scale(100.0).x(30.0).alpha(100.0), 0.25, Ease::BackOut));
                        a.now(now, &kn, Act::Play(if lk_new { "over_lock" } else { "over" }.into()));
                        p.sel = i;
                        let n = p.items().len();
                        parallax(&mut p.anim, i, n, now);
                    }
                }
            }
            // (the game over list has nothing to go back to)
            MenuEvent::Back if p.game_over.is_some() => {}
            MenuEvent::Back => {
                // `PauseScreen.BPressed`: `back`, then resume.
                sounds.play("back");
                close_screen(&mut p, &mut sounds, now, true);
            }
            MenuEvent::Activate { id, .. } => {
                let Some(i) = p.items().iter().position(|x| x.0 == id) else { continue };
                if locked(p.items()[i].0, saves.as_deref()) {
                    sounds.play("Failure");
                    continue;
                }
                p.sel = i;
                // `Confirm`: the vignette flashes, `Validation`, the entry runs 150 ms later.
                sounds.play("Validation");
                p.anim.now(now, "vig/_vignette_mc", Act::Play("flash".into()));
                p.pending = Some((now + 0.15, id));
            }
            _ => {}
        }
    }
}

fn ask(p: &mut PauseMenu, art: &MenuArt, t: &MenuTexts, sounds: &mut MenuSounds, purpose: &str, text: String, now: f64) {
    let Some(g) = art.get("global") else { return };
    p.msg = Some(MsgBox::open(&mut p.anim, g, purpose, text, vec![texts_any(t, "t_Yes"), texts_any(t, "t_No")], now));
    p.msg_focus = (0, now);
    sounds.play("G_MsgBoxOpen");
}

/// What a pause entry does once its confirm delay is over.
fn run_entry(p: &mut PauseMenu, id: &str, art: Option<&MenuArt>, t: Option<&MenuTexts>, sounds: &mut MenuSounds, saves: Option<&crate::saves::Saves>, now: f64) {
    match id {
        "pause:resume" => close_screen(p, sounds, now, true),
        "pause:options" => {
            close_screen(p, sounds, now, false);
            let mut o = OptionsMenu::default();
            if let Some(a) = art {
                o.open(a, now, true);
            }
            p.options = Some(o);
            p.input_from = now;
        }
        "pause:save" | "pause:load" | "go:load" => {
            let saving = id == "pause:save";
            let (Some(s), Some(t)) = (saves, t) else { return };
            close_screen(p, sounds, now, false);
            let list = s.list();
            let mut entries: Vec<(String, String)> = Vec::new();
            if saving {
                // `t_CreateNewSave`, then the manual saves (quick / auto / mission slots are
                // never written from the menu: `Dis_Save` only accepts slots above 14).
                entries.push((format!("save:{}", s.new_manual_slot()), t.t(SEC_MENU, "t_CreateNewSave")));
                for (_, h) in list.iter().filter(|x| x.1.slot >= dishonored_save::slot::MANUAL0) {
                    entries.push((format!("save:{}", h.slot), crate::saves::label(h, t)));
                }
            } else {
                for (path, h) in &list {
                    entries.push((format!("save:{}", path.display()), crate::saves::label(h, t)));
                }
            }
            p.list = Some((saving, entries));
            p.list_sel = 0;
            p.input_from = now;
            let a = &mut p.anim;
            a.author("ls", Props::default());
            a.now(now, "ls", Act::Set(P::new().x(-40.0).alpha(0.0)));
            a.now(now, "ls", Act::To(P::new().x(0.0).alpha(100.0), 0.3, Ease::StrongOut));
            for i in 0..7 {
                let k = format!("ls/row/{i}");
                a.forget(&k);
                a.author(&k, Props::default());
                a.now(now, &k, Act::Set(P::new().alpha(0.0)));
                a.at(now + 0.02 * i as f64, &k, Act::Set(P::new().x(20.0)));
                a.at(now + 0.02 * i as f64, &k, Act::To(P::new().alpha(100.0).x(0.0), 0.25, Ease::StrongOut));
            }
            HelpBar::open(a, "ls/help", now);
        }
        "pause:quit" => {
            close_screen(p, sounds, now, false);
            if let (Some(a), Some(t)) = (art, t) {
                ask(p, a, t, sounds, "quit", texts_any(t, "t_Q_BackToMainMenu"), now);
            }
        }
        "pause:windows" => {
            close_screen(p, sounds, now, false);
            if let (Some(a), Some(t)) = (art, t) {
                ask(p, a, t, sounds, "windows", texts_any(t, "t_Q_BackToWindows"), now);
            }
        }
        _ => {}
    }
}

fn build(p: &PauseMenu, art: &MenuArt, t: &MenuTexts, gs: &GameSettings, nav: &MenuNav, saves: Option<&crate::saves::Saves>, now: f64) -> DisplayList {
    let mut dl = DisplayList::default();
    let Some(pm) = art.get("pause") else { return dl };
    // `bkgd_mc` (white noise, particles, blades) and the vignette stay while the menu is up.
    if let Some((ch, m)) = pm.root_child("bkgd_mc") {
        let ovr = p.anim.ovr("bg", now);
        let st = States::default().clock((now - p.opened_at) as f32);
        dl.movie_fx(pm, ch, m, &st, 1.0, &ovr, false);
    }
    if let Some(v) = pm.export("m_Vignette") {
        let vp = p.anim.get("vig", now);
        let ovr = p.anim.ovr("vig", now);
        let (lab, secs) = p.anim.timeline("vig/_vignette_mc", now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("default".into(), 0.0));
        let st = States::default().with_timed("_vignette_mc", &lab, secs);
        dl.movie_fx(pm, v, vp.matrix(), &st, vp.alpha / 100.0, &ovr, false);
    }
    let focus = nav.focus.as_deref().unwrap_or("");
    let closing_for = p.closing.map(|(c, _)| now - c).unwrap_or(0.0);
    if p.closing.is_none() || closing_for < 0.5 {
        build_panel(&mut dl, p, pm, t, saves, focus, now);
    }
    if let Some(o) = &p.options {
        o.build(&mut dl, art, t, gs, nav, now);
    }
    if let Some((saving, entries)) = &p.list {
        build_list(&mut dl, p, art, t, *saving, entries, now);
    }
    if let Some(m) = &p.msg {
        for it in dl.items.iter_mut() {
            it.nav = false;
        }
        let (sel, since) = p.msg_focus;
        let f = |i: usize| -> (bool, f32) { (i == sel, (now - since) as f32) };
        m.draw(&mut dl, art, &p.anim, now, &f);
    }
    dl
}

fn build_panel(dl: &mut DisplayList, p: &PauseMenu, pm: &MovieArt, t: &MenuTexts, saves: Option<&crate::saves::Saves>, focus: &str, now: f64) {
    let Some((ch, _)) = pm.root_child("pauseMenu_mc") else { return };
    let root = p.anim.get("pm", now);
    if !root.visible {
        return;
    }
    let rm = root.matrix();
    let ovr = p.anim.ovr("pm", now);
    let clock = (now - p.opened_at) as f32;
    // Game-over decoration and the menu container are script-driven (the entries are drawn below).
    let go = p.game_over.is_some();
    let hide = |q: &str| match q {
        "_gameOver_mc" if !go => Some(Props { visible: false, ..Props::default() }),
        "_menu_mc" => Some(Props { visible: false, ..Props::default() }),
        other => ovr(other),
    };
    let (clab, csecs) = p.anim.timeline("pm/_compass_mc", now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("default".into(), 0.0));
    let st = States::default().with_timed("_compass_mc", &clab, csecs).clock(clock);
    // (the mask portrait: frame 0, or the game over slice animation)
    let st = match p.anim.timeline("pm/_corvo_mc", now) {
        Some((lab, secs)) if go => st.with_timed("_corvo_mc", lab, secs),
        _ => st.with("_corvo_mc", "#0"),
    };
    dl.movie_fx(pm, ch, rm, &st, root.alpha / 100.0, &hide, false);
    // The title text ("PAUSE", upper-cased into the text and both shadows).
    for f in DisplayList::fields_fx(pm, ch, rm, &st, &hide) {
        if f.path.starts_with("_gameOver_mc") && go {
            // `GameOverTitle.SetTitle`: the reason as it is.
            dl.field_text(pm, &f, p.game_over.as_deref().unwrap_or_default(), None);
            continue;
        }
        if !f.path.starts_with("_title_mc") {
            continue;
        }
        dl.field_text(pm, &f, &t.t(SEC_MENU, if go { "t_GameOver" } else { "t_Pause" }).to_uppercase(), None);
    }
    // Entries, bottom-aligned in `_menu_mc` (90 %, -10 deg).
    let Some(item) = pm.export("p_pauseMenu_item") else { return };
    let mp = p.anim.get("pm/_menu_mc", now);
    let menu_m = rm.mul(&Props { x: mp.x, y: mp.y, xscale: mp.xscale, yscale: mp.yscale, rotation: mp.rotation, ..Props::default() }.matrix());
    let n = p.items().len();
    let h_all = n as f32 * ITEM_H + (n as f32 - 1.0) * ITEM_GAP;
    let mask = mask_rect(pm, ch, rm, &p.anim, now);
    for (i, (id, sec, key)) in p.items().iter().enumerate() {
        let k = format!("pm/item/{i}");
        let ip = p.anim.get(&k, now);
        let a = ip.alpha / 100.0 * mp.alpha / 100.0 * root.alpha / 100.0;
        let y = -h_all + i as f32 * (ITEM_H + ITEM_GAP) + ITEM_H * 0.5;
        let im = menu_m.mul(&Props { x: ip.x, y, xscale: ip.xscale, yscale: ip.yscale, ..Props::default() }.matrix());
        let lk = locked(id, saves);
        let (lab, secs) = p.anim.timeline(&k, now).map(|(l, s)| (l.to_string(), s)).unwrap_or((if lk { "stop_lock" } else { "default" }.into(), 0.0));
        let st = States::default().with_timed("*", &lab, secs).with("_bkgd_mc", &format!("#{}", i % 4));
        let strike = move |q: &str| if q == "_strike_mc" && !lk { Some(Props { visible: false, ..Props::default() }) } else { None };
        // `_maskMenu_mc` clips the list: entries outside it are not drawn.
        let (cx, cy) = im.apply(0.0, 0.0);
        if !mask.is_none_or(|r| r.contains(Vec2::new(cx, cy) / 20.0)) {
            continue;
        }
        if a > 0.002 {
            dl.movie_fx(pm, item, im, &st, a, &strike, false);
            let fields = DisplayList::fields_fx(pm, item, im, &st, &strike);
            if let Some(f) = DisplayList::label_field(&fields) {
                let mut f = f.clone();
                f.color.mul[3] *= a;
                dl.field_text(pm, &f, &t.t(sec, key).to_uppercase(), None);
            }
        }
        let (hx, hy) = im.apply(110.0 * 20.0, 0.0);
        dl.item(*id, Rect::from_center_size(Vec2::new(hx, hy) / 20.0, Vec2::new(382.0, 44.0)), p.closing.is_none());
    }
    let _ = (focus, TEXT);
}

/// Stage bounds of `_maskMenu_mc` (the list's clip).
fn mask_rect(pm: &MovieArt, ch: u16, rm: Matrix, anim: &Anim, now: f64) -> Option<Rect> {
    let (mc, mm) = pm.child(ch, "_maskMenu_mc")?;
    let local = anim.value("pm/_maskMenu_mc", now).map(|p| p.matrix_like(&mm)).unwrap_or(mm);
    let mut out = Vec::new();
    pm.walk(mc, rm.mul(&local), Default::default(), "", &States::default(), &|_| None, &mut out);
    let mut r: Option<Rect> = None;
    for f in out {
        if let menu_art::Flat::Solid(d, _, _) = f {
            for c in d {
                let v = Vec2::new(c.0, c.1);
                r = Some(r.map_or(Rect::from_corners(v, v), |r| r.union_point(v)));
            }
        }
    }
    r
}

/// The save / load list (`UI_LoadGame` rows: date and chapter; 7 visible on a 71.5 px pitch).
fn build_list(dl: &mut DisplayList, p: &PauseMenu, art: &MenuArt, t: &MenuTexts, saving: bool, entries: &[(String, String)], now: f64) {
    let Some(load) = art.get("load") else { return };
    let o = menu_art::px_matrix(640.0, 360.0);
    if let Some(c) = art.get("common") {
        let title = t.t(SEC_MENU, if saving { "t_SaveGame" } else { "t_LoadGamePauseMenu" });
        crate::menu_widgets::TitleBar::draw(dl, c, &art.metrics, &p.anim, "ls/title", now, o, &title);
    }
    let lp = p.anim.get("ls", now);
    let la = lp.alpha / 100.0;
    if entries.is_empty() {
        dl.text(t.t(SEC_BASE, "t_EmptyList"), Rect::new(0.0, 330.0, 1280.0, 370.0), 26.0, menu_art::FontKind::Normal, TEXT.with_alpha(la), Justify::Center);
    }
    let Some(row) = load.export("m_loadGameList_item") else { return };
    let lm = o.mul(&Props { x: -515.0 + lp.x, y: -205.0, rotation: -2.5, ..Props::default() }.matrix());
    let first = p.list_sel.saturating_sub(6).min(entries.len().saturating_sub(7));
    for (vi, i) in (first..(first + 7).min(entries.len())).enumerate() {
        let (id, text) = &entries[i];
        let rp = p.anim.get(&format!("ls/row/{vi}"), now);
        let sel = i == p.list_sel;
        let x = rp.x + if sel { 15.0 } else { 0.0 };
        let rm = lm.mul(&menu_art::px_matrix(x, (36.0 + 71.5 * vi as f32).round()));
        let st = States::all(if sel { "over" } else { "default" }).with("_bkgd_mc/mc", &format!("#{}", vi % 4));
        let a = la * rp.alpha / 100.0;
        dl.movie(load, row, rm, &st, a);
        for f in DisplayList::fields(load, row, rm, &st) {
            let txt = if f.path.contains("chapter") || (saving && i == 0 && f.path.contains("Empty")) { text.clone() } else { continue };
            let mut f = f;
            f.color.mul[3] *= a;
            dl.field_text(load, &f, &txt, None);
        }
        let (cx, cy) = rm.apply(380.0 * 20.0, 0.0);
        dl.item(id, Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(753.0, 62.0)), true);
    }
    if let Some(c) = art.get("common") {
        HelpBar::draw(dl, c, &art.metrics, t, &p.anim, "ls/help", now, &[("A", "t_Validate"), ("B", "t_Back")]);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn rebuild(
    mut commands: Commands,
    mut p: ResMut<PauseMenu>,
    art: Option<Res<MenuArt>>,
    t: Option<Res<MenuTexts>>,
    gs: Res<GameSettings>,
    mut nav: ResMut<MenuNav>,
    fonts: Option<Res<crate::gfx_hud::GameFonts>>,
    saves: Option<Res<crate::saves::Saves>>,
    mut sounds: ResMut<MenuSounds>,
) {
    let (Some(art), Some(t), Some(root)) = (art, t, p.root) else { return };
    if !p.open {
        return;
    }
    let now = p.now;
    if let Some(o) = p.options.as_mut() {
        o.tick(&nav, &mut sounds, now);
        if o.closed(now) {
            p.options = None;
        }
    }
    p.dirty = false;
    let dl = build(&p, &art, &t, &gs, &nav, saves.as_deref(), now);
    nav.set_items(dl.items.clone());
    if p.msg.is_none() && p.options.is_none() && p.list.is_none() {
        let want = p.items()[p.sel].0;
        if nav.focus.as_deref() != Some(want) && nav.items.iter().any(|i| i.id == want) {
            nav.focus = Some(want.into());
        }
    } else if let Some(m) = &p.msg {
        nav.focus = Some(format!("msg:btn:{}", m.sel));
    }
    let p = &mut *p;
    menu_art::spawn_list_cached(&mut commands, root, &dl, fonts.as_deref(), &mut p.cache);
}
