//! UE3's distortion (refraction, heat-haze) pass, decoded from the original's shaders
//! (research/LOG.md "Distortion pass"; research/dishonored/EFFECTS.md "Distortion").
//!
//! Materials with `bUsesDistortion` / `bHasDistortion` (Arkane's particle bases switch it on with
//! `Dist_Enable_Distortion`) are drawn a second time after translucency by the material's
//! distortion-accumulate pixel shader (`RefShaderCache-PC-D3D-SM3.upk`, e.g. shaders 36064 /
//! 36065 for `DefaultAdditive_PMAT`):
//!
//! 1. D = the material's `Distortion` input (Arkane: `Dist_Distortion` texture x
//!    `Dist_Distortion_Intensity` x vertex colour alpha x the `O_Opacity` texture), in units of
//!    1/255 of the screen;
//! 2. pixels with |D|^2 < 0.1 are discarded;
//! 3. D is zeroed where the scene depth at the displaced pixel (`uv + (Dx, -Dy) / 255`) is in
//!    front of the distorting primitive;
//! 4. E = clamp(4 D / 255, -1, 1) is written to an RGBA8 target with additive blending, positive
//!    parts in RG and negated negative parts in BA.
//!
//! The global `AccumulatedDistortionTexture` pixel shader (global cache shader 237) then copies the
//! scene colour at `uv + (RG - BA) * (0.25, -0.25)`, or at `uv` itself when that leaves
//! `SceneColorRect`. Both passes are reproduced here; the material graph between the parameters is
//! stripped from the cooked packages, so `DistortionMaterial` keeps the terms read from the
//! compiled shaders (U-401 lists what is not decoded).

use bevy::asset::{load_embedded_asset, AssetServer, Handle};
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::core_pipeline::FullscreenShader;
use bevy::mesh::{VertexBufferLayout, VertexFormat};
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent, BlendFactor, BlendOperation, BlendState, BufferInitDescriptor, BufferUsages,
    CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState, LoadOp, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, SpecializedRenderPipeline, SpecializedRenderPipelines,
    StoreOp, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages, VertexState, VertexStepMode,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::{FallbackImage, GpuImage, TextureCache};
