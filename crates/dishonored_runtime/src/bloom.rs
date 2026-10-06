//! Arkane's material bloom ("bloom parts"), decoded from the original's shaders and
//! `Dishonored.exe` (research/LOG.md "Bloom parts"; U-323).
//!
//! Dishonored has no threshold bloom of the scene: materials draw a bloom colour, and only that
//! is blurred and added. Per frame (0x009251A0, "BloomParts" / "Bloom reduction and blur" /
//! "Bloom compose"):
//!
//! 1. "Draw blooming primitives": every primitive with bloom relevance is drawn again with its
//!    material's `TBloomPartMeshPixelShader<FBloomPartMeshPolicy>` into a scene-size target
//!    (LogLuv-encoded RGBA8 in the original; a float target here). The generic materials'
//!    bloom colour (RefShaderCache shader maps, e.g. `generic_PMAT` with `B_Enable_Bloom`) is
//!    `E_Emissive` (its masked channel) `[x E_Emissive_Color.rgb x a with
//!    E_Enable_Emissive_color] x B_Bloom_Intensity` (+ `SelectionColor`, 0 in game); without
//!    `E_Enable_Emissive` it is `0 x B_Bloom_Intensity`.
//! 2. `FBloomDownSamplePixelShader` (global shader 84) to a quarter-size target: four taps at
//!    +-1/4 of a quarter-size pixel, LogLuv-decoded and averaged, `x gBloomTint.rgb +
//!    gBloomTint.w`, at least 0; `gBloomTint` = (m_Tint.rgb x m_Scale, -m_Threshold x m_Scale)
//!    of the view's `ArkPpBloomParameters` (setter 0x0090DE60).
//! 3. "ArkGaussian" (0x0091FE50): `TBloomBlurPixelShader` (global 82, vertex 83) once along X,
//!    once along Y at quarter size: taps at 0, +-1.3973, +-3.3973 texels, weights 0.2416, 0.2643,
//!    0.1149.
//! 4. The DisFog pass adds the result (`sBloomParts`) after the fog (`FBloomComposePixelShader`,
//!    scene + bloom, when there is no fog pass); see `ark_post.rs`.

use crate::ark_post::ArkPost;
use bevy::asset::{load_embedded_asset, AssetId, AssetServer, Handle};
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::core_pipeline::FullscreenShader;
use bevy::mesh::{VertexBufferLayout, VertexFormat};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer, uniform_buffer_sized};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer, BufferInitDescriptor, BufferUsages, CachedRenderPipelineId, ColorTargetState, ColorWrites,
    Extent3d, FilterMode, FragmentState, LoadOp, Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler,
    SamplerBindingType, SamplerDescriptor, ShaderStages, SpecializedRenderPipeline, SpecializedRenderPipelines, StoreOp, TextureDescriptor, TextureDimension,
    TextureFormat, TextureSampleType, TextureUsages, TextureView, VertexState, VertexStepMode,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::{GpuImage, TextureCache};
use bevy::render::view::{ExtractedView, Msaa, ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;
use dishonored_ir::MaterialParams;
use std::sync::Arc;

/// Floats per bloom-part vertex: Bevy world position, UV0.
pub const VERTEX_FLOATS: usize = 5;

/// A material's bloom colour factor (`rgb` multiplies its emissive texture), or None when it
/// draws no bloom part (see the module docs).
pub fn bloom_color(p: &MaterialParams) -> Option<Vec3> {
    if p.switch("B_Enable_Bloom") != Some(true) || p.emissive.is_none() || p.switch("E_Enable_Emissive") == Some(false) {
        return None;
    }
    let colour = p
        .vectors
        .iter()
        .find(|(n, _)| n.trim().eq_ignore_ascii_case("E_Emissive_Color"))
        .map(|v| v.1)
        .filter(|_| p.switch("E_Enable_Emissive_color") == Some(true))
        .map(|c| Vec3::new(c[0], c[1], c[2]) * c[3])
        .unwrap_or(Vec3::ONE);
    // generic_PMAT's B_Bloom_Intensity (default 1); generic_masked_SingleSided_PMAT has none.
    let k = p.scalars.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case("B_Bloom_Intensity")).map(|v| v.1).unwrap_or(1.0);
    let c = colour * k;
    (c.max_element() > 0.0).then_some(c)
}

/// One material's bloom part triangles (world space) and its emissive texture.
#[derive(Clone)]
pub struct BloomBatch {
    pub verts: Arc<Vec<f32>>,
    pub texture: Handle<Image>,
    pub color: Vec3,
}

/// A level's bloom parts (hidden with the level).
#[derive(Component, Clone, Default)]
pub struct BloomParts {
    pub batches: Vec<BloomBatch>,
}

/// The visible bloom parts, in the render world.
#[derive(Resource, Default)]
pub struct ExtractedBloom {
    batches: Vec<(Arc<Vec<f32>>, AssetId<Image>, Vec3)>,
}

