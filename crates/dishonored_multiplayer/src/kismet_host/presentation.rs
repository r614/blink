//! Kismet presentation the clients draw: the highlight of usables, the player camera's rain
//! emitter and the camera fade.
//!
//! - `DisSeqAct_Highlight::Activated` (0x00B8F4C0): for every target implementing
//!   `DisInteractableInterface`, input "Highlight" sets bit 2 of the interactable's highlight
//!   mask (0x00A30710), "UnHighlight" clears it (0x00A30740). The mask going from 0 to non-zero
//!   applies the highlight (0x00A27FF0): when the HUD's `m_Settings.m_bShowHighlight` is on
//!   (0x00BBF730 / 0x00B87E60) and the interactable tweak (`m_pInteractableTweaks`, else the
//!   class defaults) has `m_bAllowedToHighlight` and a `m_HighLightMaterial`, interface slot 22
//!   adds a copy of the actor's mesh component (no shadows, `DisTranslucencySortPriority` 5) with
//!   that material on element 0 (usables 0x00A1C920: a `SkeletalMeshComponent` following the
//!   actor's; pickups and keys 0x00A1D8C0: a `StaticMeshComponent`). The mask going back to 0
//!   removes it (slot 23: 0x00A18B80 / 0x00A19BC0). Bit 0 is the crosshair focus' (0x00A307E0 /
//!   0x00A30810), not the Kismet's. The host keeps the Kismet bit per actor; the HUD setting, the
//!   tweak and the material are the clients' (`dishonored_runtime::highlight`).
//! - `DisSeqAct_SetRainEmitter::Activated` (0x00BA1770 -> 0x00B9ADF0): the first `Target` that
//!   is an `Emitter` whose template's first module is a `DisParticleModuleRainDrops` becomes the
//!   player camera's rain box emitter (`DishonoredPlayerCamera` 0x00AC0570: the previous one is
//!   deactivated, the new one activated; `m_NumRainDrops`, `m_fRainImpactsMinDist` / `MaxDist`
//!   and `m_RainDirection` (the `m_pRainRotationActor`'s rotation vector negated, else
//!   (0, 0, -1)) are copied), and the action caches the camera. Every camera tick with a rain
//!   emitter (0x00AD80A0) tests whether the view is under the open sky (`m_bWasUncovered`): not
//!   when the view target pawn is in a water volume (`m_pActiveWaterVolume`), else a line check
//!   (flags 0x2086) from the view location jittered by +-35 uu in X and Y to 5000 uu against
//!   the rain direction hits nothing. The latent update (0x00B85A30, never finished) fires "Rain
//!   Start" once the view has been uncovered for `m_fRainStartDelay` seconds and "Rain Stop" when
//!   it is covered again after a start. The emitter itself (the rain box before the camera) is
//!   the clients' (`dishonored_runtime::rain`).
//! - `SeqAct_CameraFade::Activated` (0x006E6960) calls `ClientSetCameraFade(true, FadeColor,
//!   (camera FadeAmount, FadeOpacity), FadeTime)` on the target controllers (all of them without
//!   targets); the update (0x006E6DE0) calls `ClientSetCameraFade(false)` when its time runs out
//!   unless `bPersistFade` with a non-zero `FadeOpacity`. The camera (`Camera::UpdateCamera`,
//!   0x005E5EB0) runs `FadeTimeRemaining` down and lerps `FadeAmount` from `FadeAlpha.X` to `.Y`;
//!   disabling sets `FadeAmount` to `FadeAlpha.Y`. The Kismet wraps its teleports in these fades
//!   (e.g. Flooded_FAssassins, Brothel, Overseer, Isle: fade to black, "Finished" -> Teleport,
//!   fade back in). One camera for all the players.

use super::H;
use crate::{CameraFadeView, HighlightView, RainView};
use dishonored_kismet::runtime::Ctx;
use dishonored_kismet::{Obj, OpId, Value};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Bit of the interactable highlight mask that the Kismet sets (`FUN_00a30710(2)`).
pub const KISMET_HIGHLIGHT_BIT: u32 = 2;

/// The host's side of the presentation actions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Presentation {
    /// Level actors with the Kismet bit of their highlight mask set.
    pub highlighted: BTreeSet<u32>,
    /// The camera's rain (`m_pRainBoxEmitter` and the copied settings), once an action set it.
    pub rain: Option<Rain>,
    /// `DishonoredPlayerCamera.m_bWasUncovered` (+0x4c0 bit 4).
    pub uncovered: bool,
    /// `m_fStartTimer` of the `SetRainEmitter` actions that cached the camera.
    pub rain_timers: BTreeMap<OpId, f32>,
    /// MSVC `rand` state of the cover test's jitter (`appFrand`).
    pub rand: u32,
    /// The camera fade (`Camera` `bEnableFading` and its fade fields).
    pub fade: Fade,
}

