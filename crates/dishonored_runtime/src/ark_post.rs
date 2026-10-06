//! Arkane's full-screen passes, decoded from the original's global shaders
//! (`GlobalShaderCache-PC-D3D-SM3.bin`) and their parameter setters in `Dishonored.exe`
//! (research/LOG.md "DisFog and ArkPp post"), as one Bevy post-process pass:
//!
//! 1. `FDisFogPixelShader<N><M>Layer`: up to four height-bounded fog layers applied to the
//!    scene colour by depth (per layer: in-slab ray length, `NearPlane` / `FarPlane` ramp or
//!    the 128-entry LUT, `Opacity`, cut at `NoFogPlane`; layer 0 weighted by the sun lobe).
//! 2. `FArkPpDofUber_*PS` far blur: `saturate((depth - FocusDistance) / InFocusRadius) *
//!    FarBlurAmount` towards a blurred copy.
//! 3. `FArkPpDofLutBlenderPS` colour LUT, evaluated per pixel instead of through the 16^3 LUT:
//!    clamp to 1, `x 2^m_Exposure`, pre-desaturation, shadow / midtone / highlight colour
//!    balance (CrMgYb), opacity, overlay, post-desaturation, `^(m_GammaAdjustment /
//!    display gamma)`, brightness, contrast `tan((c + 1) pi / 4)`.
//! 4. Film grain `+- m_FilmGrainNoise / 255`.
//!
//! The pass writes the original's display-referred (gamma 2.2) value converted back to
//! linear for Bevy's sRGB output, so `Tonemapping::None` must be used with it.