/// The blurred quarter-size bloom of the current view (None: no bloom this frame), read by
/// `ark_post`'s DisFog pass.
#[derive(Resource, Default)]
pub struct BloomResult {
    pub view: Option<TextureView>,
}

pub struct BloomPlugin;

impl Plugin for BloomPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "bloom.wgsl");
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_resource::<ExtractedBloom>()
            .init_resource::<BloomResult>()
            .init_resource::<BloomBuffers>()
            .init_resource::<SpecializedRenderPipelines<BloomPipeline>>()
            .add_systems(ExtractSchedule, extract_bloom)
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(Core3d, bloom_pass.in_set(Core3dSystems::PostProcess).before(crate::ark_post::ark_post_pass));
    }
}

/// `--no-bloom` (render comparisons) leaves the bloom parts out.
fn extract_bloom(mut out: ResMut<ExtractedBloom>, q: Extract<Query<(&BloomParts, &InheritedVisibility)>>, mut off: Local<Option<bool>>) {
    out.batches.clear();
    if *off.get_or_insert_with(|| std::env::args().any(|a| a == "--no-bloom")) {
        return;
    }
    for (parts, vis) in &q {
        if !vis.get() {
            continue;
        }
        out.batches.extend(parts.batches.iter().map(|b| (b.verts.clone(), b.texture.id(), b.color)));
    }
}

/// Vertex buffers of the bloom parts, kept while their vertex arrays live.
#[derive(Resource, Default)]
pub struct BloomBuffers {
    buffers: Vec<(Arc<Vec<f32>>, Buffer)>,
}

const BLOOM_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

#[derive(Resource)]
pub struct BloomPipeline {
    view_single: BindGroupLayoutDescriptor,
    view_multi: BindGroupLayoutDescriptor,
    part: BindGroupLayoutDescriptor,
    down: BindGroupLayoutDescriptor,
    blur: BindGroupLayoutDescriptor,
    linear_repeat: Sampler,
    linear_clamp: Sampler,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum BloomPass {
    Parts { multisampled: bool },
    Down,
    BlurX,
    BlurY,
}

#[derive(Component)]
pub struct BloomPipelineIds {
    parts: CachedRenderPipelineId,
    down: CachedRenderPipelineId,
    blur_x: CachedRenderPipelineId,
    blur_y: CachedRenderPipelineId,
    multisampled: bool,
}

fn init_pipeline(mut commands: Commands, device: Res<RenderDevice>, fullscreen: Res<FullscreenShader>, assets: Res<AssetServer>) {
    let tex = || texture_2d(TextureSampleType::Float { filterable: true });
    let view = |multisampled: bool| {
        BindGroupLayoutDescriptor::new(
            "bloom view layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (uniform_buffer::<ViewUniform>(true), if multisampled { texture_depth_2d_multisampled() } else { texture_depth_2d() }),
            ),
        )
    };
    let colour = || uniform_buffer_sized(false, std::num::NonZeroU64::new(16));
    commands.insert_resource(BloomPipeline {
        view_single: view(false),
        view_multi: view(true),
        part: BindGroupLayoutDescriptor::new("bloom part layout", &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, (tex(), sampler(SamplerBindingType::Filtering), colour()))),
        down: BindGroupLayoutDescriptor::new("bloom downsample layout", &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, (tex(), colour()))),
        blur: BindGroupLayoutDescriptor::new("bloom blur layout", &BindGroupLayoutEntries::sequential(ShaderStages::FRAGMENT, (tex(), sampler(SamplerBindingType::Filtering)))),
        linear_repeat: device.create_sampler(&SamplerDescriptor {
            address_mode_u: bevy::render::render_resource::AddressMode::Repeat,
            address_mode_v: bevy::render::render_resource::AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: bevy::render::render_resource::MipmapFilterMode::Linear,
            ..default()
        }),
        linear_clamp: device.create_sampler(&SamplerDescriptor { mag_filter: FilterMode::Linear, min_filter: FilterMode::Linear, ..default() }),
        fullscreen: fullscreen.clone(),
        shader: load_embedded_asset!(assets.as_ref(), "bloom.wgsl"),
    });
}

