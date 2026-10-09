//! The in-game HUD: Dishonored's own Scaleform movie (`UI_HUD_SF` "HUD", with the
//! `Common_assets.lib` library it imports from) played on a [`gfx_stage::Stage`], driven the way
//! `DisGFxMoviePlayerHUD` drives it (its tick 0x00BB2280 and helpers). The movie's ActionScript
//! classes are in `hud_widgets.rs`; the popup queue in `hud_popups.rs`; the HUD options in
//! `hud_settings.rs`. See research/dishonored/HUD.md.
//!
//! Per frame, from the session view:
//! - player status (0x00B9F6E0): `FillHealthGauge(health * 100 / max, smallUpg, largeUpg,
//!   health <= m_LowHealthThreshold (30), (health + regen) * 100 / max)` and `FillManaGauge`
//!   (low <= m_LowManaThreshold 19) when a value changes; the gauges `Open()` / `Close()` with
//!   the HUD visibility option (contextual: shown `m_fPlayerInfoDisplayDuration` 8 s after a
//!   change, and while low on health or mana);
//! - equipment (0x00B981F0): `SetWeapon(item, loaded + reserve, loaded)` / `SetPower(item,
//!   active, enoughMana)` / `SetEmptyEquipment()` when the left hand changes;
//! - crosshair (0x00B9FF30): the dot (`SetCrosshairState`), the item's crosshair symbol attached
//!   as `_aspect_mc` (`DisGFxMoviePlayerHUD.m_CrosshairSettings`, DefaultUI.ini) or the
//!   interaction crosshair (`m_InteractionCrosshairSettings`), `SetDispersion`, and the
//!   crosshair moving to the focused interactable (`m_bCrosshairMovement`, speed 4.5);
//! - stance icon (0x00B95ED0): "sneak" / "slide";
//! - interactions (0x00BA6150): `interactions_window_mc.SetInteractions(name, lines)` with the
//!   key tags converted to "[key]" (0x00BA4620);
//! - game messages: `gameMsg_mc.SetGameMessage(msg, m_fGameMessageDuration 5 s)`;
//! - the objective / target popups through the queue (0x00BAFE70);
//! - the pickup log (`AddLogItem(name (count), icon)`, 0x00B98630);
//! - subtitles and choices (`hud_dialogue.rs`);
//! - task markers (one per task target, with the description), awareness markers, the tutorial
//!   line and window, the crosshair texts (research/dishonored/HUD.md).
//!
//! The tutorial window's first-rune / first-bone-charm triggers and the crosshair's sleeper line
//! follow dishonored-bevy's `interact.rs` / `crosshair.rs`. Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.

use crate::gfx_stage::{Draw, Event, Fonts, ImageRef, MovieData, SlotImage, SlotText, Stage, StageTextures, StageView};
use crate::hud_popups::{PopupContent, PopupQueue};
use crate::hud_widgets::{AwarenessMarker, HudFxWidgets, HudWidgets, MarkerInput, MarkerPlace, ObjState, TW_IMG_CHARM, TW_IMG_RUNE, TargetTexts, TaskLine, TutorialWindow, awareness};
use crate::session::{Session, SessionView};
use bevy::prelude::*;
use bevy::text::{LetterSpacing, LineHeight};
use dishonored_assets::swf::Movie;
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::LeftPhase;
use std::collections::HashMap;
use std::sync::Arc;

/// `DisTweaks_GFxMoviePlayerHUD` values (Twk_InGameUI.Twk_GFxMoviePlayerHUD) and the HUD config
/// (DefaultGame.ini `[DishonoredGame.DishonoredHUD]`).
pub const LOW_HEALTH_THRESHOLD: f32 = 30.0;
pub const LOW_MANA_THRESHOLD: f32 = 19.0;
pub const PLAYER_INFO_DISPLAY_DURATION: f32 = 8.0;
pub const GAME_MESSAGE_DURATION: f32 = 5.0;
pub const TARGET_NOTIFICATION_DURATION: f32 = 5.0;
pub const LOCATION_DISCOVERY_DURATION: f32 = 5.0;
pub const CROSSHAIR_INTERPOLATION_SPEED: f32 = 4.5;
/// `m_fTutorialDuration`.
pub const TUTORIAL_DURATION: f32 = 10.0;

/// The HUD movie on its stage.
pub struct HudMovie {
    pub stage: Stage,
    pub w: HudWidgets,
    draws: Vec<Draw>,
}

/// The HUD effects movie (`UI_HUDFX.HUDFX`: stealth shroud, damage directions), drawn under
/// the HUD.
pub struct FxMovie {
    pub stage: Stage,
    pub w: HudFxWidgets,
    draws: Vec<Draw>,
    view: Entity,
}

/// `m_fMaxDamage` (Twk_GFxMoviePlayerHUD): damage giving full intensity.
pub const MAX_DAMAGE: f32 = 70.0;

/// Popup titles and HUD texts (localization: `Twk_InGameUI.int`,
/// `DishonoredGame.int [DisGFxMoviePlayerHUD_Texts]`, `RPG.int`, ...).
#[derive(Default, Clone)]
pub struct HudTexts {
    pub titles: [String; 4],
    pub target: Option<Arc<TargetTexts>>,
    pub ammo: [String; 8],
    pub ammo_icons: [Option<String>; 8],
    pub coin: (String, String),
    pub rune: (String, String),
    pub elixirs: [String; 2],
    pub charm: String,
    /// The tutorial window's texts by picture: `m_PressKeyToReadChapterNoteMessage` (1, note),
    /// `m_PressKeyToBuyPowers` (2, rune), `m_PressKeyToEquipBoneCharms` (3, bone charm).
    pub tutorial_windows: [String; 4],
    /// `m_EquipmentIcons[EDisPowerWheelItem]` (DefaultUI.ini `[DishonoredGame.DisGlobalUIManager]`).
    pub equipment_icons: HashMap<u8, String>,
    /// Key display names (Settings.int `[Keys]`).
    pub keys: HashMap<String, String>,
    /// `DisTweaks_ChapterTarget`s (DishonoredGame.upk `Twk_ChapterInfoList.ChapterTargets.*`):
    /// object name (lower case) -> (`m_TargetName`, `m_TargetPortraitPath`), read on first use.
    pub chapter_targets: Option<HashMap<String, (String, Option<String>)>>,
    /// `m_InteractionTexts[EDisUIInteraction]` (Twk_GFxMoviePlayerHUD): the interactions
    /// window's lines by interaction ("`GBA_Block` [Hold]" for the choke, ...).
    pub interactions: Vec<String>,
    /// `_root.texts.t_Hold` (the interaction gauge's "HOLD").
    pub hold: String,
    /// The interaction gauge's button tags by kind (0x00BA5B90): `m_UsableGaugeGBA` (1),
    /// `m_ChokeGaugeGBA` (2), `m_SkipSceneGaugeGBA` (4).
    pub gauge_tags: [String; 3],
}

