// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
//! The original post-process graph's screen-warping branch (`AltScreen_Effects.PostProcessChain.
//! Test_PPG`): the vector-field materials (`PPG_*Vectors`, their `TPpMaterialPixelShader`
//! translated) drawn in turn into a quarter-resolution float target, each reading the one
//! before (the graph's switches: Bend Time, Blink, Possession, the knock-out...), then the motion
//! blur node (`ArkPpNodeBlur`, `EPpBt_Motion` with `m_FullOffsetInVectorField`: the original
//! `FArkPpMotionBlur2PixelShader`) shifting and smearing the image along it. Each effect's
//! node also grades the image its own way while it runs (`m_bOverrideUberPp`, a layer of the
//! ArkPp pass: `ArkPostSource::graph_post`).
//!
//! Blink glue: the graph's materials come from `dishonored_assets::level::post_materials` (the
//! importer's shader-map links) and the shader library from `dishonored_assets::shaders`. The
//! effects' drive values are `power_pp`'s timelines measured from the running game (Bend Time's
//! `vBendTimeOpacity`, Blink's lens `vOpacity`, Possession's `vParams`, the material `Time` at the
//! player's dilation), read from the camera's `ArkPostSource`; the Kismet's screen effects
//! (`Epp_Knocked`, `Epp_Weepers`) from `MissionView::post_effects`. Not wired (no Blink state
//! for them yet): the camera under water, the spyglass zoom (`lens_*`), a searchlight's
//! blinding. While the graph runs, `ark_post`'s hand-ported fields stay off (`ArkPostSource::graph`).

use crate::ark_post::{ArkPost, ArkPostSource};
use crate::session::SessionView;
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer};
use bevy::render::render_resource::{
    AddressMode, BindGroup, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, CachedRenderPipelineId, ColorTargetState,
    ColorWrites, Extent3d, FilterMode, FragmentState, LoadOp, MipmapFilterMode, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, StoreOp,
    TextureDataOrder, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView, TextureViewDescriptor,
    UniformBuffer, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::{CachedTexture, FallbackImage, GpuImage, TextureCache};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use dishonored_ir::{PostCurve, PostSettings};
use std::collections::BTreeMap;

pub struct PostGraphPlugin;

impl Plugin for PostGraphPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PostGraph>()
            .init_resource::<GraphShaders>()
            .add_plugins(ExtractResourcePlugin::<PostGraph>::default())
            .add_plugins(ExtractComponentPlugin::<PostGraphView>::default())
            .add_systems(Startup, load_post_assets)
            .add_systems(Update, (receive_post_assets, update_graph.after(crate::power_pp::drive)).chain());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(Render, (prepare_pipelines, prepare_textures).in_set(RenderSystems::PrepareResources))
            // after the ArkPp pass (fog, the uber grade), as the original's motion blur follows
            // its grading nodes
            .add_systems(
                Core3d,
                draw_graph
                    .in_set(Core3dSystems::PostProcess)
                    .in_set(GraphPass)
                    .after(crate::ark_post::ark_post_pass)
                    .before(bevy::core_pipeline::tonemapping::tonemapping),
            );
    }
}

/// The graph's passes.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct GraphPass;

/// The game view the graph draws on (not the scene captures).
#[derive(Component, Clone, Copy, ExtractComponent)]
pub struct PostGraphView;

/// `Test_PPG_RT2D`'s reference size: the vector fields count their blur in its pixels
/// (`vNoiseParams`' 1280 x 720), whatever the screen's.
const REFERENCE: Vec2 = Vec2::new(1280.0, 720.0);
/// `ArkPpNodeBlur_0`'s `m_MotionBlurConfig.m_LengthStrength`
const LENGTH_STRENGTH: f32 = 1.0;
/// The vector fields' target (`"1/4RES 64bits"`: the scene colour's float format)
const VECTOR_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
/// `ue3prog::build_post`'s parameter slots, textures and node inputs
const MAX_PARAMS: usize = 64;
const TEXTURES: u32 = 4;
const INPUTS: u32 = 4;

/// What the graph draws this frame: the vector field's stages in order (each reading the one
/// before as its first input), and the motion blur's `gVectorFieldScales`. No stages: the
/// branch is off.
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct PostGraph {
    pub stages: Vec<GraphStage>,
    /// knocked out (`Epp_Knocked`): the last frame blended in (`KO_Combine`) and the frame
    /// kept for the next (`TEST_PPG_BackupRed`)
    pub ko: Option<(GraphStage, GraphStage)>,
    /// full-resolution material passes over the image after the blur (the mask's lens:
    /// `PPG_LensCompose`)
    pub post: Vec<GraphStage>,
    pub scales: Vec4,
    pub time: f32,
    pub blur: Option<Handle<Shader>>,
}

