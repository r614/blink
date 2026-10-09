//! Normalized Dishonored content, independent of UE3 object semantics.
//!
//! Coordinate frame: **Dishonored world space** â€” Z-up, left-handed, Unreal units (uu),
//! exactly as the original game stores it. Simulation code works in this frame so
//! original constants (speeds, gravity, ranges) apply unchanged. Presentation layers
//! convert to their own frame (see `dishonored_runtime::coords`).

pub mod ai;
pub mod env_light;
pub mod gameplay;
pub mod items;
pub mod layout;
pub mod nav;
pub mod particles;
pub mod npc_anim;
pub mod spawners;
pub mod world_actors;
pub mod interactables;

use glam::{Mat4, Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// Index into [`DishonoredMap::meshes`].
pub type MeshId = usize;
/// Index into [`DishonoredMap::materials`].
pub type MaterialId = usize;
/// Index into [`DishonoredMap::textures`].
pub type TextureId = usize;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DishonoredMap {
    pub name: String,
    pub meshes: Vec<StaticMeshAsset>,
    pub materials: Vec<MaterialAsset>,
    pub textures: Vec<TextureAsset>,
    pub instances: Vec<MeshInstance>,
    pub collision: CollisionWorld,
    pub lights: Vec<Light>,
    pub spawn_candidates: Vec<SpawnCandidate>,
    /// Objects the importer saw but did not convert, kept for later work.
    pub unconverted: Vec<UnconvertedObject>,
    /// Baked irradiance atlases referenced by [`MeshInstance::lightmap`].
    #[serde(default)]
    pub lightmaps: Vec<LightmapAtlas>,
    /// Extended material data, parallel to `materials` (same index; may be shorter,
    /// missing entries mean defaults).
    #[serde(default)]
    pub material_params: Vec<MaterialParams>,
    /// Level-wide presentation settings (fog, post, environment).
    #[serde(default)]
    pub environment: Environment,
    /// Particle systems used by the level's emitters (and any requested by the importer).
    #[serde(default)]
    pub particle_systems: Vec<particles::ParticleSystemAsset>,
    /// Placed emitters (`Emitter` actors).
    #[serde(default)]
    pub emitters: Vec<particles::EmitterInstance>,
    /// The camera lens effects the level's `DisSeqAct_SpawnCameraLensEffect` actions spawn.
    #[serde(default)]
    pub lens_effects: Vec<LensEffectDef>,
    /// Placed doors and usable objects drawn with a skeletal mesh.
    #[serde(default)]
    pub skeletal_props: Vec<SkeletalProp>,
    /// The static lights and light volumes the characters' light environments read at run time
    /// (`dishonored_runtime::char_light`).
    #[serde(default)]
    pub light_env_world: env_light::LightEnvWorld,
    /// Placed `SceneCaptureReflectActor`s: the planar reflections that fill the water's render
    /// targets (research/dishonored/systems/render_reflect.md).
    #[serde(default)]
    pub reflect_captures: Vec<ReflectCapture>,
    /// Placed `LensFlareSource` actors (research/dishonored/systems/render_lighting.md "Lens
    /// flares").
    #[serde(default)]
    pub lens_flares: Vec<LensFlareSource>,
    /// The persistent map's Kismet-streamed sublevels (`LevelStreamingKismet` packages): the
    /// renderer tags what they hold with their package (`MeshInstance::level`, the part of a
    /// source id before `:`) so `MatchView::hidden_levels` can hide it.
    #[serde(default)]
    pub streamed_levels: Vec<String>,
    /// Materials the importer was asked for by path (`ImportOptions::extra_materials`: the
    /// gameplay decals' materials): (object path, index into `materials`).
    #[serde(default)]
    pub named_materials: Vec<(String, MaterialId)>,
    /// `ShadowMap1D`s of the instances' first LOD (the dominant light's visibility per vertex).
    #[serde(default)]
    pub vertex_shadows: Vec<VertexShadow>,
}

/// `LightingChannelContainer` bits, in the class's declaration order (Engine.LightComponent): a
/// light affects a primitive when their channel sets share a bit other than `INITIALIZED`.
pub mod lighting_channel {
    pub const NAMES: [&str; 29] = [
        "bInitialized", "BSP", "Static", "Dynamic", "CompositeDynamic", "Skybox", "Unnamed_1", "Unnamed_2", "Unnamed_3", "Unnamed_4", "Unnamed_5", "Unnamed_6", "Cinematic_1", "Cinematic_2", "Cinematic_3", "Cinematic_4", "Cinematic_5",
        "Cinematic_6", "Cinematic_7", "Cinematic_8", "Cinematic_9", "Cinematic_10", "Gameplay_1", "Gameplay_2", "Gameplay_3", "Gameplay_4", "Crowd", "m_LightProbe", "m_bOutsider",
    ];
    pub const INITIALIZED: u32 = 1;
    /// `LightComponent` class defaults: bInitialized, BSP, Static, Dynamic, CompositeDynamic.
    pub const LIGHT_DEFAULT: u32 = 0b1_1111;
    /// A static mesh component without a serialized container (the component's own
    /// `PostLoad` fills it; LIKELY: bInitialized and Static).
    pub const STATIC_MESH_DEFAULT: u32 = 0b101;
    pub fn shares(a: u32, b: u32) -> bool {
        (a & b & !INITIALIZED) != 0
    }
}

/// A placed `LensFlareSource` and the `LensFlare` template its `LensFlareComponent` uses
/// (`FLensFlareSceneProxy`, Dishonored.exe 0x008eb560; the source element is drawn by
/// 0x008e5b50 in the template's `SourceDPG`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LensFlareSource {
    /// `<package>:<actor path>`.
    pub source: String,
    pub template: String,
    /// The source's world position (the actor x component transform's translation), Dishonored
    /// world space.
    pub position: Vec3,
    /// `FixedRelativeBoundingBox` of the template (`bUseFixedRelativeBoundingBox`): the flare is
    /// culled with this box about the position.
    pub bounds: Option<[Vec3; 2]>,
    /// The actor's `bHidden` (Kismet toggles the source through `SeqAct_Toggle`).
    pub hidden: bool,
    /// The component's `SourceColor` (`bModulateColorBySource`).
    pub source_color: [f32; 4],
    pub element: LensFlareElement,
    /// Entries of the template's `Reflections` array (not drawn; none in the retail data).
    pub reflections: usize,
}

