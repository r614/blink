//! The saving notification: the `UI_Global.Global` movie's `savingNotificationIc_mc` (class
//! `g_SavingNotification_Icon`: three circles, three gears and three blades opening around a
//! rotating clockwork) at the bottom-right corner, played on a [`gfx_stage::Stage`] the way
//! `DisGFxMoviePlayerGlobal` drives it. See research/dishonored/HUD.md "Saving notification".
//!
//! Native side (decoded from `Dishonored.exe`):
//! - the engine tick (0x00A17890) watches the game save: when one starts, 0x00BAA390 sets
//!   `m_GameSaveState` 2, starts `m_fGameSaveTimer` at `m_fMinSaveUIDuration` (1.0 s) and calls
//!   `ShowSavingNotification` on the icon (0x00BA4D30) with the checkpoint text closed
//!   (0x00BA4ED0); when it ends (0x00BAA3D0) the state drops to 1 while the timer runs, and
//!   the movie tick (0x00BAA610) closes the icon once the timer is out;
//! - `ShowSavingNotification(bInMenu)` places the clip at `GetMovieSpaceCoord(0.85).coordMax`.
//! The port's saves are written at once, so the icon shows for the minimum 1 s.
//!
//! The rest of the Global movie (message boxes, the mouse cursor, its black stripes) is not
//! played here.

use crate::gfx_stage::{Draw, Ease, Event, Fonts, Id, MovieData, Prop, SlotImage, SlotText, Stage, StageTextures, StageView};
use bevy::prelude::*;
use bevy::text::{LetterSpacing, LineHeight};
use std::sync::Arc;

/// `DisGFxMoviePlayerGlobal.m_fMinSaveUIDuration`.
pub const MIN_SAVE_UI_DURATION: f32 = 1.0;

const C_OPEN_CIRCLES: u16 = 1;
const C_ON_OPENED: u16 = 2;
const C_START_GEARS: u16 = 3;
const C_ROTATION: u16 = 4; // arg: animation index

fn key(code: u16, arg: u32) -> u64 {
    crate::hud_widgets::key(31, code, arg)
}

/// `_utils.RotationAnimation`: a clip turning a full circle in `duration`, again and again.
#[derive(Clone, Copy)]
struct Rotation {
    target: Option<Id>,
    duration: f32,
    clockwise: bool,
    interval: Option<u32>,
}

impl Rotation {
    fn none() -> Self {
        Self { target: None, duration: 0.0, clockwise: true, interval: None }
    }
    /// `StartRotationAnimation(targetMc, rotationDuration, bClockwise, bLoopAnimation, easeNone)`.
    fn start(&mut self, st: &mut Stage, target: Option<Id>, duration: f32, clockwise: bool, idx: u32) {
        self.target = target;
        self.duration = duration;
        self.clockwise = clockwise;
        self.play(st, idx);
    }
    /// `PlayRotationAnimation`: tween to +-360 degrees; loop after `duration`.
    fn play(&mut self, st: &mut Stage, idx: u32) {
        if let Some(i) = self.interval.take() {
            st.clear_interval(i);
        }
        let Some(t) = self.target else { return };
        st.tween_end(t, false);
        st.tween_to(t, self.duration, &[(Prop::Rotation, if self.clockwise { 360.0 } else { -360.0 })], Ease::Linear, None);
        self.interval = Some(st.set_interval(self.duration * 1000.0, key(C_ROTATION, idx)));
    }
    /// `StopRotationAnimation`.
    fn stop(&mut self, st: &mut Stage) {
        if let Some(i) = self.interval.take() {
            st.clear_interval(i);
        }
        if let Some(t) = self.target {
            st.tween_end(t, false);
        }
    }
}

/// `g_SavingNotification_Icon` (over `g_SavingNotification`).
pub struct SavingIcon {
    pub mc: Id,
    icon: Option<Id>,
    gears: Option<Id>,
    gear: [Option<Id>; 3],
    circle_i: Option<Id>,
    circle_m: Option<Id>,
    circle_e: Option<Id>,
    blade: [Option<Id>; 3],
    /// `_circleI_RAnim`, `_circleM_RAnim`, `_circleE_RAnim`, `_gears_RAnim`, `_gear0..2_RAnim`.
    anims: [Rotation; 7],
    itvl_open: Option<u32>,
    itvl_g: Option<u32>,
    pub shown: bool,
}

