//! The quick-access wheel: Dishonored's `UI_PowerWheel_SF` "powerwheel" movie (with the
//! `Common_assets.lib` icons) on a [`gfx_stage::Stage`], its ActionScript classes rewritten
//! below (`_powerWheel.WheelHandler`, `PC_ShortcutsHandler` over `ShortcutsHandler`,
//! `PC_PotionsCount` over `PotionsCount`, the root timeline's functions), driven the way
//! `DisGFxMoviePlayerPowerWheel` drives it. See research/dishonored/HUD.md "Power wheel".
//!
//! Native side (decoded from `Dishonored.exe`):
//! - `GBA_Wheel` (PC: MiddleMouseButton) runs `Dis_Wheel_Open FALSE | OnRelease Dis_Wheel_Close
//!   FALSE` (DefaultInput.ini). Opening (start mode 1, 0x00BE4280 -> 0x00BE25C0) sends
//!   `SetPotionsCount(health, mana)` (0x00BC3FE0), the shortcut layout (`SetShortcuts`,
//!   0x00BE1310) and `OpenWheel(content)`: one `{itemIndex, itemQuantity}` per available item
//!   (0x00BD5530: weapons with their known ammo types, the Heart, learned powers).
//!   `m_fWheelInputDelay` (0.25 s) only applies to the gamepad.
//! - Tick (0x00BD5250): the mouse accumulates into `m_MousePosition`, clamped to a circle of
//!   `m_fMouseZoneSize` (0.1) x the viewport size (0x00BCD660); a frame whose motion exceeds
//!   `m_fMouseDeadZone` (0.01) of the zone sends the position (over the zone); beyond
//!   `m_fStickDeadZone` (0.4) its angle (`acos`, clockwise from up, 360 - a on the left) goes to
//!   `SetSelectionAngle`.
//! - `onSelectionChanged(item)` (0x00BCD800) stores `m_CurItemIdx` and answers
//!   `UpdateItemInfos(name, ammo)` (0x00BC3B00 weapons, 0x00BC3940 powers, 0x00BC3E30 the
//!   Heart); closing (0x00BBD1D0 `CloseWheel`) keeps that item selected (equipped).
//! - Number keys (`GBA_Shortcut_n`: `Dis_WheelShortcuts_KeyboardSelect n | OnRelease
//!   Dis_WheelShortcuts_KeyboardUse n`) show the dock alone (start mode 2: `OpenShortcuts()`).
//! - Sounds: `m_pShowSound` Snd_UI_Ingame_Wheel_Hand_In, `m_pHideSound` ..._Hand_Out, the
//!   theme's itemSelection / P_AssignShortcut / P_UnassignShortcut.
//!
//! - The shortcut table (`m_KeyboardShortcuts`) is filled by `SetShortcuts` (0x00BE1310 and the
//!   auto-assignment 0x00BDC610, [`ShortcutTable::refresh`]) whenever the wheel or the dock
//!   opens; a number key shows the dock and equips the slot's item on release (0x00AB7D30); the
//!   mouse wheel steps through the filled slots and equips the shown one `m_fShortcutsInputTimer`
//!   (0.33 s) after the last step ([`WheelCycle`]).
//! - The world slows to `m_fBendTimeFactor` (0.05) while the wheel is open: [`LocalInput::wheel_open`]
//!   sets `buttons::WHEEL` in the player's input and the simulation does the rest
//!   (`dishonored_sim::powers::set_wheel_layer`, single-human worlds only).
//!
//! Not reproduced: the movie's 3D (`_xrotation`, `_yrotation`, `_z`: the glass tilt following
//! the selection), the `_noise` white texture (BitmapData noise), the gamepad wheel and d-pad
//! shortcuts, the Heart (the sim has none).

use crate::gfx_stage::{AutoSize, Draw, Ease, Event, Fonts, Id, ImageRef, MovieData, Prop, SlotImage, SlotText, Stage, StageTextures, StageView};
use bevy::prelude::*;
use bevy::text::{LetterSpacing, LineHeight};
use dishonored_assets::swf::ColorTransform;
use dishonored_ir::items::AmmoType;
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::PlayerState;
use std::collections::HashMap;
use std::sync::Arc;

// --- tweak values (Twk_InGameUI.Twk_GFxMoviePlayerPowerWheel and its base class) -------------

/// `m_fMouseDeadZone`.
pub const MOUSE_DEAD_ZONE: f32 = 0.01;
/// `m_fMouseZoneSize` (fraction of the viewport).
pub const MOUSE_ZONE_SIZE: f32 = 0.1;
/// `m_fStickDeadZone` (DisTweaks_GFxMoviePlayerBase).
pub const STICK_DEAD_ZONE: f32 = 0.4;
/// `m_fBendTimeFactor` (DisGFxMoviePlayerPowerWheel): the world's time scale while open.
pub const BEND_TIME_FACTOR: f32 = 0.05;

// --- `_powerWheel.WheelItems` (EDisPowerWheelItem) -------------------------------------------

pub const REGULAR_BOLT: u8 = 1;
pub const SLEEP_DART: u8 = 2;
pub const FLARE_BOLT: u8 = 3;
pub const HEART: u8 = 4;
pub const SPRING_RAZOR: u8 = 5;
pub const REGULAR_GRENADE: u8 = 6;
pub const STICKY_GRENADE: u8 = 7;
pub const REGULAR_BULLET: u8 = 8;
pub const EXPLOSIVE_BULLET: u8 = 9;
/// `firstPowerIndex`: items above it are powers.
pub const FIRST_POWER_INDEX: u8 = 20;
pub const DARK_VISION: u8 = 21;
pub const WIND_BLAST: u8 = 22;
pub const DEVOURING_SWARM: u8 = 23;
pub const BLINK: u8 = 24;
pub const BEND_TIME: u8 = 25;
pub const POSSESSION: u8 = 26;

/// `WheelHandler.weaponsOrder` / `powersOrder`: the wheel's item order.
const WEAPONS_ORDER: [u8; 9] = [FLARE_BOLT, REGULAR_BOLT, SLEEP_DART, HEART, SPRING_RAZOR, REGULAR_GRENADE, STICKY_GRENADE, REGULAR_BULLET, EXPLOSIVE_BULLET];
const POWERS_ORDER: [u8; 6] = [WIND_BLAST, DARK_VISION, BLINK, DEVOURING_SWARM, BEND_TIME, POSSESSION];

/// `WheelItems.item_*.schemaId`: the `item_schema` frame.
fn schema_of(item: u8) -> &'static str {
    match item {
        REGULAR_BOLT | SLEEP_DART | FLARE_BOLT => "crossbow",
        HEART => "heart",
        SPRING_RAZOR => "spring_razor",
        REGULAR_GRENADE => "regular_grenade",
        STICKY_GRENADE => "sticky_grenade",
        REGULAR_BULLET | EXPLOSIVE_BULLET => "gun",
        _ => "outsider",
    }
}

/// The port's left hand for a wheel item (the Heart and explosive bullets have none).
pub fn hand_of(item: u8) -> Option<LeftHand> {
    Some(match item {
        REGULAR_BOLT => LeftHand::Crossbow,
        SLEEP_DART => LeftHand::SleepDart,
        FLARE_BOLT => LeftHand::IncendiaryBolt,
        SPRING_RAZOR => LeftHand::SpringRazor,
        REGULAR_GRENADE => LeftHand::Grenade,
        STICKY_GRENADE => LeftHand::StickyGrenade,
        REGULAR_BULLET => LeftHand::Pistol,
        DARK_VISION => LeftHand::DarkVision,
        WIND_BLAST => LeftHand::Windblast,
        DEVOURING_SWARM => LeftHand::DevouringSwarm,
        BLINK => LeftHand::Blink,
        BEND_TIME => LeftHand::BendTime,
        POSSESSION => LeftHand::Possession,
        _ => return None,
    })
}

/// One `{itemIndex, itemQuantity}` of the wheel content.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub index: u8,
    pub quantity: Option<i32>,
}

// --- callback keys ---------------------------------------------------------------------------

const C_WHEEL_CLOSED: u16 = 1;
const C_GLASS_HIDDEN: u16 = 2;
const C_WHITE_HIDDEN: u16 = 3;
const C_SEP: u16 = 4; // arg: 0 animateWheelSep2 next, 1 animateWheelSep next
const C_BKGD0: u16 = 5; // arg: 0 -> _b, 1 -> again
const C_BKGD1: u16 = 6;
const C_DOCK_SELSLOT: u16 = 7;
const C_DOCK_CLOSE_MC: u16 = 8;
const C_DOCK_CLOSED: u16 = 9;
const C_POTION_HIDDEN: u16 = 10; // arg: 0 health, 1 mana

fn key(code: u16, arg: u32) -> u64 {
    crate::hud_widgets::key(30, code, arg)
}

/// What the movie asks the native side (`ExternalInterface.call`).
#[derive(Clone, Debug, PartialEq)]
pub enum Callback {
    /// `onSelectionChanged(itemIdxEnum)` (0: cleared).
    SelectionChanged(u8),
    /// `OnClosed` (the wheel or the dock finished closing).
    Closed,
}

/// Movie-space info of `_utils.ScreenPosition.GetMovieSpaceCoord(ratio)` on the 1280x720 stage
/// (no empty space at 16:9): (coordMin, coordMax).
pub(crate) fn movie_space(ratio: f32) -> (Vec2, Vec2) {
    let off = crate::gfx_stage::STAGE * (1.0 - ratio) * 0.5;
    (off, crate::gfx_stage::STAGE - off)
}

/// `Math.random` for the movie's `randRange` (xorshift, deterministic per session).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
    /// Root `randRange(min, max)`: `floor(random * (max - min + 1)) + min`.
    fn range(&mut self, min: f32, max: f32) -> f32 {
        (self.next() * (max - min + 1.0)).floor() + min
    }
}

/// `_common.EquipmentIcon.SetIconImage`: a fresh `ic._imgLoader_mc` with the image centred.
fn set_icon_image(st: &mut Stage, item_mc: Id, img: Option<ImageRef>) {
    let Some(ic) = st.child(item_mc, "ic") else { return };
    clear_container(st, item_mc);
    let d = st.next_depth(ic);
    let holder = st.create_empty(ic, "_imgLoader_mc", d);
    st.img_loader(holder, img, false, true);
}
/// `_common.EquipmentIcon.ClearContainer`.
fn clear_container(st: &mut Stage, item_mc: Id) {
    if let Some(old) = st.child(item_mc, "ic").and_then(|ic| st.child(ic, "_imgLoader_mc")) {
        st.remove(old);
    }
}

fn set_txt(st: &mut Stage, mc: Option<Id>, path: &str, s: &str) {
    if let Some(t) = mc.and_then(|m| st.find(m, path)) {
        st.set_text(t, s);
    }
}

// =============================================================================================
// _powerWheel.WheelHandler (`wheel_mc`)
// =============================================================================================

pub struct WheelHandler {
    pub mc: Id,
    wheel_circle: Option<Id>,
    white_stroke: Option<Id>,
    black_stroke: Option<Id>,
    indic_stroke: Option<Id>,
    wheel_bkgd: Option<Id>,
    item_schema: Option<Id>,
    item_infos: Option<Id>,
    separators: Option<Id>,
    icons: Option<Id>,
    indicator: Option<Id>,
    pub weapons: Vec<(Item, f32)>,
    pub powers: Vec<(Item, f32)>,
    weapon_mcs: Vec<Id>,
    power_mcs: Vec<Id>,
    nb_items: usize,
    diameter: f32,
    pub opened: bool,
    stick_in_use: bool,
    last_sel_angle: f32,
    absolute_angle: f32,
    /// `_selItemIdx`, `_selItemPowerState`.
    sel: Option<(usize, bool)>,
    /// `_selItemIdxEnum`.
    pub sel_enum: Option<u8>,
}

/// `selItemProp` / `defItemProp` (the 2D part; `_z` -450 / -50 not reproduced).
const SEL_ITEM_SCALE: f32 = 105.0;
const DEF_ITEM_SCALE: f32 = 75.0;
/// `IsItemSelected` stickiness (degrees) in `SetWheelSelectionAngle`.
const STICKINESS: f32 = 6.0;