use bevy::render::view::{ExtractedView, Msaa, ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;
use dishonored_ir::{MaterialParams, TextureId};

/// `DefaultAdditive_PMAT`'s `Dist_Distortion_Intensity` expression default.
const DEFAULT_INTENSITY: f32 = 1.0;

/// The distortion terms of one material, decoded from its parameters and static switches.
/// The uniform expressions of the `DefaultAdditive_PMAT` shader maps (RefShaderCache
/// `FUniformExpressionSet`s, e.g. the mesh-emitter permutation of shader 36094) give:
/// D = `O_Opacity.r`(uv + (U, V scrolling) x Time) x `D_DiffuseColorTweak.a` x
/// `MeshEmitterVertexColor.a` x vertex colour x `Dist_Distortion.r`(uv + frac(Time x
/// `Dist_Distortion_Speed`) on both axes) x `Dist_Distortion_Intensity`.
#[derive(Debug, Clone, PartialEq)]
pub struct DistortionMaterial {
    /// `Dist_Distortion`: the texture whose red channel is the distortion amount.
    pub texture: TextureId,
    /// `Dist_Distortion_Intensity` (instance value, else the base default 1).
    pub intensity: f32,
    /// `Dist_Distortion_Speed` (base default 1): the distortion texture pans by
    /// frac(Time x speed) along U and V.
    pub pan_speed: f32,
    /// `O_Opacity` multiplies D unless `D_Disable_Opacity_Texture`.
    pub opacity: Option<TextureId>,
    /// `Enable_UV_Distortion`: the distortion texture is read at
    /// `uv + UV_Distortion.rg x UV_Distortion_Intensity`, the UV-distortion texture panned by
    /// frac(Time x `UV_Distortion_Speed`): (texture, intensity, speed).
    pub uv_distortion: Option<(TextureId, f32, f32)>,
    /// `Enable_UV_Scrolling`: (`U_Scrolling_Speed`, `V_Scrolling_Speed`) per second, scrolling
    /// the opacity texture.
    pub scroll: [f32; 2],
    /// `D_DiffuseColorTweak.a` (base default 1).
    pub tweak_alpha: f32,
    /// `Enable_Vertex_Color` (on by default): D is scaled by the particle's colour alpha (mesh
    /// emitters always, through `MeshEmitterVertexColor`).
    pub vertex_alpha: bool,
    /// `D_Disable_Diffuse`: the material draws nothing in the colour pass.
    pub distortion_only: bool,
}

impl DistortionMaterial {
    /// Decode a material's distortion; `None` unless `Dist_Enable_Distortion` is on and the
    /// material has a `Dist_Distortion` texture.
    pub fn decode(p: &MaterialParams, textures: &[(String, TextureId)]) -> Option<Self> {
        if p.switch("Dist_Enable_Distortion") != Some(true) {
            return None;
        }
        let tex = |n: &str| textures.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|v| v.1);
        let scalar = |n: &str, d: f32| p.scalars.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or(d);
        let on = |n: &str| p.switch(n) == Some(true);
        Some(Self {
            texture: tex("Dist_Distortion")?,
            intensity: scalar("Dist_Distortion_Intensity", DEFAULT_INTENSITY),
            pan_speed: scalar("Dist_Distortion_Speed", 1.0),
            opacity: if on("D_Disable_Opacity_Texture") { None } else { tex("O_Opacity") },
            uv_distortion: if on("Enable_UV_Distortion") { tex("UV_Distortion").map(|t| (t, scalar("UV_Distortion_Intensity", 1.0), scalar("UV_Distortion_Speed", 1.0))) } else { None },
            scroll: if on("Enable_UV_Scrolling") { [scalar("U_Scrolling_Speed", 0.0), scalar("V_Scrolling_Speed", 0.0)] } else { [0.0; 2] },
            tweak_alpha: p.vectors.iter().find(|(k, _)| k.eq_ignore_ascii_case("D_DiffuseColorTweak")).map(|v| v.1[3]).unwrap_or(1.0),
            vertex_alpha: p.switch("Enable_Vertex_Color") != Some(false),
            distortion_only: on("D_Disable_Diffuse"),
        })
    }

    /// The accumulate shader's output for one pixel: `None` when discarded, else the RGBA
    /// value added to the offset target (positive offsets in RG, negated negative ones in BA).
    /// `d` is the material's distortion in 1/255 screen units; `occluded` = the scene at the
    /// displaced pixel is in front of the primitive.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn accumulate(d: Vec2, occluded: bool) -> Option<Vec4> {
        if d.dot(d) - 0.1 < 0.0 {
            return None;
        }
        let e = if occluded { Vec2::ZERO } else { (d * 4.0 / 255.0).clamp(Vec2::splat(-1.0), Vec2::splat(1.0)) };
        Some(Vec4::new(e.x.max(0.0), e.y.max(0.0), (-e.x).max(0.0), (-e.y).max(0.0)))
    }

    /// The apply shader: the scene colour coordinate for screen `uv` given the accumulated
    /// offset value (saturated, as the RGBA8 target is).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn apply(uv: Vec2, acc: Vec4) -> Vec2 {
        let a = acc.min(Vec4::ONE);
        let n = uv + Vec2::new(a.x - a.z, a.y - a.w) * Vec2::new(0.25, -0.25);
        if n.x < 0.0 || n.y < 0.0 || n.x > 1.0 || n.y > 1.0 { uv } else { n }
    }
}

/// Floats per vertex: position (Bevy world) 3, uv 2, distortion uv offset 2, UV-distortion uv
/// offset 2, opacity uv offset 2, (intensity x tweak alpha x vertex alpha, opacity on,
/// UV-distortion intensity, 0) 4.
pub const VERTEX_FLOATS: usize = 15;

/// One draw of the accumulate pass (all triangles of one material).
#[derive(Clone)]
pub struct DistortionBatch {
    pub texture: Handle<Image>,
    pub opacity: Option<Handle<Image>>,
    pub uv_distortion: Option<Handle<Image>>,
    /// First vertex and vertex count in `DistortionFrame::verts` (triangle list).
    pub first: u32,
    pub count: u32,
}

