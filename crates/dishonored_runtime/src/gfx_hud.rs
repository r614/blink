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
//! - subtitles and choices (`hud_dialogue.rs`).

use crate::gfx_stage::{Draw, Event, Fonts, ImageRef, MovieData, SlotImage, SlotText, Stage, StageTextures, StageView};
use crate::hud_popups::{PopupContent, PopupQueue};
use crate::hud_widgets::{HudFxWidgets, HudWidgets, ObjState, TargetTexts, TaskLine};
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
pub const CROSSHAIR_INTERPOLATION_SPEED: f32 = 4.5;

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
    /// `m_EquipmentIcons[EDisPowerWheelItem]` (DefaultUI.ini `[DishonoredGame.DisGlobalUIManager]`).
    pub equipment_icons: HashMap<u8, String>,
    /// Key display names (Settings.int `[Keys]`).
    pub keys: HashMap<String, String>,
    /// `DisTweaks_ChapterTarget`s (DishonoredGame.upk `Twk_ChapterInfoList.ChapterTargets.*`):
    /// object name (lower case) -> (`m_TargetName`, `m_TargetPortraitPath`), read on first use.
    pub chapter_targets: Option<HashMap<String, (String, Option<String>)>>,
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
    marker: Option<crate::gfx_stage::Id>,
    inv: Option<dishonored_sim::state::Inventory>,
    game_msg_seen: u32,
    last_mission: Option<(String, u32)>,
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
) {
    let hud = &mut *hud;
    let Some(m) = hud.hud.as_mut() else { return };
    let dt = time.delta_secs();
    // The game is paused by the pause menu and while a level loads (the popup timer stops).
    let paused = pause.as_ref().is_some_and(|p| p.open) || loading.as_ref().is_some_and(|l| l.active);
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
        hud.popups = PopupQueue::default();
    }

    // --- player status (gauges) ---
    let visibility = hs.visibility;
    let mut changed = false;
    if let (Some(p), Some(g)) = (me, w.gauges.as_mut()) {
        let max = dishonored_sim::powers::health_max(p, cfg).max(1.0) as i32;
        let health = p.vitals.health.max(0.0) as i32;
        // Bit 0: a bone charm raises the maximum; bit 1: Vitality (0x00B9F6E0).
        let large = p.inv.power_levels[dishonored_sim::state::power::VITALITY] > 0;
        let small = !large && max as f32 > cfg.vitals.health_max + 0.5;
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
        let upg = mmax as f32 > cfg.vitals.mana_max + 0.5;
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
    let focus = mission.zip(me).and_then(|(mv, p)| crate::hud_popups::focused_interaction(mv, &view, p, local.yaw, local.pitch));
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

    // --- task marker (`m_TaskMarkerSettings`: objectiveMarker_primary in markers_mc) ---
    let marker_at = mission.filter(|mv| mv.outcome.is_none()).and_then(|mv| mv.target).filter(|_| hs.objective_markers && alive && !cutscene);
    let placed = match (marker_at, window, me) {
        (Some(t), Some(win), Some(p)) => cams.iter().next().and_then(|(cam, gt)| {
            // m_fWorldOffsetZ 10.
            let world = glam::Vec3::from(t) + glam::Vec3::Z * 10.0;
            let v = cam.world_to_viewport(gt, crate::coords::pos(Vec3::new(world.x, world.y, world.z))).ok()?;
            let (s, off) = crate::gfx_stage::stage_xform(win);
            let sp = (v - off) / s;
            // Kept inside the marker area (m_fMarkerAreaRatio_PC 0.9 of the screen).
            let half = crate::gfx_stage::STAGE * 0.5 * 0.9;
            let c = crate::gfx_stage::STAGE * 0.5;
            let sp = c + (sp - c).clamp(-half, half);
            Some((sp, world.distance(p.position + glam::Vec3::Z * 64.0)))
        }),
        _ => None,
    };
    let markers_mc = st.child(st.root, "markers_mc");
    match (placed, markers_mc) {
        (Some((sp, dist)), Some(layer)) => {
            if n.marker.is_none_or(|m| !st.alive(m)) {
                let d = st.next_depth(layer);
                n.marker = st.attach(layer, "objectiveMarker_primary", "taskMarker0", d);
                if let Some(desc) = n.marker.and_then(|m| st.child(m, "_description_mc")) {
                    st.set_visible(desc, false);
                }
            }
            if let Some(mk) = n.marker {
                // m_ScaleVariation (10000 uu -> 90 %, 500 uu -> 150 %) and m_AlphaVariation
                // (1000 uu -> 100, 0 uu -> 0).
                let var = |d: f32, d0: f32, d1: f32, v0: f32, v1: f32| v0 + ((d - d0) / (d1 - d0)).clamp(0.0, 1.0) * (v1 - v0);
                let scale = var(dist, 10000.0, 500.0, 90.0, 150.0);
                let alpha = var(dist, 1000.0, 0.0, 100.0, 0.0);
                st.set(mk, crate::gfx_stage::Prop::X, sp.x);
                st.set(mk, crate::gfx_stage::Prop::Y, sp.y);
                st.set(mk, crate::gfx_stage::Prop::XScale, scale);
                st.set(mk, crate::gfx_stage::Prop::YScale, scale);
                st.set(mk, crate::gfx_stage::Prop::Alpha, alpha);
                st.set_visible(mk, true);
            }
        }
        _ => {
            if let Some(mk) = n.marker.take() {
                st.remove(mk);
            }
        }
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
        iw.set_interactions(st, "", lines.as_deref());
    }
    if let Some(ci) = w.crosshair_info.as_mut() {
        ci.set_text(st, None);
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

    // --- cinematic stripes ---
    if let Some(s) = w.stripes.as_mut() {
        s.set(st, cutscene, true);
    }

    // --- pickup log (inventory differences) ---
    if let Some(p) = me {
        let show_log = hs.pickup_log;
        if let Some(prev) = n.inv.clone() {
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
        crate::hud_popups::apply_test(t, &mut hud.popups, w, st, &hud.texts, &mut img);
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