/// A `LensFlareElement`: size, material list and the distributions that shade one flare quad.
/// The distributions without a `DistMap_` prefix are looked up at the radial or source distance
/// (`bUseSourceDistance`), the `DistMap_` ones at the camera distance to the source in uu.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LensFlareElement {
    pub size: Vec3,
    pub ray_distance: f32,
    pub enabled: bool,
    pub use_source_distance: bool,
    pub normalize_radial_distance: bool,
    pub modulate_color_by_source: bool,
    /// `LFMaterials` (indices into `DishonoredMap::materials`).
    pub materials: Vec<MaterialId>,
    pub material_index: particles::Dist,
    pub scaling: particles::Dist,
    pub axis_scaling: particles::Dist,
    pub rotation: particles::Dist,
    pub color: particles::Dist,
    pub alpha: particles::Dist,
    pub offset: particles::Dist,
    pub dist_scale: particles::Dist,
    pub dist_color: particles::Dist,
    pub dist_alpha: particles::Dist,
}


/// A `ShadowMap1D`: `samples[v]` is the visibility (0 shadowed .. 1 lit) of vertex `v` of
/// [`DishonoredMap::instances`]`[instance]`'s mesh to the light `light_guid`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VertexShadow {
    pub instance: usize,
    pub light_guid: [u32; 4],
    pub samples: Vec<f32>,
}

/// `RenderingChannelContainer` bit of a reflection channel (declaration order of the struct:
/// BSP, Static, Dynamic, Skeletal, Foliage, Particles, Sprites, Decals, Group_1 .. Group_10).
/// A primitive is drawn into a capture when `MeshInstance::reflect_channels` and
/// `ReflectCapture::channels` share a bit.
pub mod reflect_channel {
    pub const NAMES: [&str; 18] = ["BSP", "Static", "Dynamic", "Skeletal", "Foliage", "Particles", "Sprites", "Decals", "Group_1", "Group_2", "Group_3", "Group_4", "Group_5", "Group_6", "Group_7", "Group_8", "Group_9", "Group_10"];
    pub const GROUP_1: u32 = 1 << 8;
    /// `SceneCaptureReflectComponent` class defaults: BSP, Static, Dynamic, Skeletal, Particles
    /// and Decals on (UnrealScript defaultproperties of the class).
    pub const CAPTURE_DEFAULT: u32 = 0b1010_1111;
}

