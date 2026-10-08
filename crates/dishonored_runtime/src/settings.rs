//! The player's settings (`dishonored_settings`) inside the runtime: loaded at startup by
//! both the launcher and the game, applied to what the port implements, saved on change.
//!
//! Applied: window resolution / fullscreen / vsync (live from the options menu; at game start
//! only when launched from the launcher, so plain CLI runs keep their window), FOV, gamma
//! (camera colour grading), anti-aliasing (Off / MLAA as SMAA / FXAA), texture detail (import
//! size cap, at the next level load), head bob, mouse sensitivity / smoothing / invert, key
//! bindings, master / music / SFX / voice volume, HUD toggles (`hud_settings.rs`). The
//! options table in `research/dishonored/MENUS.md` lists what is applied, where and when, and
//! what the port cannot honour yet.

use bevy::input::mouse::MouseButton;
use bevy::prelude::*;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode};
use dishonored_settings::Settings;
use std::path::PathBuf;

#[derive(Resource, Clone)]
pub struct GameSettings {
    pub s: Settings,
    pub path: PathBuf,
    /// Bumped on every change so systems can react once.
    pub revision: u64,
}

impl GameSettings {
    pub fn load() -> Self {
        let (s, path) = Settings::load_default_path();
        info!("settings: {}", path.display());
        Self { s, path, revision: 1 }
    }

    /// Persist and notify.
    pub fn commit(&mut self) {
        self.revision += 1;
        if let Err(e) = self.s.save(&self.path) {
            warn!("settings: saving {}: {e:#}", self.path.display());
        }
    }
}

/// Texture detail -> largest texture edge imported (High keeps the previous default).
pub fn max_texture_size(s: &Settings) -> u32 {
    match s.graphics.texture_details {
        0 => 256,
        1 => 512,
        _ => 1024,
    }
}

/// A bound input: keyboard key, mouse button or wheel direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Key(KeyCode),
    Mouse(MouseButton),
    WheelUp,
    WheelDown,
}

