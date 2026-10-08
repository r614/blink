//! The powers' screen effects in the post chain (`ark_post`), from the original's post-process
//! chain `Startup` `AltScreen_Effects.PostProcessChain.Test_PPG` and the shaders captured from
//! the running game (oracle shader capture, `effects/bendtime_l1.json`; EFFECTS.md "Power
//! screen effects").
//!
//! Bend Time: `BendTimeSwitch` routes `PPG_BendTimeVectors_Std_INST` (a vector field pulling
//! towards the screen centre, wobbled by the `water2` noise, masked by `fxt_eyelid_gradient`)
//! into `ArkPpNodeBlur` (two 4-tap passes along the field). Its intensity
//! (`DisPostProcessManager.m_BendTimeIntensity`, the material's `vBendTimeOpacity`) ramps
//! linearly 0 -> 1 over the level's `m_fPostEffectWarmupTime` (0.5 s, both levels; captured:
//! +0.0333 per 60 Hz frame from the cast) and back over `m_fPostEffectCooldownTime` (0.5 s) at the
//! end. The material's `Time` runs at the player's dilation (captured: the pans advance by
//! `0.2 x 0.85 / 60` per frame at Bend Time 1's 0.85).

use crate::ark_post::ArkPostSource;
use crate::session::SessionView;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use ue3_package::ObjRef;

/// `vBendTimeVectorScales.x` of `PPG_BendTimeVectors` (50, 50, 50, 20).
pub const BEND_VECTOR_SCALE: f32 = 50.0;
/// `Twk_BendTime` `m_fPostEffectWarmupTime` / `m_fPostEffectCooldownTime` (both levels).
pub const BEND_POST_WARMUP: f32 = 0.5;
pub const BEND_POST_COOLDOWN: f32 = 0.5;

/// The eyelid pass's colour as captured (`UniformPixelVector_1`, `effects/darkvision_eyelid_timeline.json`):
/// `EyeLidColor` (0.392, 0.333, 0.275) in linear space (each ^2.2).
pub const EYELID_COLOR: [f32; 3] = [0.127_529_8, 0.089_193_5, 0.058_187_2];

/// Blink's lens field (`PPG_BlinkVectors`, captured shader 0x0b36c71ce1512549) is driven by two
/// values (`UniformPixelVector_1.xy`, the post manager's `blink` / `blinkg` channels; captured in
/// `effects/blink_lens_timeline.json`). While aiming, `x` (the lens distortion) wobbles up to its
/// rest `m_fWarmupDistorionMinStrength` x `m_fWarmupWobbleMaxStrength` (level 2: 0.35 x 0.2 =
/// 0.07): the captured warmup, relative to that rest, every 0.1 s.
pub const BLINK_WARMUP_SHAPE: [f32; 12] = [0.0024, 0.32, 0.63, 0.92, 1.11, 1.24, 1.30, 1.30, 1.24, 1.11, 1.03, 1.0];
/// `Twk_Blink` level 2: `m_fWarmupDistorionMinStrength`, `m_fWarmupWobbleMaxStrength`,
/// `m_fMoveDistortionMaxStrength` (x at the end of the move), `m_fMoveBlurMaxStrength` (y),
/// `m_fMoveReachMaxAtPercentage`, `m_fCooldownTime`, `m_CoolDownWobbleCount`.
pub const BLINK_WARMUP_MIN: f32 = 0.35;
pub const BLINK_WOBBLE_MAX: f32 = 0.2;
pub const BLINK_MOVE_DISTORTION: f32 = 1.5;
pub const BLINK_MOVE_BLUR: f32 = 2.5;
pub const BLINK_REACH_MAX_AT: f32 = 0.8;
pub const BLINK_COOLDOWN: f32 = 1.0;
pub const BLINK_COOLDOWN_WOBBLES: f32 = 10.0;
/// The captured cooldown decays by about 0.66 per 0.1 s (a 0.24 s time constant).
pub const BLINK_COOLDOWN_TAU: f32 = 0.24;

/// Blink's lens drive between frames.
#[derive(Default)]
pub struct BlinkLens {
    /// Seconds since aiming started, or since the move ended (cooldown).
    pub clock: f32,
    pub aiming: bool,
    pub travelling: bool,
    /// (x, y) when the move ended (the cooldown decays from there).
    pub end: Vec2,
    pub value: Vec2,
}