impl WheelHandler {
    fn new(st: &mut Stage, mc: Id) -> Self {
        let c = |st: &Stage, n: &str| st.child(mc, n);
        let mut w = Self {
            mc,
            wheel_circle: c(st, "wheel_circle"),
            white_stroke: c(st, "white_stroke"),
            black_stroke: c(st, "black_stroke"),
            indic_stroke: c(st, "indic_stroke"),
            wheel_bkgd: c(st, "wheel_bkgd"),
            item_schema: c(st, "item_schema"),
            item_infos: c(st, "item_infos"),
            separators: None,
            icons: None,
            indicator: None,
            weapons: Vec::new(),
            powers: Vec::new(),
            weapon_mcs: Vec::new(),
            power_mcs: Vec::new(),
            nb_items: 0,
            diameter: 0.0,
            opened: false,
            stick_in_use: false,
            last_sel_angle: -1.0,
            absolute_angle: 0.0,
            sel: None,
            sel_enum: None,
        };
        // Constructor: `wheel_diameter = wheel_circle._width`, everything transparent.
        w.diameter = w.wheel_circle.map(|i| st.width(i)).unwrap_or(0.0);
        if let Some(i) = w.item_infos {
            st.set(i, Prop::Alpha, 0.0);
        }
        st.set(mc, Prop::Alpha, 0.0);
        w
    }

    fn item_range(&self) -> f32 {
        let n = self.weapons.len() + self.powers.len();
        if n > 0 { 360.0 / n as f32 } else { 0.0 }
    }
    fn weapons_range(&self) -> f32 {
        let n = self.weapons.len() + self.powers.len();
        if !self.weapons.is_empty() { self.weapons.len() as f32 * 360.0 / n as f32 } else { 0.0 }
    }
    fn powers_range(&self) -> f32 {
        let n = self.weapons.len() + self.powers.len();
        if !self.powers.is_empty() { self.powers.len() as f32 * 360.0 / n as f32 } else { 0.0 }
    }
    /// `ComputeWeaponAngle`: the weapons centred on the top.
    fn weapon_angle(&self, idx: usize) -> f32 {
        let wr = self.weapons_range();
        let a = idx as f32 / self.weapons.len() as f32 * wr + 0.5 * self.item_range();
        (a - 0.5 * wr + 360.0) % 360.0
    }
    /// `ComputePowerAngle`: the powers after the weapons, in reverse order.
    fn power_angle(&self, idx: usize) -> f32 {
        let (wr, pr) = (self.weapons_range(), self.powers_range());
        let a = idx as f32 / self.powers.len() as f32 * pr + 0.5 * self.item_range();
        (0.5 * wr + (pr - a) + 360.0) % 360.0
    }

    fn item_mc(&self, idx: usize, power: bool) -> Option<Id> {
        if power { self.power_mcs.get(idx).copied() } else { self.weapon_mcs.get(idx).copied() }
    }
    fn item(&self, idx: usize, power: bool) -> Option<&Item> {
        if power { self.powers.get(idx).map(|x| &x.0) } else { self.weapons.get(idx).map(|x| &x.0) }
    }
    /// `isEmptyItem`: `itemQuantity == 0`.
    fn is_empty_item(&self, idx: usize, power: bool) -> bool {
        self.item(idx, power).is_some_and(|i| i.quantity == Some(0))
    }

    /// `SortItems(wheelContent, itemOrder)`.
    fn sort_items(content: &[Item], order: &[u8]) -> Vec<Item> {
        order.iter().filter_map(|o| content.iter().find(|c| c.index == *o).cloned()).collect()
    }

    /// `FillWheelIcons(wheelContent)`.
    fn fill_wheel_icons(&mut self, st: &mut Stage, content: &[Item], icon: &mut dyn FnMut(u8) -> Option<ImageRef>) {
        let weapons = Self::sort_items(content, &WEAPONS_ORDER);
        let powers = Self::sort_items(content, &POWERS_ORDER);
        self.weapons = weapons.into_iter().map(|i| (i, 0.0)).collect();
        self.powers = powers.into_iter().map(|i| (i, 0.0)).collect();
        for old in [self.separators.take(), self.icons.take(), self.indicator.take()].into_iter().flatten() {
            st.remove(old);
        }
        let d = st.next_depth(self.mc);
        let seps = st.create_empty(self.mc, "separators_mc", d);
        self.separators = Some(seps);
        let d = st.next_depth(self.mc);
        let icons = st.create_empty(self.mc, "_icons_mc", d);
        self.icons = Some(icons);
        let d = st.next_depth(self.mc);
        self.indicator = st.attach(self.mc, "wheel_indicator", "indicator_mc", d);
        self.nb_items = self.weapons.len() + self.powers.len();
        // Separators: frames 1..7 in turn, alpha 10, every 360 / nbItems degrees.
        let mut frame = 1usize;
        for i in 0..self.nb_items {
            let d = st.next_depth(seps);
            let Some(sep) = st.attach(seps, "wheel_separator", &format!("sep{i}"), d) else { continue };
            let total = st.total_frames(sep);
            st.goto_and_stop(sep, &frame.clamp(1, total.max(1)).to_string());
            if frame > total {
                frame = 0;
            } else {
                frame += 1;
            }
            st.set(sep, Prop::Alpha, 10.0);
            st.set(sep, Prop::Rotation, i as f32 * 360.0 / self.nb_items as f32);
        }
        self.weapon_mcs.clear();
        self.power_mcs.clear();
        for power in [false, true] {
            let n = if power { self.powers.len() } else { self.weapons.len() };
            for i in 0..n {
                let prefix = if power { "power" } else { "weapon" };
                let d = st.next_depth(icons);
                let Some(ic) = st.attach(icons, "itemIcons", &format!("{prefix}{i}"), d) else { continue };
                let angle = if power { self.power_angle(i) } else { self.weapon_angle(i) };
                let r = angle.to_radians();
                let (ix, iy) = (r.sin(), -r.cos());
                st.set(ic, Prop::XScale, DEF_ITEM_SCALE);
                st.set(ic, Prop::YScale, DEF_ITEM_SCALE);
                // `GetCoordFromRadius`.
                let half = 0.5 * self.diameter - 0.5 * st.width(ic);
                st.set(ic, Prop::X, ix * half);
                st.set(ic, Prop::Y, iy * half);
                let item = self.item(i, power).cloned().unwrap_or(Item { index: 0, quantity: None });
                if !power {
                    let bk = st.child(ic, "bkgd").map(|b| st.width(b)).unwrap_or(0.0);
                    if let Some(q) = st.child(ic, "quantity") {
                        st.set(q, Prop::X, ix * 0.5 * (bk + 10.0));
                        st.set(q, Prop::Y, iy * 0.5 * (bk + 10.0));
                    }
                    let q = st.child(ic, "quantity");
                    if item.quantity == Some(0) {
                        st.goto_and_stop(ic, "stop_empty");
                        if let Some(q) = q {
                            st.goto_and_stop(q, "empty");
                        }
                    } else {
                        st.goto_and_stop(ic, "1");
                        if let Some(q) = q {
                            st.goto_and_stop(q, "1");
                        }
                    }
                    set_icon_image(st, ic, icon(item.index));
                    if let Some(q) = st.child(ic, "quantity") {
                        match item.quantity {
                            None => st.set_visible(q, false),
                            Some(n) => {
                                st.set_visible(q, true);
                                set_txt(st, Some(q), "txt", &n.to_string());
                            }
                        }
                    }
                    self.weapons[i].1 = angle;
                    self.weapon_mcs.push(ic);
                } else {
                    st.goto_and_stop(ic, "stop_pow");
                    set_icon_image(st, ic, icon(item.index));
                    if let Some(q) = st.child(ic, "quantity") {
                        st.set_visible(q, false);
                    }
                    self.powers[i].1 = angle;
                    self.power_mcs.push(ic);
                }
            }
        }
    }

