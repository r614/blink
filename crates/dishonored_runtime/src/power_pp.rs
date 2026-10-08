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

fn drive(view: Option<Res<SessionView>>, time: Res<Time>, mut q: Query<&mut ArkPostSource>) {
    let me = view.as_ref().and_then(|v| v.me.as_ref());
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
    }
}