/// This frame's distorting primitives, rebuilt by the particle simulation.
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct DistortionFrame {
    pub verts: Vec<f32>,
    pub batches: Vec<DistortionBatch>,
}

impl DistortionFrame {
    pub fn clear(&mut self) {
        self.verts.clear();
        self.batches.clear();
    }

    /// Append a triangle list for one material: positions in Bevy world space, their uvs and
    /// particle alphas (`mesh`: a mesh emitter, whose alpha always applies through
    /// `MeshEmitterVertexColor`). `time` (seconds) drives the pans and the scrolling.
    pub fn push(&mut self, m: &DistortionMaterial, images: (&Handle<Image>, Option<&Handle<Image>>, Option<&Handle<Image>>), tris: impl Iterator<Item = (Vec3, Vec2, f32)>, time: f32, mesh: bool) {
        let first = (self.verts.len() / VERTEX_FLOATS) as u32;
        // The textures repeat: keep the offsets in [0, 1) (the original's `Periodic` is frac).
        let frac = |v: Vec2| v - v.floor();
        let scroll = frac(Vec2::from(m.scroll) * time);
        let pan = frac(Vec2::splat(m.pan_speed * time));
        let (uvd, uvd_pan) = m.uv_distortion.map(|v| (v.1, frac(Vec2::splat(v.2 * time)))).unwrap_or((0.0, Vec2::ZERO));
        let opacity_on = if m.opacity.is_some() { 1.0 } else { 0.0 };
        for (p, uv, a) in tris {
            let k = m.intensity * m.tweak_alpha * if m.vertex_alpha || mesh { a } else { 1.0 };
            self.verts.extend_from_slice(&[p.x, p.y, p.z, uv.x, uv.y, pan.x, pan.y, uvd_pan.x, uvd_pan.y, scroll.x, scroll.y, k, opacity_on, uvd, 0.0]);
        }
        let count = (self.verts.len() / VERTEX_FLOATS) as u32 - first;
        if count == 0 {
            return;
        }
        self.batches.push(DistortionBatch { texture: images.0.clone(), opacity: images.1.cloned(), uv_distortion: images.2.cloned(), first, count });
    }
}

/// Cameras that run the distortion pass (the game camera).
#[derive(Component, Clone, Copy, Default, ExtractComponent)]
pub struct DistortionView;

pub struct DistortionPlugin;

impl Plugin for DistortionPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "distortion.wgsl");
        bevy::asset::embedded_asset!(app, "distortion_apply.wgsl");
        app.init_resource::<DistortionFrame>()
            .add_plugins((ExtractResourcePlugin::<DistortionFrame>::default(), ExtractComponentPlugin::<DistortionView>::default()))
            .add_systems(Update, (tag_game_camera, configure_depth_usage));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<SpecializedRenderPipelines<DistortionPipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(Core3d, distortion_pass.in_set(Core3dSystems::EarlyPostProcess));
    }
}

/// `--no-distortion` (render comparisons) leaves the pass out.
fn tag_game_camera(mut commands: Commands, q: Query<Entity, (With<crate::players::GameCamera>, Without<DistortionView>)>, mut off: Local<Option<bool>>, settings: Option<Res<crate::settings::GameSettings>>) {
    if *off.get_or_insert_with(|| std::env::args().any(|a| a == "--no-distortion")) || settings.is_some_and(|s| !s.s.graphics.distortion) {
        return;
    }
    for e in &q {
        commands.entity(e).insert(DistortionView);
    }
}

/// The accumulate pass reads the main depth buffer, which Bevy only makes sampleable on request.
fn configure_depth_usage(mut q: Query<&mut Camera3d, With<DistortionView>>) {
    for mut c in &mut q {
        let u = TextureUsages::from(c.depth_texture_usages);
        if !u.contains(TextureUsages::TEXTURE_BINDING) {
            c.depth_texture_usages = (u | TextureUsages::TEXTURE_BINDING).into();
        }
    }
}

const OFFSET_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

