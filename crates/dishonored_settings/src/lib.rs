//! Player settings, modelled on Dishonored's own profile settings so the options menu can
//! offer every original option with its original value set and default.
//!
//! Origin of the model (see `research/dishonored/MENUS.md`):
//! - option identifiers and value-label keys: `Localization/INT/Settings.int`
//!   (`[ProfileSettingIDs]` `PSI_*`, `[ProfileSettingValues]`), which the menu reads at runtime
//!   for its labels (no retail text is compiled in here);
//! - value types, ranges, steps and defaults: `Engine.upk` `Default__ArkProfileSettings`
//!   (`DefaultSettings` + `ProfileMappings`, ids 29..133);
//! - menu categories: `Twk_InGameUI.int` `[Twk_GFxMoviePlayerMenuBase]` `m_SettingsCategory_*`;
//! - default key bindings: `DefaultInput.ini` `[Engine.PlayerInput]` `.m_PCBindings` (UE3 key
//!   names; gamepad schemes are `.m_PadBindingSet1..4` in the same section).
//!
//! The original keeps these in the player profile (Steam/GOG cloud or `SaveData` profile
//! blob) plus `[SystemSettings]` in `DishonoredEngine.ini` for resolution/fullscreen/vsync;
//! this port keeps everything in one RON file, [`settings_path`].

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SETTINGS_VERSION: u32 = 1;

/// Options-menu tabs (`m_SettingsCategory_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    General,
    Controls,
    Graphics,
    Audio,
}

impl Category {
    pub const ALL: [Category; 4] = [Category::General, Category::Controls, Category::Graphics, Category::Audio];
    /// `[Twk_GFxMoviePlayerMenuBase DisTweaks_GFxMoviePlayerMenuBase]` key of the tab label.
    pub fn label_key(self) -> &'static str {
        match self {
            Category::General => "m_SettingsCategory_General",
            Category::Controls => "m_SettingsCategory_Controls",
            Category::Graphics => "m_SettingsCategory_Graphics",
            Category::Audio => "m_SettingsCategory_Audio",
        }
    }
    pub fn subs(self) -> &'static [SubCategory] {
        match self {
            Category::General => &[SubCategory::Gameplay, SubCategory::Hud],
            Category::Controls => &[SubCategory::Keyboard, SubCategory::Mouse, SubCategory::Gamepad],
            Category::Graphics => &[SubCategory::Graphics],
            Category::Audio => &[SubCategory::Audio],
        }
    }
}

/// Sub-tabs (`m_SettingsSubCategory_*`). Graphics and Audio have a single list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubCategory {
    Gameplay,
    Hud,
    Keyboard,
    Mouse,
    Gamepad,
    Graphics,
    Audio,
}

impl SubCategory {
    pub fn label_key(self) -> Option<&'static str> {
        match self {
            SubCategory::Gameplay => Some("m_SettingsSubCategory_GameplaySettings"),
            SubCategory::Hud => Some("m_SettingsSubCategory_HUDSettings"),
            SubCategory::Keyboard => Some("m_SettingsSubCategory_KeyboardMapping"),
            SubCategory::Mouse => Some("m_SettingsSubCategory_MouseSettings"),
            SubCategory::Gamepad => Some("m_SettingsSubCategory_GamepadSettings"),
            SubCategory::Graphics | SubCategory::Audio => None,
        }
    }
    pub fn category(self) -> Category {
        match self {
            SubCategory::Gameplay | SubCategory::Hud => Category::General,
            SubCategory::Keyboard | SubCategory::Mouse | SubCategory::Gamepad => Category::Controls,
            SubCategory::Graphics => Category::Graphics,
            SubCategory::Audio => Category::Audio,
        }
    }
}

/// Secondary screens reached through a link line (`Link_widget`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Screen {
    /// `videoSettings_mc`: resolution / fullscreen / vsync with Accept / Cancel.
    Video,
    /// `gammaSetting_mc` (`UI_Gamma` image + brightness slider).
    Gamma,
    /// `gamepadMapping_mc`: picture of the pad for the chosen scheme.
    GamepadScheme,
}

/// Widget kind of an option line (`OptionsStepperBoolean`, `OptionsStepper`, `Slider`,
/// `DropDownMenu`, `Link`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    /// Value-label keys in `[ProfileSettingValues]`, index = stored value.
    Choice(&'static [&'static str]),
    Range { min: f32, max: f32, step: f32 },
    /// Screen resolution (drop-down of the display's modes).
    Resolution,
    Link(Screen),
}

/// Whether this port applies the option yet. Every option is shown and saved either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Applied,
    /// Saved but not acted on yet ("not yet effective" in the menu).
    SavedOnly,
}

#[derive(Debug, Clone, Copy)]
pub struct OptionDef {
    /// `PSI_*` key (label in `[ProfileSettingIDs]`); links use the `t_Modify_*` key of
    /// `[DisGFxMoviePlayerMenuBase_Texts]` instead.
    pub id: &'static str,
    /// `ArkProfileSettings` property id (0 for links / port options).
    pub profile_id: u16,
    pub sub: SubCategory,
    pub kind: Kind,
    pub effect: Effect,
    /// Shown on the Video Settings secondary screen instead of the main list.
    pub video_screen: bool,
}

const BOOL: Kind = Kind::Bool;
const PCT: Kind = Kind::Range { min: 0.0, max: 100.0, step: 10.0 };

macro_rules! opt {
    ($id:literal, $pid:literal, $sub:ident, $kind:expr, $eff:ident) => {
        OptionDef { id: $id, profile_id: $pid, sub: SubCategory::$sub, kind: $kind, effect: Effect::$eff, video_screen: false }
    };
    ($id:literal, $pid:literal, $sub:ident, $kind:expr, $eff:ident, video) => {
        OptionDef { id: $id, profile_id: $pid, sub: SubCategory::$sub, kind: $kind, effect: Effect::$eff, video_screen: true }
    };
}