#[derive(Clone)]
pub struct GraphStage {
    pub shader: Handle<Shader>,
    pub params: Vec<Vec4>,
    pub textures: Vec<Option<Handle<Image>>>,
}

/// A post-process material as the graph runs it: its shader map, parameter values (and
/// names) and textures (`Texture2D_N`). (dishonored-bevy's `level::PostMat`.)
#[derive(Clone)]
pub struct PostMat {
    pub map: u32,
    pub params: Vec<[f32; 4]>,
    pub param_names: Vec<String>,
    pub textures: Vec<Option<Handle<Image>>>,
    /// its parameters' curves over time (a `MaterialInstanceTimeVarying`)
    pub curves: Vec<PostCurve>,
}

/// The graph's materials by name, and its grading nodes' `m_UberParameters` by effect name
/// (dishonored-bevy's `GameAssets::post` and `Data::post_nodes`).
#[derive(Resource, Default)]
pub struct PostAssets {
    pub post: HashMap<String, PostMat>,
    pub post_nodes: BTreeMap<String, BTreeMap<String, f32>>,
}

/// The import of the graph's materials, running on a worker thread.
#[derive(Resource)]
struct PostImport(std::sync::Mutex<std::sync::mpsc::Receiver<Option<dishonored_assets::level::post_materials::PostMaterials>>>);

/// Import the graph's materials (with their shader-map links) off the main thread.
fn load_post_assets(mut commands: Commands) {
    if !crate::ue3mat::enabled() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("post materials".into()).spawn(move || {
        let r = (|| {
            crate::ue3mat::library()?;
            let content = dishonored_assets::Content::locate().ok()?;
            match dishonored_assets::level::post_materials::import_post_materials(&content, 512) {
                Ok(p) => Some(p),
                Err(e) => {
                    warn!("post-process graph materials: {e:#}");
                    None
                }
            }
        })();
        let _ = tx.send(r);
    });
    if spawned.is_ok() {
        commands.insert_resource(PostImport(std::sync::Mutex::new(rx)));
    }
}

fn receive_post_assets(mut commands: Commands, import: Option<Res<PostImport>>, mut images: ResMut<Assets<Image>>) {
    let Some(import) = import else { return };
    let got = import.0.lock().ok().and_then(|r| r.try_recv().ok());
    let Some(got) = got else { return };
    commands.remove_resource::<PostImport>();
    let Some(p) = got else { return };
    let mut handles: HashMap<usize, Handle<Image>> = HashMap::default();
    let mut out = PostAssets { post_nodes: p.nodes, ..Default::default() };
    for (name, id, curves) in p.materials {
        let Some(ue) = p.map.material_params.get(id).and_then(|m| m.ue3.as_ref()) else { continue };
        let textures = ue
            .textures
            .iter()
            .map(|t| {
                let t = (*t).filter(|t| *t != dishonored_ir::UE3_TEX_BLACK)?;
                let tex = p.map.textures.get(t)?;
                Some(handles.entry(t).or_insert_with(|| images.add(crate::map_render::to_image(tex))).clone())
            })
            .collect();
        out.post.insert(name, PostMat { map: ue.map, params: ue.params.clone(), param_names: ue.param_names.clone(), textures, curves });
    }
    info!("post-process graph: {} materials, {} grading nodes", out.post.len(), out.post_nodes.len());
    commands.insert_resource(out);
}

/// The programs built so far: the materials' by shader map, the motion blur's.
#[derive(Resource, Default)]
struct GraphShaders {
    maps: HashMap<u32, Option<Handle<Shader>>>,
    blur: Option<Option<Handle<Shader>>>,
}

impl GraphShaders {
    fn map(&mut self, map: u32, shaders: &mut Assets<Shader>) -> Option<Handle<Shader>> {
        self.maps
            .entry(map)
            .or_insert_with(|| {
                let lib = crate::ue3mat::library()?;
                let m = lib.maps.get(map as usize)?;
                let code = lib.code.get(&m.shader("FLocalVertexFactory", "TPpMaterialPixelShader<FPpMaterialLinearSpaceMeshPolicy>")?)?;
                match ue3_shader::ue3prog::build_post(m, code) {
                    Ok(p) => Some(shaders.add(Shader::from_wgsl(p.wgsl, format!("ue3/post_{map}.wgsl")))),
                    Err(e) => {
                        warn!("post-process material {}: {e:#}", m.name);
                        None
                    }
                }
            })
            .clone()
    }

    fn blur(&mut self, shaders: &mut Assets<Shader>) -> Option<Handle<Shader>> {
        self.blur
            .get_or_insert_with(|| {
                let lib = crate::ue3mat::library()?;
                let code = lib.globals.get("FArkPpMotionBlur2PixelShader")?;
                match ue3_shader::ue3prog::build_global(code) {
                    Ok((wgsl, _, _)) => Some(shaders.add(Shader::from_wgsl(wgsl, "ue3/post_motion_blur.wgsl"))),
                    Err(e) => {
                        warn!("motion blur shader: {e:#}");
                        None
                    }
                }
            })
            .clone()
    }
}