impl BlinkLens {
    /// Advance by `dt`: `aiming` while the left hand aims Blink, `progress` the move's share of
    /// its distance while travelling.
    pub fn step(&mut self, aiming: bool, progress: Option<f32>, dt: f32) -> Vec2 {
        match progress {
            Some(p) => {
                self.travelling = true;
                self.aiming = false;
                // Up to the move strengths at `m_fMoveReachMaxAtPercentage` of the distance (the
                // capture: 0.11 / 0.19 at 31 %, 0.38 / 0.94 at 62 %, 1.47 / 1.88 at arrival).
                let k = (p / BLINK_REACH_MAX_AT).clamp(0.0, 1.0).powi(3);
                self.value = Vec2::new(BLINK_MOVE_DISTORTION * k, BLINK_MOVE_BLUR * k);
                self.end = self.value;
                self.clock = 0.0;
            }
            None if aiming => {
                if !self.aiming {
                    self.aiming = true;
                    self.clock = 0.0;
                }
                self.travelling = false;
                self.clock += dt;
                let f = self.clock / 0.1;
                let i = (f.floor() as usize).min(BLINK_WARMUP_SHAPE.len() - 1);
                let j = (i + 1).min(BLINK_WARMUP_SHAPE.len() - 1);
                let s = BLINK_WARMUP_SHAPE[i] + (BLINK_WARMUP_SHAPE[j] - BLINK_WARMUP_SHAPE[i]) * (f - i as f32).clamp(0.0, 1.0);
                self.value = Vec2::new(BLINK_WARMUP_MIN * BLINK_WOBBLE_MAX * s, 0.0);
            }
            None => {
                if self.travelling || self.aiming {
                    self.travelling = false;
                    self.aiming = false;
                    self.end = self.value;
                    self.clock = 0.0;
                }
                self.clock += dt;
                if self.clock >= BLINK_COOLDOWN * 3.0 {
                    self.value = Vec2::ZERO;
                } else {
                    let d = (-self.clock / BLINK_COOLDOWN_TAU).exp();
                    // `m_CoolDownWobbleCount` wobbles of the distortion over `m_fCooldownTime`.
                    let w = (std::f32::consts::PI * BLINK_COOLDOWN_WOBBLES / BLINK_COOLDOWN * self.clock).cos().abs();
                    self.value = Vec2::new(self.end.x * d * w, self.end.y * d);
                }
            }
        }
        self.value
    }
}

/// The field's textures, extracted for the post pass.
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct PowerPpImages {
    pub noise: Option<Handle<Image>>,
    pub mask: Option<Handle<Image>>,
}

pub struct PowerPpPlugin;

impl Plugin for PowerPpPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PowerPpImages>()
            .add_plugins(ExtractResourcePlugin::<PowerPpImages>::default())
            .add_systems(Startup, load_images)
            .add_systems(Update, drive);
    }
}

fn load_images(mut pp: ResMut<PowerPpImages>, mut images: ResMut<Assets<Image>>) {
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    let Ok(pkg) = content.package("Startup") else { return };
    let mut load = |path: &str| -> Option<Handle<Image>> {
        let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Texture2D" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path))?;
        let t = dishonored_assets::texture::read_texture(&content, &pkg, i, 512).ok()?;
        Some(images.add(crate::map_render::to_image(&t)))
    };
    pp.noise = load("AltScreen_Effects.PostProcessTextures.water2");
    pp.mask = load("AltScreen_Effects.PostProcessTextures.fxt_eyelid_gradient");
    if pp.noise.is_none() || pp.mask.is_none() {
        warn!("power post effects: Bend Time field textures missing (noise {}, mask {})", pp.noise.is_some(), pp.mask.is_some());
    }
}

/// Bend Time's post intensity for the local caster's bend: up over the warmup from the start,
/// down over the cooldown before the end.
pub fn bend_intensity(elapsed: f32, duration: f32) -> f32 {
    let up = (elapsed / BEND_POST_WARMUP).clamp(0.0, 1.0);
    let down = ((duration - elapsed) / BEND_POST_COOLDOWN).clamp(0.0, 1.0);
    up.min(down)
}

fn drive(view: Option<Res<SessionView>>, time: Res<Time>, mut lens: Local<BlinkLens>, mut q: Query<&mut ArkPostSource>) {
    let me = view.as_ref().and_then(|v| v.me.as_ref());
    // Dark Vision's eyelid pass runs while the power is on, its `Alpha` the power's fade (the
    // capture: 0 -> 1 over `m_PowerFadeInDuration`, then 1, then the fade-out remaining /
    // `m_PowerFadeOutDuration`, one frame late).
    let eyelid = me.filter(|m| m.vitals.dark_vision_remaining > 0.0).map(|m| crate::dark_vision::weight(&m.dark));
    // Blink's lens: aiming (the left hand aims Blink), travelling (share of the move's distance).
    let aiming = me.is_some_and(|m| m.left.item == dishonored_sim::input::LeftHand::Blink && m.left.blink_target.is_some());
    let progress = me.filter(|m| m.mode == dishonored_sim::state::MoveMode::Blinking).map(|m| (m.position.distance(m.blink.origin) / m.blink.max_distance.max(1.0)).clamp(0.0, 1.0));
    let blink = lens.step(aiming, progress, time.delta_secs());
    let bend = match (view.as_ref(), me) {
        (Some(v), Some(me)) => v.time_bends.iter().find(|b| b.caster == me.id).map(|b| bend_intensity(b.elapsed, b.duration)).unwrap_or(0.0),
        _ => 0.0,
    };
    let rate = me.map(|m| m.time_scale).unwrap_or(1.0);
    for mut src in &mut q {
        let t = src.bend_time + time.delta_secs() * rate;
        src.bend_time = if t > 1.0e4 { t - 1.0e4 } else { t };
        if src.bend != bend {
            src.bend = bend;
        }
        if src.eyelid != eyelid {
            src.eyelid = eyelid;
        }
        if src.blink_lens != blink {
            src.blink_lens = blink;
        }
    }
}