/// The camera's rain settings (`DishonoredPlayerCamera` +0x4dc..+0x51c).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Rain {
    /// Bumped by every `SetRainEmitter`.
    pub id: u32,
    /// `m_pRainBoxEmitter` (none: the action found no rain emitter among its targets).
    pub emitter: Option<u32>,
    pub drops: i32,
    pub impacts_min: f32,
    pub impacts_max: f32,
    pub direction: [f32; 3],
    /// The rotation actor's rotation (the emitter's `Rotation`).
    #[serde(default)]
    pub rotation: Option<[i32; 3]>,
}

/// The camera's fade fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Fade {
    /// `bEnableFading`.
    pub enabled: bool,
    pub id: u32,
    /// `FadeColor` (RGBA bytes).
    pub color: [u8; 4],
    /// `FadeAlpha`.
    pub alpha: [f32; 2],
    pub time: f32,
    pub remaining: f32,
    /// `FadeAmount`.
    pub amount: f32,
}

impl Fade {
    /// `PlayerController.ClientSetCameraFade(true, color, (FadeAmount, opacity), time)`.
    pub fn start(&mut self, color: [u8; 4], opacity: f32, time: f32) {
        self.enabled = true;
        self.id = self.id.wrapping_add(1);
        self.color = color;
        self.alpha = [self.amount, opacity];
        self.time = time;
        self.remaining = time;
    }

    /// `ClientSetCameraFade(false)`.
    pub fn clear(&mut self) {
        self.enabled = false;
        self.amount = self.alpha[1];
    }

    /// The camera's per-tick update (`Camera::UpdateCamera`).
    pub fn step(&mut self, dt: f32) {
        if !self.enabled {
            return;
        }
        let mut v = self.view();
        v.advance(dt);
        self.remaining = v.remaining;
        self.amount = v.amount;
    }

    /// What the clients draw (None: fading disabled).
    pub fn view(&self) -> CameraFadeView {
        CameraFadeView { id: self.id, color: [0, 1, 2].map(|i| self.color[i] as f32 / 255.0), alpha: self.alpha, time: self.time, remaining: self.remaining, amount: self.amount }
    }
}

/// UE3 `FRotator::Vector` of a rotator in rotation units.
pub fn rotator_vector(r: [i32; 3]) -> Vec3 {
    let a = |u: i32| (u as f32) * std::f32::consts::TAU / 65536.0;
    let (p, y) = (a(r[0]), a(r[1]));
    Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), p.sin())
}

/// The camera's rain direction for a rotation actor's rotation (0x00AC0570: the negated X axis
/// of its rotation matrix), else straight down.
pub fn rain_direction(rotation: Option<[i32; 3]>) -> Vec3 {
    match rotation {
        Some(r) => -rotator_vector(r),
        None => Vec3::new(0.0, 0.0, -1.0),
    }
}

/// `appFrand` on MSVC `rand` (holdrand * 214013 + 2531011, 15 bits / `RAND_MAX`).
pub fn frand(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(214013).wrapping_add(2531011);
    ((*state >> 16) & 0x7fff) as f32 / 32767.0
}

/// The latent update of `SetRainEmitter` (0x00B85A30) for one tick: the output to fire (1 "Rain
/// Start", 2 "Rain Stop"), given the camera's `m_bWasUncovered`, the action's timer and its
/// `m_fRainStartDelay`.
pub fn rain_outputs(uncovered: bool, timer: &mut f32, delay: f32, dt: f32) -> Option<usize> {
    if !uncovered {
        let fire = (delay <= *timer).then_some(2);
        *timer = 0.0;
        return fire;
    }
    if *timer < delay {
        *timer += dt;
        if delay <= *timer {
            return Some(1);
        }
    }
    None
}

/// `DisSeqAct_Highlight` (0x00B8F4C0).
pub(super) fn highlight(h: &mut H, cx: &mut Ctx) {
    let on = match cx.input() {
        Some(0) => true,
        Some(1) => false,
        _ => return,
    };
    for t in cx.objects("Target") {
        let Obj::Actor(a) = t else { continue };
        if on {
            h.st.show.highlighted.insert(a);
        } else {
            h.st.show.highlighted.remove(&a);
        }
    }
}