use bevy::asset::{load_embedded_asset, AssetServer, Handle};
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::FullscreenShader;
use bevy::prelude::*;
use bevy::render::extract_component::{ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin, UniformComponentPlugin};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId, ColorTargetState, ColorWrites, FilterMode, FragmentState,
    Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderStages, ShaderType, SpecializedRenderPipeline, SpecializedRenderPipelines, TextureFormat, TextureSampleType, TextureUsages,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::{ExtractedView, Msaa, ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;
use dishonored_ir::{FogSettings, PostSettings};

/// Per-camera parameters of the pass (also the GPU uniform).
#[derive(Component, Clone, Copy, ShaderType, ExtractComponent)]
pub struct ArkPost {
    /// Fog layer world Z bounds, uu (min, max per layer).
    pub layer_min: Vec4,
    pub layer_max: Vec4,
    pub layer_near: Vec4,
    /// 1 / (FarPlane - NearPlane).
    pub layer_inv_range: Vec4,
    pub layer_no_fog: Vec4,
    pub layer_opacity: Vec4,
    pub layer_density: Vec4,
    /// 1 when the layer uses its LUT.
    pub layer_lut: Vec4,
    pub layer_color: [Vec4; 4],
    /// Sun direction (Bevy world, towards the sun) and `SunPower` of layer 0 (0 = no sun).
    pub sun: Vec4,
    /// Fog LUTs: entry i of layer l is `lut[l * 32 + i / 4][i % 4]`.
    pub lut: [Vec4; 128],
    pub cb_shadows_opacity: Vec4,
    pub cb_midtones: Vec4,
    pub cb_highlights: Vec4,
    pub overlay: Vec4,
    /// (2^m_Exposure, m_GammaAdjustment / display gamma, pre-, post-desaturation).
    pub exposure_gamma_desat: Vec4,
    /// (brightness, tan((contrast + 1) pi / 4), 0, 0).
    pub brightness_contrast: Vec4,
    /// (FocusDistance uu, 1 / InFocusRadius, FarBlurAmount, 0).
    pub dof: Vec4,
    /// (2 g / 255, -g / 255, frame seed, 0) with g = m_FilmGrainNoise.
    pub grain: Vec4,
    /// (Bevy metres per uu, fog layer count, colour pipeline on, 0).
    pub misc: Vec4,
}

/// Inputs kept on the camera so the uniform can be rebuilt (display gamma setting).
#[derive(Component, Clone, Default)]
pub struct ArkPostSource {
    pub fog: Vec<FogSettings>,
    pub post: Option<PostSettings>,
    pub fog_on: bool,
    pub post_on: bool,
    pub display_gamma: f32,
}

fn post_value(p: Option<&PostSettings>, key: &str) -> Option<f32> {
    p?.values.iter().find(|(k, _)| k.ends_with(key)).map(|v| v.1)
}
fn post_vector(p: Option<&PostSettings>, key: &str) -> Option<[f32; 4]> {
    p?.vectors.iter().find(|(k, _)| k.ends_with(key)).map(|v| v.1)
}

impl ArkPostSource {
    /// Build the uniform. Missing ArkPp fields take the `ArkPpConfig` struct defaults
    /// (Engine.upk `ArkPpSettings`): exposure 0, gamma 1, focus 1000, radius 300, far blur 0,
    /// colour balance 0, opacity 1, desaturation 0, grain / brightness / contrast 0.
    /// `cam_z` is the camera's world Z in uu (custom transitions depend on it).
    pub fn uniform(&self, cam_z: f32) -> ArkPost {
        let p = self.post.as_ref();
        let v = |k: &str, d: f32| post_value(p, k).unwrap_or(d);
        let vec3 = |k: &str| post_vector(p, k).map(|c| Vec4::new(c[0], c[1], c[2], 0.0)).unwrap_or(Vec4::ZERO);
        let mut u = ArkPost {
            layer_min: Vec4::ZERO,
            layer_max: Vec4::ZERO,
            layer_near: Vec4::ZERO,
            layer_inv_range: Vec4::ZERO,
            layer_no_fog: Vec4::ZERO,
            layer_opacity: Vec4::ZERO,
            layer_density: Vec4::ONE,
            layer_lut: Vec4::ZERO,
            layer_color: [Vec4::ZERO; 4],
            sun: Vec4::ZERO,
            lut: [Vec4::ZERO; 128],
            cb_shadows_opacity: vec3("m_CrMgYbShadTones").with_w(v("m_Opacity", 1.0)),
            cb_midtones: vec3("m_CrMgYbMidTones"),
            cb_highlights: vec3("m_CrMgYbHighTones"),
            overlay: Vec4::ZERO,
            exposure_gamma_desat: Vec4::new(
                2f32.powf(v("m_Exposure", 0.0)),
                v("m_GammaAdjustment", 1.0) / self.display_gamma.max(0.1),
                v("m_PreDesaturation", 0.0),
                v("m_PostDesaturation", 0.0),
            ),
            brightness_contrast: Vec4::new(v("m_GimpBrightness", 0.0), ((v("m_GimpContrast", 0.0) + 1.0) * std::f32::consts::FRAC_PI_4).tan(), 0.0, 0.0),
            dof: Vec4::new(v("m_FocusDistance", 1000.0), 1.0 / v("m_InFocusRadius", 300.0).max(1e-3), v("m_FarBlurAmount", 0.0), 0.0),
            grain: {
                let g = v("m_FilmGrainNoise", 0.0);
                Vec4::new(2.0 * g / 255.0, -g / 255.0, 0.0, 0.0)
            },
            misc: Vec4::new(crate::coords::UU_TO_BEVY, 0.0, if self.post_on { 1.0 } else { 0.0 }, 0.0),
        };
        if self.fog_on {
            // The sun layer goes first (only layer 0 gets the sun lobe in the shader).
            let mut layers: Vec<&FogSettings> = self.fog.iter().collect();
            layers.sort_by_key(|l| !l.is_sun);
            let n = layers.len().min(4);
            for (i, l) in layers.iter().take(n).enumerate() {
                let (a, b) = (l.origin, l.origin + l.height);
                u.layer_min[i] = a.min(b);
                u.layer_max[i] = a.max(b);
                u.layer_near[i] = l.near;
                u.layer_inv_range[i] = 1.0 / (l.far - l.near).max(1e-3);
                u.layer_no_fog[i] = l.no_fog;
                // Custom transition (0x0081C940): with the layer top `x` uu above the camera and
                // x < fCustomTransitionHeight T, opacity *= x / T. The original does not clamp;
                // above the top x / T turns negative, which this port clamps to 0 (U-321).
                u.layer_opacity[i] = match l.custom_transition {
                    Some(t) if t > 0.0 => {
                        let x = a.max(b) - cam_z;
                        if x < t { l.opacity * (x / t).max(0.0) } else { l.opacity }
                    }
                    _ => l.opacity,
                };
                u.layer_density[i] = l.height_density;
                u.layer_color[i] = Vec4::new(l.color[0], l.color[1], l.color[2], 0.0);
                if l.lut.len() == 128 {
                    u.layer_lut[i] = 1.0;
                    for (k, &b) in l.lut.iter().enumerate() {
                        u.lut[i * 32 + k / 4][k % 4] = b as f32 / 255.0;
                    }
                }
                if i == 0 && l.is_sun {
                    u.sun.w = l.sun_power;
                }
            }
            u.misc.y = n as f32;
        }
        u
    }
}

/// Keep the uniform in step with its inputs; advance the grain seed every frame.
pub fn update_ark_post(mut q: Query<(&ArkPostSource, &mut ArkPost, &GlobalTransform)>, mut frame: Local<u32>) {
    *frame = frame.wrapping_add(1);
    for (src, mut u, tf) in &mut q {
        let mut n = src.uniform(tf.translation().y / crate::coords::UU_TO_BEVY);
        n.grain.z = (*frame % 1024) as f32;
        *u = n;
    }
}

pub struct ArkPostPlugin;

impl Plugin for ArkPostPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "ark_post.wgsl");
        app.add_plugins((ExtractComponentPlugin::<ArkPost>::default(), UniformComponentPlugin::<ArkPost>::default()))
            .add_systems(PostUpdate, update_ark_post)
            .add_systems(Update, configure_depth_usage);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<SpecializedRenderPipelines<ArkPostPipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(Core3d, ark_post_pass.in_set(Core3dSystems::PostProcess).before(tonemapping));
    }
}

