//! The moon and sun discs: `Vfx_Materials.main_materials.Moon_PMAT` and
//! `Vfx_Materials.Material.Sun_PMAT`, rendered from their compiled base pass shaders
//! (RefShaderCache `TBasePassPixelShaderFNoLightMapPolicyNoSkyLightFALSEFALSE`; research/LOG.md
//! "Camera, exposure and the moon").
//!
//! - `Moon_PMAT` (translucent, lit model but no diffuse input, so only its emissive shows):
//!   emissive `Tex#0.r x Moon_Color.rgb x a`, opacity
//!   `saturate(Cloud((u x 0.15 + Cloud_Speed x Time, v)).r) x Tex#0.g`. `Tex#0` is
//!   `Vfx_Materials.Textures.Moon`: red everywhere, the disc and its halo in green, so the disc
//!   covers about 0.4 of the mesh's UV square.
//! - `Sun_PMAT` (additive, unlit): `m = max(1 - 2 |uv - 0.5|, 0)`,
//!   `m^SphereMask_Power x SunIntensity x Color.rgb` (the colour's alpha is not used), 0 where
//!   `m` is 0.

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{MaterialParams, TextureId};

pub const MOON_PMAT: &str = "Moon_PMAT";
pub const SUN_PMAT: &str = "Sun_PMAT";

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct DiscUniform {
    /// Moon: `Moon_Color.rgb x a`; sun: `Color.rgb x SunIntensity`.
    pub color: Vec4,
    /// (kind: 0 moon / 1 sun, Cloud_Speed, SphereMask_Power, 0)
    pub params: Vec4,
}

impl DiscUniform {
    /// The uniform of a moon or sun material, or None for other bases.
    pub fn decode(p: &MaterialParams) -> Option<DiscUniform> {
        let base = p.base_material.as_deref()?;
        let s = |n: &str, d: f32| p.scalars.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or(d);
        let v = |n: &str, d: [f32; 4]| p.vectors.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or(d);
        if base.ends_with(MOON_PMAT) {
            // Base defaults: Moon_Color (1, 1, 1, 1), Cloud_Speed 0.01.
            let c = v("Moon_Color", [1.0; 4]);
            Some(DiscUniform { color: Vec4::new(c[0] * c[3], c[1] * c[3], c[2] * c[3], 1.0), params: Vec4::new(0.0, s("Cloud_Speed", 0.01), 0.0, 0.0) })
        } else if base.ends_with(SUN_PMAT) {
            // Base defaults: Color (1, 0.940601, 0.74453), SunIntensity 2, SphereMask_Power 2.
            let c = v("Color", [1.0, 0.940_601, 0.744_53, 1.0]);
            let k = s("SunIntensity", 2.0);
            Some(DiscUniform { color: Vec4::new(c[0] * k, c[1] * k, c[2] * k, 1.0), params: Vec4::new(1.0, 0.0, s("SphereMask_Power", 2.0), 0.0) })
        } else {
            None
        }
    }

    pub fn is_sun(&self) -> bool {
        self.params.x > 0.5
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct SkyDiscMaterial {
    #[uniform(0)]
    pub params: DiscUniform,
    /// `Tex#0` (the moon disc), with the repeating sampler.
    #[texture(1)]
    #[sampler(2)]
    pub disc: Option<Handle<Image>>,
    /// The `Cloud` parameter.
    #[texture(3)]
    pub clouds: Option<Handle<Image>>,
}

impl SkyDiscMaterial {
    pub fn new(p: &MaterialParams, textures: &[(String, TextureId)], images: &[Handle<Image>]) -> Option<SkyDiscMaterial> {
        let params = DiscUniform::decode(p)?;
        let tex = |n: &str| textures.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).and_then(|t| images.get(t.1)).cloned();
        let (disc, clouds) = (tex("Tex0"), tex("Cloud"));
        if !params.is_sun() && (disc.is_none() || clouds.is_none()) {
            warn!("moon material without its textures (Tex0 {}, Cloud {})", disc.is_some(), clouds.is_some());
            return None;
        }
        Some(SkyDiscMaterial { params, disc, clouds })
    }
}

impl Material for SkyDiscMaterial {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/sky_disc.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        if self.params.is_sun() { AlphaMode::Add } else { AlphaMode::Blend }
    }

    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // Seen from inside the sky: no culling.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// A moon / sun entity waiting for its `SkyDiscMaterial` (spawned with a StandardMaterial by
/// `map_render::spawn_map`, swapped on the next update).
#[derive(Component, Clone)]
pub struct PendingSkyDisc(pub SkyDiscMaterial);

pub fn apply_pending(mut commands: Commands, q: Query<(Entity, &PendingSkyDisc)>, mut mats: ResMut<Assets<SkyDiscMaterial>>) {
    for (e, p) in &q {
        commands.entity(e).remove::<(PendingSkyDisc, MeshMaterial3d<StandardMaterial>)>().insert(MeshMaterial3d(mats.add(p.0.clone())));
    }
}
