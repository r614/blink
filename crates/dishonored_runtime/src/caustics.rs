//! Caustic light functions: the spot lights whose `LightFunction` is
//! `Vfx_Materials.main_materials.Casutics_LightFunction_PMAT` (Boyle Ext's canal-side facades, the
//! Brothel, the flooded Assassins' level, the island), research/dishonored/systems/render_lighting.md
//! "Light functions".
//!
//! What the original does: a light with a function is drawn dynamically (its `UseDirectLightMap` is
//! false) and the function material, rendered by `FLightFunctionPixelShader` (36479), multiplies the
//! light's attenuation. The caustics function samples `T_Caustics_2` twice (`|texel|^0.4` each,
//! multiplied, x `CausticColor`), the two samples' coordinates pushed by two scrolled samples of
//! `T_EV_DustPanner_01`, and is greyscaled. It is the material's own pixel shader here, translated
//! by `ue3_shader` (`ue3prog::build_fragment`) and appended to `caustic_light.wgsl`; it reads the
//! scene depth from the depth prepass. The lights carry `LightingChannels` {Dynamic, Unnamed_6}:
//! they light only the primitives that have the `Unnamed_6` channel (the canal-side facades), not the
//! whole map as the plain Bevy lights the importer produced before did.
//!
//! Here these lights are not Bevy lights (`lighting::spawn_lights` skips them). Each opaque static mesh
//! section that shares a channel with a light and lies in its radius is drawn once more, additively,
//! with the light's contribution as the original's light pass computes it (UE3 radial falloff,
//! spot cone, `albedo x N.L` and the specular lobe) times the function (`caustic_light.wgsl`). The
//! albedo and the normal map are the section's own (`ExtendedMaterial` over its `StandardMaterial`),
//! so the pass costs one extra draw per reached section and no per-frame CPU work.
//!
//! LIKELY (not traced in the engine): the `ScreenToLight` matrix the engine passes. The shader reads
//! columns x, y and w of (x w, y w, w, 1) x `ScreenToLight` (c6..c9), then divides by w; here the matrix
//! is built in the fragment so that this is the scene point's offset from the light along the light's
//! Y and Z axes (its X axis is the cone axis) over `LightFunction::Scale` uu per repeat, with w = 1
//! (an affine light transform): `Default__LightFunction.Scale` (1024) and the authored 600 only mean
//! something for such a world-space projection. Which two light axes the function's xy are, and the
//! `GreyscaleFactor` (1 here), are not traced.

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, DepthBiasState, ShaderType};
use dishonored_ir::{DishonoredMap, LightFunction, LightKind, MeshInstance, lighting_channel};
use std::sync::OnceLock;
use ue3_shader::shadercache::Expr;
use ue3_shader::ue3prog;

use crate::coords;
use crate::map_render::{RenderFeatures, light_unit};

/// Lights per receiver (the nearest are kept).
const MAX_LIGHTS: usize = 4;

/// The light function material (`Vfx_Materials.main_materials.Casutics_LightFunction_PMAT`), by the
/// friendly name of its shader map.
const FUNCTION_MATERIAL: &str = "casutics_lightfunction";

/// The module's parameter slots (`caustic_light.wgsl`'s `UeMaterial`): one per parameter occurrence in the\n/// shader map's uniform expressions, as `Ue3Material::params`.
const FUNCTION_SLOTS: usize = ue3prog::MAX_PARAMS;

/// The assembled module's shader (installed by [`apply_pending_caustics`]).
const CAUSTIC_SHADER: Handle<Shader> = bevy::asset::uuid_handle!("6d1f0b0a-5e1c-4a57-9f0e-3c2b7a8e41d2");

/// The function material's parameter values, in its shader map's slot order.
#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub struct FunctionParams {
    pub p: [Vec4; FUNCTION_SLOTS],
}

impl Default for FunctionParams {
    fn default() -> Self {
        FunctionParams { p: [Vec4::ZERO; FUNCTION_SLOTS] }
    }
}