/// One `SceneCaptureReflectActor` with its `SceneCaptureReflectComponent`: the scene is drawn from
/// the viewer mirrored about the plane (normal = the actor's X axis, through its location;
/// `SceneCaptureReflectComponent` vtable slot 88, Dishonored.exe 0x006E8200) into the
/// `TextureRenderTarget2D` `target`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReflectCapture {
    pub source: String,
    /// Object path of the `TextureTarget` (e.g. `boyle_ext_MS.RealTime_refl`).
    pub target: String,
    /// The render target's `m_ResolutionType` (0 fixed size, 1 full, 2 half, 3 quarter of the
    /// back buffer) and its `SizeX` / `SizeY`.
    pub resolution_type: u8,
    pub size: [u32; 2],
    /// Plane normal and offset (`normal . p = d`), Dishonored world space.
    pub normal: Vec3,
    pub d: f32,
    /// `ReflectionChannels` bit set (see [`reflect_channel`]).
    pub channels: u32,
    /// `FrameRate`: captures per second.
    pub frame_rate: f32,
    /// `bEnabled` (Kismet toggles it through `SetEnabled`), `bEnableFog`, `bEnablePostProcess`,
    /// `bSkipUpdateIfTextureUsersOccluded`.
    pub enabled: bool,
    pub fog: bool,
    pub post_process: bool,
    pub skip_if_unseen: bool,
    /// `MaxUpdateDist` (0: no limit), uu.
    pub max_update_dist: f32,
    /// A `SceneCapture2DActor` (the Tower Return lens TV): a fixed camera instead of the mirror
    /// plane (`normal`, `d` and `channels` are unused: it draws the whole scene).
    #[serde(default)]
    pub scene2d: Option<SceneCamera>,
}

/// The fixed view of a `SceneCapture2DComponent` (Dishonored world space): the actor's location and
/// its X (forward) and Z (up) axes, the horizontal field of view in degrees and the clip planes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneCamera {
    pub location: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

/// UE3 `EBlendMode` of the base material.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaterialBlend {
    #[default]
    Opaque,
    Masked,
    Translucent,
    Additive,
    Modulate,
}

/// Everything beyond diffuse/normal that the renderer needs from a material, resolved
/// through the MIC parent chain (including parents in other packages) using Arkane's
/// parameter-name prefixes (`S_`, `E_`, `O_`, `DN_`, `C_`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaterialParams {
    pub specular: Option<TextureId>,
    pub emissive: Option<TextureId>,
    pub opacity: Option<TextureId>,
    pub detail_normal: Option<TextureId>,
    /// Material samples a `C_` cubemap (reflections).
    pub uses_cubemap: bool,
    pub blend: MaterialBlend,
    /// Whether `blend` came from the base Material (true) or from naming heuristics.
    pub blend_known: bool,
    pub two_sided: bool,
    pub unlit: bool,
    pub opacity_clip: f32,
    pub scalars: Vec<(String, f32)>,
    pub vectors: Vec<(String, [f32; 4])>,
    /// Sky dome / moon / sun material (`Skies.*`, `Moon_PMAT`, `Sun_PMAT` bases): rendered
    /// unlit, without fog, shadows or lightmaps.
    #[serde(default)]
    pub sky: bool,
    /// Path of the root `Material` of the chain, if reached.
    pub base_material: Option<String>,
    /// Static switch values of the compiled permutation (base defaults filled in), U-027.
    #[serde(default)]
    pub static_switches: Vec<(String, bool)>,
    /// Static component masks of the compiled permutation that select one channel
    /// (`C_Cubemap_Mask_Channel` = B -> 2), U-027.
    #[serde(default)]
    pub mask_channels: Vec<(String, u8)>,
    /// Particle sprite shading decoded from the base material's compiled shaders
    /// (`dishonored_assets::particle_material`, EFFECTS.md "Particle materials"); the
    /// material's diffuse slot then holds the colour source with the opacity source in alpha.
    #[serde(default)]
    pub particle: Option<ParticleShading>,
    /// Texture parameters bound to a `TextureRenderTarget2D` (a planar reflection filled by a
    /// `SceneCaptureReflectActor`): (parameter name, render target object path).
    #[serde(default)]
    pub render_targets: Vec<(String, String)>,
    /// The material's compiled shaders in the install's `RefShaderCache-PC-D3D-SM3.upk`
    /// (`ue3_shader`), for the runtime's original-shader path; None when no shader map matches.
    #[serde(default)]
    pub ue3: Option<Ue3Material>,
}

/// Texture slot value of a render target (scene captures) in [`Ue3Material::textures`]: its
/// clear colour, black.
pub const UE3_TEX_BLACK: TextureId = usize::MAX;

