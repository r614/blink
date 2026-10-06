//! In-game pause menu (Esc), drawn from the original `UI_PauseMenu.PauseMenu` movie
//! (`UI_PauseMenu_SF`): the `bkgd_mc` blades/particles, the `pauseMenu_mc` panel (mask
//! portrait, splatters, compass, `PAUSE` title on its tilted band) and `p_pauseMenu_item`
//! entries: Resume (`t_ResumeGame`), Save Game / Load Game (`t_SaveGame`,
//! `t_LoadGamePauseMenu`: the save list, see `crate::saves`), Options (`t_Options`, the shared options screen) and
//! Back to Main Menu (`t_BackToMainMenu`), which returns to the launcher.
//!
//! An offline match is paused while the menu is open (the fixed tick does not run); an
//! online session keeps running, as the server does not pause.

use crate::menu_art::{self, DisplayList, MovieArt, States};
use crate::menu_audio::MenuSounds;
use crate::menu_options::{OptionsMenu, Outcome};
use crate::menu_ui::{self, MenuArt, MenuEvent, MenuNav, MenuTexts, SEC_BASE, SEC_MENU};
use crate::settings::GameSettings;
use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct PauseMenu {
    pub open: bool,
    pub options: Option<OptionsMenu>,
    root: Option<Entity>,
    dirty: bool,
    /// The game was started by the launcher (`--from-launcher`): quitting returns to it.
    pub from_launcher: bool,
    /// Open on the next frame (harness control `pause`).
    pub request: bool,
    /// Save / load list open (`saveGame_mc` / `loadGame_mc`): (saving, entries (id, text)).
    list: Option<(bool, Vec<(String, String)>)>,
}

const ITEMS: &[(&str, &str, &str)] = &[
    ("pause:resume", SEC_MENU, "t_ResumeGame"),
    ("pause:save", SEC_MENU, "t_SaveGame"),
    ("pause:load", SEC_MENU, "t_LoadGamePauseMenu"),
    ("pause:options", SEC_BASE, "t_Options"),
    ("pause:quit", SEC_MENU, "t_BackToMainMenu"),
];

pub fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let root = menu_art::spawn_layer(&mut commands, 50);
    commands.entity(root).insert(Visibility::Hidden);
    commands.insert_resource(menu_ui::load_menu_art(&["pause", "options", "gamma"], &mut images));
    commands.insert_resource(MenuTexts::load());
    commands.insert_resource(PauseMenu { root: Some(root), from_launcher: std::env::args().any(|a| a == "--from-launcher"), ..default() });
}

/// Fixed-tick run condition: an offline match stops while paused.
pub fn sim_running(p: Option<Res<PauseMenu>>, loading: Option<Res<crate::loading_screen::LoadingScreen>>, session: Res<crate::session::Session>) -> bool {
    // The loading screen also stops an offline match (the original loads levels synchronously).
    let stopped = p.is_some_and(|p| p.open) || loading.is_some_and(|l| l.active);
    !(stopped && matches!(session.kind, crate::session::SessionKind::Offline { .. }))
}

