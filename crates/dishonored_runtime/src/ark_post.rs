//! Arkane's full-screen passes, decoded from the original's global shaders
//! (`GlobalShaderCache-PC-D3D-SM3.bin`) and their parameter setters in `Dishonored.exe`
//! (research/LOG.md "DisFog and ArkPp post"), as one Bevy post-process pass:
//!
//! 1. `FDisFogPixelShader<N><M>Layer`: up to four height-bounded fog layers applied to the
//!    scene colour by depth (per layer: in-slab ray length, `NearPlane` / `FarPlane` ramp or
//!    the 128-entry LUT, `Opacity`, cut at `NoFogPlane`; layer 0 weighted by the sun lobe).
//!    The blurred bloom parts (`bloom.rs`) are added after the fog, as `sBloomParts`.
//! 2. `FArkPpDofUber_*PS` far blur: `saturate((depth - FocusDistance) / InFocusRadius) *
//!    FarBlurAmount` towards a quarter-size copy of the fogged scene, made by two
//!    `FArkPpDofDownsamplePS` passes (each one bilinear tap at the corner of a 2x2 block, 0x00922210)
//!    and sampled bilinearly at the pixel's uv (global shaders 60 - 63).
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
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState,
    Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderStages, ShaderType, SpecializedRenderPipeline, SpecializedRenderPipelines, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType,
    TextureUsages,
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
    /// (Bevy metres per uu, fog layer count, colour pipeline on, bloom on).
    pub misc: Vec4,
    /// `gBloomTint` of the bloom downsample (0x0090DE60): (m_Tint.rgb x m_Scale,
    /// -m_Threshold x m_Scale).
    pub bloom: Vec4,
    /// Bend Time's vector field (`power_pp.rs`): (vBendTimeOpacity = the post-effect intensity,
    /// vBendTimeVectorScales.x, 0, 0); 0 intensity = off.
    pub power_vectors: Vec4,
    /// The field's two noise pans (`Periodic(Time x (0.2, -0.1))`, `Periodic(Time x (-0.15, 0.3))`).
    pub power_pans: Vec4,
    /// Dark Vision's eyelid pass (`power_pp.rs`): (on, Alpha, 0, 0) and `EyeLidColor`.
    pub dv_eyelid: Vec4,
    pub dv_lid_color: Vec4,
    /// Blink's lens field (`power_pp::BlinkLens`): (x distortion, y blur, on, 0).
    pub blink_lens: Vec4,
    /// Possession's field (`power_pp::PossessPp`, `PPG_PossessionVectors`): (R, G, on, material Time).
    pub possess: Vec4,
    /// Light shaft (the level's first `bRenderLightShafts` light, `ark_post.wgsl` `shaft_*`):
    /// (Bevy world position of `GetPosition`, on).
    pub shaft_pos: Vec4,
    /// `LightShaftParameters`: (1 / OcclusionDepthRange, BloomScale, 1.5 - OcclusionMaskDarkness / 2,
    /// OcclusionMaskDarkness).
    pub shaft_params: Vec4,
    /// `BloomTintAndThreshold`: (linear tint, BloomThreshold).
    pub shaft_bloom: Vec4,
    /// (BloomScreenBlendThreshold, shaft target width, height, 0).
    pub shaft_misc: Vec4,
}

