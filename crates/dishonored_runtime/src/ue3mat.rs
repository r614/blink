// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
//! Materials rendered with their original shaders.
//!
//! Each material's compiled UE3 base pass shaders (D3D9 shader model 3 bytecode from the
//! game's shader cache, `dishonored_assets::shaders`) are translated to WGSL and assembled into
//! a program (`ue3_shader::ue3prog`) when a level spawns. A single material type renders them
//! all: its pipeline specialization swaps in the program of the material's key. The bind
//! group carries the material's parameter values and textures, the per-instance data
//! buffer (light-map placement and scales) and the light-map page arrays.
//! `DISHONORED_LEGACY_MATERIALS=1` falls back to the approximated standard materials.
//!
//! Blink changes: the library comes from the install (no cooker), the programs' WGSL can be
//! built ahead on the level's worker thread (`program_wgsl`), the blend modes are
//! `dishonored_ir::MaterialBlend`'s.

use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, Face, RenderPipelineDescriptor, ShaderType,
    SpecializedMeshPipelineError,
};
use bevy::render::storage::ShaderBuffer;
use dishonored_ir::MaterialBlend as Blend;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use crate::ue3_shader_cache::{self, Class, ProgramSource};
use ue3_shader::shadercache::CookedLibrary;
use ue3_shader::ue3prog::{self, Policy, MAX_PARAMS};

pub struct Ue3Plugin;

impl Plugin for Ue3Plugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "ue3_prepass.wgsl");
        app.add_plugins((MaterialPlugin::<Ue3Material>::default(), crate::ue3_shader_cache::Ue3ShaderCachePlugin)).init_resource::<Ue3Programs>();
    }
}

pub fn enabled() -> bool {
    dishonored_assets::shaders::enabled()
}

/// The shader library (the install's shader caches), read on first use.
pub fn library() -> Option<Arc<CookedLibrary>> {
    dishonored_assets::shaders::library().map(|s| s.cooked.clone())
}

/// Program shaders, indexed by the program id in a material's key.
static PROGRAMS: RwLock<Vec<Handle<Shader>>> = RwLock::new(Vec::new());

/// Their WGSL, by program id (composed per pipeline variant by `ue3_shader_cache`).
static PROGRAM_SRC: RwLock<Vec<ProgramSource>> = RwLock::new(Vec::new());

/// Programs' WGSL by (shader map, light-map policy, dominant light pass), built on first
/// request from any thread (a level's worker builds its programs ahead of the spawn).
static WGSL: Mutex<Option<HashMap<(u32, Policy, bool), Option<String>>>> = Mutex::new(None);

/// Depth-only programs' WGSL by (shader map, light-map policy, masked).
static DEPTH_WGSL: Mutex<Option<HashMap<(u32, Policy, bool), Option<String>>>> = Mutex::new(None);

/// Programs built so far, by (shader map, light-map policy). Programs are keyed by a hash of
/// their WGSL text, so identical programs (many materials compile to the same shaders; the
/// opaque depth-only programs differ only by vertex shader) share one shader and its pipelines.
#[derive(Resource, Default)]
pub struct Ue3Programs {
    by_key: HashMap<(u32, Policy, bool), Option<u32>>,
    by_depth: HashMap<(u32, Policy, bool), Option<u32>>,
    by_hash: HashMap<u64, u32>,
    /// distinct base (colour pass) programs, and distinct depth-only programs
    base_ids: HashSet<u32>,
    depth_ids: HashSet<u32>,
}

/// What `Ue3Programs` has built: base program requests, distinct base programs, depth-only
/// requests, distinct depth-only programs.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProgramStats {
    pub programs: usize,
    pub distinct: usize,
    pub depth_requests: usize,
    pub depth_variants: usize,
}

impl Ue3Programs {
    /// The program of a shader map for a light-map policy, with or without the dominant
    /// light's pass (built on first request).
    pub fn get(&mut self, map: u32, policy: Policy, with_light: bool, shaders: &mut Assets<Shader>) -> Option<u32> {
        if let Some(p) = self.by_key.get(&(map, policy, with_light)) {
            return *p;
        }
        let r = program_wgsl(map, policy, with_light).map(|w| self.register(w, shaders));
        if let Some(id) = r {
            self.base_ids.insert(id);
        }
        self.by_key.insert((map, policy, with_light), r);
        r
    }