/// `DisSeqAct_SetRainEmitter::Activated` (0x00B9ADF0).
pub(super) fn set_rain_emitter(h: &mut H, cx: &mut Ctx) {
    let emitter = cx.objects("Target").into_iter().find_map(|t| match t {
        Obj::Actor(a) if h.db.actors[a as usize].class == "Emitter" => Some(a),
        _ => None,
    });
    let rotation = match cx.prop_obj("m_pRainRotationActor") {
        Obj::Actor(a) => Some(h.st.actor_rot.get(&a).map(|q| quat_rotator(*q)).unwrap_or(h.db.actors[a as usize].rotation)),
        _ => None,
    };
    let id = h.st.show.rain.as_ref().map_or(1, |r| r.id.wrapping_add(1));
    h.st.show.rain = Some(Rain {
        id,
        emitter,
        drops: cx.prop("m_NumRainDrops").and_then(|v| v.as_int()).unwrap_or(15),
        impacts_min: cx.prop("m_fRainImpactsMinDist").and_then(|v| v.as_float()).unwrap_or(100.0),
        impacts_max: cx.prop("m_fRainImpactsMaxDist").and_then(|v| v.as_float()).unwrap_or(1000.0),
        direction: rain_direction(rotation).to_array(),
        rotation,
    });
    // `m_pCachedCamera` = the camera, `m_fStartTimer` kept.
    h.st.show.rain_timers.entry(cx.op).or_insert(0.0);
}

/// `SetRainEmitter`'s latent update: never done.
pub(super) fn rain_update(h: &mut H, cx: &mut Ctx, dt: f32) -> bool {
    let delay = cx.prop("m_fRainStartDelay").and_then(|v| v.as_float()).unwrap_or(1.0);
    let uncovered = h.st.show.uncovered;
    let Some(timer) = h.st.show.rain_timers.get_mut(&cx.op) else { return false };
    if let Some(out) = rain_outputs(uncovered, timer, delay, dt) {
        cx.fire(out);
    }
    false
}

/// `SeqAct_CameraFade::Activated` (0x006E6960).
pub(super) fn camera_fade(h: &mut H, cx: &mut Ctx) {
    let color = match cx.prop("FadeColor") {
        Some(Value::Array(c)) if c.len() == 4 => [0, 1, 2, 3].map(|i| match c[i] {
            Value::Byte(b) => b,
            _ => 0,
        }),
        _ => [0; 4],
    };
    let opacity = cx.prop("FadeOpacity").and_then(|v| v.as_float()).unwrap_or(1.0);
    let time = cx.prop("FadeTime").and_then(|v| v.as_float()).unwrap_or(1.0);
    h.st.show.fade.start(color, opacity, time);
}

/// `SeqAct_CameraFade`'s update at its end (0x006E6DE0).
pub(super) fn camera_fade_done(h: &mut H, cx: &mut Ctx) {
    let persist = cx.prop("bPersistFade").and_then(|v| v.as_bool()).unwrap_or(true);
    let opacity = cx.prop("FadeOpacity").and_then(|v| v.as_float()).unwrap_or(1.0);
    if !(persist && opacity != 0.0) {
        h.st.show.fade.clear();
    }
}

/// The rotator of a quaternion (UE3 rotation units), for actors the Kismet turned.
fn quat_rotator(q: glam::Quat) -> [i32; 3] {
    let x = q * Vec3::X;
    let pitch = x.z.clamp(-1.0, 1.0).asin();
    let yaw = x.y.atan2(x.x);
    let u = |r: f32| (r * 65536.0 / std::f32::consts::TAU).round() as i32;
    [u(pitch), u(yaw), 0]
}

/// The camera's cover test for one tick (0x00AD80A0): None when there is no rain emitter (the
/// flag keeps its value), else whether the view is under the open sky.
pub fn uncovered(rain: &Rain, eye: Vec3, in_water: bool, rand: &mut u32, world: &dyn dishonored_world::WorldQuery) -> Option<bool> {
    rain.emitter?;
    if in_water {
        return Some(false);
    }
    let jx = frand(rand) * 70.0 - 35.0;
    let jy = frand(rand) * 70.0 - 35.0;
    let p = eye + Vec3::new(jx, jy, 0.0);
    let up = -Vec3::from(rain.direction).normalize_or_zero();
    Some(world.raycast(p, up, 5000.0, dishonored_world::channel::PAWN).is_none())
}

/// The camera's cover test of this step, from the first human player's view (called after the
/// simulation, with the collision world). The next step's Kismet update reads it, as the
/// original's latent update reads the `m_bWasUncovered` of the previous tick: the cameras update
/// after the actors (0x0065DA60), after that tick's `UpdateOp`.
pub fn rain_cover(m: &mut super::MissionMatch, world: &dyn dishonored_world::WorldQuery, cfg: &dishonored_sim::SimConfig) {
    let Some(run) = m.kismet.as_mut() else { return };
    let Some(rain) = run.st.show.rain.clone() else { return };
    let Some(p) = m.humans.first().and_then(|id| m.world.player(*id)) else { return };
    let eye = dishonored_sim::movement::eye_position(p, &cfg.gameplay.movement);
    let in_water = world.water_at(p.position).is_some();
    let show = &mut run.st.show;
    if let Some(u) = uncovered(&rain, eye, in_water, &mut show.rand, world) {
        show.uncovered = u;
    }
}