/// A material linked to its compiled shader map, with the map's parameters and textures
/// resolved for this material (instance). Adapted from dishonored-bevy by Eamon
/// (github.com/Eamo5/dishonored-bevy), used with permission (its `format::Ue3Material`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ue3Material {
    /// Index of the shader map in the shader cache (`ShaderLibrary::maps`).
    pub map: u32,
    /// Values of the shader map's parameter nodes, in `shadercache::param_nodes` order.
    pub params: Vec<[f32; 4]>,
    /// Those parameters' names (what the level scripts set: `SeqAct_SetMatInstScalarParam`).
    pub param_names: Vec<String>,
    /// Texture of each pixel `Texture2D_N` expression ([`UE3_TEX_BLACK`]: a render target).
    pub textures: Vec<Option<TextureId>>,
    /// Faces (+X, -X, +Y, -Y, +Z, -Z) of each cube texture expression.
    pub cube_textures: Vec<Option<Vec<TextureId>>>,
    /// The base material's own blend mode, two-sidedness and lighting model.
    pub blend: MaterialBlend,
    pub two_sided: bool,
    pub unlit: bool,
}

/// A time-varying material parameter (`MaterialInstanceTimeVarying` `ScalarParameterValues[]
/// .ParameterValueCurve`): its value before it plays and its keys (seconds, value; linear).
/// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PostCurve {
    pub param: String,
    pub default: f32,
    pub keys: Vec<[f32; 2]>,
}

impl PostCurve {
    /// The value at `t` seconds (held before the first key and after the last).
    pub fn at(&self, t: f32) -> f32 {
        let Some(first) = self.keys.first() else { return self.default };
        if t <= first[0] {
            return first[1];
        }
        for w in self.keys.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t <= b[0] {
                let k = if b[0] > a[0] { (t - a[0]) / (b[0] - a[0]) } else { 1.0 };
                return a[1] + (b[1] - a[1]) * k;
            }
        }
        self.keys.last().map(|k| k[1]).unwrap_or(self.default)
    }

    /// When its last key is.
    pub fn end(&self) -> f32 {
        self.keys.last().map(|k| k[0]).unwrap_or(0.0)
    }
}

/// How a particle material's shader combines the sprite texture (colour x opacity, baked into
/// the diffuse slot by the importer) with constants and the particle's vertex colour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ParticleShading {
    /// Constant colour factor (`D_DiffuseColorTweak.rgb`, `D_Diffuse_Brightness`, `Color`...).
    pub tint: [f32; 3],
    /// Constant opacity factor (`D_DiffuseColorTweak.a`, `Opacity`...).
    pub opacity: f32,
    /// The particle's alpha erodes the opacity map instead of scaling it
    /// (`saturate((H - vertex alpha) x Mask_Multiply)`, the height-map blood materials).
    pub erode: bool,
    /// The shader ignores the texture's opacity: only the vertex alpha (and `opacity`) count
    /// (`Default_Simple_Translucent_PMAT` without `MA_Use_Masked_Alpha`).
    pub vertex_alpha_only: bool,
    /// The recipe's opacity texture did not resolve (regression checks list these).
    #[serde(default)]
    pub opacity_missing: bool,
    /// Procedural radial glow (`Glow_PMAT`): no texture, `(1 - 2r)^power`.
    pub radial_power: Option<f32>,
}

impl MaterialParams {
    pub fn switch(&self, name: &str) -> Option<bool> {
        self.static_switches.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|v| v.1)
    }
    pub fn mask_channel(&self, name: &str) -> Option<u8> {
        self.mask_channels.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|v| v.1)
    }
}

/// A camera lens effect (`DisEmitterCameraLensEffect` with its `DisTweaks_EmitterCameraLensEffect`,
/// else the effect class's defaults): a particle system held `distance` in front of the camera,
/// pulled in for a wider view (UE3 `EmitterCameraLensEffectBase::UpdateLocation`: the distance
/// times `tan(base_fov / 2) / tan(fov / 2)`), for `life_span` seconds (0: until its particles end).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LensEffectDef {
    /// The spawning action: `<package>:<object path>`.
    pub action: String,
    /// `ParticleSystem` path (`m_DefaultParticleSystem`, `PS_CameraEffect`).
    pub system: String,
    pub distance: f32,
    pub base_fov: f32,
    pub life_span: f32,
}