    /// The depth-only program (depth prepass and shadow depth) of a shader map's base pass
    /// vertex shader: for opaque materials the position only (shared by every material with the
    /// same vertex shader), for masked ones (`masked`) with their pixel shader's clipping.
    pub fn get_depth(&mut self, map: u32, policy: Policy, masked: bool, shaders: &mut Assets<Shader>) -> Option<u32> {
        if let Some(p) = self.by_depth.get(&(map, policy, masked)) {
            return *p;
        }
        let r = depth_wgsl(map, policy, masked).map(|w| self.register(w, shaders));
        if let Some(id) = r {
            self.depth_ids.insert(id);
        }
        self.by_depth.insert((map, policy, masked), r);
        r
    }

    /// The id of a program with this WGSL text (added on first sight).
    fn register(&mut self, wgsl: String, shaders: &mut Assets<Shader>) -> u32 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        wgsl.hash(&mut h);
        let hash = h.finish();
        if let Some(&id) = self.by_hash.get(&hash) {
            return id;
        }
        let src = ProgramSource::new(Arc::from(wgsl.as_str()));
        let handle = shaders.add(Shader::from_wgsl(wgsl, format!("ue3/{hash:016x}.wgsl")));
        let mut programs = PROGRAMS.write().unwrap();
        programs.push(handle);
        PROGRAM_SRC.write().unwrap().push(src);
        let id = programs.len() as u32 - 1;
        self.by_hash.insert(hash, id);
        id
    }

    pub fn stats(&self) -> ProgramStats {
        ProgramStats {
            programs: self.by_key.values().filter(|p| p.is_some()).count(),
            distinct: self.base_ids.len(),
            depth_requests: self.by_depth.values().filter(|p| p.is_some()).count(),
            depth_variants: self.depth_ids.len(),
        }
    }
}

/// Whether the programs' pipeline variants are composed here (`ue3_shader_cache`).
pub fn shader_cache_enabled() -> bool {
    ue3_shader_cache::enabled()
}

/// Start composing the pipeline variants of programs (from a level's worker): per use, the
/// program's key ((shader map, policy, with light) or, depth-only, (shader map, policy, masked)),
/// whether it is the depth-only program, and the material's `Ue3Key::blend`.
pub fn prefetch_variants(uses: Vec<((u32, Policy, bool), bool, u8)>) {
    let mut sources: HashMap<((u32, Policy, bool), bool), Option<ProgramSource>> = HashMap::new();
    let mut items = Vec::new();
    for (k, depth, blend) in uses {
        let src = sources
            .entry((k, depth))
            .or_insert_with(|| if depth { depth_wgsl(k.0, k.1, k.2) } else { program_wgsl(k.0, k.1, k.2) }.map(|w| ProgramSource::new(Arc::from(w))))
            .clone();
        let Some(src) = src else { continue };
        let class = if depth { Class { depth: true, prepass: true, blend, fg: false, fg_world: false } } else { Class::main(blend) };
        items.push((src, class));
    }
    ue3_shader_cache::prefetch(items);
}

/// The WGSL of a depth-only program (cached; None: it does not build).
pub fn depth_wgsl(map_id: u32, policy: Policy, masked: bool) -> Option<String> {
    if let Some(w) = DEPTH_WGSL.lock().unwrap().get_or_insert_with(HashMap::new).get(&(map_id, policy, masked)) {
        return w.clone();
    }
    let w = build_depth_wgsl(map_id, policy, masked);
    DEPTH_WGSL.lock().unwrap().get_or_insert_with(HashMap::new).insert((map_id, policy, masked), w.clone());
    w
}

fn build_depth_wgsl(map_id: u32, policy: Policy, masked: bool) -> Option<String> {
    let lib = library()?;
    let map = lib.maps.get(map_id as usize)?;
    let (vst, pst) = policy.shader_types();
    let vs = lib.code.get(&map.shader("FLocalVertexFactory", vst)?)?;
    let ps = if masked { Some(lib.code.get(&map.shader("FLocalVertexFactory", pst)?)?.as_slice()) } else { None };
    match ue3prog::build_depth(map, vs, ps, ue3prog::Platform::Bevy) {
        Ok(p) => Some(p.wgsl),
        Err(e) => {
            warn!("depth program {} {policy:?}: {e:#}", map.name);
            None
        }
    }
}