/// UE3 key names (`[Keys]` in `Settings.int`, `m_BindableKeyMapping`) <-> Bevy inputs.
const KEYS: &[(&str, Input)] = &[
    ("LeftMouseButton", Input::Mouse(MouseButton::Left)),
    ("RightMouseButton", Input::Mouse(MouseButton::Right)),
    ("MiddleMouseButton", Input::Mouse(MouseButton::Middle)),
    ("ThumbMouseButton", Input::Mouse(MouseButton::Back)),
    ("ThumbMouseButton2", Input::Mouse(MouseButton::Forward)),
    ("MouseScrollUp", Input::WheelUp),
    ("MouseScrollDown", Input::WheelDown),
    ("BackSpace", Input::Key(KeyCode::Backspace)),
    ("Tab", Input::Key(KeyCode::Tab)),
    ("Enter", Input::Key(KeyCode::Enter)),
    ("Pause", Input::Key(KeyCode::Pause)),
    ("CapsLock", Input::Key(KeyCode::CapsLock)),
    ("SpaceBar", Input::Key(KeyCode::Space)),
    ("PageUp", Input::Key(KeyCode::PageUp)),
    ("PageDown", Input::Key(KeyCode::PageDown)),
    ("End", Input::Key(KeyCode::End)),
    ("Home", Input::Key(KeyCode::Home)),
    ("Left", Input::Key(KeyCode::ArrowLeft)),
    ("Up", Input::Key(KeyCode::ArrowUp)),
    ("Right", Input::Key(KeyCode::ArrowRight)),
    ("Down", Input::Key(KeyCode::ArrowDown)),
    ("Insert", Input::Key(KeyCode::Insert)),
    ("Delete", Input::Key(KeyCode::Delete)),
    ("Zero", Input::Key(KeyCode::Digit0)),
    ("One", Input::Key(KeyCode::Digit1)),
    ("Two", Input::Key(KeyCode::Digit2)),
    ("Three", Input::Key(KeyCode::Digit3)),
    ("Four", Input::Key(KeyCode::Digit4)),
    ("Five", Input::Key(KeyCode::Digit5)),
    ("Six", Input::Key(KeyCode::Digit6)),
    ("Seven", Input::Key(KeyCode::Digit7)),
    ("Eight", Input::Key(KeyCode::Digit8)),
    ("Nine", Input::Key(KeyCode::Digit9)),
    ("A", Input::Key(KeyCode::KeyA)),
    ("B", Input::Key(KeyCode::KeyB)),
    ("C", Input::Key(KeyCode::KeyC)),
    ("D", Input::Key(KeyCode::KeyD)),
    ("E", Input::Key(KeyCode::KeyE)),
    ("F", Input::Key(KeyCode::KeyF)),
    ("G", Input::Key(KeyCode::KeyG)),
    ("H", Input::Key(KeyCode::KeyH)),
    ("I", Input::Key(KeyCode::KeyI)),
    ("J", Input::Key(KeyCode::KeyJ)),
    ("K", Input::Key(KeyCode::KeyK)),
    ("L", Input::Key(KeyCode::KeyL)),
    ("M", Input::Key(KeyCode::KeyM)),
    ("N", Input::Key(KeyCode::KeyN)),
    ("O", Input::Key(KeyCode::KeyO)),
    ("P", Input::Key(KeyCode::KeyP)),
    ("Q", Input::Key(KeyCode::KeyQ)),
    ("R", Input::Key(KeyCode::KeyR)),
    ("S", Input::Key(KeyCode::KeyS)),
    ("T", Input::Key(KeyCode::KeyT)),
    ("U", Input::Key(KeyCode::KeyU)),
    ("V", Input::Key(KeyCode::KeyV)),
    ("W", Input::Key(KeyCode::KeyW)),
    ("X", Input::Key(KeyCode::KeyX)),
    ("Y", Input::Key(KeyCode::KeyY)),
    ("Z", Input::Key(KeyCode::KeyZ)),
    ("NumPadZero", Input::Key(KeyCode::Numpad0)),
    ("NumPadOne", Input::Key(KeyCode::Numpad1)),
    ("NumPadTwo", Input::Key(KeyCode::Numpad2)),
    ("NumPadThree", Input::Key(KeyCode::Numpad3)),
    ("NumPadFour", Input::Key(KeyCode::Numpad4)),
    ("NumPadFive", Input::Key(KeyCode::Numpad5)),
    ("NumPadSix", Input::Key(KeyCode::Numpad6)),
    ("NumPadSeven", Input::Key(KeyCode::Numpad7)),
    ("NumPadEight", Input::Key(KeyCode::Numpad8)),
    ("NumPadNine", Input::Key(KeyCode::Numpad9)),
    ("Multiply", Input::Key(KeyCode::NumpadMultiply)),
    ("Add", Input::Key(KeyCode::NumpadAdd)),
    ("Subtract", Input::Key(KeyCode::NumpadSubtract)),
    ("Decimal", Input::Key(KeyCode::NumpadDecimal)),
    ("Divide", Input::Key(KeyCode::NumpadDivide)),
    ("F1", Input::Key(KeyCode::F1)),
    ("F2", Input::Key(KeyCode::F2)),
    ("F3", Input::Key(KeyCode::F3)),
    ("F4", Input::Key(KeyCode::F4)),
    ("F5", Input::Key(KeyCode::F5)),
    ("F6", Input::Key(KeyCode::F6)),
    ("F7", Input::Key(KeyCode::F7)),
    ("F8", Input::Key(KeyCode::F8)),
    ("F9", Input::Key(KeyCode::F9)),
    ("F10", Input::Key(KeyCode::F10)),
    ("F11", Input::Key(KeyCode::F11)),
    ("F12", Input::Key(KeyCode::F12)),
    ("NumLock", Input::Key(KeyCode::NumLock)),
    ("ScrollLock", Input::Key(KeyCode::ScrollLock)),
    ("LeftShift", Input::Key(KeyCode::ShiftLeft)),
    ("RightShift", Input::Key(KeyCode::ShiftRight)),
    ("LeftControl", Input::Key(KeyCode::ControlLeft)),
    ("RightControl", Input::Key(KeyCode::ControlRight)),
    ("LeftAlt", Input::Key(KeyCode::AltLeft)),
    ("RightAlt", Input::Key(KeyCode::AltRight)),
    ("Semicolon", Input::Key(KeyCode::Semicolon)),
    ("Equals", Input::Key(KeyCode::Equal)),
    ("Comma", Input::Key(KeyCode::Comma)),
    ("Underscore", Input::Key(KeyCode::Minus)),
    ("Period", Input::Key(KeyCode::Period)),
    ("Slash", Input::Key(KeyCode::Slash)),
    ("Tilde", Input::Key(KeyCode::Backquote)),
    ("LeftBracket", Input::Key(KeyCode::BracketLeft)),
    ("Backslash", Input::Key(KeyCode::Backslash)),
    ("RightBracket", Input::Key(KeyCode::BracketRight)),
    ("Quote", Input::Key(KeyCode::Quote)),
];