/// Inputs kept on the camera so the uniform can be rebuilt (display gamma setting).
#[derive(Component, Clone, Default)]
pub struct ArkPostSource {
    pub fog: Vec<FogSettings>,
    pub post: Option<PostSettings>,
    /// The level's `bRenderLightShafts` lights, and the video setting `PSI_GraphicsPC_LightShaftEnable`.
    pub shafts: Vec<dishonored_ir::LightShaft>,
    pub shafts_on: bool,
    pub fog_on: bool,
    pub post_on: bool,
    pub display_gamma: f32,
    /// A power's grade (Dark Vision's `m_StaticUberAdjustement`, `dark_vision.rs`): ArkPp fields
    /// overriding the level's, blended in by `power_weight` (0..1).
    pub power_post: Option<PostSettings>,
    pub power_weight: f32,
    /// The Kismet's grade (`DisSeqAct_UberPostProcess`, `MissionView::post_fx`): the same kind of
    /// override layer, stacked after the power's.
    pub kismet_post: Option<PostSettings>,
    pub kismet_weight: f32,
    /// The menus' background blur (`Epp_UberUI`, the `UI_Blur` tweak), stacked last.
    pub ui_post: Option<PostSettings>,
    pub ui_weight: f32,
    /// Bend Time's post effect (`power_pp.rs`): intensity 0..1 and the dilated material time.
    pub bend: f32,
    pub bend_time: f32,
    /// Dark Vision's eyelid pass: Some(Alpha) while it runs.
    pub eyelid: Option<f32>,
    /// Blink's lens field drive (x, y); zero = off.
    pub blink_lens: Vec2,
    /// Possession's R / G (post manager `m_PossessionParams`); zero = off.
    pub possess: Vec2,
    /// A scene capture's own fog / post (reflect.rs: bEnableFog, bEnablePostProcess): the bloom
    /// parts belong to the game view and are not added.
    pub capture: bool,
    /// The original post-process graph (`ppgraph`) draws the powers' vector fields with their
    /// translated materials: this pass's hand-ported fields (Bend Time, Blink's lens, Possession)
    /// stay off (they run under `DISHONORED_LEGACY_MATERIALS=1`, or without the shader library).
    pub graph: bool,
    /// The graph's running nodes' own grade (`ArkPpNodeMaterial::m_bOverrideUberPp`), stacked
    /// after the power's, by its fade weight.
    pub graph_post: Option<PostSettings>,
    pub graph_weight: f32,
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
        let mut u = self.level_uniform(cam_z);
        if self.bend > 0.0 && !self.graph {
            let t = self.bend_time;
            let fr = |x: f32| x - x.floor();
            u.power_vectors = Vec4::new(self.bend, crate::power_pp::BEND_VECTOR_SCALE, 0.0, 0.0);
            u.power_pans = Vec4::new(fr(t * 0.2), fr(t * -0.1), fr(t * -0.15), fr(t * 0.3));
        }
        if self.blink_lens != Vec2::ZERO && !self.graph {
            u.blink_lens = Vec4::new(self.blink_lens.x, self.blink_lens.y, 1.0, 0.0);
        }
        if self.possess != Vec2::ZERO && !self.graph {
            u.possess = Vec4::new(self.possess.x, self.possess.y, 1.0, self.bend_time);
        }
        if let Some(a) = self.eyelid {
            u.dv_eyelid = Vec4::new(1.0, a, 0.0, 0.0);
            let c = crate::power_pp::EYELID_COLOR;
            u.dv_lid_color = Vec4::new(c[0], c[1], c[2], 1.0);
        }
        // The weighted grades, in the order the original stacks them: a power's (Dark Vision's
        // `m_StaticUberAdjustement`) then the Kismet's (`DisSeqAct_UberPostProcess`, the post
        // manager's Kismet slot): the LUT terms of the overridden settings, blended by the
        // grade's weight (the colour pipeline is on while it shows).
        let mut graded = self.post_on;
        for (over, weight) in [(&self.power_post, self.power_weight), (&self.graph_post, self.graph_weight), (&self.kismet_post, self.kismet_weight), (&self.ui_post, self.ui_weight)] {
            let (Some(over), true) = (over, weight > 0.0) else { continue };
            let src = ArkPostSource {
                post: Some(crate::dark_vision::merged(self.post.as_ref(), over)),
                post_on: true,
                power_post: None,
                power_weight: 0.0,
                graph_post: None,
                graph_weight: 0.0,
                kismet_post: None,
                kismet_weight: 0.0,
                ui_post: None,
                ui_weight: 0.0,
                ..self.clone()
            };
            let g = src.level_uniform(cam_z);
            let w = weight.clamp(0.0, 1.0);
            let lerp = |a: Vec4, b: Vec4| if graded { a.lerp(b, w) } else { b };
            u.cb_shadows_opacity = lerp(u.cb_shadows_opacity, g.cb_shadows_opacity);
            u.cb_midtones = lerp(u.cb_midtones, g.cb_midtones);
            u.cb_highlights = lerp(u.cb_highlights, g.cb_highlights);
            u.exposure_gamma_desat = lerp(u.exposure_gamma_desat, g.exposure_gamma_desat);
            u.brightness_contrast = lerp(u.brightness_contrast, g.brightness_contrast);
            u.grain = lerp(u.grain, g.grain);
            u.dof = lerp(u.dof, g.dof);
            if !graded {
                // Level post off (`--no-post`): only the grade runs, faded by its weight through
                // the LUT opacity (and the far blur amount).
                u.misc.z = 1.0;
                u.cb_shadows_opacity.w *= w;
                u.dof.z *= w;
                graded = true;
            }
        }
        u
    }

    fn level_uniform(&self, cam_z: f32) -> ArkPost {
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
            misc: Vec4::new(
                crate::coords::UU_TO_BEVY,
                0.0,
                if self.post_on { 1.0 } else { 0.0 },
                if self.post_on && !self.capture && v("m_PpBloomParameters.m_bEnable", 1.0) > 0.5 { 1.0 } else { 0.0 },
            ),
            bloom: {
                // ArkPpBloomParameters defaults: tint white, threshold 0, scale 1.
                let t = post_vector(p, "m_PpBloomParameters.m_Tint").unwrap_or([1.0; 4]);
                let s = v("m_PpBloomParameters.m_Scale", 1.0);
                Vec4::new(t[0] * s, t[1] * s, t[2] * s, -v("m_PpBloomParameters.m_Threshold", 0.0) * s)
            },
            power_vectors: Vec4::ZERO,
            power_pans: Vec4::ZERO,
            dv_eyelid: Vec4::ZERO,
            dv_lid_color: Vec4::ZERO,
            blink_lens: Vec4::ZERO,
            possess: Vec4::ZERO,
            shaft_pos: Vec4::ZERO,
            shaft_params: Vec4::ZERO,
            shaft_bloom: Vec4::ZERO,
            shaft_misc: Vec4::ZERO,
        };
        // UE3 light shafts (`FSceneRenderer` light shaft pass 0x00837480): the first light with
        // `bRenderLightShafts`; the per-light constants are 0x00820300's.
        if let (true, Some(l)) = (self.shafts_on, self.shafts.first()) {
            u.shaft_pos = crate::coords::pos(l.position).extend(1.0);
            u.shaft_params = Vec4::new(1.0 / l.occlusion_depth_range.max(1e-3), l.bloom_scale, 1.5 - l.occlusion_mask_darkness * 0.5, l.occlusion_mask_darkness);
            u.shaft_bloom = Vec4::new(l.bloom_tint[0], l.bloom_tint[1], l.bloom_tint[2], l.bloom_threshold);
            u.shaft_misc = Vec4::new(l.bloom_screen_blend_threshold, 0.0, 0.0, 0.0);
        }
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
        // The light must be in front of the camera (0x00837480 tests the view's plane).
        if n.shaft_pos.w > 0.5 && (n.shaft_pos.truncate() - tf.translation()).dot(*tf.forward()) <= 0.0 {
            n.shaft_pos.w = 0.0;
        }
        *u = n;
    }
}