/// The WGSL of a program (cached; None: it does not build).
pub fn program_wgsl(map_id: u32, policy: Policy, with_light: bool) -> Option<String> {
    if let Some(w) = WGSL.lock().unwrap().get_or_insert_with(HashMap::new).get(&(map_id, policy, with_light)) {
        return w.clone();
    }
    let w = build_wgsl(map_id, policy, with_light);
    WGSL.lock().unwrap().get_or_insert_with(HashMap::new).insert((map_id, policy, with_light), w.clone());
    w
}

fn build_wgsl(map_id: u32, policy: Policy, with_light: bool) -> Option<String> {
    let lib = library()?;
    let map = lib.maps.get(map_id as usize)?;
    let (vst, pst) = policy.shader_types();
    let vs = lib.code.get(&map.shader("FLocalVertexFactory", vst)?)?;
    let ps = lib.code.get(&map.shader("FLocalVertexFactory", pst)?)?;
    // opaque surfaces also take the dominant light's pass
    // the dominant light through its distance-field shadow maps (else the plain factor variant)
    // (`DH_SUN_FACTOR` forces the plain variant, for comparison)
    let order = if std::env::var("DH_SUN_FACTOR").is_ok() { [Policy::SUN_FACTOR, Policy::SUN] } else { [Policy::SUN, Policy::SUN_FACTOR] };
    let sun = order
        .into_iter()
        .find_map(|(lv, lp)| Some((lib.code.get(&map.shader("FLocalVertexFactory", lv)?)?.as_slice(), lib.code.get(&map.shader("FLocalVertexFactory", lp)?)?.as_slice())))
        .filter(|_| with_light);
    let prog = match ue3prog::build(map, vs, ps, sun, ue3prog::Platform::Bevy) {
        Ok(p) => p,
        Err(e) => {
            warn!("program {} {policy:?}: {e:#}", map.name);
            return None;
        }
    };
    if !prog.unknown.is_empty() {
        debug!("program {} {policy:?}: unsupported constants {:?}", map.name, prog.unknown);
    }
    if std::env::var("DH_UE3_DUMP").is_ok() {
        let dir = dishonored_content::cache_dir().join("debug").join("programs");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("{map_id}_{policy:?}.wgsl")), &prog.wgsl);
    }
    // debugging: hand-edited programs override the generated ones; `DH_UE3_DEBUG=id`
    // tints each program by its shader map (logged with the colour)
    let mut wgsl = prog.wgsl;
    if std::env::var("DH_UE3_DEBUG").as_deref() == Ok("id") {
        // map id in base 16 per channel (decodable from an untonemapped screenshot)
        let c = [(map_id % 16) as f32 / 15.0, ((map_id / 16) % 16) as f32 / 15.0, ((map_id / 256) % 16) as f32 / 15.0];
        info!("program {map_id} {} {policy:?}: colour {:.2?}", map.name, c);
        wgsl = wgsl.replace("    return color;
}", &format!("    return vec4<f32>({:?}, {:?}, {:?}, 1.0);
}}", c[0], c[1], c[2]));
    }
    if let Ok(dir) = std::env::var("DH_UE3_PATCH_DIR") {
        if let Ok(p) = std::fs::read_to_string(std::path::Path::new(&dir).join(format!("{map_id}_{policy:?}.wgsl"))) {
            info!("program {map_id} {policy:?}: patched from {dir}");
            wgsl = p;
        }
    }
    Some(wgsl)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ue3Key {
    pub program: u32,
    /// the depth-only program of the depth prepass and shadow passes (`u32::MAX`: none, the
    /// program's own prepass entry points run)
    pub depth: u32,
    /// 0 opaque, 1 masked, 2 translucent, 3 additive, 4 modulate
    pub blend: u8,
    pub two_sided: bool,
    /// first-person view model (UE3's foreground depth priority group)
    pub foreground: bool,
    /// a character in the foreground group at the world's field of view (the Outsider:
    /// `DisSeqAct_OutsiderConfig` `SDPG_Foreground`)
    pub foreground_world: bool,
    /// a dynamic light's additive pass over an opaque surface (drawn just in front of it)
    pub light_pass: bool,
}

impl Ue3Key {
    pub fn new(program: u32, blend: Blend, two_sided: bool) -> Self {
        Ue3Key { program, depth: u32::MAX, blend: blend_code(blend), two_sided, foreground: false, foreground_world: false, light_pass: false }
    }
}