/// A placed actor drawn with its `SkeletalMeshComponent` (`DisDoor`, `DishonoredUsableObject`
/// and its subclasses): doors, lockers, hatches, bins, sinks, alarm panels. The original moves
/// them by animation (a door's open / close stage), so they are kept apart from the static
/// geometry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkeletalProp {
    /// `<package>:<actor name>`, as the Kismet names actors (`L_Boyle_Ext_Audio:DisDoor_4`).
    pub actor: String,
    pub class: String,
    /// Package that defines the skeletal mesh, and the mesh's object path.
    pub mesh: (String, String),
    /// Actor x component transform (Dishonored world space).
    pub transform: Mat4,
    /// The actor's placed location and rotation only (the frame the Kismet moves).
    pub actor_to_world: Mat4,
    pub hidden: bool,
    /// The component's `AnimSets`: (package, object path).
    pub anim_sets: Vec<(String, String)>,
    /// Door stage animations (`DisDoor` only).
    pub door: Option<DoorAnims>,
    /// The lighting the original's `DynamicLightEnvironmentComponent` gives the prop
    /// (`dishonored_assets::light_env`); `None` when it could not be computed.
    #[serde(default)]
    pub light_env: Option<LightEnvironment>,
    /// The looping animation the prop plays at rest (`DisRiverKrust`: its tweak's
    /// `m_AnimSequences[RiverKrustAnimState_Ambient]`); None: the reference pose.
    #[serde(default)]
    pub idle: Option<PropIdle>,
}

/// A prop's resting loop (`RiverKrustAnimSequenceSpec`): sequence, play-rate range and start-time
/// range (the original rolls both at random per instance).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PropIdle {
    pub anim: String,
    pub rate: [f32; 2],
    pub start: [f32; 2],
}

/// A light environment's synthesized lights (`FDynamicLightEnvironmentState`): one directional
/// light and an upper / lower sky light, in UE3 linear light units (colour x brightness), drawn by
/// the base pass `TBasePassPixelShader<FDirectionalLightLightMapPolicy, SkyLight>`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LightEnvironment {
    /// Unit direction towards the synthesized directional light (Dishonored world space).
    pub direction: Vec3,
    pub color: [f32; 3],
    pub upper_sky: [f32; 3],
    pub lower_sky: [f32; 3],
}

/// A `DisDoor`'s placed state and the transition animation of each of its stages
/// (`DisTweaks_Door.m_Door{Open,Close}{CW,CCW}.m_TransitionAnimName`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DoorAnims {
    /// `m_InitialDoorState` (`EDisDoorState`: 0 closed, 1 opened CW, 2 opened CCW).
    pub initial: u8,
    pub open_cw: String,
    pub open_ccw: String,
    pub close_cw: String,
    pub close_ccw: String,
}

/// Level-wide presentation settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Environment {
    /// Reflection cubemap faces (+X, -X, +Y, -Y, +Z, -Z in UE3 face order).
    pub cubemap_faces: Vec<TextureId>,
    /// Arkane `DisFog` layers (Dishonored has no HeightFog/ExponentialHeightFog actors).
    pub fog: Vec<FogSettings>,
    pub post: Option<PostSettings>,
    /// Lights with `bRenderLightShafts` (UE3 light shafts: occlusion mask, radial blur, bloom).
    #[serde(default)]
    pub light_shafts: Vec<LightShaft>,
}

/// A directional light whose `LightComponent` has `bRenderLightShafts` (the retail shaft lights
/// are all `DirectionalLightComponent`s, most with no direct light of their own).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LightShaft {
    pub source: String,
    /// `GetPosition()`: `-direction x TraceDistance`, a far world point towards the light, uu.
    pub position: Vec3,
    /// `OcclusionDepthRange`, `BloomScale`, `BloomThreshold`, `BloomScreenBlendThreshold`.
    pub occlusion_depth_range: f32,
    pub bloom_scale: f32,
    pub bloom_threshold: f32,
    pub bloom_screen_blend_threshold: f32,
    /// `BloomTint` as a linear colour.
    pub bloom_tint: [f32; 3],
    /// `RadialBlurPercent`, `OcclusionMaskDarkness`.
    pub radial_blur_percent: f32,
    pub occlusion_mask_darkness: f32,
}