pub fn input_for(name: &str) -> Option<Input> {
    KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, i)| *i)
}

pub fn name_for(i: Input) -> Option<&'static str> {
    KEYS.iter().find(|(_, x)| *x == i).map(|(n, _)| *n)
}

/// Inputs bound to the port's actions, rebuilt when settings change.
#[derive(Resource, Default, Clone)]
pub struct Bindings {
    map: Vec<(String, Input)>,
    pub revision: u64,
}

impl Bindings {
    pub fn from_settings(s: &Settings) -> Self {
        let map = s.bindings.iter().filter_map(|b| Some((b.action.clone(), input_for(&b.key)?))).collect();
        Self { map, revision: 0 }
    }
    pub fn input(&self, action: &str) -> Option<Input> {
        self.map.iter().find(|(a, _)| a == action).map(|(_, i)| *i)
    }
    pub fn pressed(&self, action: &str, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>, wheel: f32) -> bool {
        match self.input(action) {
            Some(Input::Key(k)) => keys.pressed(k),
            Some(Input::Mouse(b)) => mouse.pressed(b),
            Some(Input::WheelUp) => wheel > 0.0,
            Some(Input::WheelDown) => wheel < 0.0,
            None => false,
        }
    }
    pub fn just_pressed(&self, action: &str, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>, wheel: f32) -> bool {
        match self.input(action) {
            Some(Input::Key(k)) => keys.just_pressed(k),
            Some(Input::Mouse(b)) => mouse.just_pressed(b),
            Some(Input::WheelUp) => wheel > 0.0,
            Some(Input::WheelDown) => wheel < 0.0,
            None => false,
        }
    }
}

/// Keep [`Bindings`] in step with the settings.
pub fn sync_bindings(settings: Res<GameSettings>, mut b: ResMut<Bindings>) {
    if b.revision != settings.revision {
        *b = Bindings::from_settings(&settings.s);
        b.revision = settings.revision;
    }
}

/// Window mode / resolution / vsync from the settings (live changes only; startup values are
/// decided by `main.rs`).
pub fn apply_window(settings: Res<GameSettings>, mut last: Local<Option<(bool, bool, Option<(u32, u32)>)>>, mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    // Only when the video settings themselves change (not on every saved option).
    let g = &settings.s.graphics;
    let key = (g.fullscreen, g.vsync, g.resolution);
    let prev = last.replace(key);
    if prev.is_none_or(|p| p == key) {
        return;
    }
    for mut w in &mut windows {
        let mode = if g.fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
        if w.mode != mode {
            w.mode = mode;
        }
        if let (Some((rw, rh)), false) = (g.resolution, g.fullscreen) {
            if w.resolution.physical_width() != rw || w.resolution.physical_height() != rh {
                w.resolution.set_physical_resolution(rw, rh);
            }
        }
        let pm = if g.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
        if w.present_mode != pm {
            w.present_mode = pm;
        }
    }
}