/// `Ue3Key::blend` of a blend mode.
pub fn blend_code(blend: Blend) -> u8 {
    match blend {
        Blend::Opaque => 0,
        Blend::Masked => 1,
        Blend::Translucent => 2,
        Blend::Additive => 3,
        Blend::Modulate => 4,
    }
}

#[derive(ShaderType, Debug, Clone, Copy)]
pub struct Ue3Params {
    pub p: [Vec4; MAX_PARAMS],
}

impl Default for Ue3Params {
    fn default() -> Self {
        Ue3Params { p: [Vec4::ZERO; MAX_PARAMS] }
    }
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
#[bind_group_data(Ue3Key)]
pub struct Ue3Material {
    #[uniform(0)]
    pub params: Ue3Params,
    #[texture(1)]
    #[sampler(2)]
    pub t0: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub t1: Handle<Image>,
    #[texture(5)]
    #[sampler(6)]
    pub t2: Handle<Image>,
    #[texture(7)]
    #[sampler(8)]
    pub t3: Handle<Image>,
    #[texture(9)]
    #[sampler(10)]
    pub t4: Handle<Image>,
    #[texture(11)]
    #[sampler(12)]
    pub t5: Handle<Image>,
    #[texture(13)]
    #[sampler(14)]
    pub t6: Handle<Image>,
    #[texture(15)]
    #[sampler(16)]
    pub t7: Handle<Image>,
    #[texture(17)]
    #[sampler(18)]
    pub t8: Handle<Image>,
    #[texture(19)]
    #[sampler(20)]
    pub t9: Handle<Image>,
    #[texture(21)]
    #[sampler(22)]
    pub t10: Handle<Image>,
    #[texture(23)]
    #[sampler(24)]
    pub t11: Handle<Image>,
    #[texture(25, dimension = "cube")]
    #[sampler(26)]
    pub cube0: Handle<Image>,
    #[storage(27, read_only)]
    pub instances: Handle<ShaderBuffer>,
    #[texture(28, dimension = "2d_array")]
    pub lm0: Handle<Image>,
    #[texture(29, dimension = "2d_array")]
    #[sampler(30)]
    pub lm1: Handle<Image>,
    #[texture(31, dimension = "2d_array")]
    #[sampler(32)]
    pub shadow: Handle<Image>,
    #[texture(33)]
    #[sampler(34)]
    pub white: Handle<Image>,
    #[texture(35)]
    pub black: Handle<Image>,
    #[texture(36, dimension = "cube")]
    pub black_cube: Handle<Image>,
    /// the level's reflection cube (`SceneReflectionTexture`)
    #[texture(37, dimension = "cube")]
    pub refl: Handle<Image>,
    pub key: Ue3Key,
    /// its shader map, and whether it takes dynamic light passes (lit, opaque or masked)
    pub map: u32,
    /// the scene material it was made from (`u32::MAX`: none), for parameters set at runtime
    pub mat_id: u32,
    pub light_ok: bool,
}

impl From<&Ue3Material> for Ue3Key {
    fn from(m: &Ue3Material) -> Self {
        m.key
    }
}

impl Ue3Material {
    pub fn set_texture(&mut self, i: usize, h: Handle<Image>) {
        let slot = match i {
            0 => &mut self.t0,
            1 => &mut self.t1,
            2 => &mut self.t2,
            3 => &mut self.t3,
            4 => &mut self.t4,
            5 => &mut self.t5,
            6 => &mut self.t6,
            7 => &mut self.t7,
            8 => &mut self.t8,
            9 => &mut self.t9,
            10 => &mut self.t10,
            11 => &mut self.t11,
            _ => return,
        };
        *slot = h;
    }
}

impl Material for Ue3Material {
    fn alpha_mode(&self) -> AlphaMode {
        match self.key.blend {
            0 => AlphaMode::Opaque,
            1 => AlphaMode::Mask(0.5),
            2 => AlphaMode::Blend,
            3 => AlphaMode::Add,
            _ => AlphaMode::Multiply,
        }
    }

    // The depth prepass runs the programs' prepass entry points (selected in `specialize`;
    // this placeholder only makes Bevy create a fragment stage for masked materials).
    fn prepass_fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/ue3_prepass.wgsl".into()
    }