/// Every original option, in the original order (ids ascending, as in `Settings.int`).
/// Key bindings (ids 29..64) are the [`Binding`] list, not entries here.
pub const OPTIONS: &[OptionDef] = &[
    // Gameplay
    opt!("PSI_Gameplay_bAutoUseManaElixir", 104, Gameplay, BOOL, SavedOnly),
    opt!("PSI_Gameplay_KillCamMode", 105, Gameplay, Kind::Choice(&["KCM_Off", "KCM_Normal", "KCM_Frequent"]), SavedOnly),
    opt!("PSI_Gameplay_Difficulty", 106, Gameplay, Kind::Choice(&["EDifficulty_Easy", "EDifficulty_Normal", "EDifficulty_Hard", "EDifficulty_VeryHard"]), SavedOnly),
    opt!("PSI_Gameplay_bAutoSaveInMenu", 107, Gameplay, BOOL, SavedOnly),
    opt!("PSI_Gameplay_HeadBobAmount", 108, Gameplay, Kind::Range { min: 0.0, max: 1.0, step: 0.1 }, Applied),
    opt!("PSI_Gameplay_CameraRelativeClimbing", 109, Gameplay, BOOL, SavedOnly),
    // User Interface (HUD)
    opt!("PSI_HUD_Visibility", 87, Hud, Kind::Choice(&["HV_Off", "HV_Contextual", "HV_Always"]), Applied),
    opt!("PSI_HUD_bShowObjectivePopups", 88, Hud, BOOL, Applied),
    opt!("PSI_HUD_bShowTutorialNotifications", 89, Hud, BOOL, SavedOnly),
    opt!("PSI_HUD_bShowInteractions", 90, Hud, BOOL, Applied),
    opt!("PSI_HUD_bShowHighlight", 91, Hud, BOOL, SavedOnly),
    opt!("PSI_HUD_bShowPickupLog", 92, Hud, BOOL, SavedOnly),
    opt!("PSI_HUD_bShowContextualIcons", 93, Hud, BOOL, SavedOnly),
    opt!("PSI_HUD_bShowPlayerStance", 94, Hud, BOOL, Applied),
    opt!("PSI_HUD_bShowObjectiveMarkers", 95, Hud, BOOL, Applied),
    opt!("PSI_HUD_bShowGrenadeMarkers", 96, Hud, BOOL, SavedOnly),
    opt!("PSI_HUD_bShowAwarenessMarkers", 97, Hud, BOOL, SavedOnly),
    opt!("PSI_HUD_bShowHeartTargetMarkers", 98, Hud, BOOL, SavedOnly),
    opt!("PSI_HUD_CrosshairStyle", 99, Hud, Kind::Choice(&["CS_Off", "CS_Simple", "CS_Normal"]), Applied),
    opt!("PSI_HUD_bCrosshairMovement", 100, Hud, BOOL, SavedOnly),
    opt!("PSI_HUD_CrosshairOpacity", 101, Hud, PCT, Applied),
    // Mouse
    opt!("PSI_Mouse_Sensitivity", 67, Mouse, Kind::Range { min: 0.1, max: 3.0, step: 0.1 }, Applied),
    opt!("PSI_Mouse_bSmooting", 68, Mouse, BOOL, Applied),
    opt!("PSI_Mouse_bInvertY", 69, Mouse, BOOL, Applied),
    opt!("PSI_Mouse_bAutoAim", 70, Mouse, BOOL, SavedOnly),
    opt!("PSI_Mouse_AutoAimStrength", 71, Mouse, PCT, SavedOnly),
    opt!("PSI_Mouse_bFriction", 72, Mouse, BOOL, SavedOnly),
    opt!("PSI_Mouse_FrictionStrength", 73, Mouse, PCT, SavedOnly),
    // Gamepad
    opt!("t_Modify_ControlsScheme", 0, Gamepad, Kind::Link(Screen::GamepadScheme), SavedOnly),
    opt!("PSI_Gamepad_BindingSet", 76, Gamepad, Kind::Choice(&["PBS_1", "PBS_2", "PBS_3", "PBS_4"]), SavedOnly),
    opt!("PSI_Gamepad_bVibration", 77, Gamepad, BOOL, SavedOnly),
    opt!("PSI_Gamepad_LookXSensitivity", 78, Gamepad, PCT, SavedOnly),
    opt!("PSI_Gamepad_LookYSensitivity", 79, Gamepad, PCT, SavedOnly),
    opt!("PSI_Gamepad_bInvertY", 80, Gamepad, BOOL, SavedOnly),
    opt!("PSI_Gamepad_bAutoAim", 81, Gamepad, BOOL, SavedOnly),
    opt!("PSI_Gamepad_AutoAimStrength", 82, Gamepad, PCT, SavedOnly),
    opt!("PSI_Gamepad_bFriction", 83, Gamepad, BOOL, SavedOnly),
    opt!("PSI_Gamepad_FrictionStrength", 84, Gamepad, PCT, SavedOnly),
    // Graphics
    opt!("t_Modify_Gamma", 0, Graphics, Kind::Link(Screen::Gamma), Applied),
    opt!("t_Modify_VideoSettings", 0, Graphics, Kind::Link(Screen::Video), Applied),
    opt!("PSI_Graphics_Gamma", 112, Graphics, Kind::Range { min: 0.5, max: 5.0, step: 0.1 }, Applied, video),
    opt!("PSI_GraphicsPC_Resolution", 115, Graphics, Kind::Resolution, Applied, video),
    opt!("PSI_GraphicsPC_bFullScreen", 116, Graphics, BOOL, Applied, video),
    opt!("PSI_GraphicsPC_bVSync", 117, Graphics, BOOL, Applied, video),
    opt!("PSI_GraphicsPC_FOV", 118, Graphics, Kind::Range { min: 65.0, max: 110.0, step: 5.0 }, Applied),
    opt!("PSI_GraphicsPC_TextureDetails", 119, Graphics, Kind::Choice(&["TextureDetails_Low", "TextureDetails_Medium", "TextureDetails_High"]), Applied),
    opt!("PSI_GraphicsPC_ModelDetails", 120, Graphics, Kind::Choice(&["ModelDetails_Normal", "ModelDetails_High"]), SavedOnly),
    opt!("PSI_GraphicsPC_LightShaftEnable", 121, Graphics, BOOL, SavedOnly),
    opt!("PSI_GraphicsPC_AntiAliasingMode", 122, Graphics, Kind::Choice(&["AntialiasingMode_Off", "AntialiasingMode_MLAA", "AntialiasingMode_FXAA"]), Applied),
    opt!("PSI_GraphicsPC_RatShadows", 123, Graphics, BOOL, SavedOnly),
    // Audio
    opt!("PSI_Audio_GlobalVolume", 126, Audio, PCT, Applied),
    opt!("PSI_Audio_MusicVolume", 127, Audio, PCT, Applied),
    opt!("PSI_Audio_SFXVolume", 128, Audio, PCT, Applied),
    opt!("PSI_Audio_VoicesVolume", 129, Audio, PCT, Applied),
    opt!("PSI_Audio_SubtitlesMode", 130, Audio, Kind::Choice(&["SubtitlesMode_Off", "SubtitlesMode_Dialogs", "SubtitlesMode_All"]), SavedOnly),
    opt!("PSI_AudioPC_SpeakerConfiguration", 133, Audio, Kind::Choice(&["SpeakerConfiguration_Auto", "SpeakerConfiguration_Stereo", "SpeakerConfiguration_5Point1"]), SavedOnly),
];