/// The effects' timelines (the powers' drive values are `power_pp`'s).
#[derive(Default)]
struct Timelines {
    last: Option<(bool, bool, usize)>,
    /// the scripts' knock-out and weepers' effects, 0..1
    ko: f32,
    weepers: f32,
    /// the view's turning on screen (`vCamDir`), and where it looked last frame
    cam_dir: Vec2,
    last_fwd: Option<Vec3>,
    /// possessing: seconds into going in (`POSSESSION_IN`) or coming out (`POSSESSION_OUT`),
    /// and whether Corvo was inside a host last frame
    poss_in: Option<f32>,
    poss_out: Option<f32>,
    possessed: bool,
    /// the running nodes' grade: its weight, fades and the last fields shown
    grade: f32,
    fades: (f32, f32),
    fields: Option<PostSettings>,
}

/// A time-varying material's parameters at `t` seconds, and whether it is still playing.
fn curves_at(assets: &PostAssets, name: &str, t: f32) -> Option<Vec<(String, Vec4)>> {
    let m = assets.post.get(name)?;
    let end = m.curves.iter().map(|c| c.end()).fold(0.0, f32::max);
    (t <= end).then(|| m.curves.iter().map(|c| (c.param.clone(), Vec4::splat(c.at(t)))).collect())
}

/// Seconds a scripted screen effect fades in and out over.
const SCRIPTED_FADE: f32 = 0.5;

fn debug_once(last: &mut Option<(bool, bool, usize)>, now: (bool, bool, usize)) {
    if *last != Some(now) {
        info!("post-process graph: bending {} blinking {} materials {}", now.0, now.1, now.2);
        *last = Some(now);
    }
}

fn clear_graph(graph: &mut PostGraph) {
    graph.stages.clear();
    graph.ko = None;
    graph.post.clear();
}

/// A stage of a material by its cooked name, with its parameters overridden by name.
fn stage(assets: &PostAssets, shaders: &mut GraphShaders, shader_assets: &mut Assets<Shader>, name: &str, set: &[(&str, Vec4)]) -> Option<GraphStage> {
    let m = assets.post.get(name)?;
    let shader = shaders.map(m.map, shader_assets)?;
    let mut params: Vec<Vec4> = m.params.iter().map(|p| Vec4::from_array(*p)).collect();
    for (n, v) in set {
        for (i, pn) in m.param_names.iter().enumerate() {
            if pn == n {
                params[i] = *v;
            }
        }
    }
    Some(GraphStage { shader, params, textures: m.textures.clone() })
}