impl SavingIcon {
    fn new(st: &mut Stage, mc: Id) -> Self {
        st.set_visible(mc, false);
        Self { mc, icon: None, gears: None, gear: [None; 3], circle_i: None, circle_m: None, circle_e: None, blade: [None; 3], anims: [Rotation::none(); 7], itvl_open: None, itvl_g: None, shown: false }
    }

    fn clear_intervals(&mut self, st: &mut Stage) {
        for i in [self.itvl_g.take(), self.itvl_open.take()].into_iter().flatten() {
            st.clear_interval(i);
        }
    }

    /// `ShowSavingNotification(bInMenu)`: a fresh `gl_sIcon_ic` at (-50, -45), `InitIcon`,
    /// then the base class's `SetPosition` (`GetMovieSpaceCoord(0.85).coordMax`) and `Open`.
    pub fn show(&mut self, st: &mut Stage) {
        if let Some(old) = self.icon.take() {
            st.remove(old);
        }
        for a in self.anims.iter_mut() {
            a.stop(st);
        }
        let d = st.next_depth(self.mc);
        self.icon = st.attach(self.mc, "gl_sIcon_ic", "_icon_mc", d);
        if let Some(i) = self.icon {
            st.set(i, Prop::X, -50.0);
            st.set(i, Prop::Y, -45.0);
        }
        self.init_icon(st);
        let off = crate::gfx_stage::STAGE * (1.0 - 0.85) * 0.5;
        let max = crate::gfx_stage::STAGE - off;
        st.set(self.mc, Prop::X, max.x);
        st.set(self.mc, Prop::Y, max.y);
        self.shown = true;
        self.open(st);
    }

    /// `InitIcon`: every piece hidden.
    fn init_icon(&mut self, st: &mut Stage) {
        let Some(i) = self.icon else { return };
        self.gears = st.child(i, "gears_mc");
        self.gear = [0, 1, 2].map(|k| self.gears.and_then(|g| st.child(g, &format!("gear{k}_mc"))));
        self.circle_i = st.child(i, "circleI_mc");
        self.circle_m = st.child(i, "circleM_mc");
        self.circle_e = st.child(i, "circleE_mc");
        self.blade = [0, 1, 2].map(|k| st.child(i, &format!("blade{k}_mc")));
        for id in [self.circle_i, self.circle_m, self.circle_e].into_iter().chain(self.gear).chain(self.blade).flatten() {
            st.set_visible(id, false);
        }
    }

    /// `Open`: the clip shown, the outer circle and the clockwork in, `OpenGears`, and the
    /// inner circles 50 ms later.
    fn open(&mut self, st: &mut Stage) {
        st.set_visible(self.mc, true);
        st.set(self.mc, Prop::Alpha, 100.0);
        st.tween_end(self.mc, false);
        if let Some(c) = self.circle_e {
            st.set_visible(c, true);
            st.tween_from(c, 0.25, &[(Prop::Alpha, 0.0), (Prop::XScale, 150.0), (Prop::YScale, 150.0)], Ease::BackOut, None);
        }
        if let Some(g) = self.gears {
            st.tween_from(g, 0.3, &[(Prop::Alpha, 0.0), (Prop::XScale, 130.0), (Prop::YScale, 130.0)], Ease::StrongOut, None);
        }
        self.open_gears(st);
        self.itvl_open = Some(st.set_interval(50.0, key(C_OPEN_CIRCLES, 0)));
    }

    /// `OpenCircles`.
    fn open_circles(&mut self, st: &mut Stage) {
        if let Some(i) = self.itvl_open.take() {
            st.clear_interval(i);
        }
        if let Some(c) = self.circle_i {
            st.set_visible(c, true);
            st.tween_from(c, 0.3, &[(Prop::Alpha, 0.0), (Prop::XScale, 20.0), (Prop::YScale, 20.0)], Ease::BackOut, None);
        }
        if let Some(c) = self.circle_m {
            st.set_visible(c, true);
            st.tween_from(c, 0.3, &[(Prop::Alpha, 0.0), (Prop::XScale, 120.0), (Prop::YScale, 120.0)], Ease::BackOut, None);
        }
        self.itvl_open = Some(st.set_interval(300.0, key(C_ON_OPENED, 0)));
    }