pub fn option(id: &str) -> Option<&'static OptionDef> {
    OPTIONS.iter().find(|o| o.id == id)
}

/// Options shown in a sub-tab's list (Video Settings items live on their own screen).
pub fn options_in(sub: SubCategory) -> impl Iterator<Item = &'static OptionDef> {
    OPTIONS.iter().filter(move |o| o.sub == sub && !o.video_screen)
}

/// The Video Settings screen's items (brightness has its own screen, reached by link).
pub fn video_options() -> impl Iterator<Item = &'static OptionDef> {
    OPTIONS.iter().filter(|o| o.video_screen && o.id != "PSI_Graphics_Gamma")
}

/// A stored option value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Float(f32),
    Resolution(u32, u32),
}

impl Value {
    pub fn as_f32(self) -> f32 {
        match self {
            Value::Bool(b) => b as i32 as f32,
            Value::Int(i) => i as f32,
            Value::Float(f) => f,
            Value::Resolution(w, _) => w as f32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gameplay {
    pub auto_use_mana_elixir: bool,
    /// 0 Off, 1 Normal, 2 Frequent.
    pub kill_cam: i32,
    /// 0 Easy .. 3 Very Hard. The original stores -1 until New Game asks; Normal here.
    pub difficulty: i32,
    pub autosave_in_journal: bool,
    pub head_bob: f32,
    pub camera_relative_climbing: bool,
}

impl Default for Gameplay {
    fn default() -> Self {
        Self { auto_use_mana_elixir: true, kill_cam: 1, difficulty: 1, autosave_in_journal: true, head_bob: 1.0, camera_relative_climbing: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hud {
    /// Health/mana gauges: 0 Off, 1 Contextual, 2 Always.
    pub visibility: i32,
    pub objective_popups: bool,
    pub tutorial_notifications: bool,
    pub interactions: bool,
    pub highlight: bool,
    pub pickup_log: bool,
    pub contextual_icons: bool,
    pub player_stance: bool,
    pub objective_markers: bool,
    pub grenade_markers: bool,
    pub awareness_markers: bool,
    pub heart_markers: bool,
    /// 0 Off, 1 Simple, 2 Normal.
    pub crosshair_style: i32,
    pub crosshair_movement: bool,
    /// 0..100.
    pub crosshair_opacity: f32,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            visibility: 2,
            objective_popups: true,
            tutorial_notifications: true,
            interactions: true,
            highlight: true,
            pickup_log: true,
            contextual_icons: true,
            player_stance: true,
            objective_markers: true,
            grenade_markers: true,
            awareness_markers: true,
            heart_markers: true,
            crosshair_style: 2,
            crosshair_movement: true,
            crosshair_opacity: 100.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mouse {
    /// Multiplier on the engine's `MouseSensitivity` (0.1 .. 3.0, default 1.0).
    pub sensitivity: f32,
    pub smoothing: bool,
    pub invert_y: bool,
    pub auto_aim: bool,
    pub auto_aim_strength: f32,
    pub aim_assist: bool,
    pub aim_assist_strength: f32,
}

impl Default for Mouse {
    fn default() -> Self {
        Self { sensitivity: 1.0, smoothing: true, invert_y: false, auto_aim: false, auto_aim_strength: 100.0, aim_assist: false, aim_assist_strength: 100.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gamepad {
    /// Type A..D (0..3).
    pub binding_set: i32,
    pub vibration: bool,
    /// The original default is -1 ("not set", platform default); stored here as 50.
    pub look_x: f32,
    pub look_y: f32,
    pub invert_y: bool,
    pub auto_aim: bool,
    pub auto_aim_strength: f32,
    pub aim_assist: bool,
    pub aim_assist_strength: f32,
}

impl Default for Gamepad {
    fn default() -> Self {
        Self { binding_set: 0, vibration: true, look_x: 50.0, look_y: 50.0, invert_y: false, auto_aim: true, auto_aim_strength: 100.0, aim_assist: true, aim_assist_strength: 100.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Graphics {
    /// Display gamma (0.5 .. 5.0, default 2.2).
    pub gamma: f32,
    /// `None` = the display's current mode (the original picks the desktop mode on first run).
    pub resolution: Option<(u32, u32)>,
    pub fullscreen: bool,
    pub vsync: bool,
    /// Horizontal-ish FOV in degrees, 65..110 step 5, default 75 (`DefaultCamera.ini` too).
    pub fov: f32,
    /// 0 Low, 1 Medium, 2 High.
    pub texture_details: i32,
    /// 0 Normal, 1 High.
    pub model_details: i32,
    pub light_shafts: bool,
    /// 0 Off, 1 MLAA, 2 FXAA.
    pub anti_aliasing: i32,
    pub rat_shadows: bool,
}

impl Default for Graphics {
    fn default() -> Self {
        Self { gamma: 2.2, resolution: None, fullscreen: true, vsync: true, fov: 75.0, texture_details: 2, model_details: 0, light_shafts: true, anti_aliasing: 1, rat_shadows: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub global: f32,
    pub music: f32,
    pub sfx: f32,
    pub voices: f32,
    /// 0 Off, 1 Main Dialogue, 2 All Speeches. The original's default is -1 (unset).
    pub subtitles: i32,
    /// 0 Auto, 1 Stereo, 2 5.1.
    pub speakers: i32,
}

impl Default for Audio {
    fn default() -> Self {
        Self { global: 100.0, music: 90.0, sfx: 90.0, voices: 100.0, subtitles: 0, speakers: 0 }
    }
}

/// One key binding: original action (`GBA_*`, or `PORT_*` for actions the port adds) and
/// UE3 key name (`W`, `SpaceBar`, `LeftMouseButton`, `None` = unbound).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub action: String,
    pub key: String,
}

/// Original PC layout (`DefaultInput.ini` `m_PCBindings`, in menu order), then the port's
/// extra actions (no original binding).
pub const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("GBA_MoveForward", "W"),
    ("GBA_MoveBackward", "S"),
    ("GBA_StrafeLeft", "A"),
    ("GBA_StrafeRight", "D"),
    ("GBA_Jump", "SpaceBar"),
    ("GBA_Sneak", "C"),
    ("GBA_Sprint", "LeftShift"),
    ("GBA_LeanLeft", "Q"),
    ("GBA_LeanRight", "E"),
    ("GBA_Use", "F"),
    ("GBA_Primary", "LeftMouseButton"),
    ("GBA_Secondary", "RightMouseButton"),
    ("GBA_Block", "LeftControl"),
    ("GBA_Zoom", "LeftAlt"),
    ("GBA_Wheel", "MiddleMouseButton"),
    ("GBA_NextItem", "MouseScrollUp"),
    ("GBA_PrevItem", "MouseScrollDown"),
    ("GBA_HealthElixir", "R"),
    ("GBA_ManaElixir", "T"),
    ("GBA_Journal", "J"),
    ("GBA_OpenNotes", "None"),
    ("GBA_OpenPowers", "None"),
    ("GBA_OpenBoneCharms", "None"),
    ("GBA_OpenInventory", "None"),
    ("GBA_QuickSave", "F5"),
    ("GBA_QuickLoad", "F9"),
    ("GBA_Shortcut_0", "One"),
    ("GBA_Shortcut_1", "Two"),
    ("GBA_Shortcut_2", "Three"),
    ("GBA_Shortcut_3", "Four"),
    ("GBA_Shortcut_4", "Five"),
    ("GBA_Shortcut_5", "Six"),
    ("GBA_Shortcut_6", "Seven"),
    ("GBA_Shortcut_7", "Eight"),
    ("GBA_Shortcut_8", "Nine"),
    ("GBA_Shortcut_9", "Zero"),
    // Port additions: walk (the original has no walk key) and reload (automatic in the
    // original; R is shared with Health Elixir, which the port does not implement).
    ("PORT_Walk", "X"),
    ("PORT_Reload", "R"),
];

/// Bindings the port acts on; the rest are shown, rebindable and saved.
pub const APPLIED_ACTIONS: &[&str] = &[
    "GBA_MoveForward", "GBA_MoveBackward", "GBA_StrafeLeft", "GBA_StrafeRight", "GBA_Jump", "GBA_Sneak", "GBA_Sprint", "GBA_LeanLeft", "GBA_LeanRight", "GBA_Use",
    "GBA_Primary", "GBA_Secondary", "GBA_Block", "GBA_HealthElixir", "GBA_ManaElixir", "GBA_QuickSave", "GBA_QuickLoad", "GBA_Shortcut_0", "GBA_Shortcut_1", "GBA_Shortcut_2", "GBA_Shortcut_3",
    "GBA_Shortcut_4", "GBA_Shortcut_5", "GBA_Shortcut_6", "GBA_Shortcut_7", "GBA_Shortcut_8", "GBA_Shortcut_9", "PORT_Walk", "PORT_Reload",
];

pub fn default_bindings() -> Vec<Binding> {
    DEFAULT_BINDINGS.iter().map(|(a, k)| Binding { action: a.to_string(), key: k.to_string() }).collect()
}

/// Read `m_PCBindings` from a `DefaultInput.ini` text (`.m_PCBindings=(Name="W",Command="GBA_MoveForward")`).
/// Commands with extra parts (`GBA_Use | Dis_PlayerChoice_RequestSkip | ...`) map to the first.
pub fn pc_bindings_from_ini(text: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if !l.trim_start_matches(['.', '+']).starts_with("m_PCBindings=") {
            continue;
        }
        let field = |k: &str| -> Option<String> {
            let i = l.find(&format!("{k}=\""))? + k.len() + 2;
            let j = l[i..].find('"')? + i;
            Some(l[i..j].to_string())
        };
        if let (Some(name), Some(cmd)) = (field("Name"), field("Command")) {
            let action = cmd.split('|').next().unwrap_or("").trim().to_string();
            v.push((action, name));
        }
    }
    v
}

/// Port-specific settings: multiplayer identity, server browser and the server creator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Multiplayer {
    pub name: String,
    pub master: String,
    pub last_connect: String,
    pub host: HostSettings,
}

impl Default for Multiplayer {
    fn default() -> Self {
        Self { name: "player".into(), master: "127.0.0.1:27900".into(), last_connect: "127.0.0.1:27015".into(), host: HostSettings::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostSettings {
    pub name: String,
    pub map: String,
    /// "ffa", "contracts" or "mission".
    pub mode: String,
    /// Mission file for mode "mission".
    pub mission: String,
    pub port: u16,
    pub max_players: u32,
    pub score_limit: u32,
    /// Seconds.
    pub time_limit: u32,
    pub register: bool,
    /// Start a local control plane (`dishonored_master`) with the server.
    pub local_master: bool,
}

impl Default for HostSettings {
    fn default() -> Self {
        Self {
            name: "Blink server".into(),
            map: "L_Boyle_Ext_P".into(),
            mode: "ffa".into(),
            mission: String::new(),
            port: 27015,
            max_players: 8,
            score_limit: 20,
            time_limit: 600,
            register: true,
            local_master: true,
        }
    }
}

/// The launcher's last picks (port-specific): the offline match and free roam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherPrefs {
    /// Offline mode index on the Play screen.
    pub play_mode: usize,
    pub bots: u32,
    /// Map of the offline match.
    pub play_map: String,
    /// Map last picked for free roam.
    pub freeroam_map: String,
}

impl Default for LauncherPrefs {
    fn default() -> Self {
        Self { play_mode: 0, bots: 3, play_map: "L_Boyle_Ext_P".into(), freeroam_map: "L_Boyle_Ext_P".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub gameplay: Gameplay,
    pub hud: Hud,
    pub mouse: Mouse,
    pub gamepad: Gamepad,
    pub bindings: Vec<Binding>,
    pub graphics: Graphics,
    pub audio: Audio,
    pub multiplayer: Multiplayer,
    pub launcher: LauncherPrefs,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            gameplay: Gameplay::default(),
            hud: Hud::default(),
            mouse: Mouse::default(),
            gamepad: Gamepad::default(),
            bindings: default_bindings(),
            graphics: Graphics::default(),
            audio: Audio::default(),
            multiplayer: Multiplayer::default(),
            launcher: LauncherPrefs::default(),
        }
    }
}

/// `--settings FILE`, else `DISHONORED_RS_SETTINGS`, else `%APPDATA%/dishonored-rs/settings.ron`
/// (`$XDG_CONFIG_HOME` or `~/.config` elsewhere).
pub fn settings_path() -> PathBuf {
    let a: Vec<String> = std::env::args().collect();
    if let Some(p) = a.iter().position(|x| x == "--settings").and_then(|i| a.get(i + 1)) {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("DISHONORED_RS_SETTINGS") {
        return PathBuf::from(p);
    }
    config_dir().join("dishonored-rs").join("settings.ron")
}

/// Where the Dishonored install folder chosen on first launch is kept (next to the default
/// settings file; one line, the folder path).
pub fn game_dir_file() -> PathBuf {
    config_dir().join("dishonored-rs").join("game_dir.txt")
}

/// The install folder chosen on first launch, if one was saved.
pub fn saved_game_dir() -> Option<PathBuf> {
    let t = std::fs::read_to_string(game_dir_file()).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| PathBuf::from(t))
}

pub fn save_game_dir(dir: &Path) -> Result<()> {
    let f = game_dir_file();
    if let Some(p) = f.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&f, format!("{}\n", dir.display()))?;
    Ok(())
}

fn config_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("APPDATA") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")).unwrap_or_else(|| PathBuf::from("."))
}

impl Settings {
    /// Load, falling back to defaults when the file is missing; unknown or missing fields
    /// take defaults (older/newer files stay readable). Missing bindings are appended.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let mut s: Settings = ron::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        s.fill_bindings();
        Ok(s)
    }

    /// [`Settings::load`] from [`settings_path`], defaults on any error.
    pub fn load_default_path() -> (Self, PathBuf) {
        let p = settings_path();
        let s = Self::load(&p).unwrap_or_else(|e| {
            eprintln!("settings: {e:#}; using defaults");
            Self::default()
        });
        (s, p)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new().depth_limit(3))?;
        let tmp = path.with_extension("ron.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    fn fill_bindings(&mut self) {
        for (a, k) in DEFAULT_BINDINGS {
            if !self.bindings.iter().any(|b| b.action == *a) {
                self.bindings.push(Binding { action: a.to_string(), key: k.to_string() });
            }
        }
    }

    pub fn key_for(&self, action: &str) -> Option<&str> {
        self.bindings.iter().find(|b| b.action == action).map(|b| b.key.as_str()).filter(|k| *k != "None")
    }

    /// Rebind; a key bound elsewhere is taken from that action (the original swaps out the
    /// old binding the same way), except the shared port defaults.
    pub fn rebind(&mut self, action: &str, key: &str) {
        for b in &mut self.bindings {
            if b.action != action && b.key == key && key != "None" {
                b.key = "None".into();
            }
        }
        match self.bindings.iter_mut().find(|b| b.action == action) {
            Some(b) => b.key = key.to_string(),
            None => self.bindings.push(Binding { action: action.into(), key: key.into() }),
        }
    }

    /// Restore one sub-tab's defaults ("Restore settings" in the original applies to the
    /// current category).
    pub fn restore(&mut self, sub: SubCategory) {
        let d = Settings::default();
        match sub {
            SubCategory::Gameplay => self.gameplay = d.gameplay,
            SubCategory::Hud => self.hud = d.hud,
            SubCategory::Keyboard => self.bindings = d.bindings,
            SubCategory::Mouse => self.mouse = d.mouse,
            SubCategory::Gamepad => self.gamepad = d.gamepad,
            SubCategory::Graphics => self.graphics = d.graphics,
            SubCategory::Audio => self.audio = d.audio,
        }
    }

    pub fn get(&self, id: &str) -> Option<Value> {
        use Value::*;
        let (g, h, m, p, gr, a) = (&self.gameplay, &self.hud, &self.mouse, &self.gamepad, &self.graphics, &self.audio);
        Some(match id {
            "PSI_Gameplay_bAutoUseManaElixir" => Bool(g.auto_use_mana_elixir),
            "PSI_Gameplay_KillCamMode" => Int(g.kill_cam),
            "PSI_Gameplay_Difficulty" => Int(g.difficulty),
            "PSI_Gameplay_bAutoSaveInMenu" => Bool(g.autosave_in_journal),
            "PSI_Gameplay_HeadBobAmount" => Float(g.head_bob),
            "PSI_Gameplay_CameraRelativeClimbing" => Bool(g.camera_relative_climbing),
            "PSI_HUD_Visibility" => Int(h.visibility),
            "PSI_HUD_bShowObjectivePopups" => Bool(h.objective_popups),
            "PSI_HUD_bShowTutorialNotifications" => Bool(h.tutorial_notifications),
            "PSI_HUD_bShowInteractions" => Bool(h.interactions),
            "PSI_HUD_bShowHighlight" => Bool(h.highlight),
            "PSI_HUD_bShowPickupLog" => Bool(h.pickup_log),
            "PSI_HUD_bShowContextualIcons" => Bool(h.contextual_icons),
            "PSI_HUD_bShowPlayerStance" => Bool(h.player_stance),
            "PSI_HUD_bShowObjectiveMarkers" => Bool(h.objective_markers),
            "PSI_HUD_bShowGrenadeMarkers" => Bool(h.grenade_markers),
            "PSI_HUD_bShowAwarenessMarkers" => Bool(h.awareness_markers),
            "PSI_HUD_bShowHeartTargetMarkers" => Bool(h.heart_markers),
            "PSI_HUD_CrosshairStyle" => Int(h.crosshair_style),
            "PSI_HUD_bCrosshairMovement" => Bool(h.crosshair_movement),
            "PSI_HUD_CrosshairOpacity" => Float(h.crosshair_opacity),
            "PSI_Mouse_Sensitivity" => Float(m.sensitivity),
            "PSI_Mouse_bSmooting" => Bool(m.smoothing),
            "PSI_Mouse_bInvertY" => Bool(m.invert_y),
            "PSI_Mouse_bAutoAim" => Bool(m.auto_aim),
            "PSI_Mouse_AutoAimStrength" => Float(m.auto_aim_strength),
            "PSI_Mouse_bFriction" => Bool(m.aim_assist),
            "PSI_Mouse_FrictionStrength" => Float(m.aim_assist_strength),
            "PSI_Gamepad_BindingSet" => Int(p.binding_set),
            "PSI_Gamepad_bVibration" => Bool(p.vibration),
            "PSI_Gamepad_LookXSensitivity" => Float(p.look_x),
            "PSI_Gamepad_LookYSensitivity" => Float(p.look_y),
            "PSI_Gamepad_bInvertY" => Bool(p.invert_y),
            "PSI_Gamepad_bAutoAim" => Bool(p.auto_aim),
            "PSI_Gamepad_AutoAimStrength" => Float(p.auto_aim_strength),
            "PSI_Gamepad_bFriction" => Bool(p.aim_assist),
            "PSI_Gamepad_FrictionStrength" => Float(p.aim_assist_strength),
            "PSI_Graphics_Gamma" => Float(gr.gamma),
            "PSI_GraphicsPC_Resolution" => {
                let (w, h) = gr.resolution.unwrap_or((0, 0));
                Resolution(w, h)
            }
            "PSI_GraphicsPC_bFullScreen" => Bool(gr.fullscreen),
            "PSI_GraphicsPC_bVSync" => Bool(gr.vsync),
            "PSI_GraphicsPC_FOV" => Float(gr.fov),
            "PSI_GraphicsPC_TextureDetails" => Int(gr.texture_details),
            "PSI_GraphicsPC_ModelDetails" => Int(gr.model_details),
            "PSI_GraphicsPC_LightShaftEnable" => Bool(gr.light_shafts),
            "PSI_GraphicsPC_AntiAliasingMode" => Int(gr.anti_aliasing),
            "PSI_GraphicsPC_RatShadows" => Bool(gr.rat_shadows),
            "PSI_Audio_GlobalVolume" => Float(a.global),
            "PSI_Audio_MusicVolume" => Float(a.music),
            "PSI_Audio_SFXVolume" => Float(a.sfx),
            "PSI_Audio_VoicesVolume" => Float(a.voices),
            "PSI_Audio_SubtitlesMode" => Int(a.subtitles),
            "PSI_AudioPC_SpeakerConfiguration" => Int(a.speakers),
            _ => return None,
        })
    }

    /// Set an option, clamping ranges and choices like the original widgets. Returns false
    /// for unknown ids or mismatched value types.
    pub fn set(&mut self, id: &str, v: Value) -> bool {
        let Some(def) = option(id) else { return false };
        let v = match (def.kind, v) {
            (Kind::Bool, Value::Bool(_)) | (Kind::Resolution, Value::Resolution(..)) => v,
            (Kind::Bool, Value::Int(i)) => Value::Bool(i != 0),
            (Kind::Choice(c), Value::Int(i)) => Value::Int(i.clamp(0, c.len() as i32 - 1)),
            (Kind::Range { min, max, step }, x @ (Value::Float(_) | Value::Int(_))) => {
                let f = x.as_f32().clamp(min, max);
                // Snap to the widget's step (relative to min), keeping one decimal of precision.
                let s = ((f - min) / step).round() * step + min;
                Value::Float((s.clamp(min, max) * 1000.0).round() / 1000.0)
            }
            _ => return false,
        };
        let b = |v: Value| matches!(v, Value::Bool(true));
        let i = |v: Value| if let Value::Int(i) = v { i } else { 0 };
        let f = |v: Value| v.as_f32();
        let (g, h, m, p, gr, a) = (&mut self.gameplay, &mut self.hud, &mut self.mouse, &mut self.gamepad, &mut self.graphics, &mut self.audio);
        match id {
            "PSI_Gameplay_bAutoUseManaElixir" => g.auto_use_mana_elixir = b(v),
            "PSI_Gameplay_KillCamMode" => g.kill_cam = i(v),
            "PSI_Gameplay_Difficulty" => g.difficulty = i(v),
            "PSI_Gameplay_bAutoSaveInMenu" => g.autosave_in_journal = b(v),
            "PSI_Gameplay_HeadBobAmount" => g.head_bob = f(v),
            "PSI_Gameplay_CameraRelativeClimbing" => g.camera_relative_climbing = b(v),
            "PSI_HUD_Visibility" => h.visibility = i(v),
            "PSI_HUD_bShowObjectivePopups" => h.objective_popups = b(v),
            "PSI_HUD_bShowTutorialNotifications" => h.tutorial_notifications = b(v),
            "PSI_HUD_bShowInteractions" => h.interactions = b(v),
            "PSI_HUD_bShowHighlight" => h.highlight = b(v),
            "PSI_HUD_bShowPickupLog" => h.pickup_log = b(v),
            "PSI_HUD_bShowContextualIcons" => h.contextual_icons = b(v),
            "PSI_HUD_bShowPlayerStance" => h.player_stance = b(v),
            "PSI_HUD_bShowObjectiveMarkers" => h.objective_markers = b(v),
            "PSI_HUD_bShowGrenadeMarkers" => h.grenade_markers = b(v),
            "PSI_HUD_bShowAwarenessMarkers" => h.awareness_markers = b(v),
            "PSI_HUD_bShowHeartTargetMarkers" => h.heart_markers = b(v),
            "PSI_HUD_CrosshairStyle" => h.crosshair_style = i(v),
            "PSI_HUD_bCrosshairMovement" => h.crosshair_movement = b(v),
            "PSI_HUD_CrosshairOpacity" => h.crosshair_opacity = f(v),
            "PSI_Mouse_Sensitivity" => m.sensitivity = f(v),
            "PSI_Mouse_bSmooting" => m.smoothing = b(v),
            "PSI_Mouse_bInvertY" => m.invert_y = b(v),
            "PSI_Mouse_bAutoAim" => m.auto_aim = b(v),
            "PSI_Mouse_AutoAimStrength" => m.auto_aim_strength = f(v),
            "PSI_Mouse_bFriction" => m.aim_assist = b(v),
            "PSI_Mouse_FrictionStrength" => m.aim_assist_strength = f(v),
            "PSI_Gamepad_BindingSet" => p.binding_set = i(v),
            "PSI_Gamepad_bVibration" => p.vibration = b(v),
            "PSI_Gamepad_LookXSensitivity" => p.look_x = f(v),
            "PSI_Gamepad_LookYSensitivity" => p.look_y = f(v),
            "PSI_Gamepad_bInvertY" => p.invert_y = b(v),
            "PSI_Gamepad_bAutoAim" => p.auto_aim = b(v),
            "PSI_Gamepad_AutoAimStrength" => p.auto_aim_strength = f(v),
            "PSI_Gamepad_bFriction" => p.aim_assist = b(v),
            "PSI_Gamepad_FrictionStrength" => p.aim_assist_strength = f(v),
            "PSI_Graphics_Gamma" => gr.gamma = f(v),
            "PSI_GraphicsPC_Resolution" => {
                if let Value::Resolution(w, hh) = v {
                    gr.resolution = (w > 0 && hh > 0).then_some((w, hh));
                }
            }
            "PSI_GraphicsPC_bFullScreen" => gr.fullscreen = b(v),
            "PSI_GraphicsPC_bVSync" => gr.vsync = b(v),
            "PSI_GraphicsPC_FOV" => gr.fov = f(v),
            "PSI_GraphicsPC_TextureDetails" => gr.texture_details = i(v),
            "PSI_GraphicsPC_ModelDetails" => gr.model_details = i(v),
            "PSI_GraphicsPC_LightShaftEnable" => gr.light_shafts = b(v),
            "PSI_GraphicsPC_AntiAliasingMode" => gr.anti_aliasing = i(v),
            "PSI_GraphicsPC_RatShadows" => gr.rat_shadows = b(v),
            "PSI_Audio_GlobalVolume" => a.global = f(v),
            "PSI_Audio_MusicVolume" => a.music = f(v),
            "PSI_Audio_SFXVolume" => a.sfx = f(v),
            "PSI_Audio_VoicesVolume" => a.voices = f(v),
            "PSI_Audio_SubtitlesMode" => a.subtitles = i(v),
            "PSI_AudioPC_SpeakerConfiguration" => a.speakers = i(v),
            _ => return false,
        }
        true
    }

    /// Step an option the way the stepper arrows / slider keys do (`dir` = -1 or +1).
    /// Returns whether the value changed: the original widgets clamp at their ends (the
    /// options list creates every stepper with `isLoop` false; a boolean is a two-value
    /// stepper, Off then On), where they play `Stop` instead.
    pub fn step(&mut self, id: &str, dir: i32) -> bool {
        let (Some(def), Some(cur)) = (option(id), self.get(id)) else { return false };
        let next = match (def.kind, cur) {
            (Kind::Bool, Value::Bool(b)) => {
                let want = dir > 0;
                if want == b {
                    return false;
                }
                Value::Bool(want)
            }
            (Kind::Choice(c), Value::Int(i)) => {
                let n = i + dir.signum();
                if n < 0 || n >= c.len() as i32 {
                    return false;
                }
                Value::Int(n)
            }
            (Kind::Range { step, .. }, v) => Value::Float(v.as_f32() + step * dir as f32),
            _ => return false,
        };
        let before = self.get(id);
        self.set(id, next) && self.get(id) != before
    }

    /// Mouse look scale in degrees per mouse count: the runtime's 0.1 deg/count base times
    /// the menu multiplier.
    pub fn mouse_degrees_per_count(&self) -> f32 {
        0.1 * self.mouse.sensitivity
    }

    /// Linear gains (0..1) per audio bus.
    pub fn bus_gain(&self, bus: Bus) -> f32 {
        let g = self.audio.global / 100.0;
        g * match bus {
            Bus::Master => 1.0,
            Bus::Music => self.audio.music / 100.0,
            Bus::Sfx => self.audio.sfx / 100.0,
            Bus::Voice => self.audio.voices / 100.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Master,
    Music,
    Sfx,
    Voice,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_option_round_trips_through_get_set() {
        let mut s = Settings::default();
        for o in OPTIONS {
            if matches!(o.kind, Kind::Link(_)) {
                continue;
            }
            let v = s.get(o.id).unwrap_or_else(|| panic!("get {}", o.id));
            assert!(s.set(o.id, v), "set {}", o.id);
            assert_eq!(s.get(o.id), Some(v), "{}", o.id);
        }
    }

    #[test]
    fn defaults_match_ark_profile_settings() {
        let s = Settings::default();
        assert_eq!(s.graphics.fov, 75.0);
        assert_eq!(s.graphics.gamma, 2.2);
        assert_eq!(s.audio.music, 90.0);
        assert_eq!(s.hud.visibility, 2);
        assert_eq!(s.hud.crosshair_style, 2);
        assert_eq!(s.mouse.sensitivity, 1.0);
        assert!(s.mouse.smoothing && !s.mouse.invert_y);
        assert_eq!(s.key_for("GBA_Jump"), Some("SpaceBar"));
        assert_eq!(s.key_for("GBA_OpenNotes"), None);
    }

    #[test]
    fn ranges_snap_and_clamp() {
        let mut s = Settings::default();
        assert!(s.set("PSI_GraphicsPC_FOV", Value::Float(92.0)));
        assert_eq!(s.graphics.fov, 90.0);
        assert!(s.set("PSI_GraphicsPC_FOV", Value::Float(500.0)));
        assert_eq!(s.graphics.fov, 110.0);
        assert!(!s.step("PSI_GraphicsPC_FOV", 1), "sliders clamp");
        assert_eq!(s.graphics.fov, 110.0);
        assert!(!s.step("PSI_HUD_CrosshairStyle", 1), "steppers clamp at the last value");
        assert_eq!(s.hud.crosshair_style, 2);
        assert!(s.step("PSI_HUD_CrosshairStyle", -1));
        assert_eq!(s.hud.crosshair_style, 1);
        assert!(!s.step("PSI_Mouse_bInvertY", -1), "Off is the first value");
        assert!(s.step("PSI_Mouse_bInvertY", 1));
        assert!(s.mouse.invert_y);
        assert!(s.step("PSI_Mouse_Sensitivity", -1));
        assert!((s.mouse.sensitivity - 0.9).abs() < 1e-4);
    }

    #[test]
    fn save_load_round_trip_and_partial_files() {
        let dir = std::env::temp_dir().join(format!("dishonored_settings_test_{}", std::process::id()));
        let p = dir.join("settings.ron");
        let mut s = Settings::default();
        s.graphics.fov = 90.0;
        s.rebind("GBA_Jump", "V");
        s.save(&p).unwrap();
        let l = Settings::load(&p).unwrap();
        assert_eq!(l, s);
        std::fs::write(&p, "(graphics: (fov: 100.0))").unwrap();
        let l = Settings::load(&p).unwrap();
        assert_eq!(l.graphics.fov, 100.0);
        assert_eq!(l.audio, Audio::default());
        assert_eq!(l.bindings.len(), DEFAULT_BINDINGS.len());
        assert_eq!(Settings::load(&dir.join("missing.ron")).unwrap(), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebinding_takes_the_key_from_its_previous_action() {
        let mut s = Settings::default();
        s.rebind("GBA_Jump", "C");
        assert_eq!(s.key_for("GBA_Jump"), Some("C"));
        assert_eq!(s.key_for("GBA_Sneak"), None);
        s.restore(SubCategory::Keyboard);
        assert_eq!(s.key_for("GBA_Sneak"), Some("C"));
    }

    #[test]
    fn ini_bindings_parse() {
        let t = ".m_PCBindings=(Name=\"F\",Command=\"GBA_Use | Dis_PlayerChoice_RequestSkip\")\n.m_PCBindings=(Name=\"W\",Command=\"GBA_MoveForward\")";
        assert_eq!(pc_bindings_from_ini(t), vec![("GBA_Use".into(), "F".into()), ("GBA_MoveForward".into(), "W".into())]);
    }

    /// The compiled-in default layout must equal the install's `m_PCBindings`.
    #[test]
    fn default_bindings_match_install() {
        let root = std::env::var_os("DISHONORED_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored"));
        let Ok(t) = std::fs::read_to_string(root.join("DishonoredGame/Config/DefaultInput.ini")) else { return };
        let ini = pc_bindings_from_ini(&t);
        assert!(!ini.is_empty());
        for (a, k) in &ini {
            let d = DEFAULT_BINDINGS.iter().find(|(x, _)| x == a).unwrap_or_else(|| panic!("{a} missing"));
            assert_eq!(d.1, k, "{a}");
        }
        assert_eq!(ini.len(), DEFAULT_BINDINGS.iter().filter(|(a, _)| a.starts_with("GBA_")).count());
    }
}
