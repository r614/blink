//! The Tower Return lens TV (`saili_FT_LensTV.screen_MAT`): an unlit emissive material that samples
//! the render target of the level's `SceneCapture2DActor` (`reflect.rs`), with Arkane's TV glitch
//! effect (colour channels offset by a noisy amount, scan lines, a vignette and a wobbling flicker).
//!
//! The material graph is stripped from the cooked package (only its parameters survive:
//! `Switch_ON_OFF` 1, `EffectBaseSpeed` 0.05, `RGB effect offset strength` 2, the render target
//! `MaterialExpressionTextureSampleParameter2D_0`); every term below is read from the compiled
//! unlit base pass (RefShaderCache-PC-D3D-SM3.upk, `shader_maps ... screen_MAT`, shader 8137 for
//! `FNoLightMapPolicy`; the other lightmap policies compute the same emissive). With uv the mesh's
//! first texture coordinates and t the material time:
//!
//!   offsets (uniform expressions):
//!     o_r = frac(sin(T14 x 20.26834) + sin(T14 x 6.2831855)) x 0.01 x S,    T14 = t x speed x 0.14
//!     o_g = (0, frac(sin(T1 x 7.953399) + sin(T1 x 57.11987)) x 0.013 x S),  T1 = t x speed x 0.1
//!     o_b = (1, -2) x frac(sin(T27 x 7.953399) + sin(T27 x 57.11987)) x 0.017 x S,  T27 = t x speed x 0.27
//!     S = `RGB effect offset strength`, speed = `EffectBaseSpeed`
//!   colour = (Target(uv + o_r).r, Target(uv + o_g).g, Target(uv + o_b).b) x Switch_ON_OFF
//!   x (0.7 (0.5 sin(2 pi frac(142.857 (uv.y + frac(-0.02 t)) + 0.5) - pi) - 0.5) + 1)         scan lines
//!   x (1 - s^0.25),  s = saturate(|2 uv.x - 1|^3.5 + |2 uv.y - 1|^3.5)  (1 when s < 1e-6)      vignette
//!   x w,  p = sin(2 pi frac(2.7 uv.y + frac(-0.23 t) + 0.5) - pi) x sin(2 pi frac(1.1 uv.y +
//!         frac(-0.37 t) + 0.5) - pi),  q = p^4 (0 when |p| < 1e-6),  w = 1 - 0.7 (1 - q) when
//!         q >= 0.5, else 0.65                                                                 flicker
//!   emissive = colour x 10 (+ SelectionColor 0); the base pass adds DiffuseOverrideParameter x
//!   AmbientColorAndSkyFactor, which is zero for the normal draw.

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::MaterialParams;

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct TvUniform {
    /// (Switch_ON_OFF, EffectBaseSpeed, RGB effect offset strength, UE3 radiance to Bevy light
    /// units (`map_render::light_unit`)).
    pub params: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct TvScreenMaterial {
    #[uniform(0)]
    pub params: TvUniform,
    /// The capture's render target.
    #[texture(1)]
    #[sampler(2)]
    pub target: Handle<Image>,
}

impl Material for TvScreenMaterial {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/tv_screen.wgsl".into()
    }
}

impl TvScreenMaterial {
    /// The screen of a material that samples a scene capture's render target (defaults are the
    /// material's parameter defaults).
    pub fn new(p: &MaterialParams, target: Handle<Image>) -> TvScreenMaterial {
        let scalar = |n: &str, d: f32| p.scalars.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or(d);
        TvScreenMaterial {
            params: TvUniform { params: Vec4::new(scalar("Switch_ON_OFF", 1.0), scalar("EffectBaseSpeed", 0.05), scalar("RGB effect offset strength", 2.0), crate::map_render::light_unit()) },
            target,
        }
    }
}

/// A screen section waiting for its material (spawned with a StandardMaterial by
/// `map_render::spawn_map`, swapped on the next update).
#[derive(Component, Clone)]
pub struct PendingTvScreen(pub TvScreenMaterial);

pub struct TvScreenPlugin;

impl Plugin for TvScreenPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "tv_screen.wgsl");
        app.add_plugins(MaterialPlugin::<TvScreenMaterial>::default()).add_systems(Update, apply_pending);
    }
}

fn apply_pending(mut commands: Commands, q: Query<(Entity, &PendingTvScreen)>, mut mats: ResMut<Assets<TvScreenMaterial>>) {
    for (e, p) in &q {
        let h = mats.add(p.0.clone());
        commands.entity(e).remove::<(PendingTvScreen, MeshMaterial3d<StandardMaterial>, bevy::pbr::Lightmap)>().insert((MeshMaterial3d(h), bevy::light::NotShadowReceiver, bevy::light::NotShadowCaster));
    }
}