/// A node's `m_UberParameters` as the ArkPp override fields it sets (dishonored-bevy's
/// `postfx::uber_fields`, with Blink's ArkPp field names; distances stay in uu).
pub fn uber_fields(params: &BTreeMap<String, f32>, prefix: &str) -> PostSettings {
    let get = |k: &str| params.get(&format!("{prefix}{k}")).copied();
    let flag = |k: &str, d: bool| get(k).map(|v| v != 0.0).unwrap_or(d);
    let mut out = PostSettings { source: "ArkPpNodeMaterial.m_UberParameters".into(), ..Default::default() };
    if flag("m_bOverrideCBParameters", true) {
        for n in ["CrMgYbShadTones", "CrMgYbMidTones", "CrMgYbHighTones"] {
            if flag(&format!("m_CBParameters.m_bOverride{n}"), true) {
                let c = |a: &str| get(&format!("m_CBParameters.m_{n}.{a}")).unwrap_or(0.0);
                out.vectors.push((format!("m_{n}"), [c("x"), c("y"), c("z"), 0.0]));
            }
        }
        for (n, d, v) in [("Opacity", true, 1.0), ("PreDesaturation", true, 0.0), ("PostDesaturation", false, 0.0)] {
            if flag(&format!("m_CBParameters.m_bOverride{n}"), d) {
                out.values.push((format!("m_{n}"), get(&format!("m_CBParameters.m_{n}")).unwrap_or(v)));
            }
        }
    }
    if flag("m_bOverrideHDRParameters", true) {
        for (n, d, v) in [("Exposure", true, 0.0), ("GammaAdjustment", true, 1.0), ("FilmGrainNoise", false, 0.0), ("GimpBrightness", false, 0.0), ("GimpContrast", false, 0.0)] {
            if flag(&format!("m_HDRParameters.m_bOverride{n}"), d) {
                out.values.push((format!("m_{n}"), get(&format!("m_HDRParameters.m_{n}")).unwrap_or(v)));
            }
        }
    }
    if flag("m_bOverrideDOFParameters", true) {
        for (n, v) in [("FocusDistance", 1000.0), ("InFocusRadius", 300.0), ("FarBlurAmount", 0.0)] {
            if flag(&format!("m_DOFParameters.m_bOverride{n}"), true) {
                out.values.push((format!("m_{n}"), get(&format!("m_DOFParameters.m_{n}")).unwrap_or(v)));
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn update_graph(
    mut commands: Commands,
    time: Res<Time<bevy::time::Real>>,
    view: Option<Res<SessionView>>,
    assets: Option<Res<PostAssets>>,
    (mut graph, mut shaders, mut shader_assets): (ResMut<PostGraph>, ResMut<GraphShaders>, ResMut<Assets<Shader>>),
    mut cams: Query<(Entity, &GlobalTransform, &mut ArkPostSource, Has<PostGraphView>), (With<Camera3d>, Without<crate::reflect::ReflectCamera>)>,
    mut tl: Local<Timelines>,
) {
    let Some(assets) = assets.filter(|a| !a.post.is_empty()) else {
        clear_graph(&mut graph);
        return;
    };
    let Some((cam, cg, mut src, marked)) = cams.iter_mut().find(|c| !c.2.capture) else {
        clear_graph(&mut graph);
        return;
    };
    if !marked {
        commands.entity(cam).insert(PostGraphView);
    }
    let tl = &mut *tl;
    let dt = time.delta_secs().min(0.1);
    let off = std::env::var("DH_NO_PPG").is_ok();
    // the graph runs (its motion blur builds): `ark_post`'s hand-ported fields stay off
    let blur = shaders.blur(&mut shader_assets);
    let usable = blur.is_some() && !off;
    if src.graph != usable {
        src.graph = usable;
    }
    // the powers' drive values: `power_pp`'s measured timelines
    let bend = src.bend;
    let blink = src.blink_lens;
    let possess = src.possess;
    let bending = bend > 0.0;
    let blinking = blink != Vec2::ZERO;
    if std::env::var("DH_PPG_DEBUG").is_ok() {
        debug_once(&mut tl.last, (bending, blinking, assets.post.len()));
    }
    // the scripts' screen effects (`DisSeqAct_PostProcess`)
    let scripted = |e: &str| view.as_ref().and_then(|v| v.matchv.mission.as_ref()).is_some_and(|m| m.post_effects.iter().any(|x| x == e));
    let step = |v: f32, on: bool| if on { (v + dt / SCRIPTED_FADE).min(1.0) } else { (v - dt / SCRIPTED_FADE).max(0.0) };
    tl.ko = step(tl.ko, scripted("Epp_Knocked"));
    tl.weepers = step(tl.weepers, scripted("Epp_Weepers"));
    // the view's turning, on screen (right, down)
    let fwd = cg.forward().as_vec3();
    if let Some(prev) = tl.last_fwd {
        let d = Vec2::new(-prev.dot(cg.right().as_vec3()), prev.dot(cg.up().as_vec3()));
        if d.length() > 1e-4 {
            tl.cam_dir = tl.cam_dir.lerp(d.normalize(), 0.2);
        }
    }
    tl.last_fwd = Some(fwd);
    if tl.cam_dir.length() < 0.01 {
        tl.cam_dir = Vec2::X;
    }
    // possessing: going in and coming out
    let possessed = view.as_ref().and_then(|v| v.me.as_ref()).is_some_and(|m| m.possess.stage == dishonored_sim::state::PossessStage::Active);
    if possessed != tl.possessed {
        tl.possessed = possessed;
        if possessed {
            tl.poss_in = Some(0.0);
            tl.poss_out = None;
        } else {
            tl.poss_out = Some(0.0);
            tl.poss_in = None;
        }
    }
    for t in [&mut tl.poss_in, &mut tl.poss_out].into_iter().flatten() {
        *t += dt;
    }
    // (Blink has no camera-under-water state yet)
    let under = false;
    // the graph's switches
    let mut stages = Vec::new();
    let mut nodes: Vec<&str> = Vec::new();
    let mut s = |name: &str, set: &[(&str, Vec4)]| stage(&assets, &mut shaders, &mut shader_assets, name, set);
    if under {
        stages.extend(s("underwater_vectors", &[]));
    }
    if bending {
        // `vBendTimeOpacity` as captured: linear over the warm-up and cool-down (power_pp)
        if under {
            stages.extend(s("bendtime_vectors_water", &[("vBendTimeOpacity", Vec4::splat(bend))]));
            nodes.push("BendTimeUnderWaterVectors");
        } else {
            stages.extend(s("bendtime_vectors", &[("vBendTimeOpacity", Vec4::splat(bend))]));
            nodes.push("BendTimeVectors");
        }
    }
    if blinking {
        let v = Vec4::new(blink.x, blink.y, 0.0, 1.0);
        if stages.is_empty() {
            stages.extend(s("blink_vectors", &[("vOpacity", v)]));
            nodes.push("BlinkVectors2");
        } else {
            stages.extend(s("blink_vectors_previous", &[("vOpacity", v)]));
            nodes.push("BlinkVectors");
        }
    }
    if possess != Vec2::ZERO {
        // the post manager's possession R / G as captured (power_pp::PossessPp)
        stages.extend(s("possession_vectors", &[("vParams", Vec4::new(possess.x, possess.y, 100.0, 1.0))]));
        nodes.push("PossessionVectors");
    }
    if tl.weepers > 0.0 {
        let k = tl.weepers * tl.weepers * (3.0 - 2.0 * tl.weepers);
        stages.extend(s("adrenaline_vectors", &[("vBendTimeOpacity", Vec4::splat(k))]));
        nodes.push("PlagueVectors");
    }
    let mut post_stages = Vec::new();
    // going in and coming out (`POSSESSION_IN_INST` / `POSSESSION_OUT_INST` curves)
    for (t, name) in [(&mut tl.poss_in, "possession_in"), (&mut tl.poss_out, "possession_out")] {
        let Some(at) = *t else { continue };
        match curves_at(&assets, name, at) {
            Some(set) => {
                let set: Vec<(&str, Vec4)> = set.iter().map(|(n, v)| (n.as_str(), *v)).collect();
                post_stages.extend(s(name, &set));
            }
            None => *t = None,
        }
    }
    let mut ko = None;
    if tl.ko > 0.0 {
        // the knock-out's field takes over (`KOSwitch`): a blur along the view's turning
        let k = tl.ko * tl.ko * (3.0 - 2.0 * tl.ko);
        stages.clear();
        nodes.clear();
        stages.extend(s("ko_vectors", &[("vCamDir", tl.cam_dir.extend(0.0).extend(1.0)), ("vParams", Vec4::new(k, 0.0, 0.0, 1.0))]));
        if let (Some(c), Some(b)) = (s("ko_combine", &[("vParams", Vec4::new(0.5 * k, 0.0, 0.0, 1.0))]), s("ko_backup", &[])) {
            ko = Some((c, b));
        }
    }
    if off {
        stages.clear();
        nodes.clear();
        ko = None;
        post_stages.clear();
    }
    graph.post = post_stages;
    graph.blur = if stages.is_empty() { None } else { blur };
    if graph.blur.is_none() {
        stages.clear();
        ko = None;
    }
    graph.ko = ko;
    if stages.len() != graph.stages.len() {
        debug!("post-process graph: {} vector stages {nodes:?} (bend {:.2}, blink {:.2?})", stages.len(), bend, blink);
    }
    graph.stages = stages;
    graph.scales = Vec4::new(LENGTH_STRENGTH / REFERENCE.x, LENGTH_STRENGTH / REFERENCE.y, 1.0, 1.0);
    // the materials' `Time`, at the player's dilation (power_pp; captured for Bend Time)
    graph.time = src.bend_time;
    // the nodes' own grading while they run, faded in and out
    if !nodes.is_empty() {
        let mut fields = PostSettings::default();
        for n in &nodes {
            if let Some(p) = assets.post_nodes.get(*n) {
                let f = uber_fields(p, "");
                fields = crate::dark_vision::merged(Some(&fields), &f);
            }
        }
        tl.fields = Some(fields);
        tl.fades = if bending {
            (crate::power_pp::BEND_POST_WARMUP, crate::power_pp::BEND_POST_COOLDOWN)
        } else {
            // `Twk_Blink` `m_fWarmupTime` x 0.5, `m_fCooldownTime` x 0.5
            (0.5, crate::power_pp::BLINK_COOLDOWN * 0.5)
        };
        tl.grade = (tl.grade + dt / tl.fades.0.max(0.01)).min(1.0);
    } else {
        tl.grade = (tl.grade - dt / tl.fades.1.max(0.01)).max(0.0);
    }
    let (graph_post, graph_weight) = if tl.grade > 0.0 && !off { (tl.fields.clone(), tl.grade) } else { (None, 0.0) };
    if src.graph_weight != graph_weight || (graph_weight > 0.0 && src.graph_post.is_none()) {
        src.graph_post = graph_post;
        src.graph_weight = graph_weight;
    }
}

/// `UePost` (`ue3prog::build_post`)
#[derive(Clone, Copy, ShaderType)]
struct PostUniform {
    p: [Vec4; MAX_PARAMS],
    time: Vec4,
    res: Vec4,
}

/// `UeGlobal` (`ue3prog::build_global`)
#[derive(Clone, Copy, ShaderType)]
struct GlobalUniform {
    c: [Vec4; 16],
}

#[derive(Resource)]
struct GraphPipelines {
    /// what an unconnected node input reads: zero vectors (a cleared target; the fallback
    /// image is white, which reads as a full-screen offset)
    zero: TextureView,
    stage_layout: BindGroupLayoutDescriptor,
    blur_layout: BindGroupLayoutDescriptor,
    clamp: Sampler,
    repeat: Sampler,
    /// (program, target format) -> pipeline
    stages: HashMap<(AssetId<Shader>, TextureFormat), CachedRenderPipelineId>,
    blur: HashMap<(AssetId<Shader>, TextureFormat), CachedRenderPipelineId>,
}

fn init_pipelines(mut commands: Commands, device: Res<RenderDevice>, queue: Res<RenderQueue>) {
    let zero = device
        .create_texture_with_data(
            &queue,
            &TextureDescriptor {
                label: Some("ppg_zero"),
                size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: VECTOR_FORMAT,
                usage: TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            TextureDataOrder::default(),
            &[0u8; 8],
        )
        .create_view(&TextureViewDescriptor::default());
    let f = ShaderStages::FRAGMENT;
    let tex = || texture_2d(TextureSampleType::Float { filterable: true });
    let mut entries: Vec<BindGroupLayoutEntry> = vec![uniform_buffer::<PostUniform>(false).build(0, f)];
    for k in 0..TEXTURES {
        entries.push(sampler(SamplerBindingType::Filtering).build(1 + k, f));
        entries.push(tex().build(1 + TEXTURES + k, f));
    }
    for k in 0..INPUTS {
        entries.push(sampler(SamplerBindingType::Filtering).build(1 + 2 * TEXTURES + k, f));
        entries.push(tex().build(1 + 2 * TEXTURES + INPUTS + k, f));
    }
    let stage_layout = BindGroupLayoutDescriptor::new("ppg_stage_layout", &entries);
    let blur_entries = vec![
        uniform_buffer::<GlobalUniform>(false).build(0, f),
        sampler(SamplerBindingType::Filtering).build(1, f),
        tex().build(2, f),
        sampler(SamplerBindingType::Filtering).build(3, f),
        tex().build(4, f),
    ];
    let blur_layout = BindGroupLayoutDescriptor::new("ppg_blur_layout", &blur_entries);
    let clamp = device.create_sampler(&SamplerDescriptor { mag_filter: FilterMode::Linear, min_filter: FilterMode::Linear, ..default() });
    let repeat = device.create_sampler(&SamplerDescriptor {
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        ..default()
    });
    commands.insert_resource(GraphPipelines { zero, stage_layout, blur_layout, clamp, repeat, stages: HashMap::default(), blur: HashMap::default() });
    commands.insert_resource(KoHistory::default());
}

fn pipeline(label: &'static str, layout: BindGroupLayoutDescriptor, shader: &Handle<Shader>, format: TextureFormat) -> RenderPipelineDescriptor {
    RenderPipelineDescriptor {
        label: Some(label.into()),
        layout: vec![layout],
        vertex: VertexState { shader: shader.clone(), entry_point: Some("post_vertex".into()), ..default() },
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("post_fragment".into()),
            targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        ..default()
    }
}

fn prepare_pipelines(mut pipelines: ResMut<GraphPipelines>, graph: Res<PostGraph>, pipeline_cache: Res<PipelineCache>, views: Query<&ExtractedView, (With<ExtractedCamera>, With<PostGraphView>)>) {
    let mut want: Vec<(&Handle<Shader>, TextureFormat)> = graph.stages.iter().map(|s| (&s.shader, VECTOR_FORMAT)).collect();
    if let Some((_, backup)) = graph.ko.as_ref() {
        want.push((&backup.shader, VECTOR_FORMAT));
    }
    for view in &views {
        if let Some((combine, _)) = graph.ko.as_ref() {
            want.push((&combine.shader, view.target_format));
        }
        want.extend(graph.post.iter().map(|s| (&s.shader, view.target_format)));
        let Some(blur) = graph.blur.as_ref() else { continue };
        let key = (blur.id(), view.target_format);
        if !pipelines.blur.contains_key(&key) {
            let id = pipeline_cache.queue_render_pipeline(pipeline("ppg_motion_blur", pipelines.blur_layout.clone(), blur, view.target_format));
            pipelines.blur.insert(key, id);
        }
    }
    for (shader, format) in want {
        if !pipelines.stages.contains_key(&(shader.id(), format)) {
            let id = pipeline_cache.queue_render_pipeline(pipeline("ppg_stage", pipelines.stage_layout.clone(), shader, format));
            pipelines.stages.insert((shader.id(), format), id);
        }
    }
}

/// A view's vector fields: the stages draw into them in turn.
#[derive(Component)]
struct VectorTextures {
    a: CachedTexture,
    b: CachedTexture,
    size: UVec2,
}

/// The knock-out's last frame (`Test_PPG_RT2D_2`, a quarter the size), kept from one frame to
/// the next, by view.
#[derive(Resource, Default)]
struct KoHistory(HashMap<Entity, (UVec2, TextureView)>);

fn prepare_textures(
    mut commands: Commands,
    graph: Res<PostGraph>,
    mut cache: ResMut<TextureCache>,
    mut history: ResMut<KoHistory>,
    device: Res<RenderDevice>,
    views: Query<(Entity, &ExtractedCamera), (With<ArkPost>, With<PostGraphView>)>,
) {
    if graph.ko.is_none() {
        history.0.clear();
    }
    if graph.stages.is_empty() {
        return;
    }
    for (e, camera) in &views {
        let Some(size) = camera.physical_target_size else { continue };
        let size = UVec2::new((size.x / 4).max(1), (size.y / 4).max(1));
        let desc = |label: &'static str| TextureDescriptor {
            label: Some(label),
            size: Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: VECTOR_FORMAT,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        let a = cache.get(&device, desc("ppg_vectors_a"));
        let b = cache.get(&device, desc("ppg_vectors_b"));
        commands.entity(e).insert(VectorTextures { a, b, size });
        if graph.ko.is_some() && history.0.get(&e).is_none_or(|h| h.0 != size) {
            let view = device.create_texture(&desc("ppg_ko_history")).create_view(&TextureViewDescriptor::default());
            history.0.insert(e, (size, view));
        }
    }
}

/// A material stage's bind group: its parameters and textures, and its node inputs (the rest
/// read zero).
#[allow(clippy::too_many_arguments)]
fn stage_bind_group(
    device: &RenderDevice,
    queue: &RenderQueue,
    layout: &BindGroupLayout,
    pipelines: &GraphPipelines,
    images: &RenderAssets<GpuImage>,
    fallback: &TextureView,
    s: &GraphStage,
    inputs: &[&TextureView],
    time: f32,
    res: Vec4,
) -> Option<BindGroup> {
    let mut p = [Vec4::ZERO; MAX_PARAMS];
    for (d, v) in p.iter_mut().zip(&s.params) {
        *d = *v;
    }
    let mut uniform = UniformBuffer::from(PostUniform { p, time: Vec4::splat(time), res });
    uniform.write_buffer(device, queue);
    let binding = uniform.binding()?;
    let tex_views: Vec<_> =
        (0..TEXTURES as usize).map(|k| s.textures.get(k).and_then(|t| t.as_ref()).and_then(|h| images.get(h)).map(|g| &g.texture_view).unwrap_or(fallback)).collect();
    let mut entries = vec![BindGroupEntry { binding: 0, resource: binding }];
    for k in 0..TEXTURES {
        entries.push(BindGroupEntry { binding: 1 + k, resource: BindingResource::Sampler(&pipelines.repeat) });
        entries.push(BindGroupEntry { binding: 1 + TEXTURES + k, resource: BindingResource::TextureView(tex_views[k as usize]) });
    }
    for k in 0..INPUTS {
        let input = inputs.get(k as usize).copied().unwrap_or(&pipelines.zero);
        entries.push(BindGroupEntry { binding: 1 + 2 * TEXTURES + k, resource: BindingResource::Sampler(&pipelines.clamp) });
        entries.push(BindGroupEntry { binding: 1 + 2 * TEXTURES + INPUTS + k, resource: BindingResource::TextureView(input) });
    }
    Some(device.create_bind_group("ppg_stage", layout, &entries))
}

/// A full-screen pass into `target`.
fn full_screen(ctx: &mut RenderContext<'_, '_>, label: &'static str, target: &TextureView, pipeline: &RenderPipeline, bind_group: &BindGroup) {
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Clear(Default::default()), store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

#[allow(clippy::too_many_arguments)]
fn draw_graph(
    view: ViewQuery<(&ExtractedView, &ViewTarget, Option<&VectorTextures>, Option<&PostGraphView>)>,
    graph: Res<PostGraph>,
    pipelines: Option<Res<GraphPipelines>>,
    history: Option<Res<KoHistory>>,
    pipeline_cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    fallback: Res<FallbackImage>,
    (device, queue): (Res<RenderDevice>, Res<RenderQueue>),
    mut ctx: RenderContext,
) {
    let (Some(pipelines), Some(history)) = (pipelines, history) else { return };
    let view_entity = view.entity();
    let (extracted, target, textures, marked) = view.into_inner();
    // the game view only (not the scene captures)
    if marked.is_none() {
        return;
    }
    let format = extracted.target_format;
    let get = |shader: &Handle<Shader>, format: TextureFormat| pipelines.stages.get(&(shader.id(), format)).and_then(|id| pipeline_cache.get_render_pipeline(*id));
    let layout = pipeline_cache.get_bind_group_layout(&pipelines.stage_layout);
    let fallback_view = &fallback.d2.texture_view;
    if let (Some(textures), Some(blur)) = (textures.filter(|_| !graph.stages.is_empty()), graph.blur.as_ref()) {
        draw_field(&mut ctx, &graph, &pipelines, &history, &pipeline_cache, &images, fallback_view, (&device, &queue), (view_entity, format, target, &layout), (textures, blur));
    }
    // the image stages, at full resolution
    let size = extracted.viewport.zw().max(UVec2::ONE);
    let res = Vec4::new(size.x as f32, size.y as f32, 1.0 / size.x as f32, 1.0 / size.y as f32);
    for s in &graph.post {
        let Some(pipeline) = get(&s.shader, format) else { return };
        let post = target.post_process_write();
        let Some(bg) = stage_bind_group(&device, &queue, &layout, &pipelines, &images, fallback_view, s, &[post.source], graph.time, res) else { return };
        full_screen(&mut ctx, "ppg image", post.destination, pipeline, &bg);
    }
}

/// The vector field, the motion blur along it, the knock-out's last frame.
#[allow(clippy::too_many_arguments)]
fn draw_field(
    ctx: &mut RenderContext<'_, '_>,
    graph: &PostGraph,
    pipelines: &GraphPipelines,
    history: &KoHistory,
    pipeline_cache: &PipelineCache,
    images: &RenderAssets<GpuImage>,
    fallback_view: &TextureView,
    (device, queue): (&RenderDevice, &RenderQueue),
    (view_entity, format, target, layout): (Entity, TextureFormat, &ViewTarget, &BindGroupLayout),
    (textures, blur): (&VectorTextures, &Handle<Shader>),
) {
    let get = |shader: &Handle<Shader>, format: TextureFormat| pipelines.stages.get(&(shader.id(), format)).and_then(|id| pipeline_cache.get_render_pipeline(*id));
    let Some(blur_pipeline) = pipelines.blur.get(&(blur.id(), format)).and_then(|id| pipeline_cache.get_render_pipeline(*id)) else { return };
    let Some(stage_pipelines) = graph.stages.iter().map(|s| get(&s.shader, VECTOR_FORMAT)).collect::<Option<Vec<_>>>() else { return };
    let res = Vec4::new(textures.size.x as f32, textures.size.y as f32, 1.0 / textures.size.x as f32, 1.0 / textures.size.y as f32);
    let targets = [&textures.a, &textures.b];
    // the vector field, stage by stage
    for (i, (s, pipeline)) in graph.stages.iter().zip(stage_pipelines).enumerate() {
        let input = if i > 0 { &targets[(i - 1) % 2].default_view } else { &pipelines.zero };
        let Some(bg) = stage_bind_group(device, queue, layout, pipelines, images, fallback_view, s, &[input], graph.time, res) else { return };
        full_screen(ctx, "ppg vectors", &targets[i % 2].default_view, pipeline, &bg);
    }
    // the image along it
    let field = &targets[(graph.stages.len() - 1) % 2].default_view;
    let mut c = [Vec4::ZERO; 16];
    c[0] = graph.scales;
    let mut uniform = UniformBuffer::from(GlobalUniform { c });
    uniform.write_buffer(device, queue);
    let Some(binding) = uniform.binding() else { return };
    let post = target.post_process_write();
    let entries = [
        BindGroupEntry { binding: 0, resource: binding },
        BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&pipelines.clamp) },
        BindGroupEntry { binding: 2, resource: BindingResource::TextureView(post.source) },
        BindGroupEntry { binding: 3, resource: BindingResource::Sampler(&pipelines.clamp) },
        BindGroupEntry { binding: 4, resource: BindingResource::TextureView(field) },
    ];
    let bg = device.create_bind_group("ppg_motion_blur", &pipeline_cache.get_bind_group_layout(&pipelines.blur_layout), &entries);
    full_screen(ctx, "ppg motion blur", post.destination, blur_pipeline, &bg);
    // knocked out: this frame with the last one (`BlendLastFrame`), then this one kept
    // (`BackupLastFrame`)
    let Some((combine, backup)) = graph.ko.as_ref() else { return };
    let Some((_, last)) = history.0.get(&view_entity) else { return };
    let (Some(combine_pipeline), Some(backup_pipeline)) = (get(&combine.shader, format), get(&backup.shader, VECTOR_FORMAT)) else { return };
    let post = target.post_process_write();
    let Some(bg) = stage_bind_group(device, queue, layout, pipelines, images, fallback_view, combine, &[post.source, last], graph.time, res) else { return };
    full_screen(ctx, "ppg ko blend", post.destination, combine_pipeline, &bg);
    let Some(bg) = stage_bind_group(device, queue, layout, pipelines, images, fallback_view, backup, &[post.source], graph.time, res) else { return };
    full_screen(ctx, "ppg ko backup", last, backup_pipeline, &bg);
}