/// `Twk_InGameUI.Twk_GFxMoviePlayerHUD`'s string properties by name.
fn hud_tweak_string(content: &dishonored_assets::Content, name: &str) -> Option<String> {
    use ue3_package::properties::{Value, find};
    let pkg = content.package("DishonoredGame").ok()?;
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "DisTweaks_GFxMoviePlayerHUD" && pkg.obj_path(ue3_package::ObjRef::Export(i)).eq_ignore_ascii_case("Twk_InGameUI.Twk_GFxMoviePlayerHUD"))?;
    let h = pkg.read_object(i).ok()?;
    match find(&h.properties, name).map(|p| &p.value) {
        Some(Value::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

/// `Twk_InGameUI.Twk_GFxMoviePlayerHUD.m_InteractionTexts` (a static array of `DUI_MAX`
/// strings; the entries the object leaves out are empty), with the localization's values over
/// them (`[Twk_GFxMoviePlayerHUD DisTweaks_GFxMoviePlayerHUD]`).
pub fn interaction_texts(content: &dishonored_assets::Content, loc: &dishonored_assets::loc::Localization) -> Vec<String> {
    use ue3_package::properties::Value;
    let mut out: Vec<String> = Vec::new();
    let mut put = |i: usize, s: String| {
        if out.len() <= i {
            out.resize(i + 1, String::new());
        }
        out[i] = s;
    };
    if let Ok(pkg) = content.package("DishonoredGame") {
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "DisTweaks_GFxMoviePlayerHUD" {
                continue;
            }
            let path = pkg.obj_path(ue3_package::ObjRef::Export(i));
            if !path.eq_ignore_ascii_case("Twk_InGameUI.Twk_GFxMoviePlayerHUD") {
                continue;
            }
            let Ok(h) = pkg.read_object(i) else { continue };
            for p in h.properties.iter().filter(|p| p.name.eq_ignore_ascii_case("m_InteractionTexts")) {
                if let Value::Str(s) = &p.value {
                    put(p.array_index.max(0) as usize, s.clone());
                }
            }
        }
    }
    for i in 0..64 {
        if let Some(s) = loc.get("Twk_GFxMoviePlayerHUD DisTweaks_GFxMoviePlayerHUD", &format!("m_InteractionTexts[{i}]")) {
            put(i, s.to_string());
        }
    }
    out
}

/// Read the chapter targets (the target notification's name and portrait).
fn chapter_targets(content: &dishonored_assets::Content) -> HashMap<String, (String, Option<String>)> {
    use ue3_package::properties::{Value, find};
    let mut out = HashMap::new();
    let Ok(pkg) = content.package("DishonoredGame") else { return out };
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "DisTweaks_ChapterTarget" {
            continue;
        }
        let path = pkg.obj_path(ue3_package::ObjRef::Export(i));
        let key = path.rsplit('.').next().unwrap_or(&path).to_ascii_lowercase();
        let Ok(h) = pkg.read_object(i) else { continue };
        let s = |n: &str| {
            find(&h.properties, n).and_then(|p| match &p.value {
                Value::Str(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
        };
        if let Some(name) = s("m_TargetName") {
            out.insert(key, (name, s("m_TargetPortraitPath")));
        }
    }
    out
}

/// What the native side last sent (so values are sent only when they change, as the original).
#[derive(Default)]
struct Native {
    health: Option<(i32, i32, i32, u8)>,
    mana: Option<(i32, i32, i32, u8)>,
    info_timer: f32,
    status_visible: bool,
    ticked_once: bool,
    equipment: Option<(u8, bool, bool, i32, i32)>,
    crosshair_pos: Option<Vec2>,
    /// `m_TaskMarkers`: the marker clips and whether each is the optional symbol.
    task_markers: Vec<(crate::gfx_stage::Id, bool)>,
    inv: Option<dishonored_sim::state::Inventory>,
    /// The mission's tutorial line / window last shown (view ids) and the time left on the line.
    tutorial_seen: u32,
    tutorial_left: f32,
    tw_seen: u32,
    /// Tutorial windows waiting (picture, text), the seconds the one up has been (0 when none) and
    /// the first-time hints given (bit 0 a rune, bit 1 a bone charm).
    tw_queue: Vec<(u8, String)>,
    tw_timer: f32,
    hints: u8,
    /// The air gauge is open (the HUD's +0x1FC bit 0x40, set by 0x00B8CE70).
    oxygen_open: bool,
    /// The interaction gauge's kind open (the HUD's +0x3EB: 1 usable, 2 choke, 3 grenade
    /// cooking, 4 cutscene skip; 0 none).
    gauge_kind: u8,
    /// Task markers the harness placed (`{"hud": {"kind": "task_markers"}}`).
    test_markers: Vec<(glam::Vec3, bool, String)>,
    game_msg_seen: u32,
    location_seen: u32,
    last_mission: Option<(String, u32)>,
    /// `m_AwarenessMarkers`: one marker per NPC pawn.
    markers: HashMap<u32, AwarenessMarker>,
    /// The detection stinger played (flag 0x20000 of the HUD, 0x00BAD960).
    stinger: bool,
}

#[derive(Resource, Default)]
pub struct GfxHud {
    pub loaded: bool,
    pub hud: Option<HudMovie>,
    pub fx: Option<FxMovie>,
    pub view: Option<Entity>,
    pub texts: HudTexts,
    pub popups: PopupQueue,
    native: Native,
    /// Requests from the harness (`{"hud": ...}` steps).
    pub tests: Vec<crate::hud_popups::HudTest>,
    /// The install (its package cache serves the external textures).
    pub content: Option<Arc<dishonored_assets::Content>>,
}

// --- assets ---------------------------------------------------------------------------------

fn read_ini_lines(content: &dishonored_assets::Content, file: &str) -> Vec<String> {
    let p = content.root.join("DishonoredGame").join("Config").join(file);
    std::fs::read(&p).map(|b| dishonored_assets::loc::decode(&b).lines().map(|l| l.trim().to_string()).collect()).unwrap_or_default()
}

fn load_texts(content: &dishonored_assets::Content) -> HudTexts {
    let loc = dishonored_assets::loc::Localization::load(&content.root, "INT", &["DishonoredGame", "RPG", "Money_twk_Abs", "Runes_twk", "Twk_InGameUI", "Settings"]);
    let g = |s: &str, k: &str, d: &str| loc.get(s, k).map(str::to_string).unwrap_or_else(|| d.to_string());
    let tw = "Twk_GFxMoviePlayerHUD DisTweaks_GFxMoviePlayerHUD";
    let hud = "DisGFxMoviePlayerHUD_Texts";
    let mut t = HudTexts {
        titles: [
            g(tw, "m_NewObjectivePopupTitle", "New Objective"),
            g(tw, "m_UpdatedObjectivePopupTitle", "Objective Updated"),
            g(tw, "m_CompletedObjectivePopupTitle", "Objective Completed"),
            g(tw, "m_FailedObjectivePopupTitle", "Objective Failed"),
        ],
        target: Some(Arc::new(TargetTexts {
            assassinated: g(hud, "t_targetAssassinated", "Target assassinated"),
            neutralized: g(hud, "t_targetNeutralized", "Target neutralized"),
            rescued: g(hud, "t_targetRescued", "Target rescued"),
            spared: g(hud, "t_targetSpared", "Target spared"),
        })),
        ..Default::default()
    };
    let ammo_keys = ["Bullet", "Bullet_Explosive", "Arrow", "Arrow_Sleep", "Arrow_Flare", "SpringRazor", "Grenade", "StickyGrenade"];
    for (i, k) in ammo_keys.iter().enumerate() {
        t.ammo[i] = loc.get("DisAmmoType", k).unwrap_or(*k).to_string();
    }
    t.coin = (g("AbstractItems.Coins_AbsItm DisAbstractItem", "m_ItemName", "Coin"), g("AbstractItems.Coins_AbsItm DisAbstractItem", "m_PluralItemName", "Coins"));
    t.rune = (g("AbstractItems.Rune_AbsItm DisAbstractItem", "m_ItemName", "Rune"), g("AbstractItems.Rune_AbsItm DisAbstractItem", "m_PluralItemName", "Runes"));
    t.elixirs = [g("DisUISelectionType", "Health", "Health"), g("DisUISelectionType", "Mana", "Mana")];
    t.charm = g("DisUISelectionType", "BoneCharm", "Bone Charm");
    t.tutorial_windows = [
        String::new(),
        g(tw, "m_PressKeyToReadChapterNoteMessage", "Mission Clues updated<br />Press `GBA_Journal` to read"),
        g(tw, "m_PressKeyToBuyPowers", "New Rune added<br />Press `GBA_Journal` to acquire Powers"),
        g(tw, "m_PressKeyToEquipBoneCharms", "New Bone Charm added<br />Press `GBA_Journal` to equip"),
    ];
    t.interactions = interaction_texts(content, &loc);
    t.hold = g(hud, "t_Hold", "HOLD").to_uppercase();
    // The class defaults when the object leaves one out (`DisTweaks_GFxMoviePlayerHUD`).
    t.gauge_tags = ["m_UsableGaugeGBA", "m_ChokeGaugeGBA", "m_SkipSceneGaugeGBA"].map(|n| hud_tweak_string(content, n).unwrap_or_default());
    for ((sec, k), v) in &loc.entries {
        if sec == "Keys" {
            t.keys.insert(k.clone(), v.clone());
        }
    }
    // DefaultUI.ini: equipment icons and the journal's ammo icon names.
    for l in read_ini_lines(content, "DefaultUI.ini") {
        if let Some(rest) = l.strip_prefix("m_EquipmentIcons[") {
            if let Some((i, v)) = rest.split_once("]=") {
                if let Ok(i) = i.parse::<u8>() {
                    t.equipment_icons.insert(i, v.trim_matches('"').to_string());
                }
            }
        }
        if let Some(rest) = l.strip_prefix("m_AmmoTypeIconNames[") {
            if let Some((i, v)) = rest.split_once("]=") {
                if let Ok(i) = i.parse::<usize>() {
                    if i < 8 {
                        t.ammo_icons[i] = Some(format!("UI_ItemIcons_Small.{}_Small", v.trim_matches('"')));
                    }
                }
            }
        }
    }
    t
}

pub fn setup_gfx_hud(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut textures: ResMut<StageTextures>) {
    let mut hud = GfxHud::default();
    let Ok(content) = dishonored_assets::Content::locate() else { return commands.insert_resource(hud) };
    let fonts = Fonts::load(&content);
    let Some(main) = MovieData::load(&content, "UI_HUD_SF", "HUD", &fonts, "") else {
        warn!("HUD movie not readable");
        return commands.insert_resource(hud);
    };
    let lib = MovieData::load(&content, "Startup", "Common_assets.lib", &fonts, "common_assets");
    textures.load_movie(&content, "UI_HUD_SF", &main.movie, &mut images);
    if let Some(l) = &lib {
        textures.load_movie(&content, "Startup", &l.movie, &mut images);
    }
    let mut stage = Stage::new(Arc::new(main), lib.into_iter().map(Arc::new).collect());
    let w = HudWidgets::new(&mut stage);
    hud.texts = load_texts(&content);
    hud.hud = Some(HudMovie { stage, w, draws: Vec::new() });
    // The effects movie first (drawn under the HUD).
    if let Some(fx) = MovieData::load(&content, "DishonoredGame", "UI_HUDFX.HUDFX", &fonts, "") {
        textures.load_movie(&content, "DishonoredGame", &fx.movie, &mut images);
        let mut stage = Stage::new(Arc::new(fx), Vec::new());
        let w = HudFxWidgets::new(&mut stage);
        let view = crate::gfx_stage::spawn_view(&mut commands, 1, -2);
        hud.fx = Some(FxMovie { stage, w, draws: Vec::new(), view });
    }
    hud.view = Some(crate::gfx_stage::spawn_view(&mut commands, 0, -1));
    hud.loaded = true;
    hud.content = Some(Arc::new(content));
    info!("HUD: UI_HUD_SF movie on a stage ({} textures)", textures.movie.len());
    commands.insert_resource(hud);
}

// --- inputs ---------------------------------------------------------------------------------

/// `EDisPowerWheelItem` of the left hand (DisGlobalEnums).
pub fn wheel_item(h: LeftHand) -> u8 {
    match h {
        LeftHand::Empty => 0,
        LeftHand::Crossbow => 1,
        LeftHand::SleepDart => 2,
        LeftHand::IncendiaryBolt => 3,
        LeftHand::SpringRazor => 5,
        LeftHand::Grenade => 6,
        LeftHand::StickyGrenade => 7,
        LeftHand::Pistol => 8,
        LeftHand::DarkVision => 21,
        LeftHand::Windblast => 22,
        LeftHand::DevouringSwarm => 23,
        LeftHand::Blink => 24,
        LeftHand::BendTime => 25,
        LeftHand::Possession => 26,
    }
}

/// `m_CrosshairSettings[item]` (DefaultUI.ini): crosshair symbol, dot shown, dispersion.
pub fn crosshair_settings(item: u8) -> (Option<&'static str>, bool, bool) {
    match item {
        1..=3 | 10 => (Some("crosshair_crossbow"), true, true),
        4 => (None, true, false),
        5 | 11 | 13 => (Some("crosshair_springRazor"), true, false),
        6 | 12 => (Some("crosshair_grenade"), true, false),
        7 => (Some("crosshair_stickyGrenade"), true, false),
        8 | 9 => (Some("crosshair_gun"), false, true),
        21..=30 => (Some("crosshair_power"), true, false),
        _ => (None, true, false),
    }
}

fn ammo_of(m: &dishonored_sim::state::PlayerState, item: u8) -> Option<(i32, i32)> {
    let inv = &m.inv;
    match item {
        8 => Some(((m.left.pistol_loaded as i32) + m.left.pistol_reserve as i32, m.left.pistol_loaded as i32)),
        1 => Some(((m.left.bolts_loaded as i32) + m.left.bolts_reserve as i32, m.left.bolts_loaded as i32)),
        2 => Some((inv.ammo[3] as i32, 0)),
        3 => Some((inv.ammo[4] as i32, 0)),
        5 => Some((inv.ammo[5] as i32, 0)),
        6 => Some((inv.ammo[6] as i32, 0)),
        7 => Some((inv.ammo[7] as i32, 0)),
        _ => None,
    }
}

/// Key tags of the original texts (`` `GBA_Use` ``, `` `k ``) as the HUD shows them on PC: the
/// bound key's name (Settings.int `[Keys]`) in brackets (0x00BA4620 wraps it in "[" "]";
/// mouse buttons are `$LMouseButton$`-style icons in the original, named here).
pub fn convert_key_tags(s: &str, bindings: Option<&crate::settings::Bindings>, keys: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('`') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(stripped) = after.strip_prefix('k') {
            out.push_str(&format!("[{}]", key_name(bindings, keys, "GBA_Use")));
            rest = stripped;
            continue;
        }
        match after.find('`') {
            Some(j) => {
                let tag = &after[..j];
                out.push_str(&format!("[{}]", key_name(bindings, keys, tag)));
                rest = &after[j + 1..];
            }
            None => {
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Display name of the key bound to an action (`DefaultInput.ini` defaults when unbound here).
fn key_name(bindings: Option<&crate::settings::Bindings>, keys: &HashMap<String, String>, action: &str) -> String {
    let ue = bindings.and_then(|b| b.input(action)).and_then(crate::settings::name_for).map(str::to_string).unwrap_or_else(|| {
        match action {
            "GBA_Use" => "F",
            "GBA_Jump" => "SpaceBar",
            "GBA_Journal" => "J",
            "GBA_Primary" => "LeftMouseButton",
            "GBA_Secondary" => "RightMouseButton",
            "GBA_Block" => "LeftControl",
            "GBA_Zoom" => "Z",
            other => other.trim_start_matches("GBA_").trim_start_matches("KEY_"),
        }
        .to_string()
    });
    keys.get(&ue).cloned().unwrap_or(ue)
}

// --- marker placement -----------------------------------------------------------------------

/// The marker area: `m_fMarkerAreaRatio_PC` of the stage (0.9).
pub const MARKER_AREA_RATIO: f32 = 0.9;

/// Where a world point (Dishonored units) lands on the HUD stage for a marker, and whether it is
/// off screen (outside the marker area or behind the camera: the point is then moved onto the
/// edge of the area along the direction to it). `tan_h` is `tan(horizontal fov / 2)`.
pub fn marker_projection(cam: &GlobalTransform, window: Vec2, tan_h: f32, world_uu: glam::Vec3) -> (Vec2, bool, f32) {
    let w = crate::coords::pos(Vec3::new(world_uu.x, world_uu.y, world_uu.z));
    let v = cam.affine().inverse().transform_point3(w);
    let depth = -v.z;
    let aspect = (window.x / window.y.max(1.0)).max(0.1);
    let (stage_scale, offset) = crate::gfx_stage::stage_xform(window);
    let centre = crate::gfx_stage::STAGE * 0.5;
    let half = centre * MARKER_AREA_RATIO;
    let dist = v.length() / crate::coords::UU_TO_BEVY;
    // Direction on screen (y down).
    let dir = Vec2::new(v.x, -v.y);
    if depth > 0.01 {
        let ndc = Vec2::new(v.x / (depth * tan_h), v.y / (depth * tan_h / aspect));
        let px = Vec2::new((ndc.x * 0.5 + 0.5) * window.x, (0.5 - ndc.y * 0.5) * window.y);
        let sp = (px - offset) / stage_scale;
        let d = sp - centre;
        if d.x.abs() <= half.x && d.y.abs() <= half.y {
            return (sp, false, dist);
        }
        return (centre + d * (half.x / d.x.abs().max(1e-4)).min(half.y / d.y.abs().max(1e-4)).min(1.0), true, dist);
    }
    let d = if dir.length_squared() < 1e-8 { Vec2::new(0.0, 1.0) } else { dir.normalize() };
    (centre + d * (half.x / d.x.abs().max(1e-4)).min(half.y / d.y.abs().max(1e-4)), true, dist)
}

/// `m_fMarkerFocusDistance` (Twk_GFxMoviePlayerHUD): the marker nearest the middle of the stage,
/// within this many stage pixels, is focused and shows its description (`m_bCanFocus`).
pub const MARKER_FOCUS_DISTANCE: f32 = 225.0;

/// The task markers (`m_TaskMarkers`, 0x00BAD550 builds them from the player's active task
/// targets; the settings are `m_TaskMarkerSettings` / `m_OptionalTaskMarkerSettings`):
/// `m_fWorldOffsetZ` 10 above the target, scale 90 % at 10000 uu growing to 150 % at 500 uu,
/// alpha 100 from 1000 uu down to 0 at the target. The tweak sets no `m_bKeepBBInMarkerArea`, so
/// a marker outside the marker area (or behind the camera) is simply hidden. The description
/// (`_description_mc`) is shown on the focused marker: the target's `m_TargetName` and, with
/// `m_TaskMarkerSpecificSettings.m_bShowDistance`, its distance in metres (100 uu).
///
/// The focus rule and the distance text are Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
fn update_task_markers(st: &mut Stage, n: &mut Native, layer: Option<crate::gfx_stage::Id>, wanted: &[(glam::Vec3, bool, String)], cam: Option<&GlobalTransform>, window: Option<Vec2>, tan_h: f32) {
    use crate::gfx_stage::Prop;
    let (Some(layer), Some(cam), Some(win)) = (layer, cam, window) else {
        for (id, _) in n.task_markers.drain(..) {
            if st.alive(id) {
                st.remove(id);
            }
        }
        return;
    };
    while n.task_markers.len() > wanted.len() {
        if let Some((id, _)) = n.task_markers.pop() {
            if st.alive(id) {
                st.remove(id);
            }
        }
    }
    let centre = crate::gfx_stage::STAGE * 0.5;
    let mut placed: Vec<Option<(Vec2, f32)>> = Vec::new();
    for (i, (pos, optional, _)) in wanted.iter().enumerate() {
        if let Some(&(id, opt)) = n.task_markers.get(i)
            && (opt != *optional || !st.alive(id))
        {
            if st.alive(id) {
                st.remove(id);
            }
            n.task_markers.truncate(i);
        }
        if n.task_markers.len() <= i {
            let d = st.next_depth(layer);
            let Some(mk) = st.attach(layer, if *optional { "objectiveMarker_secondary" } else { "objectiveMarker_primary" }, &format!("taskMarker{i}"), d) else { break };
            if let Some(desc) = st.child(mk, "_description_mc") {
                st.set_visible(desc, false);
            }
            n.task_markers.push((mk, *optional));
        }
        // m_fWorldOffsetZ 10.
        let (sp, off, dist) = marker_projection(cam, win, tan_h, *pos + glam::Vec3::Z * 10.0);
        placed.push((!off).then_some((sp, dist)));
    }
    // The focused marker: nearest the middle, within m_fMarkerFocusDistance.
    let focused = placed.iter().enumerate().filter_map(|(i, p)| p.map(|(sp, _)| (i, (sp - centre).length()))).filter(|(_, d)| *d < MARKER_FOCUS_DISTANCE).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
    let var = |d: f32, d0: f32, d1: f32, v0: f32, v1: f32| v0 + ((d - d0) / (d1 - d0)).clamp(0.0, 1.0) * (v1 - v0);
    for (i, &(mk, _)) in n.task_markers.iter().enumerate() {
        let Some(Some((sp, dist))) = placed.get(i).copied() else {
            st.set_visible(mk, false);
            continue;
        };
        // m_ScaleVariation (10000 uu -> 90 %, 500 uu -> 150 %), m_AlphaVariation (1000 uu -> 100,
        // 0 -> 0).
        let scale = var(dist, 10000.0, 500.0, 90.0, 150.0);
        let alpha = var(dist, 1000.0, 0.0, 100.0, 0.0);
        st.set(mk, Prop::X, sp.x);
        st.set(mk, Prop::Y, sp.y);
        st.set(mk, Prop::XScale, scale);
        st.set(mk, Prop::YScale, scale);
        st.set(mk, Prop::Alpha, alpha);
        st.set_visible(mk, true);
        if let Some(desc) = st.child(mk, "_description_mc") {
            let on = focused == Some(i);
            st.set_visible(desc, on);
            if on && let Some(t) = st.child(desc, "txt") {
                let name = &wanted[i].2;
                let metres = (dist / 100.0).round() as i32;
                st.set_text(t, &if name.is_empty() { format!("{metres} m") } else { format!("{name} ({metres} m)") });
            }
        }
    }
}

/// The awareness marker manager (`FUN_00BAD960`) and the markers' update: one marker per NPC
/// with a displayable attention level toward the player.
#[allow(clippy::too_many_arguments)]
fn update_awareness(
    st: &mut Stage,
    n: &mut Native,
    dt: f32,
    enabled: bool,
    view: &SessionView,
    me: Option<&dishonored_sim::state::PlayerState>,
    cam: Option<&GlobalTransform>,
    window: Option<Vec2>,
    hfov_deg: f32,
    heads: Option<&crate::effects::EffectSockets>,
    world: &dishonored_content::MatchContent,
    sounds: &mut crate::hud_sounds::HudSounds,
) {
    use dishonored_world::WorldQuery;
    let Some(layer) = st.child(st.root, "markers_mc") else { return };
    // Candidates: the marks toward the local player whose pawn is alive.
    let mut cands: Vec<(u32, u8)> = Vec::new();
    let mut stinger_cond = false;
    if let (Some(p), true) = (me, enabled) {
        if p.alive {
            for m in view.matchv.marks.iter().filter(|m| m.who == p.id) {
                let allowed = awareness::ALLOWED.contains(&m.awareness);
                // `FUN_00BA0DE0`: a non-zero level and an allowed awareness.
                if m.level == 0 || !allowed {
                    continue;
                }
                if m.level >= 2 {
                    stinger_cond = true;
                }
                let Some(pawn) = view.others.iter().find(|o| o.id == m.id && o.alive) else { continue };
                // `FUN_00BA6E50`: the NPC's idea of where the player is must be near the player,
                // and the NPC within the display distance.
                if glam::Vec3::from(m.proxy).distance(p.position) >= awareness::PLAYER_PROXY_MAX_DIST || pawn.position.distance(p.position) >= awareness::NPC_MAX_DIST {
                    continue;
                }
                cands.push((m.id, m.level));
            }
        }
    }
    let ids: Vec<u32> = n.markers.keys().copied().chain(cands.iter().map(|c| c.0)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    let tan_h = (hfov_deg.to_radians() * 0.5).tan();
    for id in ids {
        let cand = cands.iter().find(|c| c.0 == id).copied();
        let level = cand.map(|c| c.1).unwrap_or(0);
        let pawn = view.others.iter().find(|o| o.id == id);
        let m = n.markers.entry(id).or_insert_with(AwarenessMarker::new);
        if pawn.is_none_or(|p| !p.alive) && m.mc.is_none() {
            n.markers.remove(&id);
            continue;
        }
        let head = heads.and_then(|h| h.heads.get(&id).copied()).map(|h| glam::Vec3::new(h.x, h.y, h.z)).or_else(|| pawn.map(|p| p.position + glam::Vec3::Z * 60.0));
        let mut seen = false;
        let mut place = None;
        if let (Some(head), Some(cam), Some(win), Some(p)) = (head, cam, window, me) {
            let (pos, off, dist) = marker_projection(cam, win, tan_h, head);
            // "Rendered within the last 0.2 s": on screen with a clear line to the head.
            if !off {
                let eye = p.position + glam::Vec3::Z * 64.0;
                let d = head - eye;
                let len = d.length();
                seen = len < 1.0 || world.collision.raycast(eye, d / len, (len - 20.0).max(0.0), dishonored_world::channel::VISION).is_none();
            }
            place = Some(MarkerPlace { pos: pos.to_array(), offscreen: off, distance: dist, tan_half_fov: tan_h });
        }
        let name = format!("awareness{id}");
        if m.update(st, layer, &name, dt, &MarkerInput { seen, level, place }) || (cand.is_none() && m.mc.is_none()) {
            m.remove(st);
            n.markers.remove(&id);
        }
    }
    // The detection stinger (0x00BAD960, flag 0x20000): once a marker is up with an NPC at
    // `m_AttentionLevelToPlayAwarenessStinger` (TurnToFace) or more; re-armed once none is.
    let any = !cands.is_empty();
    if !n.stinger {
        if any && stinger_cond {
            n.stinger = true;
            sounds.play("MusicPlayerDetected");
        }
    } else if !any && !stinger_cond {
        n.stinger = false;
    }
}

// --- the per-frame native update ------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn update_gfx_hud(
    mut hud: ResMut<GfxHud>,
    view: Res<SessionView>,
    session: Res<Session>,
    time: Res<Time>,
    windows: Query<&Window>,
    cams: Query<(&Camera, &GlobalTransform), With<crate::players::GameCamera>>,
    settings: Option<Res<crate::settings::GameSettings>>,
    bindings: Option<Res<crate::settings::Bindings>>,
    local: Res<crate::input_map::LocalInput>,
    pause: Option<Res<crate::pause_menu::PauseMenu>>,
    loading: Option<Res<crate::loading_screen::LoadingScreen>>,
    mut textures: ResMut<StageTextures>,
    mut images: ResMut<Assets<Image>>,
    mut sounds: ResMut<crate::hud_sounds::HudSounds>,
    sockets: Option<Res<crate::effects::EffectSockets>>,
) {
    let hud = &mut *hud;
    let Some(m) = hud.hud.as_mut() else { return };
    let dt = time.delta_secs();
    // The game is paused by the pause menu and while a level loads (the popup timer stops).
    let paused = pause.as_ref().is_some_and(|p| p.open || p.overlay != 0) || loading.as_ref().is_some_and(|l| l.active);
    let hs = crate::hud_settings::options(settings.as_deref());
    let cfg = &session.content.sim.gameplay;
    let me = view.me.as_ref();
    let alive = me.is_some_and(|p| p.alive);
    let mission = view.matchv.mission.as_ref();
    let cutscene = mission.is_some_and(|mv| !mv.cutscenes.is_empty());
    let content = hud.content.clone();
    let mut img = |path: &str| -> Option<ImageRef> {
        let c = content.as_ref()?;
        let (_, size) = textures.external(c, path, &mut images)?;
        Some(ImageRef { path: path.trim_start_matches("img://").to_string(), size })
    };
    let st = &mut m.stage;
    let w = &mut m.w;
    let n = &mut hud.native;

    // A new match / mission: reset what was sent and the popup queue (game start delay).
    let game = Some((view.matchv.mode.clone(), view.matchv.match_number));
    if n.last_mission != game {
        n.last_mission = game;
        n.inv = None;
        n.game_msg_seen = mission.and_then(|mv| mv.messages.last().map(|x| x.0)).unwrap_or(0);
        n.location_seen = mission.and_then(|mv| mv.location.as_ref().map(|x| x.0)).unwrap_or(0);
        n.tutorial_seen = mission.and_then(|mv| mv.tutorial.as_ref().map(|x| x.0)).unwrap_or(0);
        n.tw_seen = mission.and_then(|mv| mv.tutorial_window.as_ref().map(|x| x.0)).unwrap_or(0);
        n.tw_queue.clear();
        n.tw_timer = 0.0;
        n.tutorial_left = 0.0;
        n.hints = 0;
        hud.popups = PopupQueue::default();
    }

    // --- player status (gauges) ---
    let visibility = hs.visibility;
    let mut changed = false;
    if let (Some(p), Some(g)) = (me, w.gauges.as_mut()) {
        let max = dishonored_sim::powers::health_max(p, cfg).max(1.0) as i32;
        let health = p.vitals.health.max(0.0) as i32;
        // Bit 0: the attribute carries a "Whale Bone Charm Attribute_HealthMax" modifier; bit 1:
        // a "Vitality 1 (0)" / "Vitality 2 (0)" one (0x00B9F6E0, `FUN_00C7E1C0`; U-887).
        let large = p.inv.power_levels[dishonored_sim::state::power::VITALITY] > 0;
        let small = dishonored_sim::powers::charm_modifies(p, cfg, "Attribute_HealthMax");
        let flags = small as u8 | (large as u8) << 1;
        let hv = (health, 0, max, flags);
        if n.health != Some(hv) {
            let valid = n.health.is_some();
            n.health = Some(hv);
            g.fill_health(st, health as f32 * 100.0 / max as f32, small, large, health as f32 <= LOW_HEALTH_THRESHOLD, health as f32 * 100.0 / max as f32);
            changed |= valid;
        }
        let mmax = dishonored_sim::powers::mana_max(p, cfg).max(1.0) as i32;
        let mana = p.vitals.mana.max(0.0) as i32;
        let regen = (p.vitals.mana_regen_cap - p.vitals.mana).max(0.0) as i32;
        let has_powers = p.inv.power_levels.iter().take(6).any(|l| *l > 0);
        let available = p.possess.host.is_none();
        let upg = dishonored_sim::powers::charm_modifies(p, cfg, "Attribute_ManaMax");
        let mflags = has_powers as u8 | (available as u8) << 1 | (upg as u8) << 2;
        let mv = (mana, regen, mmax, mflags);
        if n.mana != Some(mv) {
            let valid = n.mana.is_some();
            n.mana = Some(mv);
            g.fill_mana(st, mana as f32 * 100.0 / mmax as f32, upg, available, !has_powers, mana as f32 <= LOW_MANA_THRESHOLD, (mana + regen) as f32 * 100.0 / mmax as f32);
            changed |= valid;
        }
    }
    // Elixirs: the refill animation (`RefillHealthGauge` / `RefillManaGauge`, 0x00B8CDA0).
    for e in view.audio_events.iter() {
        if let dishonored_sim::events::SimEvent::ElixirUsed { id, mana } = e {
            if Some(*id) == me.map(|p| p.id) {
                if let Some(g) = w.gauges.as_mut() {
                    if *mana {
                        g.refill_mana(st);
                    } else {
                        g.refill_health(st);
                    }
                }
                sounds.play("H_quickPotion");
            }
        }
    }
    // Visibility: Always / Contextual (8 s after a change; kept while low) / Off.
    if changed && visibility == 1 {
        n.info_timer = PLAYER_INFO_DISPLAY_DURATION;
    }
    n.info_timer = (n.info_timer - dt).max(0.0);
    let low = n.health.is_some_and(|h| h.0 as f32 <= LOW_HEALTH_THRESHOLD) || n.mana.is_some_and(|m| m.0 as f32 <= LOW_MANA_THRESHOLD && m.3 & 1 != 0);
    let want = alive
        && !cutscene
        && match visibility {
            0 => false,
            1 => n.info_timer > 0.0 || low,
            _ => true,
        };
    if want != n.status_visible {
        let open = n.ticked_once && want;
        if let Some(g) = w.gauges.as_mut() {
            if open {
                g.open(st);
            } else {
                g.close(st);
            }
        }
        n.status_visible = open;
    }
    n.ticked_once = true;

    // --- equipment ---
    if let (Some(p), Some(g)) = (me, w.gauges.as_mut()) {
        let item = wheel_item(p.left.item);
        let power = item >= 21;
        let active = power && p.left.phase == LeftPhase::Aiming;
        let cost_ok = p.vitals.mana > 0.0;
        let (total, loaded) = ammo_of(p, item).unwrap_or((-1, -1));
        let eq = (item, active, cost_ok, total, loaded);
        if n.equipment != Some(eq) {
            n.equipment = Some(eq);
            let icon = hud.texts.equipment_icons.get(&item).cloned();
            let icon_ref = icon.as_deref().and_then(&mut img);
            if item == 0 {
                g.set_empty_equipment(st);
            } else if power {
                g.set_power(st, icon_ref, active, cost_ok);
            } else {
                g.set_weapon(st, icon_ref, (total >= 0).then_some(total), (item == 8).then_some(loaded));
            }
        }
    }

    // --- crosshair ---
    let style = hs.crosshair_style;
    let opacity = hs.crosshair_opacity;
    let movement = hs.crosshair_movement;
    let window = windows.iter().next().map(|w| Vec2::new(w.width(), w.height()));
    let focus = mission.zip(me).and_then(|(mv, p)| crate::hud_popups::focused_interaction(mv, &view.matchv, p, &hud.texts.interactions));
    let (item_cross, item_dot, dispersion) = me.map(|p| crosshair_settings(wheel_item(p.left.item))).unwrap_or((None, true, false));
    let (aspect, show_dot) = if style == 2 { (item_cross, item_dot) } else { (None, true) };
    let interacting = focus.is_some();
    w.crosshairs.set_aspect(st, aspect);
    w.crosshairs.set_dot_state(st, if interacting { 2 } else { 0 });
    // Empty: the item has no ammo.
    let empty = me.and_then(|p| ammo_of(p, wheel_item(p.left.item))).is_some_and(|(t, _)| t <= 0);
    w.crosshairs.set_aspect_state(st, empty as u8);
    if dispersion {
        if let Some((d, max)) = me.and_then(|p| dishonored_sim::left_hand::Dispersion::of(p, cfg, &session.content.sim.tuning).map(|d| (p.left.dispersion, d.max))) {
            let pct = if max > 0.0 { d / max * 100.0 } else { 0.0 };
            w.crosshairs.set_dispersion(st, d, pct);
        }
    }
    // Position: centre, or towards the focused interactable (interpolated).
    let centre = Vec2::new(640.0, 360.0);
    let target = match (movement, focus.as_ref().and_then(|f| f.point), window) {
        (true, Some(pt), Some(win)) => cams.iter().next().and_then(|(cam, gt)| {
            let v = cam.world_to_viewport(gt, crate::coords::pos(Vec3::from(pt))).ok()?;
            let (s, off) = crate::gfx_stage::stage_xform(win);
            Some((v - off) / s)
        }),
        _ => None,
    }
    .unwrap_or(centre);
    let cur = n.crosshair_pos.unwrap_or(centre);
    let k = (CROSSHAIR_INTERPOLATION_SPEED * dt).clamp(0.0, 1.0);
    let pos = cur + (target - cur) * k;
    n.crosshair_pos = Some(pos);
    let shown = alive && !cutscene && style > 0;
    w.crosshairs.place(st, [pos.x, pos.y], shown && show_dot, shown && aspect.is_some(), opacity);

    // --- task markers (`m_TaskMarkers`, `m_TaskMarkerSettings`: objectiveMarker_primary / _secondary
    // in markers_mc, one per visible active task with a target) ---
    {
        let wanted: Vec<(glam::Vec3, bool, String)> = if !n.test_markers.is_empty() && alive {
            n.test_markers.clone()
        } else {
            mission
            .filter(|mv| mv.outcome.is_none() && hs.objective_markers && alive && !cutscene)
            .map(|mv| {
                if mv.task_markers.is_empty() {
                    mv.target.map(|t| vec![(glam::Vec3::from(t), false, String::new())]).unwrap_or_default()
                } else {
                    mv.task_markers.iter().map(|m| (glam::Vec3::from(m.pos), m.optional, m.name.clone())).collect()
                }
            })
            .unwrap_or_default()
        };
        let hfov = settings.as_ref().map(|s| s.s.graphics.fov).unwrap_or(crate::viewmodel::HORIZONTAL_FOV_DEG);
        let tan_h = (hfov.to_radians() * 0.5).tan();
        let layer = st.child(st.root, "markers_mc");
        let cam = cams.iter().next().map(|(_, gt)| gt);
        update_task_markers(st, n, layer, &wanted, cam, window, tan_h);
    }
    // --- awareness markers (m_AwarenessMarkers, 0x00BAD960) ---
    {
        let hfov = settings.as_ref().map(|s| s.s.graphics.fov).unwrap_or(crate::viewmodel::HORIZONTAL_FOV_DEG);
        let cam = cams.iter().next().map(|(_, gt)| gt);
        update_awareness(st, n, dt, hs.awareness_markers && !cutscene, &view, me, cam, window, hfov, sockets.as_deref(), &session.content, &mut sounds);
    }

    // --- stance icon ---
    if let Some(icon) = w.state_icon.as_mut() {
        let show = hs.player_stance && alive && !cutscene;
        let name = me.filter(|_| show).and_then(|p| {
            if p.slide.active {
                Some("slide")
            } else if p.crouched {
                Some("sneak")
            } else {
                None
            }
        });
        icon.set_icon(st, name);
    }

    // --- interactions window and crosshair info ---
    let show_interactions = hs.interactions;
    let lines = focus.as_ref().filter(|_| show_interactions && alive && !cutscene).map(|f| convert_key_tags(&f.text, bindings.as_deref(), &hud.texts.keys));
    if let Some(iw) = w.interaction_window.as_mut() {
        iw.set_interactions(st, focus.as_ref().map(|f| f.name.as_str()).unwrap_or(""), lines.as_deref());
    }
    // The crosshair's info line over someone asleep (`m_CrosshairFocusText`).
    let sleeper = me.filter(|_| alive && !cutscene).and_then(|p| crate::hud_popups::focused_sleeper(&view, p, local.yaw, local.pitch)).map(|_| crate::hud_popups::ASLEEP_CROSSHAIR_TEXT);
    if let Some(ci) = w.crosshair_info.as_mut() {
        ci.set_text(st, sleeper);
    }

    // --- game messages (DisSeqAct_ShowHUDMessage) ---
    if let (Some(mv), Some(gm)) = (mission, w.game_msg.as_mut()) {
        if let Some((id, text)) = mv.messages.last() {
            if *id > n.game_msg_seen {
                n.game_msg_seen = *id;
                gm.set_game_message(st, Some(&convert_key_tags(text, bindings.as_deref(), &hud.texts.keys)), GAME_MESSAGE_DURATION);
            }
        }
    }

    // --- location discovery (DisSeqAct_ShowLocationDiscovery, 0x00B95160) ---
    if let (Some(mv), Some(loc)) = (mission, w.location.as_mut()) {
        if let Some((id, name)) = &mv.location {
            if *id > n.location_seen {
                n.location_seen = *id;
                if name.trim().is_empty() {
                    loc.close(st);
                } else {
                    loc.set_location(st, name, LOCATION_DISCOVERY_DURATION);
                }
            }
        }
    }

    // --- tutorial line (DisSeqAct_SetTutorialMessage, 0x00BAE4A0: `m_fTutorialDuration` 10 s
    // unless the op gives one) and window (0x00BAD2F0 -> `SetTutorialWindow`, kept up
    // `m_fTutorialWindowDuration` 10 s, one after the other) ---
    {
        let tutorials_on = hs.tutorial_notifications;
        if let Some(mv) = mission {
            if let Some((id, text, secs)) = &mv.tutorial {
                if *id > n.tutorial_seen {
                    n.tutorial_seen = *id;
                    if tutorials_on && let Some(tm) = w.tutorial.as_mut() {
                        tm.set_message(st, Some(&convert_key_tags(text, bindings.as_deref(), &hud.texts.keys)));
                        n.tutorial_left = if *secs > 0.0 { *secs } else { TUTORIAL_DURATION };
                    }
                }
            }
            if let Some((id, picture, text)) = &mv.tutorial_window {
                if *id > n.tw_seen {
                    n.tw_seen = *id;
                    let text = if text.is_empty() { hud.texts.tutorial_windows.get(*picture as usize).cloned().unwrap_or_default() } else { text.clone() };
                    n.tw_queue.push((*picture, text));
                }
            }
        }
        if n.tutorial_left > 0.0 && !paused {
            n.tutorial_left -= dt;
            if n.tutorial_left <= 0.0 {
                if let Some(tm) = w.tutorial.as_mut() {
                    tm.set_message(st, None);
                }
            }
        }
        if !tutorials_on {
            n.tw_queue.clear();
        }
        if let Some(tw) = w.tutorial_window.as_mut() {
            if tw.opened {
                if !paused {
                    n.tw_timer += dt;
                }
                if n.tw_timer >= TutorialWindow::DURATION {
                    tw.close(st);
                    // (the next one waits for the slide out)
                    n.tw_timer = -0.5;
                }
            } else if n.tw_timer < 0.0 {
                n.tw_timer = (n.tw_timer + dt).min(0.0);
                if n.tw_timer == 0.0 {
                    n.tw_timer = 0.0;
                }
            } else if !n.tw_queue.is_empty() && !paused && alive {
                let (picture, text) = n.tw_queue.remove(0);
                tw.set_tutorial_window(st, &mut w.sounds, &convert_key_tags(&text, bindings.as_deref(), &hud.texts.keys), picture);
                n.tw_timer = 0.0;
            }
        }
    }

    // --- cinematic stripes ---
    if let Some(s) = w.stripes.as_mut() {
        s.set(st, cutscene, true);
    }

    // --- air gauge: the pawn's breath tick (0x00AD7BE0) under water opens it once (0x00B8CE70)
    // and fills it with the air left over the capacity (0x00B94EB0, `FillOxygenGauge(f x 100)`);
    // surfacing closes it (0x00B8CF30) while the air refills ---
    if let (Some(o), Some(p)) = (w.oxygen.as_mut(), me) {
        let under = p.alive && p.water.under;
        if under {
            if !n.oxygen_open {
                n.oxygen_open = true;
                o.open(st);
            }
            let breath = dishonored_sim::powers::attribute(p, cfg, "Attribute_BreathCapacityIncrease", 0.0);
            let max = dishonored_sim::swim::air_capacity(&cfg.movement.water, breath);
            let left = dishonored_sim::swim::air_left(p, &cfg.movement, breath);
            let f = if max > 0.0 { left / max } else { 0.0 };
            if (f * 100.0 - o.value).abs() > 1e-3 {
                o.fill(st, f * 100.0);
            }
        } else if n.oxygen_open {
            n.oxygen_open = false;
            o.close(st);
        }
    }

    // --- interaction gauge, kind 2 (the choke): opened by the choke's enter (0x00A7AA00 ->
    // 0x00BA5B90(2): `Open("HOLD " + m_ChokeGaugeGBA, false)`, red), set every In / Loop tick
    // to `(1 - timer / required) x 100` (0x00A71100 -> 0x00B94D40), closed by the win, the
    // loss (0x00B94C10) and the state's exit ---
    if let (Some(g), Some(p)) = (w.interaction_gauge.as_mut(), me) {
        use dishonored_sim::state::ChokePhase;
        let choking = p.alive && matches!(p.choke.phase, ChokePhase::In | ChokePhase::Loop);
        if choking {
            if n.gauge_kind != 2 {
                if n.gauge_kind != 0 {
                    g.close(st);
                }
                n.gauge_kind = 2;
                let tag = convert_key_tags(&hud.texts.gauge_tags[1], bindings.as_deref(), &hud.texts.keys);
                g.open(st, &format!("{} {tag}", hud.texts.hold), false);
            }
            let f = if p.choke.required > 0.0 { 1.0 - p.choke.timer / p.choke.required } else { 0.0 };
            let v = (f.clamp(0.0, 1.0) * 100.0).floor() as i32;
            if v != g.value {
                g.set_gauge(st, v);
            }
        } else if n.gauge_kind == 2 {
            n.gauge_kind = 0;
            g.close(st);
        }
    }

    // --- pickup log (inventory differences) ---
    if let Some(p) = me {
        let show_log = hs.pickup_log;
        if let Some(prev) = n.inv.clone() {
            // The first rune / bone charm gets its tutorial window (`m_PressKeyToBuyPowers` /
            // `m_PressKeyToEquipBoneCharms`, `m_NumRunesPickedUp` / `m_bHasNewBoneCharm`).
            if p.inv.runes > prev.runes && n.hints & 1 == 0 {
                n.hints |= 1;
                n.tw_queue.push((TW_IMG_RUNE, hud.texts.tutorial_windows[TW_IMG_RUNE as usize].clone()));
            }
            if p.inv.charms_owned & !prev.charms_owned != 0 && n.hints & 2 == 0 {
                n.hints |= 2;
                n.tw_queue.push((TW_IMG_CHARM, hud.texts.tutorial_windows[TW_IMG_CHARM as usize].clone()));
            }
            let items = crate::hud_popups::pickup_items(&prev, &p.inv, &hud.texts);
            if show_log {
                if let Some(log) = w.pickup_log.as_mut() {
                    for (name, icon) in items {
                        let r = icon.as_deref().and_then(&mut img);
                        log.add_log_item(st, &name, r);
                    }
                }
            }
        }
        n.inv = Some(p.inv.clone());
    }

    // --- popups (objectives, target notifications) ---
    let popups_on = hs.objective_popups;
    if let Some(mv) = mission {
        hud.popups.observe(mv);
    }
    for t in std::mem::take(&mut hud.tests) {
        match t {
            crate::hud_popups::HudTest::Tutorial { text, seconds } => {
                if let Some(tm) = w.tutorial.as_mut() {
                    tm.set_message(st, Some(&convert_key_tags(&text, bindings.as_deref(), &hud.texts.keys)));
                    n.tutorial_left = seconds.unwrap_or(TUTORIAL_DURATION);
                }
            }
            crate::hud_popups::HudTest::TutorialWindow { image, text } => {
                let text = text.unwrap_or_else(|| hud.texts.tutorial_windows.get(image as usize).cloned().unwrap_or_default());
                n.tw_queue.push((image, text));
            }
            crate::hud_popups::HudTest::TaskMarkers { markers } => {
                // Metres ahead of / beside the player along the view (yaw).
                n.test_markers = match me {
                    Some(p) => {
                        let f = dishonored_sim::movement::view_vector(local.yaw, 0.0);
                        let (fx, fy) = (f.x, f.y);
                        markers.into_iter().map(|(ahead, side, up, optional, name)| (p.position + glam::Vec3::new(fx * ahead * 100.0 + fy * side * 100.0, fy * ahead * 100.0 - fx * side * 100.0, up * 100.0), optional, name)).collect()
                    }
                    None => Vec::new(),
                };
            }
            t => crate::hud_popups::apply_test(t, &mut hud.popups, w, st, &hud.texts, &mut img),
        }
    }
    let obj_open = w.objectives.as_ref().is_some_and(|o| o.opened);
    let tgt_open = w.target.as_ref().is_some_and(|o| o.opened);
    if let Some(p) = hud.popups.tick(dt, paused, popups_on, obj_open, tgt_open) {
        match p {
            PopupContent::Objective { name, event, tasks } => {
                let title = hud.texts.titles.get(event as usize).cloned().unwrap_or_default();
                if let Some(o) = w.objectives.as_mut() {
                    let tasks: Vec<TaskLine> = tasks.into_iter().map(|(d, e)| TaskLine { state: ObjState::from_index(e), description: d }).collect();
                    o.set_objectives(st, &mut w.sounds, &name, &title, ObjState::from_index(event), if event == 3 { Vec::new() } else { tasks });
                }
            }
            PopupContent::Target { name, portrait, kind } => {
                // A mission names its `DisTweaks_ChapterTarget`: its `m_TargetName` and
                // `m_TargetPortraitPath` (0x00B990D0: "img://" + the portrait).
                if hud.texts.chapter_targets.is_none() {
                    hud.texts.chapter_targets = Some(content.as_deref().map(chapter_targets).unwrap_or_default());
                }
                let found = hud.texts.chapter_targets.as_ref().and_then(|m| m.get(&name.to_ascii_lowercase()).cloned());
                let (name, portrait) = match found {
                    Some((n, p)) => (n, p.or(portrait)),
                    None => (name, portrait),
                };
                let portrait = portrait.as_deref().and_then(&mut img);
                if let (Some(t), Some(tx)) = (w.target.as_mut(), hud.texts.target.clone()) {
                    t.set_notification(st, &mut w.sounds, &tx, &name, portrait, kind, TARGET_NOTIFICATION_DURATION);
                }
            }
        }
    }

    // --- run the movie ---
    let until = st.time + dt as f64;
    while let Some(ev) = st.poll(until) {
        match ev {
            Event::Timer(k) | Event::TweenDone(k) => w.on_event(st, k),
        }
    }
    for s in w.sounds.0.drain(..) {
        sounds.play(s);
    }

    // --- HUD effects: stealth shroud (0x00B96050), damage directions (0x00B970F0) ---
    if let Some(fx) = hud.fx.as_mut() {
        let (fst, fw) = (&mut fx.stage, &mut fx.w);
        let sneaking = me.is_some_and(|p| p.alive && p.crouched) && !cutscene;
        if let Some(s) = fw.shroud.as_mut() {
            s.set(fst, sneaking);
        }
        // At a keyhole: the frame and the black fade of the peek (interactables.md).
        fw.show_keyhole(fst, crate::tripwires::peek_view(&view).map(|v| v.fade));
        if let Some(p) = me {
            // The frame's damage, summed (`m_LastDamageInfo`); the direction is towards the
            // damage causer, ahead (0) without one.
            let mut total = 0.0;
            let mut from: Option<glam::Vec3> = None;
            for e in view.audio_events.iter() {
                if let dishonored_sim::events::SimEvent::Damage { victim, attacker, amount, .. } = e {
                    if *victim == p.id && *amount > 0.0 {
                        total += *amount;
                        if let Some(a) = (*attacker).and_then(|a| view.others.iter().find(|o| o.id == a)) {
                            from = Some(a.position);
                        }
                    }
                }
            }
            if total > 0.0 && p.alive {
                let angle = match from {
                    Some(at) => {
                        let d = at - p.position;
                        let f = dishonored_sim::movement::view_vector(local.yaw, 0.0);
                        let ahead = d.x * f.x + d.y * f.y;
                        let right = d.y * f.x - d.x * f.y;
                        right.atan2(ahead).to_degrees()
                    }
                    None => 0.0,
                };
                let intensity = (total / MAX_DAMAGE).clamp(0.0, 1.0) * 100.0;
                if let Some(dd) = fw.damages.as_mut() {
                    dd.take_damage(fst, angle, intensity, crate::gfx_stage::STAGE.x, crate::gfx_stage::STAGE.y);
                }
            }
        }
        let until = fst.time + dt as f64;
        while let Some(ev) = fst.poll(until) {
            match ev {
                Event::Timer(k) | Event::TweenDone(k) => fw.on_event(fst, k),
            }
        }
    }
}

/// Draw the HUD stage into its UI entities.
#[allow(clippy::type_complexity)]
pub fn draw_gfx_hud(
    mut commands: Commands,
    mut hud: ResMut<GfxHud>,
    textures: Res<StageTextures>,
    fonts: Option<Res<GameFonts>>,
    windows: Query<&Window>,
    mut views: Query<&mut StageView>,
    mut images: Query<(&mut ImageNode, &mut Node, &mut UiTransform, &mut Visibility), (With<SlotImage>, Without<SlotText>)>,
    mut texts: Query<(&mut Text, &mut TextFont, &mut TextColor, &mut TextLayout, &mut LineHeight, &mut LetterSpacing, &mut Node, &mut UiTransform, &mut Visibility), (With<SlotText>, Without<SlotImage>)>,
) {
    let hud = &mut *hud;
    let Some(window) = windows.iter().next() else { return };
    let size = Vec2::new(window.width(), window.height());
    if let Some(fx) = hud.fx.as_mut() {
        if let Ok(mut view) = views.get_mut(fx.view) {
            fx.stage.draw(&mut fx.draws);
            crate::gfx_stage::sync_view(&mut commands, fx.view, &mut view, &fx.draws, &textures, fonts.as_deref(), size, &mut images, &mut texts);
        }
    }
    let (Some(m), Some(root)) = (hud.hud.as_mut(), hud.view) else { return };
    let Ok(mut view) = views.get_mut(root) else { return };
    m.stage.draw(&mut m.draws);
    crate::gfx_stage::sync_view(&mut commands, root, &mut view, &m.draws, &textures, fonts.as_deref(), size, &mut images, &mut texts);
}

// Writing a UI component marks it changed even when the value is the same, and any changed
// node, image or text re-runs the UI layout (and re-shapes the text) that frame. The HUD is
// recomputed every frame, so these write only real changes.

/// Move / resize an absolutely placed node when its box changed.
pub fn place(node: &mut Mut<Node>, left: Val, top: Val, width: Option<Val>, height: Option<Val>) {
    let n: &Node = &**node;
    if !crate::profile::perf_baseline() && n.left == left && n.top == top && width.is_none_or(|w| n.width == w) && height.is_none_or(|h| n.height == h) {
        return;
    }
    node.left = left;
    node.top = top;
    if let Some(w) = width {
        node.width = w;
    }
    if let Some(h) = height {
        node.height = h;
    }
}

/// Set a text's string when it changed.
pub fn set_text(text: &mut Mut<Text>, s: String) {
    if text.0 != s || crate::profile::perf_baseline() {
        text.0 = s;
    }
}

/// Set a font size (pixels) when it changed.
pub fn set_font_size(font: &mut Mut<TextFont>, px: f32) {
    let size = FontSize::Px(px);
    if font.font_size != size || crate::profile::perf_baseline() {
        font.font_size = size;
    }
}

/// Dishonored's UI typefaces, converted at startup from `DisFonts_SF` (`fonts_efigs`):
/// "Emerge BF" (body) and "ChaletComprime-CologneEighty" (titles).
#[derive(Resource, Clone)]
pub struct GameFonts {
    pub normal: Handle<Font>,
    pub title: Handle<Font>,
}

pub fn load_game_fonts(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    let Some(movie) = content.package("DisFonts_SF").ok().and_then(|p| crate::gfx_stage::movie_raw(&p, "fonts_efigs")).and_then(|r| Movie::parse(&r).ok()) else { return };
    let mut by_name = HashMap::new();
    for f in &movie.fonts {
        by_name.insert(f.name.clone(), fonts.add(Font::from_bytes(f.to_ttf(&f.name))));
    }
    let normal = by_name.iter().find(|(n, _)| n.starts_with("Emerge")).map(|(_, h)| h.clone());
    let title = by_name.iter().find(|(n, _)| n.starts_with("Chalet")).map(|(_, h)| h.clone());
    if let (Some(normal), Some(title)) = (normal, title) {
        info!("game fonts: {:?}", by_name.keys().collect::<Vec<_>>());
        commands.insert_resource(GameFonts { normal, title });
    }
}

/// Give the debug HUD text the game's fonts once they exist (body font; scoreboard uses titles).
pub fn apply_game_fonts(
    fonts: Option<Res<GameFonts>>,
    mut done: Local<bool>,
    mut body: Query<&mut TextFont, (With<crate::hud::HudText>, Without<crate::hud::Scoreboard>)>,
    mut titles: Query<&mut TextFont, (With<crate::hud::Scoreboard>, Without<crate::hud::HudText>)>,
) {
    let Some(fonts) = fonts else { return };
    if *done || (body.is_empty() && titles.is_empty()) {
        return;
    }
    for mut f in &mut body {
        f.font = fonts.normal.clone().into();
    }
    for mut f in &mut titles {
        f.font = fonts.title.clone().into();
    }
    *done = true;
}