    /// `Open()`.
    fn open(&mut self, st: &mut Stage) {
        self.opened = true;
        if let Some(s) = self.item_schema {
            st.goto_and_stop(s, "1");
        }
        set_txt(st, self.item_infos, "name_txt", "");
        set_txt(st, self.item_infos, "ammo_txt", "");
        for ic in self.weapon_mcs.iter().chain(self.power_mcs.iter()).copied() {
            st.tween_end(ic, true);
            st.tween_from(ic, 0.35, &[(Prop::X, 0.0), (Prop::Y, 0.0), (Prop::Alpha, 0.0)], Ease::BackOut, None);
        }
        let mc = self.mc;
        st.clear_key(key(C_WHEEL_CLOSED, 0));
        st.tween_end(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        st.set(mc, Prop::Rotation, -20.0);
        st.set(mc, Prop::XScale, 155.0);
        st.set(mc, Prop::YScale, 155.0);
        st.tween_to(mc, 0.25, &[(Prop::Rotation, 0.0), (Prop::Alpha, 100.0), (Prop::XScale, 100.0), (Prop::YScale, 100.0)], Ease::StrongOut, None);
        let back = [(Prop::Rotation, 0.0), (Prop::XScale, 100.0), (Prop::YScale, 100.0)];
        for (id, rot, scale, d) in [(self.white_stroke, -50.0, 150.0, 0.8), (self.black_stroke, 160.0, 250.0, 0.9), (self.indic_stroke, 90.0, 450.0, 0.6), (self.wheel_bkgd, -60.0, 15.0, 0.6)] {
            if let Some(id) = id {
                st.set(id, Prop::Rotation, rot);
                st.set(id, Prop::XScale, scale);
                st.set(id, Prop::YScale, scale);
                st.tween_to(id, d, &back, Ease::StrongOut, None);
            }
        }
        if let Some(s) = self.separators {
            st.set(s, Prop::Rotation, -120.0);
            st.tween_from(s, 0.45, &[(Prop::Rotation, 0.0)], Ease::StrongOut, None);
        }
        if let Some(c) = self.wheel_circle {
            st.set(c, Prop::XScale, 100.0);
            st.set(c, Prop::YScale, 100.0);
            st.set(c, Prop::Alpha, 100.0);
            st.tween_from(c, 0.45, &[(Prop::XScale, 20.0), (Prop::YScale, 20.0), (Prop::Alpha, 0.0)], Ease::StrongOut, None);
        }
        for id in [self.item_schema, self.indicator, self.item_infos].into_iter().flatten() {
            st.set(id, Prop::Alpha, 0.0);
        }
    }

    /// `Close()`: fade out over 0.25 s, then `OnClosed`.
    fn close(&mut self, st: &mut Stage) {
        self.opened = false;
        st.tween_to(self.mc, 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, Some(key(C_WHEEL_CLOSED, 0)));
    }

    /// `OnClosed()`: the icons and the indicator go; the native side is told.
    fn on_closed(&mut self, st: &mut Stage, out: &mut Vec<Callback>) {
        for id in [self.icons.take(), self.indicator.take()].into_iter().flatten() {
            st.remove(id);
        }
        self.weapon_mcs.clear();
        self.power_mcs.clear();
        out.push(Callback::Closed);
    }

    /// `IsItemSelected(selectionAngle, item, stickiness)`.
    fn is_item_selected(&self, sel: f32, item_angle: f32, stick: f32) -> bool {
        let half = 0.5 * self.item_range();
        is_angle_in_range(sel, item_angle - half - stick, item_angle + half + stick)
    }

    /// `SetWheelSelectionAngle(Angle)` (the 3D tilt left out).
    fn set_selection_angle(&mut self, st: &mut Stage, dock: Option<&mut PcShortcuts>, angle: f32, out: &mut Vec<Callback>, sounds: &mut Vec<&'static str>) {
        if !self.stick_in_use {
            for (id, d, e) in [(self.item_schema, 0.85, Ease::RegularOut), (self.item_infos, 0.85, Ease::RegularOut), (self.indicator, 0.35, Ease::StrongOut)] {
                if let Some(id) = id {
                    st.set(id, Prop::Alpha, 0.0);
                    st.tween_end(id, false);
                    st.tween_to(id, d, &[(Prop::Alpha, 100.0)], e, None);
                }
            }
            self.stick_in_use = true;
        }
        if self.last_sel_angle >= 0.0 {
            let diff = (angle - self.last_sel_angle + 360.0) % 360.0;
            if diff < 180.0 {
                self.absolute_angle += diff;
            } else {
                self.absolute_angle -= 360.0 - diff;
            }
        }
        if let Some(b) = self.black_stroke {
            st.set(b, Prop::Rotation, self.absolute_angle / 80.0);
        }
        if let Some(w) = self.white_stroke {
            st.set(w, Prop::Rotation, -self.absolute_angle / 20.0);
        }
        self.last_sel_angle = angle;
        if let Some(i) = self.indicator {
            st.set(i, Prop::Rotation, angle);
        }
        if let Some(i) = self.indic_stroke {
            st.set(i, Prop::Rotation, self.absolute_angle / 5.0);
        }
        let mut done = false;
        if let Some((idx, power)) = self.sel {
            let a = if power { self.powers.get(idx).map(|x| x.1) } else { self.weapons.get(idx).map(|x| x.1) };
            if let Some(a) = a {
                done = self.is_item_selected(angle, a, STICKINESS);
            }
        }
        let wr = self.weapons_range();
        let r12 = (angle + wr / 2.0 + 360.0) % 360.0;
        let mut dock = dock;
        if !done && wr > 0.0 && r12 <= wr {
            let n = self.weapons.len();
            let idx = ((r12 / wr * n as f32).floor().max(0.0) as usize).min(n - 1);
            self.set_item_focus(st, dock.as_deref_mut(), idx, false, out, sounds);
            done = true;
        }
        let pr = self.powers_range();
        let r14 = angle - (180.0 - pr / 2.0);
        if !done && pr > 0.0 && r14 <= pr {
            let n = self.powers.len();
            let idx = ((r14 / pr * n as f32).floor().max(0.0) as usize).min(n - 1);
            self.set_item_focus(st, dock.as_deref_mut(), n - 1 - idx, true, out, sounds);
        }
    }

    /// `setItemFocus(idx, isPower)`.
    fn set_item_focus(&mut self, st: &mut Stage, dock: Option<&mut PcShortcuts>, idx: usize, power: bool, out: &mut Vec<Callback>, sounds: &mut Vec<&'static str>) {
        let mc = self.item_mc(idx, power);
        if mc.is_some() && mc == self.sel.and_then(|(i, p)| self.item_mc(i, p)) {
            return;
        }
        if let Some(d) = dock {
            d.play_glow_anim(st, true);
        }
        if let Some((i, p)) = self.sel {
            self.play_selection_out(st, i, p);
        }
        self.sel = Some((idx, power));
        self.sel_enum = self.item(idx, power).map(|i| i.index);
        out.push(Callback::SelectionChanged(self.sel_enum.unwrap_or(0)));
        self.play_selection_in(st, idx, power);
        sounds.push("itemSelection");
    }

    /// `UpdateItemInfos(itemName, ammoType)`.
    fn update_item_infos(&mut self, st: &mut Stage, name: &str, ammo: Option<&str>) {
        let Some(infos) = self.item_infos else { return };
        let qty = self.sel.and_then(|(i, p)| self.item(i, p)).and_then(|i| i.quantity);
        if ammo.is_none() && qty.is_none() {
            st.goto_and_stop(infos, "no_ammo");
        } else {
            st.goto_and_stop(infos, "1");
            let mut s = ammo.unwrap_or("").to_string();
            if let Some(q) = qty {
                s = format!("{s} ({q})");
            }
            set_txt(st, Some(infos), "ammo_txt", &s);
        }
        set_txt(st, Some(infos), "name_txt", &name.to_uppercase());
        if let (Some(s), Some(item)) = (self.item_schema, self.sel_enum) {
            st.goto_and_stop(s, schema_of(item));
        }
    }

    /// `playSelectionIn(mc)`.
    fn play_selection_in(&mut self, st: &mut Stage, idx: usize, power: bool) {
        let Some(mc) = self.item_mc(idx, power) else { return };
        st.tween_end(mc, true);
        let label = if power {
            "over_pow"
        } else if self.is_empty_item(idx, power) {
            "over_empty"
        } else {
            "over"
        };
        st.goto_and_play(mc, label);
        if let Some(g) = st.child(mc, "animGlow") {
            st.goto_and_play(g, "over");
        }
        st.tween_to(mc, 0.35, &[(Prop::XScale, SEL_ITEM_SCALE), (Prop::YScale, SEL_ITEM_SCALE)], Ease::StrongOut, None);
    }

    /// `playSelectionOut(mc)`.
    fn play_selection_out(&mut self, st: &mut Stage, idx: usize, power: bool) {
        let Some(mc) = self.item_mc(idx, power) else { return };
        st.tween_end(mc, true);
        let label = if power {
            "out_pow"
        } else if self.is_empty_item(idx, power) {
            "out_empty"
        } else {
            "out"
        };
        st.goto_and_play(mc, label);
        if let Some(g) = st.child(mc, "animGlow") {
            st.goto_and_play(g, "out");
        }
        st.tween_to(mc, 0.25, &[(Prop::XScale, DEF_ITEM_SCALE), (Prop::YScale, DEF_ITEM_SCALE)], Ease::BackOut, None);
    }

    /// `ClearCurrentSelection()`.
    fn clear_current_selection(&mut self, st: &mut Stage, dock: Option<&mut PcShortcuts>) {
        if let Some(d) = dock {
            d.play_glow_anim(st, false);
        }
        if let Some((i, p)) = self.sel {
            self.play_selection_out(st, i, p);
        }
        self.last_sel_angle = -1.0;
        if let Some(s) = self.item_schema {
            st.tween_end(s, false);
            st.goto_and_stop(s, "1");
        }
        for id in [self.item_infos, self.indicator].into_iter().flatten() {
            st.tween_end(id, false);
        }
        self.stick_in_use = false;
        self.sel = None;
        self.sel_enum = None;
    }

    /// `ClearSelectionAngle()`.
    fn clear_selection_angle(&mut self, st: &mut Stage, dock: Option<&mut PcShortcuts>, out: &mut Vec<Callback>) {
        self.clear_current_selection(st, dock);
        for id in [self.item_infos, self.indicator].into_iter().flatten() {
            st.tween_to(id, 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
        }
        out.push(Callback::SelectionChanged(0));
    }

    /// `getItemQuantity` of the selection.
    fn selected_quantity(&self) -> Option<i32> {
        self.sel.and_then(|(i, p)| self.item(i, p)).and_then(|i| i.quantity)
    }
}

/// `IsAngleInRange(angle, minAngle, maxAngle)`.
pub fn is_angle_in_range(a: f32, mn: f32, mx: f32) -> bool {
    let a = (a + 360.0) % 360.0;
    let mn = (mn + 360.0) % 360.0;
    let mx = (mx + 360.0) % 360.0;
    if mn < mx {
        a >= mn && a <= mx
    } else if a < 180.0 {
        a + 360.0 >= mn && a <= mx
    } else {
        a >= mn && a - 360.0 <= mx
    }
}

// =============================================================================================
// _powerWheel.PC_ShortcutsHandler (`shortcutsPC_mc`, symbol `shPC_`): the PC dock
// =============================================================================================

/// One `_shortcutsLayout` entry: `{itemIndex, itemQuantity}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shortcut {
    pub item: Option<u8>,
    pub quantity: Option<i32>,
}

pub struct PcShortcuts {
    pub mc: Id,
    bkgd: Option<Id>,
    dock: Id,
    slots: Vec<Id>,
    pub layout: Vec<Shortcut>,
    pub opened: bool,
    pub in_wheel: bool,
    anim_offset_y: f32,
    pos_y: f32,
    def_slot_y: f32,
    pub sel_slot: i32,
    slot_over: bool,
    interval: Option<u32>,
}

/// `_nbSlots`.
const NB_SLOTS: usize = 10;
/// PC `Init`: `_slotOutScaleValue` / `_slotOverScaleValue`.
const SLOT_OUT_SCALE: f32 = 100.0;
const SLOT_OVER_SCALE: f32 = 110.0;

impl PcShortcuts {
    /// `PowerWheelBase` attaches `shPC_` at (Stage.width / 2, Stage.height); the
    /// `ShortcutsHandler` constructor scales it to 90 % and runs the PC `Init`.
    fn new(st: &mut Stage, root: Id) -> Option<Self> {
        let d = st.next_depth(root);
        let mc = st.attach(root, "shPC_", "shortcutsPC_mc", d)?;
        st.set(mc, Prop::X, 0.5 * crate::gfx_stage::STAGE.x);
        st.set(mc, Prop::Y, crate::gfx_stage::STAGE.y);
        st.set(mc, Prop::XScale, 90.0);
        st.set(mc, Prop::YScale, 90.0);
        st.set_visible(mc, false);
        st.set(mc, Prop::Alpha, 0.0);
        let d = st.next_depth(mc);
        let dock = st.create_empty(mc, "_dock_mc", d);
        // `_utils.MovieClipCreator.CreateElements(_dock_mc, "shPC_item", "slot", 10, 1, 10)`:
        // origin centre / centre, `_offsetOriginY` -25, `_offsetX` -5, one row.
        let mut slots = Vec::new();
        for i in 0..NB_SLOTS {
            let d = st.next_depth(dock);
            let Some(s) = st.attach(dock, "shPC_item", &format!("slot{i}"), d) else { continue };
            let (w, h) = (st.width(s), st.height(s));
            let x0 = w / 2.0;
            let y0 = (-25.0 + h / 2.0).round();
            st.set(s, Prop::X, (x0 + i as f32 * (-5.0 + w)).round());
            st.set(s, Prop::Y, y0);
            slots.push(s);
        }
        let dw = st.width(dock);
        st.set(dock, Prop::X, -0.5 * dw);
        let def_slot_y = slots.first().map(|s| st.get(*s, Prop::Y)).unwrap_or(0.0);
        Some(Self { mc, bkgd: st.child(mc, "_bkgd_mc"), dock, slots, layout: vec![Shortcut::default(); NB_SLOTS], opened: false, in_wheel: false, anim_offset_y: 10.0, pos_y: 0.0, def_slot_y, sel_slot: -1, slot_over: false, interval: None })
    }

    fn clear_interval(&mut self, st: &mut Stage) {
        if let Some(i) = self.interval.take() {
            st.clear_interval(i);
        }
    }

    /// `SetShortcuts(shortcutsArray)`.
    fn set_shortcuts(&mut self, st: &mut Stage, layout: Vec<Shortcut>, icon: &mut dyn FnMut(u8) -> Option<ImageRef>) {
        self.layout = layout;
        self.layout.resize(NB_SLOTS, Shortcut::default());
        self.set_icons(st, icon);
    }

    /// PC `SetIcons`: the key numbers, then each slot's icon.
    fn set_icons(&mut self, st: &mut Stage, icon: &mut dyn FnMut(u8) -> Option<ImageRef>) {
        for i in 0..self.slots.len() {
            let s = self.slots[i];
            st.tween_end(s, false);
            // `GetDockKeyIdxFromIdx`: 1..9 then 0.
            let n = if i < NB_SLOTS - 1 { i + 1 } else { 0 };
            if let Some(t) = st.find(s, "key_mc/txt") {
                st.set_text(t, &n.to_string());
                st.set_auto_size(t, AutoSize::Left);
            }
            st.set(s, Prop::Y, self.def_slot_y);
            st.set(s, Prop::Alpha, 100.0);
        }
        for i in 0..self.slots.len() {
            self.set_icon(st, i, icon);
        }
    }

    /// `SetIcon(idx)`.
    fn set_icon(&mut self, st: &mut Stage, i: usize, icon: &mut dyn FnMut(u8) -> Option<ImageRef>) {
        let Some(&s) = self.slots.get(i) else { return };
        let Some(ic) = st.child(s, "ic") else { return };
        let sc = self.layout.get(i).cloned().unwrap_or_default();
        match sc.item {
            None => {
                st.goto_and_stop(ic, "1");
                clear_container(st, ic);
                if let Some(q) = st.child(ic, "quantity") {
                    st.set_visible(q, false);
                }
                st.set(s, Prop::Alpha, 55.0);
                st.set(ic, Prop::Alpha, 0.0);
            }
            Some(item) => {
                st.goto_and_stop(ic, if item > FIRST_POWER_INDEX { "stop_pow" } else { "1" });
                set_icon_image(st, ic, icon(item));
                if let Some(q) = st.child(ic, "quantity") {
                    match sc.quantity {
                        None => st.set_visible(q, false),
                        Some(n) => {
                            st.set_visible(q, true);
                            set_txt(st, Some(q), "txt", &n.to_string());
                        }
                    }
                }
                st.set(ic, Prop::Alpha, 100.0);
            }
        }
    }

    fn is_power(&self, i: usize) -> bool {
        self.layout.get(i).and_then(|s| s.item).is_some_and(|x| x > FIRST_POWER_INDEX)
    }