/// One `DisFog` actor's `DisFogComponent`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FogSettings {
    pub source: String,
    /// Linear colour (from `LightColor`).
    pub color: [f32; 3],
    /// Maximum fog opacity (`Opacity`).
    pub opacity: f32,
    /// Fog fades in between these view distances, uu (`NearPlane`, `FarPlane`).
    pub near: f32,
    pub far: f32,
    /// The layer spans world Z `origin .. origin + height` (`Origin`, `Height`).
    pub origin: f32,
    pub height: f32,
    /// Scene depth (uu) beyond which this layer adds no fog (`NoFogPlane`).
    #[serde(default)]
    pub no_fog: f32,
    /// Scales the ray's height-slab parameters (`HeightDensityFactor`, default 1).
    #[serde(default)]
    pub height_density: f32,
    /// Sun layer (`bIsSun`): its fog is weighted by `max(cos(view, sun), 0)^SunPower`.
    #[serde(default)]
    pub is_sun: bool,
    #[serde(default)]
    pub sun_power: f32,
    /// 128-entry fog curve over the normalised in-layer distance (`m_RawLut`, baked from
    /// `FogLUT`); empty = linear ramp.
    #[serde(default)]
    pub lut: Vec<u8>,
    /// `bCustomTransition` + `fCustomTransitionHeight` (opacity fades in over that distance
    /// below the layer top, 0x0081C940); `bInteriorFog` and `m_IsExclusive` affect the layer
    /// selection, which is not decoded (U-321).
    #[serde(default)]
    pub custom_transition: Option<f32>,
    #[serde(default)]
    pub interior: bool,
    #[serde(default)]
    pub exclusive: bool,
}

/// Arkane post-process configuration (`ArkPpConfig`), only the fields set on the level.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PostSettings {
    pub source: String,
    pub values: Vec<(String, f32)>,
    pub vectors: Vec<(String, [f32; 4])>,
}