#[derive(Resource)]
pub struct DistortionPipeline {
    view_single: BindGroupLayoutDescriptor,
    view_multi: BindGroupLayoutDescriptor,
    material: BindGroupLayoutDescriptor,
    apply: BindGroupLayoutDescriptor,
    linear_repeat: Sampler,
    linear_clamp: Sampler,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
    apply_shader: Handle<Shader>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DistortionKey {
    /// None: the accumulate pipeline (with multisampled depth or not); Some: the apply pipeline.
    apply_format: Option<TextureFormat>,
    multisampled: bool,
}

#[derive(Component)]
pub struct DistortionPipelineIds {
    accumulate: CachedRenderPipelineId,
    apply: CachedRenderPipelineId,
    multisampled: bool,
}

fn view_layout(multisampled: bool) -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "distortion view layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (uniform_buffer::<ViewUniform>(true), if multisampled { texture_depth_2d_multisampled() } else { texture_depth_2d() }),
        ),
    )
}

fn init_pipeline(mut commands: Commands, device: Res<RenderDevice>, fullscreen: Res<FullscreenShader>, assets: Res<AssetServer>) {
    let tex = || texture_2d(TextureSampleType::Float { filterable: true });
    commands.insert_resource(DistortionPipeline {
        view_single: view_layout(false),
        view_multi: view_layout(true),
        material: BindGroupLayoutDescriptor::new(
            "distortion material layout",
            &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, (tex(), tex(), tex(), sampler(SamplerBindingType::Filtering))),
        ),
        apply: BindGroupLayoutDescriptor::new(
            "distortion apply layout",
            &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, (tex(), tex(), sampler(SamplerBindingType::Filtering))),
        ),
        linear_repeat: device.create_sampler(&SamplerDescriptor {
            address_mode_u: bevy::render::render_resource::AddressMode::Repeat,
            address_mode_v: bevy::render::render_resource::AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        linear_clamp: device.create_sampler(&SamplerDescriptor { mag_filter: FilterMode::Linear, min_filter: FilterMode::Linear, ..default() }),
        fullscreen: fullscreen.clone(),
        shader: load_embedded_asset!(assets.as_ref(), "distortion.wgsl"),
        apply_shader: load_embedded_asset!(assets.as_ref(), "distortion_apply.wgsl"),
    });
}

impl SpecializedRenderPipeline for DistortionPipeline {
    type Key = DistortionKey;
    fn specialize(&self, key: DistortionKey) -> RenderPipelineDescriptor {
        let mut shader_defs = Vec::new();
        if key.multisampled {
            shader_defs.push("MULTISAMPLED".into());
        }
        if let Some(format) = key.apply_format {
            // `DISH_DISTORTION_DEBUG`: show the accumulated offsets over a dimmed scene.
            if std::env::var_os("DISH_DISTORTION_DEBUG").is_some() {
                shader_defs.push("DISTORTION_DEBUG".into());
            }
            return RenderPipelineDescriptor {
                label: Some("distortion apply".into()),
                layout: vec![self.apply.clone()],
                vertex: self.fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: self.apply_shader.clone(),
                    shader_defs,
                    entry_point: Some("apply".into()),
                    targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                }),
                ..default()
            };
        }
        let add = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
        RenderPipelineDescriptor {
            label: Some("distortion accumulate".into()),
            layout: vec![if key.multisampled { self.view_multi.clone() } else { self.view_single.clone() }, self.material.clone()],
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: shader_defs.clone(),
                entry_point: Some("vertex".into()),
                buffers: vec![VertexBufferLayout::from_vertex_formats(
                    VertexStepMode::Vertex,
                    [VertexFormat::Float32x3, VertexFormat::Float32x2, VertexFormat::Float32x2, VertexFormat::Float32x2, VertexFormat::Float32x2, VertexFormat::Float32x4],
                )],
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs,
                entry_point: Some("accumulate".into()),
                targets: vec![Some(ColorTargetState { format: OFFSET_FORMAT, blend: Some(BlendState { color: add, alpha: add }), write_mask: ColorWrites::ALL })],
            }),
            ..default()
        }
    }
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<DistortionPipeline>>,
    pipeline: Option<Res<DistortionPipeline>>,
    views: Query<(Entity, &ExtractedView, &Msaa), With<DistortionView>>,
) {
    let Some(pipeline) = pipeline else { return };
    for (e, view, msaa) in &views {
        let multisampled = *msaa != Msaa::Off;
        let accumulate = pipelines.specialize(&cache, &pipeline, DistortionKey { apply_format: None, multisampled });
        let apply = pipelines.specialize(&cache, &pipeline, DistortionKey { apply_format: Some(view.target_format), multisampled: false });
        commands.entity(e).insert(DistortionPipelineIds { accumulate, apply, multisampled });
    }
}