    /// PC `Open(bInWheel)` (`_inWheel`: the argument was given).
    fn open(&mut self, st: &mut Stage, in_wheel: bool, potions: Option<&mut PcPotions>) {
        st.set_visible(self.mc, true);
        self.clear_interval(st);
        self.opened = true;
        self.in_wheel = in_wheel;
        self.anim_offset_y = if in_wheel { 40.0 } else { 10.0 };
        self.pos_y = movie_space(1.0).1.y;
        st.clear_key(key(C_DOCK_CLOSED, 0));
        st.tween_end(self.mc, false);
        st.set(self.mc, Prop::Alpha, 0.0);
        st.set(self.mc, Prop::Y, self.pos_y + self.anim_offset_y);
        st.tween_to(self.mc, 0.2, &[(Prop::Alpha, 100.0), (Prop::Y, self.pos_y)], Ease::BackOut, None);
        let dh = st.height(self.dock);
        st.set(self.dock, Prop::Y, -0.5 * dh - 22.5);
        if let Some(b) = self.bkgd {
            st.tween_end(b, false);
            st.set(b, Prop::Alpha, if in_wheel { 100.0 } else { 30.0 });
        }
        for i in 0..self.slots.len() {
            let s = self.slots[i];
            st.tween_end(s, true);
            if let Some(g) = st.find(s, "ic/animGlow") {
                st.goto_and_stop(g, "1");
            }
            if let Some(b) = st.find(s, "ic/bkgd") {
                st.set_visible(b, true);
            }
            let sc = if i as i32 == self.sel_slot { SLOT_OVER_SCALE } else { SLOT_OUT_SCALE };
            st.tween_to(s, 0.25, &[(Prop::Alpha, 100.0), (Prop::XScale, sc), (Prop::YScale, sc)], Ease::BackOut, None);
        }
        if in_wheel {
            if let Some(p) = potions {
                p.open(st);
            }
        } else {
            let s = self.sel_slot;
            self.set_shortcut_selection(st, s);
        }
    }