/// The pass reads the main depth buffer, which Bevy only makes sampleable on request.
fn configure_depth_usage(mut q: Query<&mut Camera3d, With<ArkPost>>) {
    for mut c in &mut q {
        let u = TextureUsages::from(c.depth_texture_usages);
        if !u.contains(TextureUsages::TEXTURE_BINDING) {
            c.depth_texture_usages = (u | TextureUsages::TEXTURE_BINDING).into();
        }
    }
}

#[derive(Resource)]
pub struct ArkPostPipeline {
    layout_single: BindGroupLayoutDescriptor,
    layout_multi: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArkPostKey {
    format: TextureFormat,
    multisampled: bool,
}

#[derive(Component)]
pub struct ArkPostPipelineId(CachedRenderPipelineId, bool);

fn layout(multisampled: bool) -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "ark post layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer::<ViewUniform>(true),
                if multisampled { texture_depth_2d_multisampled() } else { texture_depth_2d() },
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<ArkPost>(true),
            ),
        ),
    )
}

fn init_pipeline(mut commands: Commands, device: Res<RenderDevice>, fullscreen: Res<FullscreenShader>, assets: Res<AssetServer>) {
    commands.insert_resource(ArkPostPipeline {
        layout_single: layout(false),
        layout_multi: layout(true),
        sampler: device.create_sampler(&SamplerDescriptor { mag_filter: FilterMode::Linear, min_filter: FilterMode::Linear, ..default() }),
        fullscreen: fullscreen.clone(),
        shader: load_embedded_asset!(assets.as_ref(), "ark_post.wgsl"),
    });
}