pub struct ArkPostPlugin;

impl Plugin for ArkPostPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "ark_post.wgsl");
        bevy::asset::embedded_asset!(app, "ark_downsample.wgsl");
        app.add_plugins((ExtractComponentPlugin::<ArkPost>::default(), UniformComponentPlugin::<ArkPost>::default()))
            .add_systems(PostUpdate, update_ark_post)
            .add_systems(Update, configure_depth_usage);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<SpecializedRenderPipelines<ArkPostPipeline>>()
            .init_resource::<crate::bloom::BloomResult>()
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

/// Format of the DOF's half and quarter size copies.
const LOW_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

#[derive(Resource)]
pub struct ArkPostPipeline {
    layout_single: BindGroupLayoutDescriptor,
    layout_multi: BindGroupLayoutDescriptor,
    layout_down: BindGroupLayoutDescriptor,
    sampler: Sampler,
    /// Wrapping sampler of the Bend Time noise (its pans run past 1).
    repeat_sampler: Sampler,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
    down_shader: Handle<Shader>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArkPass {
    /// DisFog + bloom parts.
    Fog,
    /// The DOF's 2x2 box downsample.
    Down,
    /// Far DOF, colour LUT, grain.
    Uber,
    /// Light shafts: the occlusion / bloom downsample (global shader 158), the radial blur (157)
    /// and the apply (155).
    ShaftDown,
    ShaftBlur,
    ShaftApply,
}

/// Format of the light shaft targets: their downsample stores the bloom as x 0.25 and the mask in
/// alpha (the apply multiplies by 4), which fits an 8-bit target.
const SHAFT_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
/// The shaft targets are the scene divided by this (`DAT_0144e320`; keep in step with
/// `SHAFT_FACTOR` in `ark_post.wgsl`).
const SHAFT_FACTOR: u32 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArkPostKey {
    format: TextureFormat,
    multisampled: bool,
    pass: ArkPass,
}

#[derive(Component)]
pub struct ArkPostPipelineId {
    fog: CachedRenderPipelineId,
    down: CachedRenderPipelineId,
    uber: CachedRenderPipelineId,
    shaft_down: CachedRenderPipelineId,
    shaft_blur: CachedRenderPipelineId,
    shaft_apply: CachedRenderPipelineId,
    multisampled: bool,
}

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
                texture_2d(TextureSampleType::Float { filterable: true }),
                // Bend Time's field textures (`water2` noise, wrapping; `fxt_eyelid_gradient` mask).
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                // Possession's field noise (`PPG_PossessionVectors` `Texture2D_0`).
                texture_2d(TextureSampleType::Float { filterable: true }),
            ),
        ),
    )
}