impl SpecializedRenderPipeline for BloomPipeline {
    type Key = BloomPass;
    fn specialize(&self, key: BloomPass) -> RenderPipelineDescriptor {
        let target = vec![Some(ColorTargetState { format: BLOOM_FORMAT, blend: None, write_mask: ColorWrites::ALL })];
        let full = |label: &str, layout: &BindGroupLayoutDescriptor, defs: Vec<&str>, entry: &str| RenderPipelineDescriptor {
            label: Some(label.to_string().into()),
            layout: vec![layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: defs.into_iter().map(Into::into).collect(),
                entry_point: Some(entry.to_string().into()),
                targets: target.clone(),
            }),
            ..default()
        };
        match key {
            BloomPass::Down => full("bloom downsample", &self.down, vec!["DOWNSAMPLE"], "downsample"),
            BloomPass::BlurX => full("bloom blur x", &self.blur, vec!["BLUR", "HORIZONTAL"], "blur"),
            BloomPass::BlurY => full("bloom blur y", &self.blur, vec!["BLUR"], "blur"),
            BloomPass::Parts { multisampled } => {
                let mut defs: Vec<bevy::shader::ShaderDefVal> = vec!["PARTS".into()];
                if multisampled {
                    defs.push("MULTISAMPLED".into());
                }
                RenderPipelineDescriptor {
                    label: Some("bloom parts".into()),
                    layout: vec![if multisampled { self.view_multi.clone() } else { self.view_single.clone() }, self.part.clone()],
                    vertex: VertexState {
                        shader: self.shader.clone(),
                        shader_defs: defs.clone(),
                        entry_point: Some("part_vertex".into()),
                        buffers: vec![VertexBufferLayout::from_vertex_formats(VertexStepMode::Vertex, [VertexFormat::Float32x3, VertexFormat::Float32x2])],
                    },
                    fragment: Some(FragmentState { shader: self.shader.clone(), shader_defs: defs, entry_point: Some("part_fragment".into()), targets: target }),
                    ..default()
                }
            }
        }
    }
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<BloomPipeline>>,
    pipeline: Option<Res<BloomPipeline>>,
    views: Query<(Entity, &Msaa), (With<ExtractedView>, With<ArkPost>)>,
) {
    let Some(pipeline) = pipeline else { return };
    for (e, msaa) in &views {
        let multisampled = *msaa != Msaa::Off;
        let mut id = |k| pipelines.specialize(&cache, &pipeline, k);
        let ids = BloomPipelineIds { parts: id(BloomPass::Parts { multisampled }), down: id(BloomPass::Down), blur_x: id(BloomPass::BlurX), blur_y: id(BloomPass::BlurY), multisampled };
        commands.entity(e).insert(ids);
    }
}

