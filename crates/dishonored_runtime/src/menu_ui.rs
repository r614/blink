//! Shared menu machinery for the launcher and the in-game pause menu: original menu art and
//! text, focus/hover/keyboard navigation, text entry and key capture, and the UI sound theme.
//!
//! Screens build a [`DisplayList`] (see `menu_art.rs`) whose items carry string ids such as
//! `main:options` or `opt:arrow:PSI_GraphicsPC_FOV:+1`; input becomes [`MenuEvent`]s naming
//! those ids, and the screen handlers interpret them. The harness drives the same ids through
//! the control socket (`launcher.rs`).

use crate::menu_art::{DisplayList, Item, MovieArt};
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use dishonored_assets::loc::{Localization, plain};
use std::collections::HashMap;

/// Localized menu text from the install (`Settings.int`, `DishonoredGame.int`,
/// `Twk_InGameUI.int`), with identifier fallbacks when the install has none.
#[derive(Resource, Default)]
pub struct MenuTexts {
    pub loc: Localization,
}

pub const SEC_SETTINGS_IDS: &str = "ProfileSettingIDs";
pub const SEC_SETTINGS_VALUES: &str = "ProfileSettingValues";
pub const SEC_KEYS: &str = "Keys";
pub const SEC_BASE: &str = "DisGFxMoviePlayerBase_Texts";
pub const SEC_MENU: &str = "DisGFxMoviePlayerMenuBase_Texts";
pub const SEC_MAIN: &str = "DisGFxMoviePlayerMainMenu_Texts";
pub const SEC_TWK_MENU: &str = "Twk_GFxMoviePlayerMenuBase DisTweaks_GFxMoviePlayerMenuBase";

impl MenuTexts {
    pub fn load() -> Self {
        let loc = match dishonored_assets::Content::locate() {
            Ok(c) => Localization::load(&c.root, "INT", &["Settings", "DishonoredGame", "Twk_InGameUI", "Twk_UI_MainMenu"]),
            Err(_) => Localization::default(),
        };
        info!("menu texts: {} localized strings", loc.entries.len());
        Self { loc }
    }

    /// Localized string or a readable fallback derived from the key.
    pub fn t(&self, section: &str, key: &str) -> String {
        match self.loc.get(section, key) {
            Some(s) => plain(s),
            None => fallback(key),
        }
    }

    pub fn option_label(&self, id: &str) -> String {
        if id.starts_with("t_") {
            return self.t(SEC_MENU, id);
        }
        self.t(SEC_SETTINGS_IDS, id)
    }

    pub fn value_label(&self, key: &str) -> String {
        self.t(SEC_SETTINGS_VALUES, key)
    }

    pub fn action_label(&self, action: &str) -> String {
        match action {
            "PORT_Walk" => "Walk".into(),
            "PORT_Reload" => "Reload".into(),
            a => self.t(SEC_SETTINGS_IDS, &format!("PSI_{a}")),
        }
    }

    pub fn key_label(&self, key: &str) -> String {
        if key == "None" || key.is_empty() {
            return self.t(SEC_KEYS, "Unbound_Game");
        }
        self.t(SEC_KEYS, key)
    }
}

fn fallback(key: &str) -> String {
    let k = key.trim_start_matches("PSI_").trim_start_matches("t_").trim_start_matches("m_");
    k.replace('_', " ")
}

/// Original movies used by the menus, loaded once at startup.
#[derive(Resource, Default)]
pub struct MenuArt {
    pub movies: HashMap<&'static str, MovieArt>,
    /// Loose textures the movies load at runtime (`img://`), with their pixel size.
    pub images: HashMap<&'static str, (Handle<Image>, Vec2)>,
}

impl MenuArt {
    pub fn get(&self, k: &str) -> Option<&MovieArt> {
        self.movies.get(k)
    }
}

/// `(key, package, movie)` of the menu movies.
pub const MOVIES: &[(&str, &str, &str)] = &[
    ("main", "Dishonored_MainMenu", "MainMenu"),
    ("options", "Startup", "OptionsMenu"),
    ("pause", "UI_PauseMenu_SF", "PauseMenu"),
    ("gamma", "UI_Gamma_SF", "GammaImage"),
];