fn init_pipeline(mut commands: Commands, device: Res<RenderDevice>, fullscreen: Res<FullscreenShader>, assets: Res<AssetServer>) {
    commands.insert_resource(ArkPostPipeline {
        layout_single: layout(false),
        layout_multi: layout(true),
        layout_down: BindGroupLayoutDescriptor::new(
            "ark downsample layout",
            &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, (texture_2d(TextureSampleType::Float { filterable: true }),)),
        ),
        sampler: device.create_sampler(&SamplerDescriptor { mag_filter: FilterMode::Linear, min_filter: FilterMode::Linear, ..default() }),
        repeat_sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            address_mode_u: bevy::render::render_resource::AddressMode::Repeat,
            address_mode_v: bevy::render::render_resource::AddressMode::Repeat,
            ..default()
        }),
        fullscreen: fullscreen.clone(),
        shader: load_embedded_asset!(assets.as_ref(), "ark_post.wgsl"),
        down_shader: load_embedded_asset!(assets.as_ref(), "ark_downsample.wgsl"),
    });
}

impl SpecializedRenderPipeline for ArkPostPipeline {
    type Key = ArkPostKey;
    fn specialize(&self, key: ArkPostKey) -> RenderPipelineDescriptor {
        let mut shader_defs = Vec::new();
        if key.multisampled {
            shader_defs.push("MULTISAMPLED".into());
        }
        let (label, layout, shader, entry) = match key.pass {
            ArkPass::Fog => ("ark fog", if key.multisampled { self.layout_multi.clone() } else { self.layout_single.clone() }, self.shader.clone(), "fog"),
            ArkPass::Uber => ("ark uber", if key.multisampled { self.layout_multi.clone() } else { self.layout_single.clone() }, self.shader.clone(), "fragment"),
            ArkPass::Down => ("ark dof downsample", self.layout_down.clone(), self.down_shader.clone(), "downsample"),
            ArkPass::ShaftDown => ("ark shaft down", if key.multisampled { self.layout_multi.clone() } else { self.layout_single.clone() }, self.shader.clone(), "shaft_down"),
            ArkPass::ShaftBlur => ("ark shaft blur", if key.multisampled { self.layout_multi.clone() } else { self.layout_single.clone() }, self.shader.clone(), "shaft_blur"),
            ArkPass::ShaftApply => ("ark shaft apply", if key.multisampled { self.layout_multi.clone() } else { self.layout_single.clone() }, self.shader.clone(), "shaft_apply"),
        };
        RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader,
                shader_defs,
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState { format: key.format, blend: None, write_mask: ColorWrites::ALL })],
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
        let mut id = |format, pass| pipelines.specialize(&cache, &pipeline, ArkPostKey { format, multisampled, pass });
        let ids = ArkPostPipelineId {
            fog: id(view.target_format, ArkPass::Fog),
            down: id(LOW_FORMAT, ArkPass::Down),
            uber: id(view.target_format, ArkPass::Uber),
            shaft_down: id(SHAFT_FORMAT, ArkPass::ShaftDown),
            shaft_blur: id(SHAFT_FORMAT, ArkPass::ShaftBlur),
            shaft_apply: id(view.target_format, ArkPass::ShaftApply),
            multisampled,
        };
        commands.entity(e).insert(ids);
    }
}