fn target(textures: &mut TextureCache, device: &RenderDevice, label: &'static str, w: u32, h: u32) -> TextureView {
    textures
        .get(
            device,
            TextureDescriptor {
                label: Some(label),
                size: Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: BLOOM_FORMAT,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )
        .default_view
}

fn colour_pass<'a>(ctx: &'a mut RenderContext, label: &'a str, dst: &'a TextureView) -> bevy::render::render_phase::TrackedRenderPass<'a> {
    ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: dst,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Clear(Default::default()), store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn bloom_pass(
    view: ViewQuery<(&ViewUniformOffset, &ViewTarget, &ViewDepthTexture, &BloomPipelineIds, &ArkPost)>,
    extracted: Res<ExtractedBloom>,
    mut buffers: ResMut<BloomBuffers>,
    mut result: ResMut<BloomResult>,
    cache: Res<PipelineCache>,
    pipeline: Res<BloomPipeline>,
    view_uniforms: Res<ViewUniforms>,
    images: Res<RenderAssets<GpuImage>>,
    mut textures: ResMut<TextureCache>,
    mut ctx: RenderContext,
) {
    result.view = None;
    let (view_offset, target_tex, depth, ids, ark) = view.into_inner();
    if extracted.batches.is_empty() || ark.misc.w < 0.5 {
        return;
    }
    let (Some(parts_rp), Some(down_rp), Some(bx_rp), Some(by_rp), Some(view_binding)) = (
        cache.get_render_pipeline(ids.parts),
        cache.get_render_pipeline(ids.down),
        cache.get_render_pipeline(ids.blur_x),
        cache.get_render_pipeline(ids.blur_y),
        view_uniforms.uniforms.binding(),
    ) else {
        return;
    };
    let device = ctx.render_device().clone();
    // Vertex buffers: keep those still drawn, upload new ones.
    buffers.buffers.retain(|(v, _)| extracted.batches.iter().any(|b| Arc::ptr_eq(&b.0, v)));
    for (v, _, _) in &extracted.batches {
        if !buffers.buffers.iter().any(|(x, _)| Arc::ptr_eq(x, v)) {
            let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
            let b = device.create_buffer_with_data(&BufferInitDescriptor { label: Some("bloom part vertices"), contents: &bytes, usage: BufferUsages::VERTEX });
            buffers.buffers.push((v.clone(), b));
        }
    }
    let size = target_tex.main_texture().size();
    let (qw, qh) = (size.width.div_ceil(4), size.height.div_ceil(4));
    let parts = target(&mut textures, &device, "bloom parts", size.width, size.height);
    let quarter = target(&mut textures, &device, "bloom quarter", qw, qh);
    let blurred_x = target(&mut textures, &device, "bloom blur x", qw, qh);
    let uniform = |v: Vec4| device.create_buffer_with_data(&BufferInitDescriptor { label: Some("bloom uniform"), contents: &v.to_array().iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>(), usage: BufferUsages::UNIFORM });
    // 1. Bloom parts.
    {
        let view_layout = if ids.multisampled { &pipeline.view_multi } else { &pipeline.view_single };
        let view_group = device.create_bind_group(Some("bloom view"), &cache.get_bind_group_layout(view_layout), &BindGroupEntries::sequential((view_binding, depth.view())));
        let mut draws = Vec::new();
        for (v, tex, colour) in &extracted.batches {
            let (Some(img), Some((_, buf))) = (images.get(*tex), buffers.buffers.iter().find(|(x, _)| Arc::ptr_eq(x, v))) else { continue };
            let cbuf = uniform(colour.extend(1.0));
            let g = device.create_bind_group(Some("bloom part"), &cache.get_bind_group_layout(&pipeline.part), &BindGroupEntries::sequential((&img.texture_view, &pipeline.linear_repeat, cbuf.as_entire_binding())));
            draws.push((g, buf.clone(), (v.len() / VERTEX_FLOATS) as u32, cbuf));
        }
        let mut pass = colour_pass(&mut ctx, "bloom parts", &parts);
        pass.set_render_pipeline(parts_rp);
        pass.set_bind_group(0, &view_group, &[view_offset.offset]);
        for (g, buf, n, _) in &draws {
            pass.set_bind_group(1, g, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..*n, 0..1);
        }
    }
    // 2. Tinted downsample to quarter size.
    let tint = uniform(ark.bloom);
    let g = device.create_bind_group(Some("bloom downsample"), &cache.get_bind_group_layout(&pipeline.down), &BindGroupEntries::sequential((&parts, tint.as_entire_binding())));
    {
        let mut pass = colour_pass(&mut ctx, "bloom downsample", &quarter);
        pass.set_render_pipeline(down_rp);
        pass.set_bind_group(0, &g, &[]);
        pass.draw(0..3, 0..1);
    }
    // 3. ArkGaussian: X then Y.
    for (rp, src, dst, label) in [(bx_rp, &quarter, &blurred_x, "bloom blur x"), (by_rp, &blurred_x, &quarter, "bloom blur y")] {
        let g = device.create_bind_group(Some(label), &cache.get_bind_group_layout(&pipeline.blur), &BindGroupEntries::sequential((src, &pipeline.linear_clamp)));
        let mut pass = colour_pass(&mut ctx, label, dst);
        pass.set_render_pipeline(rp);
        pass.set_bind_group(0, &g, &[]);
        pass.draw(0..3, 0..1);
    }
    result.view = Some(quarter);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(switches: &[(&str, bool)], scalars: &[(&str, f32)], vectors: &[(&str, [f32; 4])]) -> MaterialParams {
        MaterialParams {
            emissive: Some(0),
            static_switches: switches.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            scalars: scalars.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            vectors: vectors.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            ..Default::default()
        }
    }

    /// generic_PMAT shader maps: bloom = emissive [x colour with E_Enable_Emissive_color] x
    /// B_Bloom_Intensity, only with B_Enable_Bloom.
    #[test]
    fn bloom_colour_follows_the_switches() {
        let colour = [("E_Emissive_Color", [0.98443985, 1.0, 0.90217394, 2.0])];
        let on = params(&[("B_Enable_Bloom", true), ("E_Enable_Emissive", true), ("E_Enable_Emissive_color", true)], &[("B_Bloom_Intensity", 0.5)], &colour);
        let c = bloom_color(&on).unwrap();
        assert!((c - Vec3::new(0.98443985, 1.0, 0.90217394)).length() < 1e-5, "{c}");
        // Without E_Enable_Emissive_color the texture alone (x intensity).
        let plain = params(&[("B_Enable_Bloom", true), ("E_Enable_Emissive", true), ("E_Enable_Emissive_color", false)], &[], &colour);
        assert_eq!(bloom_color(&plain), Some(Vec3::ONE));
        // B_Enable_Bloom off (or absent), or no emissive: no bloom part.
        assert_eq!(bloom_color(&params(&[("B_Enable_Bloom", false), ("E_Enable_Emissive", true)], &[], &colour)), None);
        assert_eq!(bloom_color(&params(&[("E_Enable_Emissive", true)], &[], &colour)), None);
        let mut no_tex = on.clone();
        no_tex.emissive = None;
        assert_eq!(bloom_color(&no_tex), None);
        // Intensity 0 (the sky instances' value): nothing to draw.
        assert_eq!(bloom_color(&params(&[("B_Enable_Bloom", true)], &[("B_Bloom_Intensity", 0.0)], &[])), None);
    }
}