/// Display gamma (2.2 neutral) as the colour-grading exponent of every 3D camera, and the
/// anti-aliasing mode (`PSI_GraphicsPC_AntiAliasingMode`): Off, MLAA (the morphological pass:
/// Bevy's SMAA, the MLAA derivative) or FXAA, each without MSAA as the original renders.
/// `DISHONORED_MSAA=1` keeps the earlier 4x MSAA (comparison renders).
pub fn apply_camera(
    mut commands: Commands,
    settings: Res<GameSettings>,
    mut cams: Query<(Entity, &mut bevy::render::view::ColorGrading, Option<&mut Msaa>, Option<&mut crate::ark_post::ArkPostSource>, Has<bevy::anti_alias::fxaa::Fxaa>, Has<bevy::anti_alias::smaa::Smaa>)>,
) {
    let n = (2.2 / settings.s.graphics.gamma.max(0.1)).clamp(0.2, 5.0);
    let mode = settings.s.graphics.anti_aliasing;
    let legacy_msaa = std::env::var_os("DISHONORED_MSAA").is_some();
    let msaa = if legacy_msaa && mode != 0 { Msaa::Sample4 } else { Msaa::Off };
    for (e, mut cg, m, ark, has_fxaa, has_smaa) in &mut cams {
        let (want_fxaa, want_smaa) = if legacy_msaa { (false, false) } else { (mode == 2, mode == 1) };
        if want_fxaa != has_fxaa {
            if want_fxaa {
                commands.entity(e).insert(bevy::anti_alias::fxaa::Fxaa::default());
            } else {
                commands.entity(e).remove::<bevy::anti_alias::fxaa::Fxaa>();
            }
        }
        if want_smaa != has_smaa {
            if want_smaa {
                commands.entity(e).insert(bevy::anti_alias::smaa::Smaa::default());
            } else {
                commands.entity(e).remove::<bevy::anti_alias::smaa::Smaa>();
            }
        }
        // With the ArkPp pass the setting is the original's display gamma (the LUT exponent is
        // m_GammaAdjustment / display gamma, 0x0090ED00); otherwise Bevy's grading gamma.
        if let Some(mut ark) = ark {
            let g = settings.s.graphics.gamma.max(0.1);
            if (ark.display_gamma - g).abs() > 1e-4 {
                ark.display_gamma = g;
            }
            // `PSI_GraphicsPC_LightShaftEnable`.
            if ark.shafts_on != settings.s.graphics.light_shafts {
                ark.shafts_on = settings.s.graphics.light_shafts;
            }
        } else if (cg.midtones.gamma - n).abs() > 1e-4 {
            cg.shadows.gamma = n;
            cg.midtones.gamma = n;
            cg.highlights.gamma = n;
        }
        if let Some(mut m) = m {
            if *m != msaa && std::env::var_os("DISHONORED_NOMSAA").is_none() {
                *m = msaa;
            }
        }
    }
}

/// Window to open at startup: `--size WxH` wins, then (`--fullscreen`) the settings.
pub fn startup_window(args_size: Option<(u32, u32)>, fullscreen_flag: bool, s: &Settings) -> ((u32, u32), WindowMode, PresentMode) {
    let size = args_size.or(s.graphics.resolution).unwrap_or((1600, 900));
    let mode = if fullscreen_flag { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
    let vsync = s.graphics.vsync && std::env::var_os("DISHONORED_NOVSYNC").is_none();
    (size, mode, if vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync })
}