fn fullscreen_pass(ctx: &mut RenderContext, label: &str, rp: &bevy::render::render_resource::RenderPipeline, group: &bevy::render::render_resource::BindGroup, offsets: &[u32], dst: &bevy::render::render_resource::TextureView) {
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment { view: dst, depth_slice: None, resolve_target: None, ops: Operations::default() })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_render_pipeline(rp);
    pass.set_bind_group(0, group, offsets);
    pass.draw(0..3, 0..1);
}

/// DisFog (+ bloom), the DOF's two downsamples, then the uber pass.
#[allow(clippy::too_many_arguments)]
pub fn ark_post_pass(
    view: ViewQuery<(&ViewUniformOffset, &ViewTarget, &ViewDepthTexture, &ArkPostPipelineId, &DynamicUniformIndex<ArkPost>, &ArkPost)>,
    cache: Res<PipelineCache>,
    pipeline: Res<ArkPostPipeline>,
    view_uniforms: Res<ViewUniforms>,
    uniforms: Res<ComponentUniforms<ArkPost>>,
    bloom: Res<crate::bloom::BloomResult>,
    zero: Res<bevy::render::texture::FallbackImageZero>,
    mut textures: ResMut<bevy::render::texture::TextureCache>,
    gpu_images: Res<bevy::render::render_asset::RenderAssets<bevy::render::texture::GpuImage>>,
    power_images: Option<Res<crate::power_pp::PowerPpImages>>,
    mut ctx: RenderContext,
) {
    // Bend Time's field textures (black until loaded: no field).
    let image_view = |h: Option<&Handle<Image>>| h.and_then(|h| gpu_images.get(h)).map(|g| g.texture_view.clone()).unwrap_or_else(|| zero.texture_view.clone());
    let noise_view = image_view(power_images.as_ref().and_then(|p| p.noise.as_ref()));
    let mask_view = image_view(power_images.as_ref().and_then(|p| p.mask.as_ref()));
    let possess_view = image_view(power_images.as_ref().and_then(|p| p.possession.as_ref()));
    let (view_offset, target, depth, id, index, ark) = view.into_inner();
    let (Some(fog_rp), Some(down_rp), Some(uber_rp), Some(view_binding), Some(uniform_binding)) =
        (cache.get_render_pipeline(id.fog), cache.get_render_pipeline(id.down), cache.get_render_pipeline(id.uber), view_uniforms.uniforms.binding(), uniforms.binding())
    else {
        return;
    };
    let device = ctx.render_device().clone();
    let layout = cache.get_bind_group_layout(if id.multisampled { &pipeline.layout_multi } else { &pipeline.layout_single });
    let offsets = [view_offset.offset, index.index()];
    // 0. Light shafts (0x00837480): downsample the scene into the occlusion mask / bloom target,
    // radial blur it towards the light, apply it to the scene.
    if ark.shaft_pos.w > 0.5 {
        if let (Some(down_rp), Some(blur_rp), Some(apply_rp)) = (cache.get_render_pipeline(id.shaft_down), cache.get_render_pipeline(id.shaft_blur), cache.get_render_pipeline(id.shaft_apply)) {
            let size = target.main_texture().size();
            let (w, h) = ((size.width / SHAFT_FACTOR).max(1), (size.height / SHAFT_FACTOR).max(1));
            let mut make = |label: &'static str| {
                textures.get(
                    &device,
                    TextureDescriptor {
                        label: Some(label),
                        size: Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: TextureDimension::D2,
                        format: SHAFT_FORMAT,
                        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                )
            };
            let (a, b) = (make("ark shaft a"), make("ark shaft b"));
            let post = target.post_process_write();
            let bind = |label: &str, src: &bevy::render::render_resource::TextureView, aux: &bevy::render::render_resource::TextureView| {
                device.create_bind_group(
                    Some(label),
                    &layout,
                    &BindGroupEntries::sequential((view_binding.clone(), depth.view(), src, &pipeline.sampler, uniform_binding.clone(), aux, &noise_view, &mask_view, &pipeline.repeat_sampler, &possess_view)),
                )
            };
            let g = bind("ark shaft down", post.source, &zero.texture_view);
            fullscreen_pass(&mut ctx, "ark shaft down", down_rp, &g, &offsets, &a.default_view);
            let g = bind("ark shaft blur", &a.default_view, &zero.texture_view);
            fullscreen_pass(&mut ctx, "ark shaft blur", blur_rp, &g, &offsets, &b.default_view);
            let g = bind("ark shaft apply", post.source, &b.default_view);
            fullscreen_pass(&mut ctx, "ark shaft apply", apply_rp, &g, &offsets, post.destination);
        }
    }
    // 1. DisFog and the bloom parts.
    let bloom_view = bloom.view.clone().unwrap_or_else(|| zero.texture_view.clone());
    let post = target.post_process_write();
    let group = device.create_bind_group(
        Some("ark fog bind group"),
        &layout,
        &BindGroupEntries::sequential((view_binding.clone(), depth.view(), post.source, &pipeline.sampler, uniform_binding.clone(), &bloom_view, &noise_view, &mask_view, &pipeline.repeat_sampler, &possess_view)),
    );
    fullscreen_pass(&mut ctx, "ark fog", fog_rp, &group, &offsets, post.destination);
    let fogged = post.destination.clone();
    // 2. The DOF's quarter-size copy of the fogged scene (only when it is used).
    let size = target.main_texture().size();
    let mut low = zero.texture_view.clone();
    if ark.misc.z > 0.5 && ark.dof.z > 0.0 {
        let mut src = fogged;
        let (mut w, mut h) = (size.width, size.height);
        for label in ["ark dof half", "ark dof quarter"] {
            (w, h) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
            let t = textures.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size: Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: LOW_FORMAT,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            );
            let g = device.create_bind_group(Some(label), &cache.get_bind_group_layout(&pipeline.layout_down), &BindGroupEntries::sequential((&src,)));
            fullscreen_pass(&mut ctx, label, down_rp, &g, &[], &t.default_view);
            src = t.default_view.clone();
        }
        low = src;
    }
    // 3. Far DOF, colour LUT and grain.
    let post = target.post_process_write();
    let group = device.create_bind_group(
        Some("ark uber bind group"),
        &layout,
        &BindGroupEntries::sequential((view_binding, depth.view(), post.source, &pipeline.sampler, uniform_binding, &low, &noise_view, &mask_view, &pipeline.repeat_sampler, &possess_view)),
    );
    fullscreen_pass(&mut ctx, "ark uber", uber_rp, &group, &offsets, post.destination);
}