impl SpecializedRenderPipeline for ArkPostPipeline {
    type Key = ArkPostKey;
    fn specialize(&self, key: ArkPostKey) -> RenderPipelineDescriptor {
        let mut shader_defs = Vec::new();
        if key.multisampled {
            shader_defs.push("MULTISAMPLED".into());
        }
        RenderPipelineDescriptor {
            label: Some("ark post".into()),
            layout: vec![if key.multisampled { self.layout_multi.clone() } else { self.layout_single.clone() }],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs,
                targets: vec![Some(ColorTargetState { format: key.format, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        }
    }
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<ArkPostPipeline>>,
    pipeline: Option<Res<ArkPostPipeline>>,
    views: Query<(Entity, &ExtractedView, &Msaa), With<ArkPost>>,
) {
    let Some(pipeline) = pipeline else { return };
    for (e, view, msaa) in &views {
        let multisampled = *msaa != Msaa::Off;
        let id = pipelines.specialize(&cache, &pipeline, ArkPostKey { format: view.target_format, multisampled });
        commands.entity(e).insert(ArkPostPipelineId(id, multisampled));
    }
}

fn ark_post_pass(
    view: ViewQuery<(&ViewUniformOffset, &ViewTarget, &ViewDepthTexture, &ArkPostPipelineId, &DynamicUniformIndex<ArkPost>)>,
    cache: Res<PipelineCache>,
    pipeline: Res<ArkPostPipeline>,
    view_uniforms: Res<ViewUniforms>,
    uniforms: Res<ComponentUniforms<ArkPost>>,
    mut ctx: RenderContext,
) {
    let (view_offset, target, depth, id, index) = view.into_inner();
    let (Some(rp), Some(view_binding), Some(uniform_binding)) = (cache.get_render_pipeline(id.0), view_uniforms.uniforms.binding(), uniforms.binding()) else {
        return;
    };
    let post = target.post_process_write();
    let layout = if id.1 { &pipeline.layout_multi } else { &pipeline.layout_single };
    let bind_group = ctx.render_device().create_bind_group(
        Some("ark post bind group"),
        &cache.get_bind_group_layout(layout),
        &BindGroupEntries::sequential((view_binding, depth.view(), post.source, &pipeline.sampler, uniform_binding)),
    );
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("ark post"),
        color_attachments: &[Some(RenderPassColorAttachment { view: post.destination, depth_slice: None, resolve_target: None, ops: Operations::default() })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_render_pipeline(rp);
    pass.set_bind_group(0, &bind_group, &[view_offset.offset, index.index()]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_defaults_and_boyle_values() {
        // No ArkPp fields: the struct defaults (exposure 0 -> x1, gamma 1 / 2.2, radius 300).
        let u = ArkPostSource { display_gamma: 2.2, post_on: true, ..Default::default() }.uniform(0.0);
        assert!((u.exposure_gamma_desat.x - 1.0).abs() < 1e-6);
        assert!((u.exposure_gamma_desat.y - 1.0 / 2.2).abs() < 1e-6);
        assert!((u.dof.y - 1.0 / 300.0).abs() < 1e-9);
        assert!((u.brightness_contrast.y - 1.0).abs() < 1e-5);
        // Boyle Ext: exposure 0.3, gamma 0.95, contrast 0.05.
        let post = PostSettings {
            source: String::new(),
            values: vec![
                ("m_HDRParameters.m_Exposure".into(), 0.3),
                ("m_HDRParameters.m_GammaAdjustment".into(), 0.95),
                ("m_HDRParameters.m_GimpContrast".into(), 0.05),
            ],
            vectors: vec![],
        };
        let u = ArkPostSource { post: Some(post), display_gamma: 2.2, post_on: true, ..Default::default() }.uniform(0.0);
        assert!((u.exposure_gamma_desat.x - 2f32.powf(0.3)).abs() < 1e-6);
        assert!((u.exposure_gamma_desat.y - 0.95 / 2.2).abs() < 1e-6);
        assert!((u.brightness_contrast.y - (1.05 * std::f32::consts::FRAC_PI_4).tan()).abs() < 1e-5);
    }

    #[test]
    fn custom_transition_scales_opacity() {
        let layer = FogSettings {
            opacity: 0.4,
            origin: -2075.0,
            height: 5500.0,
            near: 3000.0,
            far: 10000.0,
            custom_transition: Some(5000.0),
            height_density: 1.0,
            ..Default::default()
        };
        let src = ArkPostSource { fog: vec![layer], fog_on: true, display_gamma: 2.2, ..Default::default() };
        // Layer top 3425; camera at 170 -> 3255 / 5000 of the opacity.
        let u = src.uniform(170.0);
        assert!((u.layer_opacity.x - 0.4 * 3255.0 / 5000.0).abs() < 1e-5);
        assert_eq!(u.misc.y, 1.0);
        // Above the top: clamped to zero (the original goes negative, U-321).
        assert_eq!(src.uniform(4000.0).layer_opacity.x, 0.0);
    }
}