#[allow(clippy::too_many_arguments)]
pub fn distortion_pass(
    view: ViewQuery<(&ViewUniformOffset, &ViewTarget, &ViewDepthTexture, &DistortionPipelineIds)>,
    frame: Res<DistortionFrame>,
    cache: Res<PipelineCache>,
    pipeline: Res<DistortionPipeline>,
    view_uniforms: Res<ViewUniforms>,
    images: Res<RenderAssets<GpuImage>>,
    fallback: Res<FallbackImage>,
    mut textures: ResMut<TextureCache>,
    mut ctx: RenderContext,
) {
    static FRAMES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let log = std::env::var_os("DISH_DISTORTION_DEBUG").is_some() && FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 30 == 0;
    if log {
        info!("distortion: {} batches, {} vertices", frame.batches.len(), frame.verts.len() / VERTEX_FLOATS);
    }
    if frame.batches.is_empty() {
        return;
    }
    let (view_offset, target, depth, ids) = view.into_inner();
    let (Some(acc_rp), Some(apply_rp), Some(view_binding)) = (cache.get_render_pipeline(ids.accumulate), cache.get_render_pipeline(ids.apply), view_uniforms.uniforms.binding()) else {
        if log {
            info!("distortion: pipelines not ready");
        }
        return;
    };
    let device = ctx.render_device().clone();
    let size = target.main_texture().size();
    let offsets = textures.get(
        &device,
        TextureDescriptor {
            label: Some("distortion offsets"),
            size: Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: OFFSET_FORMAT,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
    );
    let bytes: Vec<u8> = frame.verts.iter().flat_map(|f| f.to_le_bytes()).collect();
    let vbuf = device.create_buffer_with_data(&BufferInitDescriptor { label: Some("distortion vertices"), contents: &bytes, usage: BufferUsages::VERTEX });
    let view_layout = if ids.multisampled { &pipeline.view_multi } else { &pipeline.view_single };
    let view_group = device.create_bind_group(Some("distortion view"), &cache.get_bind_group_layout(view_layout), &BindGroupEntries::sequential((view_binding, depth.view())));
    let white = &fallback.d2.texture_view;
    let view_of = |h: Option<&Handle<Image>>| h.and_then(|h| images.get(h)).map(|g| &g.texture_view);
    let mut groups = Vec::new();
    for b in &frame.batches {
        let Some(t) = view_of(Some(&b.texture)) else { continue };
        let o = view_of(b.opacity.as_ref()).unwrap_or(white);
        let u = view_of(b.uv_distortion.as_ref()).unwrap_or(white);
        let g = device.create_bind_group(Some("distortion material"), &cache.get_bind_group_layout(&pipeline.material), &BindGroupEntries::sequential((t, o, u, &pipeline.linear_repeat)));
        groups.push((g, b.first, b.count));
    }
    {
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("distortion accumulate"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &offsets.default_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Default::default()), store: StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_render_pipeline(acc_rp);
        pass.set_bind_group(0, &view_group, &[view_offset.offset]);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        for (g, first, count) in &groups {
            pass.set_bind_group(1, g, &[]);
            pass.draw(*first..*first + *count, 0..1);
        }
    }
    let post = target.post_process_write();
    let apply_group = device.create_bind_group(
        Some("distortion apply"),
        &cache.get_bind_group_layout(&pipeline.apply),
        &BindGroupEntries::sequential((post.source, &offsets.default_view, &pipeline.linear_clamp)),
    );
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("distortion apply"),
        color_attachments: &[Some(RenderPassColorAttachment { view: post.destination, depth_slice: None, resolve_target: None, ops: Operations::default() })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_render_pipeline(apply_rp);
    pass.set_bind_group(0, &apply_group, &[]);
    pass.draw(0..3, 0..1);
}