    /// `OpenGears`: the blades, then the three gears sliding in; their rotation after 0.3 s.
    fn open_gears(&mut self, st: &mut Stage) {
        self.clear_intervals(st);
        self.open_blades(st);
        for (k, (dx, dy)) in [(-20.0, -20.0), (25.0, 25.0), (-25.0, 25.0)].into_iter().enumerate() {
            if let Some(g) = self.gear[k] {
                st.set_visible(g, true);
                let (x, y) = (st.get(g, Prop::X), st.get(g, Prop::Y));
                st.tween_from(g, 0.3, &[(Prop::Alpha, 0.0), (Prop::X, x + dx), (Prop::Y, y + dy)], Ease::StrongOut, None);
            }
        }
        self.itvl_g = Some(st.set_interval(300.0, key(C_START_GEARS, 0)));
    }

    /// `StartGearsRotation`: the clockwork 10 s a turn, gear 0 2.5 s, gear 1 10 s, gear 2 3.5 s
    /// the other way.
    fn start_gears_rotation(&mut self, st: &mut Stage) {
        if let Some(i) = self.itvl_g.take() {
            st.clear_interval(i);
        }
        let (gears, g) = (self.gears, self.gear);
        self.anims[3].start(st, gears, 10.0, true, 3);
        self.anims[4].start(st, g[0], 2.5, true, 4);
        self.anims[5].start(st, g[1], 10.0, true, 5);
        self.anims[6].start(st, g[2], 3.5, false, 6);
    }

    /// `OpenBlades`.
    fn open_blades(&mut self, st: &mut Stage) {
        for (k, dx) in [-20.0, -20.0, 20.0].into_iter().enumerate() {
            if let Some(b) = self.blade[k] {
                st.set_visible(b, true);
                let x = st.get(b, Prop::X);
                st.tween_from(b, 0.25, &[(Prop::Alpha, 0.0), (Prop::X, x + dx)], Ease::StrongOut, None);
            }
        }
    }

    /// `OnOpened`: the circles turn (inner 10 s the other way, middle 10 s, outer 35 s).
    fn on_opened(&mut self, st: &mut Stage) {
        self.clear_intervals(st);
        let (ci, cm, ce) = (self.circle_i, self.circle_m, self.circle_e);
        self.anims[0].start(st, ci, 10.0, false, 0);
        self.anims[1].start(st, cm, 10.0, true, 1);
        self.anims[2].start(st, ce, 35.0, true, 2);
    }

    /// `Close`: the clip fades over 0.45 s, the clockwork and the circles shrink away.
    pub fn close(&mut self, st: &mut Stage) {
        self.shown = false;
        st.tween_to(self.mc, 0.45, &[(Prop::Alpha, 0.0)], Ease::StrongOut, None);
        self.anims[3].stop(st);
        if let Some(g) = self.gears {
            st.tween_to(g, 0.5, &[(Prop::Alpha, 0.0), (Prop::XScale, 120.0), (Prop::YScale, 120.0)], Ease::StrongOut, None);
        }
        self.anims[0].stop(st);
        if let Some(c) = self.circle_i {
            st.tween_to(c, 0.3, &[(Prop::Alpha, 0.0), (Prop::XScale, 120.0), (Prop::YScale, 120.0)], Ease::StrongOut, None);
        }
        self.anims[1].stop(st);
        self.anims[2].stop(st);
        if let Some(c) = self.circle_e {
            let r = self.circle_i.map(|i| st.get(i, Prop::Rotation)).unwrap_or(0.0) - 10.0;
            st.tween_to(c, 0.25, &[(Prop::Rotation, r), (Prop::Alpha, 0.0), (Prop::XScale, 60.0), (Prop::YScale, 60.0)], Ease::StrongOut, None);
        }
    }

    fn on_event(&mut self, st: &mut Stage, code: u16, arg: u32) {
        match code {
            C_OPEN_CIRCLES => self.open_circles(st),
            C_ON_OPENED => self.on_opened(st),
            C_START_GEARS => self.start_gears_rotation(st),
            C_ROTATION => {
                if let Some(a) = self.anims.get_mut(arg as usize) {
                    a.play(st, arg);
                }
            }
            _ => {}
        }
    }
}

/// The native save UI state (`m_GameSaveState`: 2 saving, 1 done but within the minimum time).
#[derive(Default)]
struct SaveUi {
    state: u8,
    timer: f32,
    written: u32,
}