/// `caustic_light.wgsl` with the function material's translated pixel shader appended.
struct Module {
    wgsl: String,
    /// The shader map's parameter nodes: (name, is a vector parameter).
    slots: Vec<(String, bool)>,
}

impl Module {
    fn params(&self, speed: f32, color: [f32; 3]) -> FunctionParams {
        let mut out = FunctionParams::default();
        for (i, (name, vector)) in self.slots.iter().enumerate().take(FUNCTION_SLOTS) {
            out.p[i] = match (name.as_str(), vector) {
                ("Speed", _) => Vec4::splat(speed),
                ("CausticColor", true) => Vec4::new(color[0], color[1], color[2], 1.0),
                _ => Vec4::ZERO,
            };
        }
        out
    }
}

/// The module, built once (None: no shader library, or the shader does not translate).
fn module() -> Option<&'static Module> {
    static MODULE: OnceLock<Option<Module>> = OnceLock::new();
    MODULE.get_or_init(build_module).as_ref()
}

fn build_module() -> Option<Module> {
    let lib = crate::ue3mat::library()?;
    let map = lib.maps.iter().find(|m| m.name.to_ascii_lowercase().contains(FUNCTION_MATERIAL))?;
    let ps = lib.code.get(&map.shader("FLocalVertexFactory", "FLightFunctionPixelShader")?)?;
    let nodes = ue3_shader::shadercache::param_nodes(map);
    if nodes.len() > FUNCTION_SLOTS {
        warn!("caustics: {} has {} parameters (at most {FUNCTION_SLOTS})", map.name, nodes.len());
        return None;
    }
    let frag = match ue3prog::build_fragment(map, ps) {
        Ok(f) => f,
        Err(e) => {
            warn!("caustics: the light function shader does not translate: {e:#}");
            return None;
        }
    };
    if !frag.unknown.is_empty() {
        warn!("caustics: unsupported constants {:?}", frag.unknown);
    }
    let slots = nodes
        .iter()
        .map(|e| match e {
            Expr::VectorParameter(n, _) => (n.clone(), true),
            Expr::ScalarParameter(n, _) => (n.clone(), false),
            _ => (String::new(), false),
        })
        .collect();
    let wgsl = format!("{}{}", include_str!("caustic_light.wgsl"), frag.wgsl);
    if std::env::var("DH_UE3_DUMP").is_ok() {
        let dir = dishonored_content::cache_dir().join("debug").join("programs");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("caustic_light.wgsl"), &wgsl);
    }
    Some(Module { wgsl, slots })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct CausticLightData {
    /// Bevy world position, radius (Bevy units).
    pub pos_radius: Vec4,
    /// Cone axis (Bevy world), cos of the outer half angle.
    pub dir_cos_outer: Vec4,
    /// LightColor x Brightness x UE3 radiance to Bevy light units, cos of the inner half angle.
    pub color_cos_inner: Vec4,
    /// (falloff exponent, function scale along the light's Y, along its Z (Bevy units), 1 = spot).
    pub shape: Vec4,
    /// The light's Y and Z axes (Bevy world): the function's u and v.
    pub right: Vec4,
    pub up: Vec4,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct CausticUniform {
    /// (light count, the material's `Speed`, 0, 0).
    pub head: Vec4,
    /// `CausticColor`.rgb x its alpha.
    pub color: Vec4,
    pub lights: [CausticLightData; MAX_LIGHTS],
}

/// The material extension of the caustic pass (`caustic_light.wgsl`).
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct CausticLight {
    #[uniform(100)]
    pub params: CausticUniform,
    /// `Texture2D_0`: `T_EV_DustPanner_01`.
    #[texture(101)]
    #[sampler(102)]
    pub noise: Option<Handle<Image>>,
    /// `Texture2D_1`: `T_Caustics_2`.
    #[texture(103)]
    #[sampler(104)]
    pub caustic: Option<Handle<Image>>,
    /// The function material's parameters (`Speed`, `CausticColor`).
    #[uniform(105)]
    pub function: FunctionParams,
}

pub type CausticMaterial = bevy::pbr::ExtendedMaterial<StandardMaterial, CausticLight>;

impl bevy::pbr::MaterialExtension for CausticLight {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        bevy::shader::ShaderRef::Handle(CAUSTIC_SHADER)
    }
    fn alpha_mode() -> Option<AlphaMode> {
        Some(AlphaMode::Add)
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _pipeline: &bevy::pbr::MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // The pass redraws geometry the opaque pass drew (merged into batches, so the vertices are
        // not bit-identical): pull it towards the camera by a few depth steps (reverse Z: larger is
        // nearer) so it passes the depth test where it is level with the surface.
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.bias = DepthBiasState { constant: 4, slope_scale: 1.0, clamp: 0.0 };
        }
        Ok(())
    }
}