/// Advance the camera fade (`Camera::UpdateCamera`) and write what the clients draw.
pub(super) fn step_and_view(h: &mut H, dt: f32) {
    h.st.show.fade.step(dt);
    let db = h.db.clone();
    let map = h.st.map.clone();
    let on_map = |a: u32| {
        let Some(map) = map.as_deref() else { return true };
        let act = &db.actors[a as usize];
        db.packages.iter().position(|p| p.eq_ignore_ascii_case(&act.package)).and_then(|p| db.map_of_package(p as u16)).is_none_or(|m| m.eq_ignore_ascii_case(map))
    };
    let view = &mut h.m.view;
    view.highlights = h
        .st
        .show
        .highlighted
        .iter()
        .filter(|a| !h.st.gone.contains(a) && on_map(**a))
        .map(|&a| {
            let act = &db.actors[a as usize];
            HighlightView { actor: format!("{}:{}", act.package, act.name), placed: act.location }
        })
        .collect();
    view.rain = h.st.show.rain.as_ref().and_then(|r| {
        let a = r.emitter.filter(|a| on_map(*a))?;
        let act = &db.actors[a as usize];
        Some(RainView { id: r.id, emitter: format!("{}:{}", act.package, act.name), drops: r.drops, impacts_min: r.impacts_min, impacts_max: r.impacts_max, direction: r.direction, rotation: r.rotation })
    });
    view.camera_fade = h.st.show.fade.enabled.then(|| h.st.show.fade.view());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fade_runs_from_the_current_amount_to_the_opacity() {
        let mut f = Fade::default();
        f.start([0, 0, 0, 255], 1.0, 0.25);
        assert_eq!(f.alpha, [0.0, 1.0]);
        f.step(0.125);
        assert!((f.amount - 0.5).abs() < 1e-5 && (f.remaining - 0.125).abs() < 1e-6);
        f.step(1.0);
        assert_eq!((f.amount, f.remaining), (1.0, 0.0));
        // Persisted black; the fade back in starts from it.
        f.start([0; 4], 0.0, 0.5);
        assert_eq!(f.alpha, [1.0, 0.0]);
        f.step(0.25);
        assert!((f.amount - 0.5).abs() < 1e-5);
        assert!(f.view().amount == f.amount && f.view().id == 2);
        // Cleared: not drawn, the amount at the target.
        f.clear();
        assert!(!f.enabled && f.amount == 0.0);
    }

    #[test]
    fn an_untimed_fade_keeps_its_amount() {
        // `FadeTimeRemaining` 0: the camera does not update `FadeAmount`.
        let mut f = Fade { amount: 0.3, ..Default::default() };
        f.start([0; 4], 1.0, 0.0);
        f.step(0.1);
        assert_eq!(f.amount, 0.3);
    }

    #[test]
    fn rain_starts_after_the_delay_and_stops_when_covered() {
        let mut t = 0.0;
        assert_eq!(rain_outputs(true, &mut t, 1.0, 0.5), None);
        assert_eq!(rain_outputs(true, &mut t, 1.0, 0.5), Some(1));
        assert_eq!(rain_outputs(true, &mut t, 1.0, 0.5), None, "started once");
        assert_eq!(rain_outputs(false, &mut t, 1.0, 0.5), Some(2));
        assert_eq!(t, 0.0);
        assert_eq!(rain_outputs(false, &mut t, 1.0, 0.5), None, "not started: no stop");
        // Covered before the delay: the timer restarts, no outputs.
        assert_eq!(rain_outputs(true, &mut t, 1.0, 0.5), None);
        assert_eq!(rain_outputs(false, &mut t, 1.0, 0.5), None);
        assert_eq!(t, 0.0);
    }

    #[test]
    fn rain_falls_against_the_rotation_actor() {
        assert_eq!(rain_direction(None), Vec3::new(0.0, 0.0, -1.0));
        // Pitched up 90 degrees: the rain falls straight down.
        let d = rain_direction(Some([16384, 0, 0]));
        assert!((d - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5);
        // Yaw 90: along -Y.
        let d = rain_direction(Some([0, 16384, 0]));
        assert!((d - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-5);
        assert_eq!(quat_rotator(glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)), [0, 16384, 0]);
    }

    #[test]
    fn msvc_frand_is_in_range() {
        let mut s = 1;
        for _ in 0..1000 {
            let v = frand(&mut s);
            assert!((0.0..=1.0).contains(&v));
        }
        // MSVC rand() from seed 1: 41, 18467.
        let mut s = 1;
        frand(&mut s);
        assert_eq!((s >> 16) & 0x7fff, 41);
    }
}