#[derive(Resource, Default)]
pub struct GlobalMovie {
    stage: Option<Stage>,
    icon: Option<SavingIcon>,
    view: Option<Entity>,
    save: SaveUi,
    draws: Vec<Draw>,
}

pub fn setup_global_movie(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut textures: ResMut<StageTextures>) {
    let mut g = GlobalMovie::default();
    let Ok(content) = dishonored_assets::Content::locate() else { return commands.insert_resource(g) };
    let fonts = Fonts::load(&content);
    let Some(main) = MovieData::load(&content, "DishonoredGame", "UI_Global.Global", &fonts, "") else {
        warn!("global movie not readable");
        return commands.insert_resource(g);
    };
    textures.load_movie(&content, "DishonoredGame", &main.movie, &mut images);
    let mut st = Stage::new(Arc::new(main), Vec::new());
    let root = st.root;
    // Only the saving notification is played from this movie.
    for n in ["msgBox_mc", "mouseCursor_mc", "blackStripes_mc", "savingNotificationTxt_mc"] {
        if let Some(m) = st.child(root, n) {
            st.set_visible(m, false);
        }
    }
    g.icon = st.child(root, "savingNotificationIc_mc").map(|m| SavingIcon::new(&mut st, m));
    g.stage = Some(st);
    g.view = Some(crate::gfx_stage::spawn_view(&mut commands, 3, 1));
    commands.insert_resource(g);
}

/// The save UI (0x00BAA390 / 0x00BAA3D0 / 0x00BAA610) from the saves the game wrote.
pub fn update_global_movie(mut g: ResMut<GlobalMovie>, saves: Option<Res<crate::saves::Saves>>, time: Res<Time>, mut hud: ResMut<crate::gfx_hud::GfxHud>) {
    let g = &mut *g;
    let (Some(st), Some(icon)) = (g.stage.as_mut(), g.icon.as_mut()) else { return };
    let dt = time.delta_secs();
    let mut started = 0;
    hud.tests.retain(|t| {
        if matches!(t, crate::hud_popups::HudTest::Saving) {
            started += 1;
            false
        } else {
            true
        }
    });
    let written = saves.as_ref().map(|s| s.written).unwrap_or(0);
    if written != g.save.written {
        g.save.written = written;
        started += 1;
    }
    if started > 0 {
        // A save started: the icon (0x00BAA390), then (written at once) it ended (0x00BAA3D0):
        // shown until the minimum time is out.
        g.save.state = 2;
        g.save.timer = MIN_SAVE_UI_DURATION;
        if !icon.shown {
            icon.show(st);
        }
        g.save.state = 1;
    }
    if g.save.state != 0 {
        g.save.timer -= dt;
        if g.save.state == 1 && g.save.timer <= 0.0 {
            g.save.state = 0;
            icon.close(st);
        }
    }
    let until = st.time + dt as f64;
    while let Some(ev) = st.poll(until) {
        let k = match ev {
            Event::Timer(k) | Event::TweenDone(k) => k,
        };
        icon.on_event(st, ((k >> 32) & 0xffff) as u16, k as u32);
    }
}

#[allow(clippy::type_complexity)]
pub fn draw_global_movie(
    mut commands: Commands,
    mut g: ResMut<GlobalMovie>,
    textures: Res<StageTextures>,
    fonts: Option<Res<crate::gfx_hud::GameFonts>>,
    windows: Query<&Window>,
    mut views: Query<&mut StageView>,
    mut images: Query<(&mut ImageNode, &mut Node, &mut UiTransform, &mut Visibility), (With<SlotImage>, Without<SlotText>)>,
    mut texts: Query<(&mut Text, &mut TextFont, &mut TextColor, &mut TextLayout, &mut LineHeight, &mut LetterSpacing, &mut Node, &mut UiTransform, &mut Visibility), (With<SlotText>, Without<SlotImage>)>,
) {
    let g = &mut *g;
    let Some(window) = windows.iter().next() else { return };
    let size = Vec2::new(window.width(), window.height());
    let (Some(st), Some(root)) = (g.stage.as_ref(), g.view) else { return };
    let Ok(mut view) = views.get_mut(root) else { return };
    st.draw(&mut g.draws);
    crate::gfx_stage::sync_view(&mut commands, root, &mut view, &g.draws, &textures, fonts.as_deref(), size, &mut images, &mut texts);
}