/// A caustic receiver waiting for its material (spawned without one, given it on the next update).
#[derive(Component, Clone)]
pub struct PendingCaustic(pub CausticMaterial);

/// The function textures of a level's caustic lights.
#[derive(Clone, Default)]
pub struct CausticImages {
    pub noise: Option<Handle<Image>>,
    pub caustic: Option<Handle<Image>>,
}

struct Entry {
    data: CausticLightData,
    /// Dishonored world position and radius, for the receiver test.
    position: Vec3,
    radius: f32,
    channels: u32,
    speed: f32,
    color: [f32; 3],
    textures: [Option<usize>; 2],
}

/// The level's caustic lights.
#[derive(Default)]
pub struct CausticLights {
    lights: Vec<Entry>,
}

/// Whether a light is drawn by this module (and so is not a Bevy light).
pub fn is_caustic(l: &dishonored_ir::Light) -> bool {
    matches!(l.function, Some(LightFunction::Caustics { .. })) && !matches!(l.kind, LightKind::Directional)
}

impl CausticLights {
    pub fn new(map: &DishonoredMap, features: &RenderFeatures) -> CausticLights {
        let mut out = CausticLights::default();
        if !features.caustics || !features.dynamic_lights || !features.materials || module().is_none() {
            return out;
        }
        for l in map.lights.iter().filter(|l| is_caustic(l)) {
            let Some(LightFunction::Caustics { speed, color, scale, textures }) = &l.function else { continue };
            let (spot, inner, outer) = match l.kind {
                LightKind::Spot { inner_cone_deg, outer_cone_deg } => (1.0, inner_cone_deg.min(outer_cone_deg), outer_cone_deg.clamp(1.0, 89.0)),
                _ => (0.0, 0.0, 90.0),
            };
            let unit = light_unit() * l.brightness;
            let u = coords::UU_TO_BEVY;
            let data = CausticLightData {
                pos_radius: coords::pos(l.position).extend(l.radius * u),
                dir_cos_outer: coords::dir(l.direction).normalize_or(Vec3::NEG_Y).extend(outer.to_radians().cos()),
                color_cos_inner: Vec4::new(l.color[0] * unit, l.color[1] * unit, l.color[2] * unit, inner.to_radians().cos()),
                shape: Vec4::new(l.falloff_exponent, scale[1].max(1.0) * u, scale[2].max(1.0) * u, spot),
                right: coords::dir(l.axes[0]).extend(0.0),
                up: coords::dir(l.axes[1]).extend(0.0),
            };
            out.lights.push(Entry { data, position: l.position, radius: l.radius, channels: l.lighting_channels, speed: *speed, color: *color, textures: *textures });
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.lights.is_empty()
    }

    pub fn len(&self) -> usize {
        self.lights.len()
    }

    /// The function textures (the first light's: every caustic light shares the one material).
    pub fn images(&self, handles: &[Handle<Image>]) -> CausticImages {
        let Some(e) = self.lights.first() else { return CausticImages::default() };
        CausticImages { noise: e.textures[0].and_then(|t| handles.get(t)).cloned(), caustic: e.textures[1].and_then(|t| handles.get(t)).cloned() }
    }

    /// The lights that reach a static mesh instance: it shares a lighting channel with the light
    /// (`ULightComponent::AffectsPrimitive`) and its bounds touch the light's sphere. At most
    /// `MAX_LIGHTS`, nearest first, as indices for [`CausticLights::pending`].
    pub fn reaching(&self, map: &DishonoredMap, inst: &MeshInstance) -> Vec<usize> {
        if self.lights.is_empty() || !self.lights.iter().any(|l| lighting_channel::shares(l.channels, inst.lighting_channels)) {
            return Vec::new();
        }
        let mesh = &map.meshes[inst.mesh];
        let (lo, hi) = (mesh.bounds_min, mesh.bounds_max);
        let (mut a, mut b) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for k in 0..8 {
            let c = Vec3::new(if k & 1 == 0 { lo.x } else { hi.x }, if k & 2 == 0 { lo.y } else { hi.y }, if k & 4 == 0 { lo.z } else { hi.z });
            let w = inst.transform.transform_point3(c);
            a = a.min(w);
            b = b.max(w);
        }
        let mut hits: Vec<(f32, usize)> = self
            .lights
            .iter()
            .enumerate()
            .filter(|(_, l)| lighting_channel::shares(l.channels, inst.lighting_channels))
            .filter_map(|(i, l)| {
                let d = (l.position - l.position.clamp(a, b)).length();
                (d <= l.radius).then_some((d, i))
            })
            .collect();
        hits.sort_by(|x, y| x.0.total_cmp(&y.0));
        hits.into_iter().take(MAX_LIGHTS).map(|h| h.1).collect()
    }

    /// The receiver pass of a section drawn with `base` (its `StandardMaterial`) under the lights
    /// `reach` (from [`CausticLights::reaching`]).
    pub fn pending(&self, reach: &[usize], base: StandardMaterial, images: &CausticImages) -> PendingCaustic {
        let mut params = CausticUniform::default();
        let first = &self.lights[reach[0]];
        params.head = Vec4::new(reach.len().min(MAX_LIGHTS) as f32, first.speed, 0.0, 0.0);
        // `CausticColor` (its alpha, 1 in the retail material, multiplies the colour: `c14 x c14.w`).
        params.color = Vec4::new(first.color[0], first.color[1], first.color[2], 1.0);
        for (k, &i) in reach.iter().take(MAX_LIGHTS).enumerate() {
            params.lights[k] = self.lights[i].data;
        }
        let function = module().map(|m| m.params(first.speed, first.color)).unwrap_or_default();
        PendingCaustic(bevy::pbr::ExtendedMaterial { base, extension: CausticLight { params, noise: images.noise.clone(), caustic: images.caustic.clone(), function } })
    }
}

pub struct CausticsPlugin;

impl Plugin for CausticsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<CausticMaterial>::default()).add_systems(Update, apply_pending_caustics);
    }
}

fn apply_pending_caustics(
    mut commands: Commands,
    q: Query<(Entity, &PendingCaustic)>,
    mut mats: ResMut<Assets<CausticMaterial>>,
    mut shaders: ResMut<Assets<Shader>>,
    mut installed: Local<bool>,
) {
    if q.is_empty() {
        return;
    }
    // The module (the translated function shader behind `caustic_light.wgsl`), once.
    if !*installed {
        if let Some(m) = module() {
            let _ = shaders.insert(CAUSTIC_SHADER.id(), Shader::from_wgsl(m.wgsl.clone(), "ue3/caustic_light.wgsl"));
        }
        *installed = true;
    }
    for (e, p) in &q {
        let h = mats.add(p.0.clone());
        commands.entity(e).remove::<PendingCaustic>().insert(MeshMaterial3d(h));
    }
}