    // dynamic objects (characters, props) cast into the dominant directional light's shadow
    // map (the world is `NotShadowCaster`: its shadows are baked)
    fn enable_shadows() -> bool {
        true
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let k = key.bind_group_data;
        let prepass = descriptor.vertex.shader_defs.iter().any(|d| matches!(d, bevy::shader::ShaderDefVal::Bool(n, true) if n == "PREPASS_PIPELINE"));
        // The depth prepass and the shadow passes run UE3's depth-only shaders: the base pass
        // vertex shader's position alone (opaque, shared), plus the clipping for masked ones.
        let depth_program = prepass && k.depth != u32::MAX;
        let id = if depth_program { k.depth } else { k.program };
        let shader = PROGRAMS.read().unwrap().get(id as usize).cloned();
        // materials are only created once their program is registered
        let Some(shader) = shader else { return Ok(()) };
        // (last step, once the defs are final: the variant composed off the render thread)
        let precompose = |descriptor: &mut RenderPipelineDescriptor| {
            if !ue3_shader_cache::enabled() {
                return;
            }
            let Some(src) = PROGRAM_SRC.read().unwrap().get(id as usize).cloned() else { return };
            let class = Class { depth: depth_program, prepass, blend: k.blend, fg: k.foreground, fg_world: k.foreground_world };
            ue3_shader_cache::use_precomposed(&src, class, descriptor);
        };
        if k.foreground || k.foreground_world {
            descriptor.vertex.shader_defs.push("UE_FOREGROUND".into());
        }
        if k.foreground_world {
            descriptor.vertex.shader_defs.push("UE_FOREGROUND_WORLD".into());
        }
        descriptor.vertex.shader = shader.clone();
        descriptor.vertex.entry_point = Some(if prepass { "prepass_vertex" } else { "vertex" }.into());
        let mut attributes = vec![
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(2),
            Mesh::ATTRIBUTE_UV_1.at_shader_location(3),
            Mesh::ATTRIBUTE_TANGENT.at_shader_location(4),
            Mesh::ATTRIBUTE_COLOR.at_shader_location(5),
        ];
        if layout.0.contains(Mesh::ATTRIBUTE_JOINT_INDEX) && layout.0.contains(Mesh::ATTRIBUTE_JOINT_WEIGHT) {
            attributes.push(Mesh::ATTRIBUTE_JOINT_INDEX.at_shader_location(6));
            attributes.push(Mesh::ATTRIBUTE_JOINT_WEIGHT.at_shader_location(7));
        }
        descriptor.vertex.buffers = vec![layout.0.get_layout(&attributes)?];
        descriptor.primitive.cull_mode = if k.two_sided { None } else { Some(Face::Back) };
        if k.light_pass {
            // the light pass's vertex shader places the surface where the base pass did, up
            // to rounding: a nudge towards the camera (reverse Z) keeps it on top
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.bias.constant = 4;
                ds.bias.slope_scale = 1.0;
                ds.depth_write_enabled = Some(false);
            }
        }
        if k.foreground || k.foreground_world {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("UE_FOREGROUND".into());
                if k.foreground_world {
                    fragment.shader_defs.push("UE_FOREGROUND_WORLD".into());
                }
            }
        }
        if prepass {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader = shader;
                fragment.entry_point = Some("prepass_fragment".into());
            }
            precompose(descriptor);
            return Ok(());
        }
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = shader;
            fragment.entry_point = Some("fragment".into());
            let add = |src: BlendFactor, dst: BlendFactor| BlendComponent { src_factor: src, dst_factor: dst, operation: BlendOperation::Add };
            let blend = match k.blend {
                2 => Some(BlendState {
                    color: add(BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha),
                    alpha: add(BlendFactor::One, BlendFactor::OneMinusSrcAlpha),
                }),
                3 => Some(BlendState { color: add(BlendFactor::One, BlendFactor::One), alpha: add(BlendFactor::Zero, BlendFactor::One) }),
                4 => Some(BlendState { color: add(BlendFactor::Dst, BlendFactor::Zero), alpha: add(BlendFactor::Zero, BlendFactor::One) }),
                _ => None,
            };
            for t in fragment.targets.iter_mut().flatten() {
                t.blend = blend;
            }
        }
        precompose(descriptor);
        Ok(())
    }
}