pub fn load_menu_art(keys: &[&'static str], images: &mut Assets<Image>) -> MenuArt {
    let mut art = MenuArt::default();
    let Ok(content) = dishonored_assets::Content::locate() else { return art };
    for (k, pkg, movie) in MOVIES.iter().filter(|m| keys.contains(&m.0)) {
        match MovieArt::load(&content, pkg, movie, images) {
            Some(m) => {
                info!("menu movie {pkg}.{movie}: {} textures", m.textures.len());
                art.movies.insert(k, m);
            }
            None => warn!("menu movie {pkg}.{movie} not readable"),
        }
    }
    // Controller picture of the scheme screen (`UI_GamepadMapping`, loaded by `ImgLoader`).
    if keys.contains(&"options") {
        if let Some(t) = crate::menu_art::load_texture(&content, "UI_GamepadMapping", "Options_GamepadMapping_Xenon", images) {
            art.images.insert("gamepad", t);
        }
    }
    art
}

/// Input turned into menu terms.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuEvent {
    /// Enter / click on an item (click position in stage pixels).
    Activate { id: String, at: Option<Vec2> },
    /// Left / right on the focused item.
    Step { id: String, dir: i32 },
    Back,
    Scroll(i32),
    /// Text entry: committed field value.
    TextCommit { id: String, text: String },
    /// Key capture finished: UE3 key name, or `None` when cancelled.
    Captured { id: String, key: Option<String> },
    /// Focus moved (sound + redraw).
    Focus(String),
}

/// Focus, text entry and key capture state of the active menu.
#[derive(Resource, Default)]
pub struct MenuNav {
    pub focus: Option<String>,
    pub items: Vec<Item>,
    /// Field being edited and its text.
    pub editing: Option<(String, String)>,
    /// Item waiting for a key press (keyboard mapping).
    pub capturing: Option<String>,
    pub events: Vec<MenuEvent>,
    pub last_cursor: Option<Vec2>,
    /// Ignore input for a frame after opening (the click that opened it).
    pub settle: u8,
    pub enabled: bool,
}

impl MenuNav {
    pub fn set_items(&mut self, items: Vec<Item>) {
        if let Some(f) = &self.focus {
            if !items.iter().any(|i| &i.id == f) {
                self.focus = None;
            }
        }
        if self.focus.is_none() {
            self.focus = items.iter().find(|i| i.nav).map(|i| i.id.clone());
        }
        self.items = items;
    }

    fn move_focus(&mut self, dir: i32) -> Option<String> {
        let nav: Vec<&Item> = self.items.iter().filter(|i| i.nav).collect();
        if nav.is_empty() {
            return None;
        }
        let cur = self.focus.as_ref().and_then(|f| nav.iter().position(|i| &i.id == f));
        let next = match cur {
            Some(c) => (c as i32 + dir).rem_euclid(nav.len() as i32) as usize,
            None => 0,
        };
        self.focus = Some(nav[next].id.clone());
        self.focus.clone()
    }
}

/// Item under a stage point (topmost = last built).
pub fn hit(items: &[Item], p: Vec2) -> Option<&Item> {
    items.iter().rev().find(|i| i.rect.contains(p))
}