    /// PC `Close()`.
    fn close(&mut self, st: &mut Stage, potions: Option<&mut PcPotions>) {
        if self.in_wheel {
            self.opened = false;
            if let Some(p) = potions {
                p.close(st);
            }
            self.close_mc(st);
        } else {
            if let Some(b) = self.bkgd {
                st.tween_end(b, false);
                st.tween_to(b, 0.2, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
            }
            // `ShortcutsHandler.Close`: every slot but the selected one fades, then
            // `Close_SelSlot` after 200 ms.
            self.clear_interval(st);
            self.opened = false;
            for i in 0..self.slots.len() {
                if i as i32 == self.sel_slot {
                    continue;
                }
                st.tween_to(self.slots[i], 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
            }
            self.interval = Some(st.set_interval(200.0, key(C_DOCK_SELSLOT, 0)));
        }
    }

    /// PC `Close_SelSlot()`.
    fn close_sel_slot(&mut self, st: &mut Stage) {
        self.clear_interval(st);
        if let Some(&s) = usize::try_from(self.sel_slot).ok().and_then(|i| self.slots.get(i)) {
            if let Some(g) = st.find(s, "ic/animGlow") {
                st.goto_and_play(g, "select");
            }
            let y = st.get(s, Prop::Y);
            st.tween_to(s, 0.25, &[(Prop::Y, y - 20.0)], Ease::StrongOut, None);
        }
        self.interval = Some(st.set_interval(350.0, key(C_DOCK_CLOSE_MC, 0)));
        self.sel_slot = -1;
    }

    /// PC `Close_Mc()`: fade and slide down 0.2 s, then hidden and `OnClosed`.
    fn close_mc(&mut self, st: &mut Stage) {
        self.clear_interval(st);
        st.tween_end(self.mc, false);
        st.tween_to(self.mc, 0.2, &[(Prop::Alpha, 0.0), (Prop::Y, self.pos_y + self.anim_offset_y)], Ease::StrongOut, Some(key(C_DOCK_CLOSED, 0)));
    }

    /// `SetShortcutSelection(idx)`.
    fn set_shortcut_selection(&mut self, st: &mut Stage, idx: i32) {
        if idx <= -1 {
            return;
        }
        if self.sel_slot > -1 {
            let s = self.sel_slot as usize;
            self.play_selection_out(st, s);
        }
        self.sel_slot = idx;
        self.play_selection_in(st, idx as usize);
    }

    fn play_selection_in(&mut self, st: &mut Stage, i: usize) {
        let Some(&s) = self.slots.get(i) else { return };
        st.tween_end(s, true);
        st.tween_to(s, 0.2, &[(Prop::XScale, SLOT_OVER_SCALE), (Prop::YScale, SLOT_OVER_SCALE)], Ease::StrongOut, None);
        if let Some(g) = st.find(s, "ic/animGlow") {
            st.goto_and_play(g, "over");
        }
        if self.layout.get(i).is_some_and(|x| x.item.is_some())
            && let Some(ic) = st.child(s, "ic")
        {
            st.goto_and_play(ic, if self.is_power(i) { "over_pow" } else { "over" });
        }
    }

    fn play_selection_out(&mut self, st: &mut Stage, i: usize) {
        let Some(&s) = self.slots.get(i) else { return };
        st.tween_end(s, true);
        st.tween_to(s, 0.2, &[(Prop::XScale, SLOT_OUT_SCALE), (Prop::YScale, SLOT_OUT_SCALE)], Ease::StrongOut, None);
        if let Some(g) = st.find(s, "ic/animGlow") {
            st.goto_and_play(g, "out");
        }
        if self.layout.get(i).is_some_and(|x| x.item.is_some())
            && let Some(ic) = st.child(s, "ic")
        {
            st.goto_and_play(ic, if self.is_power(i) { "out_pow" } else { "out" });
        }
    }

    /// `PlayGlowAnim(glowVisible)`.
    fn play_glow_anim(&mut self, st: &mut Stage, visible: bool) {
        if self.slot_over == visible {
            return;
        }
        for &s in &self.slots {
            if let Some(g) = st.find(s, "ic/animGlow") {
                st.goto_and_play(g, if visible { "over" } else { "out" });
            }
        }
        self.slot_over = visible;
    }

    /// The slot just emptied: faded to 55 % at 95 %, its icon shrinking away.
    fn play_emptied(&mut self, st: &mut Stage, i: usize) {
        let Some(&s) = self.slots.get(i) else { return };
        st.set(s, Prop::Alpha, 100.0);
        st.tween_to(s, 0.2, &[(Prop::Alpha, 55.0), (Prop::XScale, 95.0), (Prop::YScale, 95.0)], Ease::StrongOut, None);
        if let Some(ic) = st.child(s, "ic") {
            st.set(ic, Prop::Alpha, 100.0);
            st.tween_to(ic, 0.15, &[(Prop::Alpha, 0.0), (Prop::XScale, 60.0), (Prop::YScale, 60.0)], Ease::StrongOut, None);
        }
    }

    /// The native `AssignShortcut(slot, 0)` call for a slot that held the item just assigned
    /// elsewhere (0x00BC3400 empties every other slot with the same item).
    fn clear_slot(&mut self, st: &mut Stage, i: usize, icon: &mut dyn FnMut(u8) -> Option<ImageRef>) {
        if i >= self.layout.len() || self.layout[i].item.is_none() {
            return;
        }
        self.layout[i] = Shortcut::default();
        self.set_icon(st, i, icon);
        self.play_emptied(st, i);
    }

    /// PC `AssignShortcut(shortcutIdx)` with the base class's: the wheel's selection goes to
    /// the slot (or leaves it when it already holds it).
    fn assign_shortcut(&mut self, st: &mut Stage, i: usize, wheel: &WheelHandler, icon: &mut dyn FnMut(u8) -> Option<ImageRef>, sounds: &mut Vec<&'static str>) {
        let Some(&s) = self.slots.get(i) else { return };
        let current = self.layout.get(i).and_then(|x| x.item);
        if wheel.sel_enum.is_none() && current.is_none() {
            return;
        }
        if wheel.sel_enum.is_none() || wheel.sel_enum == current {
            self.layout[i] = Shortcut::default();
            sounds.push("P_UnassignShortcut");
        } else {
            self.layout[i] = Shortcut { item: wheel.sel_enum, quantity: wheel.selected_quantity() };
            sounds.push("P_AssignShortcut");
        }
        if wheel.stick_in_use
            && let Some(ic) = st.child(s, "ic")
        {
            st.goto_and_play(ic, if self.is_power(i) { "out_pow" } else { "out" });
        }
        self.set_icon(st, i, icon);
        let ic = st.child(s, "ic");
        if self.layout[i].item.is_none() {
            self.play_emptied(st, i);
        } else {
            let label = if self.is_power(i) { "out_pow" } else { "out" };
            st.set(s, Prop::XScale, 110.0);
            st.set(s, Prop::YScale, 110.0);
            st.tween_to(s, 0.2, &[(Prop::Alpha, 100.0), (Prop::XScale, 100.0), (Prop::YScale, 100.0)], Ease::StrongOut, None);
            if let Some(ic) = ic {
                st.tween_to(ic, 0.2, &[(Prop::Alpha, 100.0), (Prop::XScale, 47.5), (Prop::YScale, 47.5)], Ease::StrongOut, None);
                st.goto_and_play(ic, label);
            }
        }
    }
}

// =============================================================================================
// _powerWheel.PC_PotionsCount (`potionsPC_mc`)
// =============================================================================================

pub struct PcPotions {
    pub mc: Id,
    health: Option<Id>,
    mana: Option<Id>,
    quantity: [i32; 2],
    bindings: [String; 2],
}

impl PcPotions {
    fn new(st: &mut Stage, mc: Id) -> Self {
        let health = st.child(mc, "_potionsHealth_mc");
        let mana = st.child(mc, "_potionsMana_mc");
        st.set_visible(mc, false);
        for (p, f) in [(health, "1"), (mana, "2")] {
            if let Some(p) = p {
                if let Some(k) = st.child(p, "keyPC_mc") {
                    st.set_visible(k, false);
                }
                if let Some(m) = st.find(p, "ic/stroke_mc/mc") {
                    st.goto_and_stop(m, f);
                }
            }
        }
        let mut s = Self { mc, health, mana, quantity: [-1, -1], bindings: [String::new(), String::new()] };
        s.set_position(st);
        s
    }

    /// PC `SetPosition`: `GetMovieSpaceCoord(0.9).coordMax`.
    fn set_position(&mut self, st: &mut Stage) {
        let (_, max) = movie_space(0.9);
        st.set(self.mc, Prop::X, max.x);
        st.set(self.mc, Prop::Y, max.y);
    }

    /// PC `Open`: the key bindings, then the two counters slide in from 150 px right.
    fn open(&mut self, st: &mut Stage) {
        self.set_potions_binding(st);
        self.set_position(st);
        st.set_visible(self.mc, true);
        for (p, d, k) in [(self.health, 0.45, 0u32), (self.mana, 0.55, 1)] {
            let Some(p) = p else { continue };
            st.clear_key(key(C_POTION_HIDDEN, k));
            let x = st.get(p, Prop::X);
            st.set_visible(p, true);
            st.set(p, Prop::X, x + 150.0);
            st.tween_end(p, true);
            st.tween_to(p, d, &[(Prop::X, x), (Prop::Alpha, 100.0)], Ease::StrongOut, None);
        }
    }

    fn close(&mut self, st: &mut Stage) {
        for (p, k) in [(self.health, 0u32), (self.mana, 1)] {
            if let Some(p) = p {
                st.tween_end(p, true);
                st.tween_to(p, 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, Some(key(C_POTION_HIDDEN, k)));
            }
        }
    }

    /// `SetPotionsCount(health, mana)`.
    fn set_potions_count(&mut self, st: &mut Stage, h: i32, m: i32) {
        self.quantity = [h, m];
        for (p, id, q, km) in [(self.health, "Health", h, "B"), (self.mana, "Mana", m, "X")] {
            let Some(p) = p else { continue };
            if let Some(ic) = st.child(p, "ic") {
                st.goto_and_stop(ic, id);
            }
            set_txt(st, Some(p), "quantity/txt", &q.to_string());
            if let Some(k) = st.child(p, "key_mc") {
                st.goto_and_stop(k, km);
            }
            // `SetColorTransform`: reset, or `SetColor_Lock(false)` (x 0.4, alpha included).
            for target in [st.child(p, "ic"), st.child(p, "keyPC_mc")].into_iter().flatten() {
                if q > 0 {
                    st.set_color(target, ColorTransform::default());
                    st.set(target, Prop::Alpha, 100.0);
                } else {
                    st.set_color(target, ColorTransform { mul: [0.4, 0.4, 0.4, 0.4], add: [0.0; 4] });
                    st.set(target, Prop::Alpha, 40.0);
                }
            }
        }
    }

    /// PC `SetPotionsBinding`: the elixir keys ("[Key]" from `FormatText`).
    fn set_potions_binding(&mut self, st: &mut Stage) {
        for (i, p) in [self.health, self.mana].into_iter().enumerate() {
            let Some(p) = p else { continue };
            if let Some(k) = st.child(p, "keyPC_mc") {
                st.set_visible(k, self.quantity[i] > 0);
            }
            if let Some(k) = st.child(p, "key_mc") {
                st.set_visible(k, false);
            }
            let Some(kpc) = st.child(p, "keyPC_mc") else { continue };
            let b = self.bindings[i].clone();
            if let Some(t) = st.child(kpc, "txt") {
                if b.starts_with('[') {
                    let s = b.replace(['[', ']'], "").to_uppercase();
                    st.set_text(t, &s);
                    st.set_auto_size(t, AutoSize::Left);
                    let w = st.width(t);
                    st.set(t, Prop::X, -0.5 * w);
                    let h = st.height(t);
                    if let Some(bk) = st.child(kpc, "bkgd_mc") {
                        st.set_width(bk, w + 5.0);
                        st.set_height(bk, h + 5.0);
                        st.set_visible(bk, true);
                    }
                } else {
                    st.set_text(t, &b);
                    st.set_auto_size(t, AutoSize::Left);
                    if let Some(bk) = st.child(kpc, "bkgd_mc") {
                        st.set_visible(bk, false);
                    }
                    let w = st.width(kpc);
                    st.set(kpc, Prop::X, -0.5 * w - 60.0);
                }
            }
        }
    }

    /// `Play{Health,Mana}PotionConsumptionAnim`.
    fn play_consumption(&mut self, st: &mut Stage, mana: bool) {
        let p = if mana { self.mana } else { self.health };
        if let Some(s) = p.and_then(|p| st.find(p, "ic/stroke_mc")) {
            st.goto_and_play(s, "consume");
        }
    }
}

// =============================================================================================
// The movie (root timeline functions)
// =============================================================================================

pub struct WheelMovie {
    pub stage: Stage,
    pub wheel: Option<WheelHandler>,
    pub dock: Option<PcShortcuts>,
    pub potions: Option<PcPotions>,
    glass: Option<Id>,
    white: Option<Id>,
    rng: Rng,
    pub callbacks: Vec<Callback>,
    pub sounds: Vec<&'static str>,
    draws: Vec<Draw>,
}

impl WheelMovie {
    pub fn new(mut stage: Stage) -> Self {
        let root = stage.root;
        let wheel = stage.child(root, "wheel_mc").map(|m| WheelHandler::new(&mut stage, m));
        let glass = stage.child(root, "glass_bkgd");
        let white = stage.child(root, "white_bkgd");
        for id in [glass, white].into_iter().flatten() {
            stage.set_visible(id, false);
        }
        let potions = stage.child(root, "potionsPC_mc").map(|m| PcPotions::new(&mut stage, m));
        // The gamepad pieces stay hidden on PC.
        for n in ["potions_mc", "shortcuts_mc", "quickPotion_mc"] {
            if let Some(m) = stage.child(root, n) {
                stage.set_visible(m, false);
            }
        }
        let dock = PcShortcuts::new(&mut stage, root);
        Self { stage, wheel, dock, potions, glass, white, rng: Rng(0x2545_f491), callbacks: Vec::new(), sounds: Vec::new(), draws: Vec::new() }
    }

    pub fn wheel_open(&self) -> bool {
        self.wheel.as_ref().is_some_and(|w| w.opened)
    }

    /// Root `OpenWheel(wheelContent)`: icons, open, dock (in wheel), separators, background.
    pub fn open_wheel(&mut self, content: &[Item], icon: &mut dyn FnMut(u8) -> Option<ImageRef>) {
        let st = &mut self.stage;
        if let Some(w) = self.wheel.as_mut() {
            w.fill_wheel_icons(st, content, icon);
            w.open(st);
        }
        if let Some(d) = self.dock.as_mut() {
            d.open(st, true, self.potions.as_mut());
        }
        self.animate_wheel_sep(false);
        self.open_bkgd();
    }

    /// Root `CloseWheel()`.
    pub fn close_wheel(&mut self) {
        self.close_bkgd();
        let st = &mut self.stage;
        if let Some(w) = self.wheel.as_mut() {
            w.clear_current_selection(st, self.dock.as_mut());
            w.close(st);
        }
        if let Some(d) = self.dock.as_mut() {
            d.close(st, self.potions.as_mut());
        }
    }

    /// Root `SetSelectionAngle(Angle)`.
    pub fn set_selection_angle(&mut self, angle: f32) {
        if let Some(w) = self.wheel.as_mut() {
            w.set_selection_angle(&mut self.stage, self.dock.as_mut(), angle, &mut self.callbacks, &mut self.sounds);
        }
    }
    /// Root `ClearSelectionAngle()`.
    pub fn clear_selection_angle(&mut self) {
        if let Some(w) = self.wheel.as_mut() {
            w.clear_selection_angle(&mut self.stage, self.dock.as_mut(), &mut self.callbacks);
        }
    }
    /// Root `UpdateItemInfos(itemName, ammoType)`.
    pub fn update_item_infos(&mut self, name: &str, ammo: Option<&str>) {
        if let Some(w) = self.wheel.as_mut() {
            w.update_item_infos(&mut self.stage, name, ammo);
        }
    }
    /// Root `OpenShortcuts()` without an argument: the dock alone (number keys).
    pub fn open_shortcuts(&mut self) {
        if let Some(d) = self.dock.as_mut() {
            d.open(&mut self.stage, false, self.potions.as_mut());
        }
    }
    pub fn close_shortcuts(&mut self) {
        if let Some(d) = self.dock.as_mut() {
            d.close(&mut self.stage, self.potions.as_mut());
        }
    }
    pub fn set_shortcuts(&mut self, layout: Vec<Shortcut>, icon: &mut dyn FnMut(u8) -> Option<ImageRef>) {
        if let Some(d) = self.dock.as_mut() {
            d.set_shortcuts(&mut self.stage, layout, icon);
        }
    }
    pub fn set_shortcut_selection(&mut self, idx: i32) {
        if let Some(d) = self.dock.as_mut() {
            d.set_shortcut_selection(&mut self.stage, idx);
        }
    }
    /// Root `AssignShortcut(idx)` (a number key while the wheel is open).
    pub fn assign_shortcut(&mut self, idx: usize, icon: &mut dyn FnMut(u8) -> Option<ImageRef>) {
        if let (Some(d), Some(w)) = (self.dock.as_mut(), self.wheel.as_ref()) {
            d.assign_shortcut(&mut self.stage, idx, w, icon, &mut self.sounds);
        }
    }
    /// Empty a dock slot (the native side took its item away).
    pub fn clear_shortcut(&mut self, idx: usize, icon: &mut dyn FnMut(u8) -> Option<ImageRef>) {
        if let Some(d) = self.dock.as_mut() {
            d.clear_slot(&mut self.stage, idx, icon);
        }
    }
    pub fn set_potions_count(&mut self, h: i32, m: i32) {
        if let Some(p) = self.potions.as_mut() {
            p.set_potions_count(&mut self.stage, h, m);
        }
    }
    pub fn set_potion_bindings(&mut self, health: String, mana: String) {
        if let Some(p) = self.potions.as_mut() {
            p.bindings = [health, mana];
        }
    }
    pub fn play_potion_consumption(&mut self, mana: bool) {
        if let Some(p) = self.potions.as_mut() {
            p.play_consumption(&mut self.stage, mana);
        }
    }

    /// Root `AnimateGlassParts(targetMc, animType)`: each piece flies in from (or out to) a
    /// random point 800-1200 px out, 30-80 degrees turned, at half alpha.
    fn animate_glass_parts(&mut self, target: Option<Id>, kind: u8) {
        let Some(t) = target else { return };
        for c in self.stage.children(t) {
            let (x, y) = (self.stage.get(c, Prop::X), self.stage.get(c, Prop::Y));
            let dx = self.rng.range(800.0, 1200.0);
            let x2 = if x < 640.0 { x - dx } else { x + dx };
            let dy = self.rng.range(650.0, 900.0);
            let y2 = if y < 360.0 { y - dy } else { y + dy };
            let rot = self.rng.range(30.0, 80.0);
            let props = [(Prop::Alpha, 50.0), (Prop::X, x2), (Prop::Y, y2), (Prop::Rotation, rot)];
            match kind {
                0 => {
                    self.stage.tween_end(c, true);
                    let d = self.rng.range(1.0, 1.2);
                    self.stage.tween_from(c, d, &props, Ease::StrongOut, None);
                }
                1 => {
                    self.stage.tween_end(c, false);
                    self.stage.tween_to(c, 1.05, &props, Ease::StrongOut, None);
                }
                _ => self.stage.tween_end(c, true),
            }
        }
    }

    /// Root `OpenBkgd()`.
    fn open_bkgd(&mut self) {
        let (b0, b1) = (self.glass.and_then(|g| self.stage.child(g, "bkgd0")), self.glass.and_then(|g| self.stage.child(g, "bkgd1")));
        self.animate_glass_parts(b0, 0);
        self.animate_glass_parts(b1, 0);
        let st = &mut self.stage;
        for (id, a, k) in [(self.glass, 100.0, C_GLASS_HIDDEN), (self.white, 10.0, C_WHITE_HIDDEN)] {
            if let Some(id) = id {
                st.clear_key(key(k, 0));
                st.tween_end(id, false);
                st.set(id, Prop::Alpha, 0.0);
                st.set_visible(id, true);
                st.tween_to(id, 0.25, &[(Prop::Alpha, a)], Ease::StrongOut, None);
            }
        }
        self.animate_bkgd(0, false);
        self.animate_bkgd(1, false);
    }

    /// Root `CloseBkgd()`.
    fn close_bkgd(&mut self) {
        let (b0, b1) = (self.glass.and_then(|g| self.stage.child(g, "bkgd0")), self.glass.and_then(|g| self.stage.child(g, "bkgd1")));
        self.animate_glass_parts(b0, 2);
        self.animate_glass_parts(b1, 2);
        let st = &mut self.stage;
        for (id, k) in [(self.glass, C_GLASS_HIDDEN), (self.white, C_WHITE_HIDDEN)] {
            if let Some(id) = id {
                st.tween_end(id, false);
                st.tween_to(id, 0.25, &[(Prop::Alpha, 0.0)], Ease::StrongOut, Some(key(k, 0)));
            }
        }
    }

    /// Root `animateBkgd0` / `animateBkgd0_b` / `animateBkgd1` / `animateBkgd1_b`: the two glass
    /// layers drift for as long as the movie runs.
    fn animate_bkgd(&mut self, layer: u32, second: bool) {
        let Some(mc) = self.glass.and_then(|g| self.stage.child(g, if layer == 0 { "bkgd0" } else { "bkgd1" })) else { return };
        let (x, a, d) = match (layer, second) {
            (0, false) => (self.rng.range(100.0, 120.0), 50.0, 5.0),
            (0, true) => (self.rng.range(-20.0, 0.0), 100.0, 3.0),
            (_, false) => (self.rng.range(-100.0, 100.0), 80.0, 6.0),
            (_, true) => (self.rng.range(-20.0, 0.0), 100.0, 6.0),
        };
        let code = if layer == 0 { C_BKGD0 } else { C_BKGD1 };
        self.stage.tween_to(mc, d, &[(Prop::X, x), (Prop::Alpha, a)], Ease::StrongOut, Some(key(code, if second { 1 } else { 0 })));
    }

    /// Root `animateWheelSep` (`second`: `animateWheelSep2`): the separators' inner pieces drift.
    fn animate_wheel_sep(&mut self, second: bool) {
        let Some(w) = self.wheel.as_ref() else { return };
        let Some(seps) = w.separators else { return };
        let n = w.nb_items;
        for i in 0..n {
            let Some(sep) = self.stage.child(seps, &format!("sep{i}")).and_then(|s| self.stage.child(s, "sep")) else { continue };
            let (y, r, d) = if second { (self.rng.range(-5.0, 0.0), self.rng.range(-4.5, 3.5), 2.5) } else { (self.rng.range(-50.0, -25.0), self.rng.range(-5.0, 5.0), 5.0) };
            let k = (i == 0).then(|| key(C_SEP, if second { 1 } else { 0 }));
            self.stage.tween_to(sep, d, &[(Prop::Y, y), (Prop::Rotation, r)], Ease::StrongOut, k);
        }
    }

    /// Run the movie up to `until` (stage seconds), handling its callbacks.
    pub fn run(&mut self, dt: f32) {
        let until = self.stage.time + dt as f64;
        while let Some(ev) = self.stage.poll(until) {
            let k = match ev {
                Event::Timer(k) | Event::TweenDone(k) => k,
            };
            let (code, arg) = (((k >> 32) & 0xffff) as u16, k as u32);
            match code {
                C_WHEEL_CLOSED => {
                    if let Some(w) = self.wheel.as_mut() {
                        w.on_closed(&mut self.stage, &mut self.callbacks);
                    }
                }
                C_GLASS_HIDDEN => {
                    if let Some(g) = self.glass {
                        self.stage.set_visible(g, false);
                    }
                }
                C_WHITE_HIDDEN => {
                    if let Some(g) = self.white {
                        self.stage.set_visible(g, false);
                    }
                }
                C_SEP => self.animate_wheel_sep(arg == 0),
                C_BKGD0 => self.animate_bkgd(0, arg == 0),
                C_BKGD1 => self.animate_bkgd(1, arg == 0),
                C_DOCK_SELSLOT => {
                    if let Some(d) = self.dock.as_mut() {
                        d.close_sel_slot(&mut self.stage);
                    }
                }
                C_DOCK_CLOSE_MC => {
                    if let Some(d) = self.dock.as_mut() {
                        d.close_mc(&mut self.stage);
                    }
                }
                C_DOCK_CLOSED => {
                    if let Some(d) = self.dock.as_ref() {
                        self.stage.set_visible(d.mc, false);
                    }
                    self.callbacks.push(Callback::Closed);
                }
                C_POTION_HIDDEN => {
                    if let Some(p) = self.potions.as_ref() {
                        let id = if arg == 0 { p.health } else { p.mana };
                        if let Some(id) = id {
                            self.stage.set_visible(id, false);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

// =============================================================================================
// Native side
// =============================================================================================

/// Item names and ammo names (RPG.int `[DisUISelectionType]` / `[DisAmmoType]`).
#[derive(Default, Clone)]
pub struct WheelTexts {
    pub names: HashMap<u8, String>,
    pub ammo: HashMap<u8, String>,
    /// `<Name>_ShortDescription` of the items without a level or an ammo name: the Spring Razor
    /// ("Proximity Mine", 0x00C45C90) and the Heart (0x00C45C10).
    pub short: HashMap<u8, String>,
    /// `<Name>_<level>_ShortDescription` of the powers (0x00C45930), by item and level (1, 2).
    pub power_short: HashMap<(u8, u8), String>,
}

/// The player's level (0: not learned) of the power behind a wheel item.
pub fn power_level(p: &PlayerState, item: u8) -> u8 {
    use dishonored_sim::state::power;
    let idx = match item {
        WIND_BLAST => power::WINDBLAST,
        DARK_VISION => power::DARK_VISION,
        BLINK => power::BLINK,
        DEVOURING_SWARM => power::DEVOURING_SWARM,
        BEND_TIME => power::BEND_TIME,
        POSSESSION => power::POSSESSION,
        _ => return 0,
    };
    p.inv.power_levels[idx]
}

/// What `UpdateItemInfos(name, ammo)` carries for a wheel item (0x00BCD800 picks the sender:
/// powers 0x00BC3B00, the Heart 0x00BC3E30, weapons 0x00BC3940). `level` is the player's level
/// of the power; an unlearned power sends nothing.
pub fn item_infos(t: &WheelTexts, item: u8, level: u8) -> Option<(String, Option<String>)> {
    let name = t.names.get(&item).cloned().unwrap_or_default();
    if item > FIRST_POWER_INDEX {
        if level == 0 {
            return None;
        }
        return Some((name, t.power_short.get(&(item, level)).cloned()));
    }
    let info = match item {
        HEART | SPRING_RAZOR => t.short.get(&item).cloned(),
        _ => t.ammo.get(&item).cloned(),
    };
    Some((name, info))
}

fn load_texts(content: &dishonored_assets::Content) -> WheelTexts {
    let loc = dishonored_assets::loc::Localization::load(&content.root, "INT", &["RPG"]);
    let g = |s: &str, k: &str| loc.get(s, k).map(str::to_string).unwrap_or_else(|| k.to_string());
    let mut t = WheelTexts::default();
    for (item, key) in [(HEART, "Heart_ShortDescription"), (SPRING_RAZOR, "SpringRazor_ShortDescription")] {
        if let Some(s) = loc.get("DisUISelectionType", key) {
            t.short.insert(item, s.to_string());
        }
    }
    for (item, sel) in [
        (DARK_VISION, "DarkVision"),
        (WIND_BLAST, "WindBlast"),
        (DEVOURING_SWARM, "DevouringSwarm"),
        (BLINK, "Blink"),
        (BEND_TIME, "BendTime"),
        (POSSESSION, "Possess"),
    ] {
        for level in 1..=2u8 {
            if let Some(s) = loc.get("DisUISelectionType", &format!("{sel}_{level}_ShortDescription")) {
                t.power_short.insert((item, level), s.to_string());
            }
        }
    }
    for (item, sel) in [
        (REGULAR_BOLT, "Crossbow"),
        (SLEEP_DART, "Crossbow"),
        (FLARE_BOLT, "Crossbow"),
        (HEART, "Heart"),
        (SPRING_RAZOR, "SpringRazor"),
        (REGULAR_GRENADE, "Grenade"),
        (STICKY_GRENADE, "Grenade"),
        (REGULAR_BULLET, "Pistol"),
        (EXPLOSIVE_BULLET, "Pistol"),
        (DARK_VISION, "DarkVision"),
        (WIND_BLAST, "WindBlast"),
        (DEVOURING_SWARM, "DevouringSwarm"),
        (BLINK, "Blink"),
        (BEND_TIME, "BendTime"),
        (POSSESSION, "Possess"),
    ] {
        t.names.insert(item, g("DisUISelectionType", sel));
    }
    for (item, a) in [
        (REGULAR_BOLT, "Arrow"),
        (SLEEP_DART, "Arrow_Sleep"),
        (FLARE_BOLT, "Arrow_Flare"),
        (SPRING_RAZOR, "SpringRazor"),
        (REGULAR_GRENADE, "Grenade"),
        (STICKY_GRENADE, "StickyGrenade"),
        (REGULAR_BULLET, "Bullet"),
        (EXPLOSIVE_BULLET, "Bullet_Explosive"),
    ] {
        t.ammo.insert(item, g("DisAmmoType", a));
    }
    t
}

// --- keyboard shortcuts: auto-assignment and cycling (U-882) -----------------------------------

/// `DisGFxMoviePlayerPowerWheel.m_AutoAssign_KeyboardItemOrder` (class default; the DLC06 items
/// between the powers and the sleep dart are left out: the port has no DLC06 content).
pub const AUTO_ASSIGN_ORDER: [u8; 15] = [REGULAR_BULLET, REGULAR_BOLT, REGULAR_GRENADE, SPRING_RAZOR, HEART, BLINK, DARK_VISION, WIND_BLAST, DEVOURING_SWARM, BEND_TIME, POSSESSION, SLEEP_DART, FLARE_BOLT, STICKY_GRENADE, EXPLOSIVE_BULLET];

/// What a slot accepts (`DisShortcutAutoAssignSettings`: `m_bWeapon`, `m_bPower`, `m_bHeart`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotKind {
    Weapon,
    Power,
    Heart,
}

/// `Twk_GFxMoviePlayerPowerWheel.m_AutoAssign_KeyboardShortcut[10]`: (accepts, priority).
pub const AUTO_ASSIGN_SLOTS: [(SlotKind, i32); 10] = [
    (SlotKind::Weapon, 10),
    (SlotKind::Weapon, 9),
    (SlotKind::Weapon, 8),
    (SlotKind::Weapon, 7),
    (SlotKind::Weapon, 6),
    (SlotKind::Heart, 5),
    (SlotKind::Power, 4),
    (SlotKind::Power, 3),
    (SlotKind::Power, 2),
    (SlotKind::Power, 1),
];

/// `m_KeyboardShortcuts` and what goes with it: the slots' items (0: empty), the bits of the
/// slots the player assigned or emptied himself (`m_ShortcutPlayerAssignFlags`: those are
/// never auto-filled) and the last used slot (`m_LastUsedShortcutIndex`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShortcutTable {
    pub slots: [u8; 10],
    pub player_flags: u16,
    pub last_used: usize,
}

impl ShortcutTable {
    /// `SetShortcuts` (0x00BE1310): the slots whose item is not available are emptied, then
    /// `FUN_00BDC610` fills the free ones that were not assigned by the player: the slots from
    /// the highest priority down, each taking the first item of the order that is available,
    /// not on a slot yet, and of the kind the slot accepts.
    pub fn refresh(&mut self, available: &[u8]) {
        for s in self.slots.iter_mut() {
            if *s != 0 && !available.contains(s) {
                *s = 0;
            }
        }
        let mut free: Vec<usize> = Vec::new();
        for i in 0..10 {
            if self.slots[i] == 0 && self.player_flags >> i & 1 == 0 {
                // Inserted before the first slot of a lower priority (stable for equal ones).
                let at = free.iter().position(|&j| AUTO_ASSIGN_SLOTS[j].1 < AUTO_ASSIGN_SLOTS[i].1).unwrap_or(free.len());
                free.insert(at, i);
            }
        }
        let mut candidates: Vec<u8> = AUTO_ASSIGN_ORDER.iter().copied().filter(|i| available.contains(i) && !self.slots.contains(i)).collect();
        for slot in free {
            let kind = AUTO_ASSIGN_SLOTS[slot].0;
            let fits = |item: u8| match item {
                HEART => kind == SlotKind::Heart,
                i if i > FIRST_POWER_INDEX => kind == SlotKind::Power,
                _ => kind == SlotKind::Weapon,
            };
            if let Some(k) = candidates.iter().position(|&c| fits(c)) {
                self.slots[slot] = candidates.remove(k);
            }
        }
    }

    /// `AssignShortcut` (0x00BC3400): the current item goes to the slot, or leaves it when it
    /// already holds it; the slot is the player's from then on and any other slot with the item
    /// is emptied. Returns the slots emptied that way.
    pub fn assign(&mut self, slot: usize, current: u8) -> Vec<usize> {
        let mut cleared = Vec::new();
        if slot >= 10 {
            return cleared;
        }
        let item = if self.slots[slot] == current { 0 } else { current };
        self.slots[slot] = item;
        self.player_flags |= 1 << slot;
        if item != 0 {
            for j in 0..10 {
                if j != slot && self.slots[j] == item {
                    self.slots[j] = 0;
                    cleared.push(j);
                    if self.last_used == j {
                        self.last_used = slot;
                    }
                }
            }
        }
        cleared
    }

    /// `FUN_00BBD0F0`: the slot `steps` filled slots after (before, when negative) the last used
    /// one, wrapping around; the last used one itself without steps; None when it is empty.
    pub fn cycle(&self, steps: i32) -> Option<usize> {
        let mut i = self.last_used as i32;
        if steps != 0 && self.slots.iter().any(|&s| s != 0) {
            let dir = if steps > 0 { 1 } else { -1 };
            let mut left = steps.abs();
            while left != 0 {
                i = (i + dir).rem_euclid(10);
                if self.slots[i as usize] != 0 {
                    left -= 1;
                }
            }
        }
        (self.slots[i as usize] != 0).then_some(i as usize)
    }
}

/// `DishonoredPlayerController.m_fShortcutsInputTimer` (DishonoredGame's class default): how
/// long the dock waits after the last mouse-wheel step before using the item it shows.
pub const SHORTCUTS_INPUT_TIMER: f32 = 0.33;

/// The mouse wheel stepping through the shortcuts (controller 0x00AA0A10 counts the steps and
/// restarts the timer, 0x00AB7D30 shows the dock on the first step, moves the selection on the
/// others and uses the item when the timer runs out).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelCycle {
    /// Steps so far (`+0x654`); GBA_NextItem adds one, GBA_PrevItem takes one.
    pub steps: i32,
    /// Seconds until the item is used (`+0x658`).
    pub timer: f32,
}

impl WheelCycle {
    pub fn active(&self) -> bool {
        self.timer > 0.0
    }
    /// A step of the mouse wheel.
    pub fn step(&mut self, dir: i32) {
        self.steps += dir;
        self.timer = SHORTCUTS_INPUT_TIMER;
    }
    /// Count the timer down: true when it ran out this frame (use the item).
    pub fn tick(&mut self, dt: f32) -> bool {
        if self.timer <= 0.0 {
            return false;
        }
        self.timer -= dt;
        self.timer <= 0.0
    }
}

#[derive(Default)]
struct Native {
    open: bool,
    mouse: Vec2,
    first_input: bool,
    /// `m_CurItemIdx`.
    cur_item: u8,
    /// Ammo types picked up this match (the wheel lists the gadgets once one was held).
    known_ammo: u8,
    shortcuts: ShortcutTable,
    dock_key: Option<usize>,
    /// The dock was brought up by the mouse wheel.
    cycle: WheelCycle,
    /// Seconds since the wheel closed (the controller ignores the mouse wheel for
    /// `m_fShortcutsInputTimer` after it: `m_dLastWheelCloseTime`).
    since_close: f32,
    elixirs: Option<[u8; 2]>,
    last_match: Option<(String, u32)>,
}

#[derive(Resource, Default)]
pub struct PowerWheel {
    pub movie: Option<WheelMovie>,
    view: Option<Entity>,
    texts: WheelTexts,
    /// `m_EquipmentIcons` (from the HUD's texts, copied on first use).
    icons: Option<HashMap<u8, String>>,
    native: Native,
}

impl PowerWheel {
    pub fn is_open(&self) -> bool {
        self.native.open
    }
}

pub fn setup_power_wheel(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut textures: ResMut<StageTextures>) {
    let mut pw = PowerWheel::default();
    let Ok(content) = dishonored_assets::Content::locate() else { return commands.insert_resource(pw) };
    let fonts = Fonts::load(&content);
    let Some(main) = MovieData::load(&content, "UI_PowerWheel_SF", "powerwheel", &fonts, "") else {
        warn!("power wheel movie not readable");
        return commands.insert_resource(pw);
    };
    let lib = MovieData::load(&content, "Startup", "Common_assets.lib", &fonts, "common_assets");
    textures.load_movie(&content, "UI_PowerWheel_SF", &main.movie, &mut images);
    if let Some(l) = &lib {
        textures.load_movie(&content, "Startup", &l.movie, &mut images);
    }
    let stage = Stage::new(Arc::new(main), lib.into_iter().map(Arc::new).collect());
    pw.movie = Some(WheelMovie::new(stage));
    pw.texts = load_texts(&content);
    pw.view = Some(crate::gfx_stage::spawn_view(&mut commands, 2, 0));
    commands.insert_resource(pw);
}

/// The weapons a player carries and the ammo types each lists (0x00BD5530): the pistol and the
/// crossbow always; the Spring Razor and the grenades once one was held (`known_ammo`, the
/// port keeps no weapon items).
const WEAPON_GROUPS: [(&[AmmoType], Option<u8>); 4] = [
    (&[AmmoType::Bullet, AmmoType::BulletExplosive], None),
    (&[AmmoType::Bolt, AmmoType::SleepDart, AmmoType::IncendiaryBolt], None),
    (&[AmmoType::SpringRazor], Some(1 << AmmoType::SpringRazor as u8)),
    (&[AmmoType::Grenade, AmmoType::StickyGrenade], Some(1 << AmmoType::Grenade as u8 | 1 << AmmoType::StickyGrenade as u8)),
];

/// The wheel content for a player (0x00BD5530): every ammo type of each weapon carried that
/// `FUN_00C1C9D0` lists (all but the explosive bullets and the sticky grenades, which need their
/// unlock attribute; the port also lists one it holds), with the count (an empty one shows
/// "stop_empty"); the learned powers. The Heart has no sim state and is left out.
pub fn wheel_content(p: &PlayerState, cfg: &dishonored_ir::gameplay::GameplayConfig, known_ammo: u8) -> Vec<Item> {
    let mut out = Vec::new();
    let mut listed: Vec<Item> = Vec::new();
    for (types, owned) in WEAPON_GROUPS {
        if owned.is_some_and(|mask| known_ammo & mask == 0) {
            continue;
        }
        for &t in types {
            let count = dishonored_sim::inventory::ammo(p, t) as i32;
            if !dishonored_sim::powers::ammo_unlocked(p, cfg, t) && count == 0 {
                continue;
            }
            let item = match t {
                AmmoType::Bullet => REGULAR_BULLET,
                AmmoType::BulletExplosive => EXPLOSIVE_BULLET,
                AmmoType::Bolt => REGULAR_BOLT,
                AmmoType::SleepDart => SLEEP_DART,
                AmmoType::IncendiaryBolt => FLARE_BOLT,
                AmmoType::SpringRazor => SPRING_RAZOR,
                AmmoType::Grenade => REGULAR_GRENADE,
                AmmoType::StickyGrenade => STICKY_GRENADE,
            };
            listed.push(Item { index: item, quantity: Some(count) });
        }
    }
    for item in WEAPONS_ORDER {
        out.extend(listed.iter().filter(|i| i.index == item).cloned());
    }
    use dishonored_sim::state::power;
    for (item, pw) in [(WIND_BLAST, power::WINDBLAST), (DARK_VISION, power::DARK_VISION), (BLINK, power::BLINK), (DEVOURING_SWARM, power::DEVOURING_SWARM), (BEND_TIME, power::BEND_TIME), (POSSESSION, power::POSSESSION)] {
        if p.inv.power_levels[pw] > 0 {
            out.push(Item { index: item, quantity: None });
        }
    }
    out
}

/// The angle `SetSelectionAngle` receives for a mouse position (y up), or None inside the dead
/// zone (0x00BD5250).
pub fn selection_angle(v: Vec2) -> Option<f32> {
    if v.length_squared() <= STICK_DEAD_ZONE * STICK_DEAD_ZONE {
        return None;
    }
    let n = v.normalize();
    let a = n.y.clamp(-1.0, 1.0).acos().to_degrees();
    Some(if n.x < 0.0 { 360.0 - a } else { a })
}

/// Accumulate a frame's mouse motion (pixels, y up) into the wheel cursor (0x00BCD660):
/// returns the position over the zone when this frame moved beyond the dead zone.
pub fn accumulate_mouse(pos: &mut Vec2, first: &mut bool, delta: Vec2, zone: f32) -> Option<Vec2> {
    if *first {
        *first = false;
        *pos = Vec2::ZERO;
    } else {
        *pos += delta;
        if pos.length_squared() > zone * zone {
            *pos = pos.normalize() * zone;
        }
    }
    let inv = 1.0 / zone;
    let d = delta * inv;
    (d.length_squared() > MOUSE_DEAD_ZONE * MOUSE_DEAD_ZONE).then_some(*pos * inv)
}

#[allow(clippy::too_many_arguments)]
pub fn update_power_wheel(
    mut pw: ResMut<PowerWheel>,
    mut hud: ResMut<crate::gfx_hud::GfxHud>,
    view: Res<crate::session::SessionView>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    windows: Query<&Window>,
    bindings: Option<Res<crate::settings::Bindings>>,
    pause: Option<Res<crate::pause_menu::PauseMenu>>,
    mut local: ResMut<crate::input_map::LocalInput>,
    mut textures: ResMut<StageTextures>,
    mut images: ResMut<Assets<Image>>,
    mut sounds: ResMut<crate::hud_sounds::HudSounds>,
    session: Res<crate::session::Session>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
) {
    let pw = &mut *pw;
    let Some(m) = pw.movie.as_mut() else { return };
    let n = &mut pw.native;
    let dt = time.delta_secs();
    let me = view.me.as_ref();
    let cfg = &session.content.sim.gameplay;
    let content = hud.content.clone();
    let icons = pw.icons.get_or_insert_with(|| hud.texts.equipment_icons.clone());
    let mut icon = |item: u8| -> Option<ImageRef> {
        let c = content.as_ref()?;
        let path = icons.get(&item)?;
        let (_, size) = textures.external(c, path, &mut images)?;
        Some(ImageRef { path: path.trim_start_matches("img://").to_string(), size })
    };
    // A new match: forget the known ammo and the shortcuts.
    let game = Some((view.matchv.mode.clone(), view.matchv.match_number));
    if n.last_match != game {
        n.last_match = game;
        n.known_ammo = 0;
        n.shortcuts = ShortcutTable::default();
        n.cycle = WheelCycle::default();
        n.elixirs = None;
    }
    if let Some(p) = me {
        for t in AmmoType::ALL {
            if dishonored_sim::inventory::ammo(p, t) > 0 {
                n.known_ammo |= 1 << t as u8;
            }
        }
    }
    n.since_close += dt;
    let paused = pause.as_ref().is_some_and(|p| p.open || p.overlay != 0);
    let cutscene = view.matchv.mission.as_ref().is_some_and(|mv| !mv.cutscenes.is_empty());
    let can_open = me.is_some_and(|p| p.alive) && !paused && !cutscene && !local.choice_open && local.enabled;
    let default_bindings;
    let bind = match &bindings {
        Some(b) => b.as_ref(),
        None => {
            default_bindings = crate::settings::Bindings::from_settings(&dishonored_settings::Settings::default());
            &default_bindings
        }
    };
    let held = bind.pressed("GBA_Wheel", &keys, &mouse, 0.0);

    // Harness requests.
    let mut want: Option<bool> = None;
    let mut angles = Vec::new();
    let mut dock_tests = Vec::new();
    let mut step_tests: Vec<i32> = Vec::new();
    let mut assign_tests: Vec<usize> = Vec::new();
    hud.tests.retain(|t| match t {
        crate::hud_popups::HudTest::ShortcutStep { dir } => {
            step_tests.push(*dir);
            false
        }
        crate::hud_popups::HudTest::Assign { slot } => {
            assign_tests.push(*slot);
            false
        }
        crate::hud_popups::HudTest::Wheel { open } => {
            want = Some(*open);
            false
        }
        crate::hud_popups::HudTest::WheelAngle { angle } => {
            angles.push(*angle);
            false
        }
        crate::hud_popups::HudTest::Shortcut { slot, release } => {
            dock_tests.push((*slot, *release));
            false
        }
        _ => true,
    });
    let open_now = match want {
        Some(o) => o && me.is_some(),
        None if local.override_input.is_some() => n.open,
        None => held && can_open,
    };

    // `SetShortcuts` (0x00BE1310): empty the slots of items no longer available, auto-assign the
    // free ones, and the dock's layout from what is left.
    fn dock_layout(t: &mut ShortcutTable, content: &[Item]) -> Vec<Shortcut> {
        let available: Vec<u8> = content.iter().map(|c| c.index).collect();
        t.refresh(&available);
        t.slots.iter().map(|&s| if s == 0 { Shortcut::default() } else { Shortcut { item: Some(s), quantity: content.iter().find(|c| c.index == s).and_then(|c| c.quantity) } }).collect()
    }
    // The shortcut keys and the mouse wheel use the dock, never while the wheel is open.
    // The item a slot holds is used (equipped) when the dock closes with `use`.
    let equip = |local: &mut crate::input_map::LocalInput, item: u8| {
        if let Some(h) = hand_of(item) {
            local.left_hand = h;
        }
    };

    if open_now && !n.open {
        // Start mode 1 (0x00BE4280 / 0x00BE25C0).
        n.open = true;
        n.first_input = true;
        n.mouse = Vec2::ZERO;
        n.cur_item = 0;
        // (a dock the mouse wheel brought up is gone: the wheel takes over)
        n.cycle = WheelCycle::default();
        n.dock_key = None;
        let items = me.map(|p| wheel_content(p, cfg, n.known_ammo)).unwrap_or_default();
        let el = me.map(|p| p.inv.elixirs).unwrap_or([0, 0]);
        n.elixirs = Some(el);
        let kb = |a: &str| crate::gfx_hud::convert_key_tags(&format!("`{a}`"), bindings.as_deref(), &hud.texts.keys);
        m.set_potion_bindings(kb("GBA_HealthElixir"), kb("GBA_ManaElixir"));
        m.set_potions_count(el[0] as i32, el[1] as i32);
        m.set_shortcuts(dock_layout(&mut n.shortcuts, &items), &mut icon);
        m.open_wheel(&items, &mut icon);
        sounds.play("W_show");
    } else if !open_now && n.open {
        n.open = false;
        n.since_close = 0.0;
        // Closing keeps `m_CurItemIdx` selected.
        if let Some(h) = hand_of(n.cur_item) {
            local.left_hand = h;
        }
        m.close_wheel();
        sounds.play("W_hide");
    }
    local.wheel_open = n.open;

    if n.open {
        // Mouse selection (0x00BD5250).
        let zone = windows.iter().next().map(|w| w.physical_height() as f32).unwrap_or(720.0) * MOUSE_ZONE_SIZE;
        let d = motion.delta;
        if let Some(v) = accumulate_mouse(&mut n.mouse, &mut n.first_input, Vec2::new(d.x, -d.y), zone)
            && let Some(a) = selection_angle(v)
        {
            m.set_selection_angle(a);
        }
        for a in angles.drain(..) {
            m.set_selection_angle(a);
        }
        // Number keys assign the selection to a dock slot (`AssignShortcut`, 0x00BC3400).
        let mut assigns = std::mem::take(&mut assign_tests);
        if local.override_input.is_none() {
            assigns.extend((0..10).filter(|i| bind.just_pressed(&format!("GBA_Shortcut_{i}"), &keys, &mouse, 0.0)));
        }
        for i in assigns {
            if i >= 10 {
                continue;
            }
            m.assign_shortcut(i, &mut icon);
            let cleared = n.shortcuts.assign(i, n.cur_item);
            for j in cleared {
                m.clear_shortcut(j, &mut icon);
            }
        }
        // Elixirs drunk while open: the counters and the consumption flash.
        if let Some(p) = me {
            let el = p.inv.elixirs;
            if let Some(prev) = n.elixirs
                && el != prev
            {
                m.set_potions_count(el[0] as i32, el[1] as i32);
                if el[0] < prev[0] {
                    m.play_potion_consumption(false);
                }
                if el[1] < prev[1] {
                    m.play_potion_consumption(true);
                }
            }
            n.elixirs = Some(el);
        }
    } else {
        // The dock alone on a number key (start mode 2), used and closed on release
        // (controller 0x00AA0900 / 0x00AA0950, 0x00AB7D30).
        let mut presses: Vec<(usize, bool)> = dock_tests;
        let live = local.override_input.is_none() && can_open;
        if live {
            for i in 0..10 {
                let a = format!("GBA_Shortcut_{i}");
                // (not while the mouse wheel's timer runs: `m_fRelativeShortcutTimer`)
                if bind.just_pressed(&a, &keys, &mouse, 0.0) && !n.cycle.active() {
                    presses.push((i, false));
                }
                if n.dock_key == Some(i) && !bind.pressed(&a, &keys, &mouse, 0.0) {
                    presses.push((i, true));
                }
            }
        }
        for (i, release) in presses {
            if !release {
                let items = me.map(|p| wheel_content(p, cfg, n.known_ammo)).unwrap_or_default();
                m.set_shortcuts(dock_layout(&mut n.shortcuts, &items), &mut icon);
                m.open_shortcuts();
                m.set_shortcut_selection(i as i32);
                n.dock_key = Some(i);
            } else if n.dock_key == Some(i) {
                // `KeyboardUse`: the slot's item becomes the current one (0x00BBD090) and is
                // equipped as the dock closes.
                let item = n.shortcuts.slots[i];
                n.shortcuts.last_used = i;
                if item != 0 {
                    equip(&mut local, item);
                }
                m.close_shortcuts();
                n.dock_key = None;
            }
        }
        // The mouse wheel (`Dis_WheelShortcuts_MouseNext / MousePrevious`): not while a key is
        // held, not within `m_fShortcutsInputTimer` of the wheel closing.
        let mut steps: Vec<i32> = std::mem::take(&mut step_tests);
        if live && n.dock_key.is_none() {
            let w = scroll.delta.y;
            if bind.just_pressed("GBA_NextItem", &keys, &mouse, w) {
                steps.push(1);
            }
            if bind.just_pressed("GBA_PrevItem", &keys, &mouse, w) {
                steps.push(-1);
            }
        }
        if n.dock_key.is_none() && can_open {
            for dir in steps {
                if n.since_close < SHORTCUTS_INPUT_TIMER {
                    continue;
                }
                let first = !n.cycle.active();
                if first {
                    n.cycle = WheelCycle::default();
                }
                n.cycle.step(dir);
                if first {
                    let items = me.map(|p| wheel_content(p, cfg, n.known_ammo)).unwrap_or_default();
                    m.set_shortcuts(dock_layout(&mut n.shortcuts, &items), &mut icon);
                    m.open_shortcuts();
                }
                if let Some(s) = n.shortcuts.cycle(n.cycle.steps) {
                    m.set_shortcut_selection(s as i32);
                }
            }
            if n.cycle.tick(dt) {
                // Still for the timer: the shown slot is used and the dock closes.
                if let Some(s) = n.shortcuts.cycle(n.cycle.steps) {
                    n.shortcuts.last_used = s;
                    equip(&mut local, n.shortcuts.slots[s]);
                }
                n.cycle = WheelCycle::default();
                m.close_shortcuts();
            }
        }
    }

    m.run(dt);
    // The movie's calls to the native side.
    for cb in std::mem::take(&mut m.callbacks) {
        match cb {
            Callback::SelectionChanged(item) => {
                n.cur_item = item;
                if item != 0 {
                    let level = me.map(|p| power_level(p, item)).unwrap_or(0);
                    if let Some((name, info)) = item_infos(&pw.texts, item, level) {
                        m.update_item_infos(&name, info.as_deref());
                    }
                }
            }
            Callback::Closed => {}
        }
    }
    for s in m.sounds.drain(..) {
        sounds.play(s);
    }
}

/// Draw the wheel stage into its UI entities.
#[allow(clippy::type_complexity)]
pub fn draw_power_wheel(
    mut commands: Commands,
    mut pw: ResMut<PowerWheel>,
    textures: Res<StageTextures>,
    fonts: Option<Res<crate::gfx_hud::GameFonts>>,
    windows: Query<&Window>,
    mut views: Query<&mut StageView>,
    mut images: Query<(&mut ImageNode, &mut Node, &mut UiTransform, &mut Visibility), (With<SlotImage>, Without<SlotText>)>,
    mut texts: Query<(&mut Text, &mut TextFont, &mut TextColor, &mut TextLayout, &mut LineHeight, &mut LetterSpacing, &mut Node, &mut UiTransform, &mut Visibility), (With<SlotText>, Without<SlotImage>)>,
) {
    let pw = &mut *pw;
    let Some(window) = windows.iter().next() else { return };
    let size = Vec2::new(window.width(), window.height());
    let (Some(m), Some(root)) = (pw.movie.as_mut(), pw.view) else { return };
    let Ok(mut view) = views.get_mut(root) else { return };
    m.stage.draw(&mut m.draws);
    crate::gfx_stage::sync_view(&mut commands, root, &mut view, &m.draws, &textures, fonts.as_deref(), size, &mut images, &mut texts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_ranges() {
        assert!(is_angle_in_range(10.0, 350.0, 20.0));
        assert!(is_angle_in_range(355.0, 350.0, 20.0));
        assert!(!is_angle_in_range(30.0, 350.0, 20.0));
        assert!(is_angle_in_range(90.0, 80.0, 100.0));
        assert!(!is_angle_in_range(101.0, 80.0, 100.0));
    }

    #[test]
    fn mouse_angle() {
        assert_eq!(selection_angle(Vec2::new(0.0, 0.3)), None);
        assert!((selection_angle(Vec2::new(0.0, 1.0)).unwrap() - 0.0).abs() < 1e-3);
        assert!((selection_angle(Vec2::new(1.0, 0.0)).unwrap() - 90.0).abs() < 1e-3);
        assert!((selection_angle(Vec2::new(0.0, -1.0)).unwrap() - 180.0).abs() < 1e-3);
        assert!((selection_angle(Vec2::new(-1.0, 0.0)).unwrap() - 270.0).abs() < 1e-3);
    }

    #[test]
    fn mouse_zone() {
        let (mut pos, mut first) = (Vec2::ZERO, true);
        // The first input after opening resets the cursor.
        assert_eq!(accumulate_mouse(&mut pos, &mut first, Vec2::new(50.0, 0.0), 72.0), Some(Vec2::ZERO));
        // Clamped to the zone circle.
        let v = accumulate_mouse(&mut pos, &mut first, Vec2::new(500.0, 0.0), 72.0).unwrap();
        assert!((v.length() - 1.0).abs() < 1e-4);
        // Below the dead zone: nothing is sent.
        assert_eq!(accumulate_mouse(&mut pos, &mut first, Vec2::new(0.5, 0.0), 72.0), None);
    }

    #[test]
    fn auto_assign_follows_the_item_order_and_slot_kinds() {
        let all = [REGULAR_BULLET, REGULAR_BOLT, REGULAR_GRENADE, SPRING_RAZOR, SLEEP_DART, HEART, BLINK, DARK_VISION, WIND_BLAST, DEVOURING_SWARM, BEND_TIME, POSSESSION];
        let mut t = ShortcutTable::default();
        t.refresh(&all);
        assert_eq!(t.slots, [REGULAR_BULLET, REGULAR_BOLT, REGULAR_GRENADE, SPRING_RAZOR, SLEEP_DART, HEART, BLINK, DARK_VISION, WIND_BLAST, DEVOURING_SWARM]);
        // Without the Heart its slot stays empty (it takes nothing else); the powers fill from 7.
        let mut t = ShortcutTable::default();
        t.refresh(&[REGULAR_BULLET, REGULAR_BOLT, BLINK, WIND_BLAST]);
        assert_eq!(t.slots, [REGULAR_BULLET, REGULAR_BOLT, 0, 0, 0, 0, BLINK, WIND_BLAST, 0, 0]);
        // A new item takes the best free slot of its kind; an item gone empties its slot.
        t.refresh(&[REGULAR_BULLET, REGULAR_BOLT, BLINK, WIND_BLAST, REGULAR_GRENADE, HEART]);
        assert_eq!(t.slots, [REGULAR_BULLET, REGULAR_BOLT, REGULAR_GRENADE, 0, 0, HEART, BLINK, WIND_BLAST, 0, 0]);
        t.refresh(&[REGULAR_BULLET, BLINK]);
        assert_eq!(t.slots, [REGULAR_BULLET, 0, 0, 0, 0, 0, BLINK, 0, 0, 0]);
    }

    #[test]
    fn assigned_slots_belong_to_the_player() {
        let mut t = ShortcutTable::default();
        t.refresh(&[REGULAR_BULLET, REGULAR_BOLT, BLINK]);
        // Blink to slot 0 (weapon slot): the player may; slot 6 (its old one) is emptied.
        assert_eq!(t.assign(0, BLINK), vec![6]);
        assert_eq!(t.slots[0], BLINK);
        // Assigning the same item again empties the slot, and it is not refilled.
        assert!(t.assign(0, BLINK).is_empty());
        assert_eq!(t.slots[0], 0);
        t.refresh(&[REGULAR_BULLET, REGULAR_BOLT, BLINK]);
        assert_eq!(t.slots[0], 0);
        // (Blink is now nowhere: the free power slot 6 is not the player's, so it takes it back)
        assert_eq!(t.slots[6], BLINK);
    }

    #[test]
    fn mouse_wheel_cycles_filled_slots() {
        let mut t = ShortcutTable { slots: [8, 0, 6, 0, 0, 0, 24, 0, 0, 0], ..Default::default() };
        assert_eq!(t.cycle(0), Some(0));
        assert_eq!(t.cycle(1), Some(2));
        assert_eq!(t.cycle(2), Some(6));
        assert_eq!(t.cycle(3), Some(0));
        assert_eq!(t.cycle(-1), Some(6));
        t.last_used = 6;
        assert_eq!(t.cycle(1), Some(0));
        t.last_used = 1;
        assert_eq!(t.cycle(0), None);
        assert_eq!(t.cycle(1), Some(2));
        let mut c = WheelCycle::default();
        c.step(1);
        c.step(1);
        assert_eq!(c.steps, 2);
        assert!(!c.tick(0.2));
        assert!(c.tick(0.2));
        assert!(!c.active());
    }

    #[test]
    fn item_texts_follow_the_sender() {
        let mut t = WheelTexts::default();
        t.names.insert(BLINK, "Blink".into());
        t.power_short.insert((BLINK, 2), "Short Teleport".into());
        t.names.insert(HEART, "The Heart".into());
        t.short.insert(HEART, "Reveal Runes & Bone Charms".into());
        t.names.insert(REGULAR_BOLT, "Crossbow".into());
        t.ammo.insert(REGULAR_BOLT, "Crossbow Bolt".into());
        assert_eq!(item_infos(&t, BLINK, 2), Some(("Blink".into(), Some("Short Teleport".into()))));
        assert_eq!(item_infos(&t, BLINK, 0), None);
        assert_eq!(item_infos(&t, HEART, 0), Some(("The Heart".into(), Some("Reveal Runes & Bone Charms".into()))));
        assert_eq!(item_infos(&t, REGULAR_BOLT, 0), Some(("Crossbow".into(), Some("Crossbow Bolt".into()))));
    }

    #[test]
    fn hands() {
        assert_eq!(hand_of(BLINK), Some(LeftHand::Blink));
        assert_eq!(hand_of(HEART), None);
        assert_eq!(schema_of(SLEEP_DART), "crossbow");
        assert_eq!(schema_of(POSSESSION), "outsider");
    }
}