#[allow(clippy::too_many_arguments)]
pub fn toggle(keys: Res<ButtonInput<KeyCode>>, mut p: ResMut<PauseMenu>, mut nav: ResMut<MenuNav>, mut local: ResMut<crate::input_map::LocalInput>, mut sounds: ResMut<MenuSounds>, mut vis: Query<&mut Visibility, With<menu_art::MenuLayer>>) {
    if !p.open && ((keys.just_pressed(KeyCode::Escape) && local.override_input.is_none()) || p.request) {
        p.request = false;
        p.open = true;
        p.dirty = true;
        p.options = None;
        p.list = None;
        nav.enabled = true;
        nav.settle = 1;
        nav.focus = None;
        local.enabled = false;
        sounds.play("OpenScreen");
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

fn close(p: &mut PauseMenu, nav: &mut MenuNav, local: &mut crate::input_map::LocalInput) {
    p.open = false;
    p.options = None;
    p.list = None;
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
) {
    if !p.open {
        nav.events.clear();
        return;
    }
    let events = std::mem::take(&mut nav.events);
    for ev in events {
        p.dirty = true;
        if let Some(o) = p.options.as_mut() {
            if o.handle(&ev, &mut gs, &mut nav, &mut sounds) == Outcome::Close {
                p.options = None;
                nav.focus = Some("pause:options".into());
            }
            continue;
        }
        // Save / load list: pick an entry (saving: a new manual slot or an existing save to
        // overwrite; loading: any save, newest first). Back returns to the pause entries.
        if let Some((saving, _)) = p.list.clone() {
            match ev {
                MenuEvent::Focus(_) => sounds.play("Move"),
                MenuEvent::Back => {
                    sounds.play("CloseScreen");
                    p.list = None;
                    nav.focus = Some(if saving { "pause:save" } else { "pause:load" }.into());
                }
                MenuEvent::Activate { id, .. } => {
                    if let (Some(s), Some(rest)) = (saves.as_mut(), id.strip_prefix("save:")) {
                        sounds.play("validation");
                        if saving {
                            let slot = rest.parse().unwrap_or(dishonored_save::slot::MANUAL0);
                            s.save(slot);
                        } else {
                            s.load_file(std::path::PathBuf::from(rest));
                        }
                        close(&mut p, &mut nav, &mut local);
                    }
                }
                _ => {}
            }
            continue;
        }
        match ev {
            MenuEvent::Focus(_) => sounds.play("Move"),
            MenuEvent::Activate { ref id, .. } if id == "pause:save" || id == "pause:load" => {
                let saving = id == "pause:save";
                let (Some(s), Some(t)) = (saves.as_ref(), texts.as_ref()) else { continue };
                sounds.play("validation");
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
                    if entries.is_empty() {
                        entries.push(("none".into(), t.t(SEC_BASE, "t_EmptyList")));
                    }
                }
                entries.truncate(8);
                p.list = Some((saving, entries));
                nav.focus = None;
            }
            MenuEvent::Back => {
                sounds.play("CloseScreen");
                close(&mut p, &mut nav, &mut local);
            }
            MenuEvent::Activate { id, .. } => match id.as_str() {
                "pause:resume" => {
                    sounds.play("validation");
                    close(&mut p, &mut nav, &mut local);
                }
                "pause:options" => {
                    sounds.play("validation");
                    p.options = Some(OptionsMenu::default());
                    nav.focus = None;
                }
                "pause:quit" => {
                    sounds.play("validation");
                    if !p.from_launcher {
                        // Started from the command line: open a launcher for the player.
                        if let Ok(exe) = std::env::current_exe() {
                            let _ = std::process::Command::new(exe).spawn();
                        }
                    }
                    exit.write(AppExit::Success);
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn build(p: &PauseMenu, art: &MenuArt, t: &MenuTexts, gs: &GameSettings, nav: &MenuNav) -> DisplayList {
    let mut dl = DisplayList::default();
    dl.fill(Rect::new(-640.0, -360.0, 1920.0, 1080.0), Color::srgba(0.0, 0.0, 0.0, 0.55));
    if let (Some(o), Some(opts)) = (art.get("options"), p.options.as_ref()) {
        let _ = o;
        opts.build(&mut dl, art, t, gs, nav, true);
        return dl;
    }
    let Some(pm) = art.get("pause") else {
        return dl;
    };
    let (title, items): (String, Vec<(String, String)>) = match &p.list {
        Some((saving, entries)) => (t.t(SEC_MENU, if *saving { "t_SaveGame" } else { "t_LoadGamePauseMenu" }), entries.clone()),
        None => (t.t(SEC_MENU, "t_Pause"), ITEMS.iter().map(|(id, sec, key)| (id.to_string(), t.t(sec, key))).collect()),
    };
    build_pause(&mut dl, pm, &title, &items, nav.focus.as_deref().unwrap_or(""));
    dl
}

fn build_pause(dl: &mut DisplayList, art: &MovieArt, title: &str, items: &[(String, String)], focus: &str) {
    if let Some((c, m)) = art.root_child("bkgd_mc") {
        dl.movie(art, c, m, &States::default(), 1.0);
    }
    let Some((pm, m)) = art.root_child("pauseMenu_mc") else { return };
    let sp = art.sprite(pm).cloned().unwrap_or_default();
    for pl in art.movie.display_list(&sp, 0) {
        let Some(c) = pl.character else { continue };
        // Game-over decoration and the menu mask are script-driven.
        if matches!(pl.name.as_deref(), Some("_gameOver_mc") | Some("_maskMenu_mc") | Some("_menu_mc")) {
            continue;
        }
        let cm = m.mul(&pl.matrix.unwrap_or_default());
        dl.movie(art, c, cm, &States::default(), 1.0);
        if pl.name.as_deref() == Some("_title_mc") {
            for f in DisplayList::fields(art, c, cm, &States::default()) {
                dl.field_text(art, &f, &title.to_uppercase(), None);
            }
        }
    }
    // Entries stacked down the menu area (the mask region), following its tilt.
    let mask = art.child(pm, "_maskMenu_mc").map(|(c, mm)| (c, m.mul(&mm)));
    let area = mask.and_then(|(c, mm)| art.bounds(c, mm, &States::default())).unwrap_or(Rect::new(700.0, 250.0, 1200.0, 600.0));
    let (dx, dy) = mask.map(|(_, mm)| {
        let l = (mm.a * mm.a + mm.b * mm.b).sqrt().max(1e-3);
        (mm.a / l, mm.b / l)
    }).unwrap_or((1.0, 0.0));
    let rot = dishonored_assets::swf::Matrix { a: dx, b: dy, c: -dy, d: dx, tx: 0.0, ty: 0.0 };
    let start = Vec2::new(area.min.x + area.width() * 0.25, area.min.y + area.height() * 0.3);
    // Five pause entries fit the band at 62 px; longer save lists close up.
    let step = if items.len() > 5 { 62.0 * 5.0 / items.len() as f32 } else { 62.0 };
    for (i, (id, text)) in items.iter().enumerate() {
        let off = Vec2::new(-dy, dx) * (i as f32 * step);
        let at = start + off;
        let bm = dishonored_assets::swf::Matrix { tx: at.x * 20.0, ty: at.y * 20.0, ..rot };
        menu_ui::movie_button_with(dl, art, "p_pauseMenu_item", bm, id, &text.to_uppercase(), focus == id.as_str(), Vec2::new(380.0, step.min(50.0)), &[]);
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
    mut last: Local<(Option<String>, Option<String>, bool)>,
) {
    let (Some(art), Some(t), Some(root)) = (art, t, p.root) else { return };
    if !p.open {
        return;
    }
    let key = (nav.focus.clone(), nav.capturing.clone(), fonts.is_some());
    if !p.dirty && *last == key {
        return;
    }
    *last = key;
    p.dirty = false;
    let dl = build(&p, &art, &t, &gs, &nav);
    nav.set_items(dl.items.clone());
    menu_art::spawn_list(&mut commands, root, &dl, fonts.as_deref());
}