/// Read keyboard / mouse into [`MenuNav::events`].
#[allow(clippy::too_many_arguments)]
pub fn gather_menu_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    mut typed: MessageReader<KeyboardInput>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut nav: ResMut<MenuNav>,
) {
    let typed: Vec<KeyboardInput> = typed.read().cloned().collect();
    if !nav.enabled {
        return;
    }
    if nav.settle > 0 {
        nav.settle -= 1;
        return;
    }
    let cursor = windows.iter().next().and_then(crate::menu_art::cursor_stage);
    // Key capture: the next key or mouse button press is the binding.
    if let Some(id) = nav.capturing.clone() {
        let mut got: Option<Option<String>> = None;
        if keys.just_pressed(KeyCode::Escape) {
            got = Some(None);
        } else if let Some(k) = keys.get_just_pressed().next() {
            got = crate::settings::name_for(crate::settings::Input::Key(*k)).map(|n| Some(n.to_string()));
        } else if let Some(b) = mouse.get_just_pressed().next() {
            got = crate::settings::name_for(crate::settings::Input::Mouse(*b)).map(|n| Some(n.to_string()));
        } else if scroll.delta.y != 0.0 {
            got = Some(Some(if scroll.delta.y > 0.0 { "MouseScrollUp" } else { "MouseScrollDown" }.to_string()));
        }
        if let Some(key) = got {
            nav.capturing = None;
            nav.events.push(MenuEvent::Captured { id, key });
        }
        return;
    }
    // Text entry.
    if let Some((id, mut text)) = nav.editing.clone() {
        for k in &typed {
            if k.state != bevy::input::ButtonState::Pressed {
                continue;
            }
            match &k.logical_key {
                Key::Enter | Key::Escape | Key::Tab => {
                    nav.editing = None;
                    nav.events.push(MenuEvent::TextCommit { id: id.clone(), text: text.clone() });
                    return;
                }
                Key::Backspace => {
                    text.pop();
                }
                _ => {
                    if let Some(t) = &k.text {
                        for c in t.chars().filter(|c| !c.is_control()) {
                            if text.chars().count() < 64 {
                                text.push(c);
                            }
                        }
                    }
                }
            }
        }
        if mouse.just_pressed(MouseButton::Left) && cursor.is_some_and(|c| hit(&nav.items, c).is_none_or(|i| i.id != id)) {
            nav.editing = None;
            nav.events.push(MenuEvent::TextCommit { id, text });
            return;
        }
        nav.editing = Some((id, text));
        return;
    }
    // Hover.
    if let Some(c) = cursor {
        let moved = nav.last_cursor.is_none_or(|l| l.distance(c) > 0.5);
        nav.last_cursor = Some(c);
        if moved {
            if let Some(i) = hit(&nav.items, c).filter(|i| i.nav).map(|i| i.id.clone()) {
                if nav.focus.as_ref() != Some(&i) {
                    nav.focus = Some(i.clone());
                    nav.events.push(MenuEvent::Focus(i));
                }
            }
        }
        if mouse.just_pressed(MouseButton::Left) {
            if let Some(i) = hit(&nav.items, c).map(|i| i.id.clone()) {
                nav.events.push(MenuEvent::Activate { id: i, at: Some(c) });
            }
        }
    }
    if mouse.just_pressed(MouseButton::Right) {
        nav.events.push(MenuEvent::Back);
    }
    let up = keys.just_pressed(KeyCode::ArrowUp);
    let down = keys.just_pressed(KeyCode::ArrowDown) || (keys.just_pressed(KeyCode::Tab) && !keys.pressed(KeyCode::ShiftLeft));
    if up || down {
        if let Some(f) = nav.move_focus(if up { -1 } else { 1 }) {
            nav.events.push(MenuEvent::Focus(f));
        }
    }
    for (k, dir) in [(KeyCode::ArrowLeft, -1), (KeyCode::ArrowRight, 1)] {
        if keys.just_pressed(k) {
            if let Some(f) = nav.focus.clone() {
                nav.events.push(MenuEvent::Step { id: f, dir });
            }
        }
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) || keys.just_pressed(KeyCode::Space) {
        if let Some(f) = nav.focus.clone() {
            nav.events.push(MenuEvent::Activate { id: f, at: None });
        }
    }
    if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace) {
        nav.events.push(MenuEvent::Back);
    }
    // Shoulder-button equivalents on the options screen: Q / E change tab, R restores.
    let has_tabs = nav.items.iter().any(|i| i.id == "opt:tab:0");
    let has_restore = nav.items.iter().any(|i| i.id == "opt:restore");
    if has_tabs {
        for (k, dir) in [(KeyCode::KeyQ, -1), (KeyCode::KeyE, 1)] {
            if keys.just_pressed(k) {
                nav.events.push(MenuEvent::Step { id: "opt:tab:0".into(), dir });
            }
        }
    }
    if has_restore && keys.just_pressed(KeyCode::KeyR) {
        nav.events.push(MenuEvent::Activate { id: "opt:restore".into(), at: None });
    }
    if scroll.delta.y != 0.0 {
        nav.events.push(MenuEvent::Scroll(if scroll.delta.y > 0.0 { -1 } else { 1 }));
    }
}

/// Build a main-menu style button (`m_MainMenu_btn_`, `m_nGame_btn`, `p_pauseMenu_item`,
/// ...) at stage matrix `m`, in its `default` or `over` state, with `text` in its field, and
/// register the hit rectangle.
pub fn movie_button(dl: &mut DisplayList, art: &MovieArt, export: &str, m: dishonored_assets::swf::Matrix, id: &str, text: &str, focused: bool, min_hit: Vec2) -> Option<Rect> {
    movie_button_with(dl, art, export, m, id, text, focused, min_hit, &[])
}

/// [`movie_button`] with extra per-path states (`("_bkgd_mc", "#3")`).
#[allow(clippy::too_many_arguments)]
pub fn movie_button_with(dl: &mut DisplayList, art: &MovieArt, export: &str, m: dishonored_assets::swf::Matrix, id: &str, text: &str, focused: bool, min_hit: Vec2, extra: &[(&str, &str)]) -> Option<Rect> {
    let ch = art.export(export)?;
    let mut states = crate::menu_art::States::all(if focused { "over" } else { "default" });
    for (p, l) in extra {
        states = states.with(p, l);
    }
    dl.movie(art, ch, m, &states, 1.0);
    let fields = DisplayList::fields(art, ch, m, &states);
    // The first visible field is the label (`_txt_mc/txt`); glow copies are skipped.
    if let Some(f) = fields.iter().find(|f| !f.path.contains("Glow") && !f.path.contains("Shadow")).or(fields.first()) {
        // The over state brightens the label (its paper strip is script-faded and not drawn).
        dl.field_text(art, f, text, Some(if focused { crate::menu_art::TEXT } else { crate::menu_art::TEXT_DIM }));
    }
    // Hit area: around the label (the art's bounds include long decorative strips).
    let centre = match dl.prims.last() {
        Some(crate::menu_art::Prim::Text { rect, .. }) => rect.center(),
        _ => art.bounds(ch, m, &states).map(|r| r.center()).unwrap_or(Vec2::new(m.tx, m.ty) / 20.0),
    };
    let r = Rect::from_center_size(centre, min_hit);
    dl.item(id, r, true);
    Some(r)
}