/// Dishonored's two-coefficient directional lightmap (`NormalizedAverageColor_N` +
/// `DirectionalMaxComponent_N`), sRGB-decoded and multiplied by each component's coefficient
/// scales, in the original atlas layout. The base pass reconstructs (research/LOG.md
/// "Directional lightmaps"): diffuse = albedo * color * sum_i dir_i * saturate(N.B_i)^2 and
/// specular = spec * color * sum_i dir_i * saturate(R.B_i)^16 over the tangent-space HL2 basis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightmapAtlas {
    pub source: String,
    pub width: u32,
    pub height: u32,
    /// Row-major scaled average colour, packed as RGB9E5 (`Rgb9e5Ufloat`).
    pub texels: Vec<u32>,
    /// Row-major scaled intensities along the three basis directions (R, G, B = B0, B1, B2),
    /// RGB9E5. Empty for atlases without directional data.
    #[serde(default)]
    pub directional: Vec<u32>,
    /// Row-major signed distance field of the dominant directional light's shadow (the
    /// components' `ShadowMap2D` PF_G8 textures resampled into this layout; 0.5 = shadow edge),
    /// 255 where no component has one. Empty when no component of the atlas has one.
    #[serde(default)]
    pub shadow: Vec<u8>,
    /// The same for the dominant point / spot light of each component
    /// (`InstanceLightmap::df_light`), 255 where none. Empty when no component has one.
    #[serde(default)]
    pub shadow_local: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct InstanceLightmap {
    pub atlas: usize,
    /// Lightmap UV = mix(uv_min, uv_max, mesh UV1) (UE3 CoordinateBias / Bias+Scale).
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    /// The component has a distance-field shadow map of the dominant directional light
    /// (`LightmapAtlas::shadow`): the original shadows that light with it instead of a
    /// dynamic shadow map.
    #[serde(default)]
    pub df_shadow: bool,
    /// Index (in `DishonoredMap::lights`) of the dominant point / spot light whose distance-field
    /// shadow map this component has (`LightmapAtlas::shadow_local`).
    #[serde(default)]
    pub df_light: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticMeshAsset {
    /// Source object path, e.g. `air_duct_01.air_duct_09`.
    pub source: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub tangents: Vec<Vec3>,
    pub uv0: Vec<Vec2>,
    /// Lightmap UVs (the set selected by `LightMapCoordinateIndex`).
    pub uv1: Vec<Vec2>,
    /// The mesh's texture coordinate 1 (what the material shaders read as `TEXCOORD1`) when
    /// it differs from `uv1`, i.e. when `LightMapCoordinateIndex` is not 1; empty otherwise.
    #[serde(default)]
    pub uv_set1: Vec<Vec2>,
    /// The mesh's vertex colours (R, G, B, A bytes; the shaders read them as byte / 255), one per
    /// position; empty when the mesh has none.
    #[serde(default)]
    pub colors: Vec<[u8; 4]>,
    pub indices: Vec<u32>,
    pub sections: Vec<MeshSection>,
    /// Collision triangles from the mesh's kDOP tree (indices into `positions`) with the
    /// per-triangle material index. Empty when the mesh has no per-poly collision.
    pub collision_triangles: Vec<[u32; 3]>,
    /// Material (section) index of each collision triangle (kDOP `MaterialIndex`).
    #[serde(default)]
    pub collision_materials: Vec<u16>,
    /// `UseSimpleLineCollision` / `UseSimpleBoxCollision`: zero-extent / extent checks use the
    /// simplified collision instead of the kDOP triangles (and nothing without a body setup).
    #[serde(default)]
    pub simple_line: bool,
    #[serde(default)]
    pub simple_box: bool,
    /// The body setup's simplified collision (`AggGeom` elements) as mesh-space triangles;
    /// `None` without a body setup.
    #[serde(default)]
    pub simple_collision: Option<Vec<Vec<[Vec3; 3]>>>,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshSection {
    pub first_index: u32,
    pub num_triangles: u32,
    /// Default material for this section (may be overridden per instance).
    pub material: Option<MaterialId>,
    pub enable_collision: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaterialAsset {
    pub source: String,
    pub diffuse: Option<TextureId>,
    pub normal: Option<TextureId>,
    /// Masked/translucent materials (foliage, glass) where known.
    pub blend: BlendHint,
    /// Every texture parameter found while resolving, for later material work.
    pub texture_params: Vec<(String, TextureId)>,
    /// Contact type class of the material's physical material (`PhysMaterial` ->
    /// `PhysicalMaterialProperty` = `DisContactPhysicalMaterial.m_pContactType`, following
    /// `Parent`), e.g. `DisContactType_Env_Wood`: drives footstep and impact sounds.
    #[serde(default)]
    pub contact_type: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendHint {
    #[default]
    Opaque,
    Masked,
    Translucent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureFormat {
    /// BC1 / DXT1
    Bc1,
    /// BC2 / DXT3
    Bc2,
    /// BC3 / DXT5
    Bc3,
    /// BC5 / ATI2 (two-channel normal maps)
    Bc5,
    /// 8-bit BGRA
    Bgra8,
    /// 8-bit single channel
    R8,
    /// Signed two-channel (UE3 PF_V8U8 normal maps)
    Rg8Snorm,
}

impl TextureFormat {
    /// Bytes per 4x4 block for block formats, or per pixel for uncompressed.
    pub fn is_block(self) -> bool {
        matches!(self, Self::Bc1 | Self::Bc2 | Self::Bc3 | Self::Bc5)
    }
    pub fn block_bytes(self) -> usize {
        match self {
            Self::Bc1 => 8,
            Self::Bc2 | Self::Bc3 | Self::Bc5 => 16,
            Self::Bgra8 => 4,
            Self::R8 => 1,
            Self::Rg8Snorm => 2,
        }
    }
    pub fn mip_size(self, w: u32, h: u32) -> usize {
        if self.is_block() {
            (w.div_ceil(4) * h.div_ceil(4)) as usize * self.block_bytes()
        } else {
            (w * h) as usize * self.block_bytes()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureAsset {
    pub source: String,
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    /// Mip chain, largest first, starting at `width`x`height`.
    pub mips: Vec<Vec<u8>>,
    pub srgb: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshInstance {
    pub mesh: MeshId,
    /// Mesh-local to world (Dishonored world space), column-major glam matrix.
    pub transform: Mat4,
    /// Per-section material overrides (`None` = use the mesh default).
    pub materials: Vec<Option<MaterialId>>,
    pub hidden: bool,
    pub collides: bool,
    pub source: String,
    /// The level package the component was placed in (empty: unknown, e.g. layout maps).
    #[serde(default)]
    pub level: String,
    /// Baked lighting for this component (LOD0), if any.
    #[serde(default)]
    pub lightmap: Option<InstanceLightmap>,
    /// The component belongs to a mover (`InterpActor`) that blocks pawns: it collides where
    /// the actor is at run time (Kismet matinees and attachments move it), not in the static
    /// collision.
    #[serde(default)]
    pub mover: Option<MoverRef>,
    /// The component's `m_CollisionTraceTypes` (`DisPrimTraceMask` bits: 0 move non-pawn,
    /// 1 move NPC, 2 move player, 3 crosshair, 4 projectile, 5 melee, 6 vision).
    #[serde(default = "all_trace_types")]
    pub trace_mask: u8,
    /// The component's `ReflectionChannels` (see [`reflect_channel`]); 0 = in no reflection capture.
    #[serde(default)]
    pub reflect_channels: u32,
    /// The stealth light's visibility line check (`TRACE_ShadowCast`, flags 0x2397 at 0x004FF5C2)
    /// hits this component: it is in the primitive octree (`CollideActors`, its actor's
    /// `bCollideActors`), `CastShadow` and `bUsePrecomputedShadows`, `bAcceptsLights`, and has no
    /// `LightEnvironment` (the octree's line check 0x006B12C0 and `ULightComponent::AffectsPrimitive`
    /// 0x00519760; research/dishonored/systems/stealth.md section 14).
    #[serde(default)]
    pub stealth_shadow: bool,
    /// The component's `LightingChannels` ([`lighting_channel`] bits; the serialized struct over
    /// `lighting_channel::STATIC_MESH_DEFAULT`). 0 for instances that were not read from a component.
    #[serde(default)]
    pub lighting_channels: u32,
}

pub fn all_trace_types() -> u8 {
    0x7F
}

/// The actor of a mover component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoverRef {
    /// `<package>:<actor name>`, as the Kismet names actors (e.g. `L_BoyleStreet_Ext_Script:InterpActor_1`).
    pub actor: String,
    /// The actor's placed transform (location and rotation) when the map was imported.
    pub actor_to_world: Mat4,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollisionWorld {
    /// World-space collision triangles (filled by the collision builder).
    pub triangles: Vec<[Vec3; 3]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LightKind {
    Point,
    Spot { inner_cone_deg: f32, outer_cone_deg: f32 },
    Directional,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Light {
    pub kind: LightKind,
    pub position: Vec3,
    pub direction: Vec3,
    pub color: [f32; 3],
    pub brightness: f32,
    pub radius: f32,
    pub source: String,
    /// UE3 radial falloff `(1 - (d / radius)^2)^falloff_exponent`.
    #[serde(default)]
    pub falloff_exponent: f32,
    /// Dominant lights: direct light is not in the lightmaps and they cast the main shadows.
    #[serde(default)]
    pub dominant: bool,
    #[serde(default)]
    pub casts_shadows: bool,
    /// The component's `LightGuid` (what `ShadowMap2D::LightGuid` refers to).
    #[serde(default)]
    pub guid: [u32; 4],
    /// The component's LightFunction (its material's parameters), if any.
    #[serde(default)]
    pub function: Option<LightFunction>,
    /// The light's local Y and Z axes in world space (`direction` is its X axis): the plane a
    /// projected light function lies in.
    #[serde(default)]
    pub axes: [Vec3; 2],
    /// `LightingChannels` ([`lighting_channel`] bits): the light affects primitives sharing one.
    #[serde(default)]
    pub lighting_channels: u32,
}

/// A light function (ULightComponent::Function): the light's colour is multiplied by the
/// function material's value. Bodies come from the compiled light function shaders
/// (research/dishonored/systems/render_lighting.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LightFunction {
    /// *_light_function_PMAT (Flickering, WoodStove, Boiler): grey value
    /// clamp(cos(2 pi t s1), 0.5, 1) + clamp(cos(2 pi t s2), 0.2, 0.7) (shader 1315).
    Flicker { speed1: f32, speed2: f32, material: String },
    /// Casutics_LightFunction_PMAT (pixel shader 36479): two scrolled samples of `textures[1]`
    /// (`T_Caustics_2`), their offsets pushed by two samples of `textures[0]`
    /// (`T_EV_DustPanner_01`), projected onto the plane of the light's Y and Z axes at `scale` uu
    /// per texture repeat (`LightFunction::Scale`).
    Caustics { speed: f32, color: [f32; 3], scale: [f32; 3], textures: [Option<TextureId>; 2] },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnCandidate {
    pub position: Vec3,
    pub yaw_deg: f32,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnconvertedObject {
    pub class: String,
    pub path: String,
}

/// A skinned character part (body, head, mask...) in UE3 mesh space, plus the transform
/// from mesh space to pawn space (cylinder centre, Z-up, uu).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletalMeshAsset {
    pub source: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uv0: Vec<Vec2>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub sections: Vec<MeshSection>,
    pub bones: Vec<BoneAsset>,
    pub mesh_to_pawn: Mat4,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoneAsset {
    pub name: String,
    pub parent: i32,
    pub rotation: glam::Quat,
    pub translation: Vec3,
}

/// A playable/NPC character archetype assembled from parts sharing one skeleton.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterModel {
    pub name: String,
    pub parts: Vec<SkeletalMeshAsset>,
    pub materials: Vec<MaterialAsset>,
    /// Arkane material parameters per material (same meaning as `DishonoredMap::material_params`).
    #[serde(default)]
    pub material_params: Vec<MaterialParams>,
    pub textures: Vec<TextureAsset>,
}

