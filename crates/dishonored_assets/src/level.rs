//! Level import: StaticMeshActors and StaticMeshCollectionActors from a set of
//! level packages into a normalized [`DishonoredMap`].

use crate::content::Content;
use crate::lightmap::{LightMap2DRef, LightMapRef, decode_bc1, pack_rgb9e5, read_static_mesh_component_lods, srgb_to_linear};
use crate::material_chain;
use crate::spline_mesh;
use crate::texture::decode_rgba8;
use crate::static_mesh::read_static_mesh;
use crate::texture::read_texture;
use crate::util::{fmatrix_rows, rotator_matrix};
use anyhow::{Context, Result};
use dishonored_ir::*;
use glam::{Mat4, Vec2, Vec3};
use std::collections::HashMap;
use std::sync::Arc;
use ue3_package::properties::{Property, Value, find};
use ue3_package::reader::Reader;
use ue3_package::{ObjRef, Package};

// [playground] Maps built from a layout of original assets (`dishonored_ir::layout`).
#[path = "layout_import.rs"]
pub mod layout_import;

pub struct ImportOptions {
    /// Largest texture side to load (smaller mips are kept).
    pub max_texture_size: u32,
    pub load_textures: bool,
    /// Decode and bake lightmaps (only when `load_textures` is also set).
    pub load_lightmaps: bool,
    /// Import StaticMesh/SplineMesh components of non-static actor classes.
    pub other_actors: bool,
    /// Decode the ParticleSystems of placed `Emitter` actors (and `extra_particles`).
    pub particles: bool,
    /// Further ParticleSystem paths to decode (gameplay effects), resolved through the index.
    pub extra_particles: Vec<String>,
    /// File caching the baked lightmap pages (lightmap_cache.rs); None bakes every time.
    pub lightmap_cache: Option<std::path::PathBuf>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            max_texture_size: 1024,
            load_textures: true,
            load_lightmaps: true,
            other_actors: true,
            particles: true,
            extra_particles: Vec::new(),
            lightmap_cache: None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ImportStats {
    /// Wall time of the import's stages, seconds (package loading, actors, lightmaps).
    pub timings: Vec<(&'static str, f32)>,
    /// The lightmap pages came from the on-disk cache (`ImportOptions::lightmap_cache`).
    pub lightmaps_cached: bool,
    pub static_mesh_actors: usize,
    pub collection_components: usize,
    /// Instances from StaticMeshComponents of other actor classes (render-only).
    pub other_actor_components: usize,
    /// SplineMeshComponents baked into deformed mesh copies.
    pub spline_meshes: usize,
    /// Spline loft actors (`SplineLoftActor`, `DisSplineLoftActor`: cables, rails) imported
    /// through their cooked `SplineMeshComponent`s (`SplineMeshComps`, the loft of `DeformMesh`
    /// along the actor's spline to its `Connections`, generated in the editor and cooked).
    pub spline_loft_actors: usize,
    /// [foliage-survey] Placed SkeletalMeshComponents (doors, usable objects, sinks, alarm bells,
    /// climbable chains) imported as render-only instances in their reference pose.
    pub skeletal_props: usize,
    /// Static light actors skipped because their light is baked into the lightmaps.
    pub static_lights_skipped: usize,
    /// Static lights with a light function imported as dynamic lights.
    pub function_lights: usize,
    pub instances: usize,
    pub meshes: usize,
    pub mesh_failures: usize,
    pub materials: usize,
    pub textures: usize,
    pub texture_failures: usize,
    pub unresolved_refs: usize,
    /// Components by LOD0 lightmap kind, and LODData parse failures.
    pub lightmaps_texture: usize,
    pub lightmaps_vertex: usize,
    pub lightmaps_none: usize,
    pub lightmap_parse_failures: usize,
    pub lightmap_atlases: usize,
    /// Pages after packing the atlases for batching.
    pub lightmap_pages: usize,
    /// Lightmapped components with a distance-field shadow map of the dominant directional light.
    pub shadow_map_components: usize,
    /// ... and with one of a dominant point / spot light (U-403).
    pub local_shadow_map_components: usize,
    /// Distinct cross-package references that no indexed package defines.
    pub unresolved_paths: Vec<String>,
    /// MIC parent chains that end at an undefined import.
    pub material_chain_breaks: Vec<String>,
    /// Materials whose diffuse resolves to nothing (with their texture parameter names).
    pub materials_without_diffuse: Vec<String>,
    pub errors: Vec<String>,
}

struct PendingLightmap {
    instance: usize,
    slot: usize,
    lm: LightMap2DRef,
    /// The component's distance-field `ShadowMap2D`s (`bIsShadowFactorTexture` false).
    shadows: Vec<ShadowMapRef>,
}

/// One `ShadowMap2D`: shadow map UV = UV1 x `CoordinateScale` + `CoordinateBias`.
struct ShadowMapRef {
    light_guid: [u32; 4],
    texture: Key,
    scale: [f32; 2],
    bias: [f32; 2],
}

type Key = (usize, usize); // (package slot, export index)

struct Importer<'a> {
    content: &'a Content,
    opts: &'a ImportOptions,
    pkgs: Vec<Arc<Package>>,
    /// Lowercase object path -> definition, for resolving cross-package imports.
    index: HashMap<String, Key>,
    map: DishonoredMap,
    mesh_ids: HashMap<Key, Option<MeshId>>,
    mat_ids: HashMap<Key, Option<MaterialId>>,
    tex_ids: HashMap<Key, Option<TextureId>>,
    pending_lightmaps: Vec<PendingLightmap>,
    merged_ids: HashMap<(TextureId, TextureId, u32, u32), Option<TextureId>>,
    stats: ImportStats,
    ps_ids: HashMap<Key, Option<usize>>,
    class_defaults: HashMap<String, Vec<Property>>,
    /// [foliage-survey] Skeletal meshes imported as static scenery.
    skel_meshes: HashMap<Key, Option<SkeletalProp>>,
    /// Effective (archetype-resolved) properties for the collision flags.
    resolver: crate::collision_audit::PropResolver<'a>,
    /// Tweaks of the map, loaded when a physics prop needs its `m_bFixed`.
    tweaks: Option<Option<crate::tweaks::TweakDb>>,
}

/// Single-channel static component masks kept in `MaterialParams::mask_channels`.
const MASK_CHANNELS: &[&str] = &["C_Cubemap_Mask_Channel", "O_Opacity_Mask_Channel", "S_Specular_Mask_Channel", "E_Emissive_Channel", "F_Fresnel_Mask_Channel", "D2_Diffuse_Variation_mask_channel", "S2_Specular_variation_Mask_Channel", "D_Diffuse_Mask_Chanel", "W_Distorsion_Mask_Chanel", "F_Foam_Mask_Chanel", "Storm_mask"];

/// The textures a base material's compiled base pass samples, kept by name in
/// `MaterialAsset::texture_params` for the runtime's decoded shaders (None: the generic slots only).
fn base_raw_textures(base: &str) -> Option<&'static [&'static str]> {
    match base.rsplit('.').next().unwrap_or(base) {
        // Light beams (runtime volume_light.rs): `Use_rays` samples `LightRays_Mask_R` (6633).
        "Volume_Light_PMAT" => Some(&["LightRays_Mask_R"]),
        // Falling water sheets (runtime spillway.rs): base passes 1584 / 1585 / 1595 / 1596.
        "water_spillway_translu_PMAT" => Some(&["Normal", "SpillWay_Texture"]),
        // The island sea (runtime water_foam.rs): base passes 37072 .. 37076.
        "water_Foam_MAT" => Some(&["W_Waves_normalmap", "W_Waves_normalrain", "F_Foam_Mask", "F_Foam_Distortion", "F_Foam_Texture"]),
        _ => None,
    }
}

/// Cube textures a base material's base pass samples (kept face by face).
fn base_cube_textures(base: &str) -> Option<&'static [&'static str]> {
    match base.rsplit('.').next().unwrap_or(base) {
        "water_spillway_translu_PMAT" => Some(&["C_Cubemap_texture"]),
        _ => None,
    }
}

/// Base materials whose base pass samples unnamed expression textures (`Tex#i`).
fn base_uses_expression_textures(base: &str) -> bool {
    matches!(base.rsplit('.').next().unwrap_or(base), "water_spillway_translu_PMAT")
}

const RESOURCE_CLASSES: &[&str] = &[
    "StaticMesh",
    "Texture2D",
    "Material",
    "MaterialInstanceConstant",
    "TextureCube",
    "ParticleSystem",
    "SkeletalMesh",
];

/// [foliage-survey] Actor classes whose placed `SkeletalMeshComponent` is static scenery until
/// gameplay moves it: drawn in the mesh's reference pose (doors closed, lids shut, chains hanging).
/// Plain `SkeletalMeshActor`s join them unless Kismet references them. Not included: `DisFish`
/// (swimming crowd agents) and `SkeletalMeshActorMAT` (matinee props, drawn by the cutscene
/// player); see UNKNOWNS U-571. Usable actors (doors, sinks, alarm bells) are drawn animated from
/// `DishonoredMap::skeletal_props` (`usable_props`), so they are not instanced here.
const SKELETAL_PROP_CLASSES: &[&str] = &["DisClimbable"];

/// Always-loaded packages whose resources level packages import without cooking a copy
/// (U-005: every unresolved Boyle Ext import was found in one of these).
pub const LIBRARY_PACKAGES: &[&str] = &["DishonoredGame", "Startup"];

/// Import the given level packages (e.g. `L_Boyle_Ext_P`, `L_Boyle_Ext_Env`, ...).
pub fn import_level(
    content: &Content,
    name: &str,
    packages: &[String],
    opts: &ImportOptions,
) -> Result<(DishonoredMap, ImportStats)> {
    let t0 = std::time::Instant::now();
    let mut pkgs = Vec::new();
    for p in packages {
        pkgs.push(content.package(p).with_context(|| format!("loading {p}"))?);
    }
    let level_slots = pkgs.len();
    {
        for p in LIBRARY_PACKAGES {
            if content.has_package(p) {
                pkgs.push(content.package(p).with_context(|| format!("loading {p}"))?);
            }
        }
    }
    let mut index = HashMap::new();
    for (slot, pkg) in pkgs.iter().enumerate() {
        for i in 0..pkg.exports.len() {
            let class = pkg.export_class(i);
            if RESOURCE_CLASSES.contains(&class.as_str()) {
                index
                    .entry(pkg.obj_path(ObjRef::Export(i)).to_ascii_lowercase())
                    .or_insert((slot, i));
            }
        }
    }
    let mut imp = Importer {
        content,
        opts,
        pkgs,
        index,
        map: DishonoredMap {
            name: name.to_string(),
            ..Default::default()
        },
        mesh_ids: HashMap::new(),
        mat_ids: HashMap::new(),
        tex_ids: HashMap::new(),
        pending_lightmaps: Vec::new(),
        merged_ids: HashMap::new(),
        stats: ImportStats::default(),
        ps_ids: HashMap::new(),
        class_defaults: HashMap::new(),
        skel_meshes: HashMap::new(),
        resolver: crate::collision_audit::PropResolver::new(content),
        tweaks: None,
    };
    imp.stats.timings.push(("packages", t0.elapsed().as_secs_f32()));
    let t = std::time::Instant::now();
    // The level (slot) of every instance, for the light environments' floor check.
    let mut instance_level: Vec<usize> = Vec::new();
    for slot in 0..level_slots {
        imp.import_actors(slot)?;
        instance_level.resize(imp.map.instances.len(), slot);
    }
    // [foliage-survey] After every static component, so existing instance indices are unchanged.
    if opts.other_actors {
        for slot in 0..level_slots {
            imp.import_skeletal_props(slot);
            instance_level.resize(imp.map.instances.len(), slot);
        }
    }
    if opts.particles && opts.load_textures {
        for p in &opts.extra_particles {
            imp.particle_by_path(p);
        }
    }
    imp.stats.timings.push(("actors", t.elapsed().as_secs_f32()));
    let t = std::time::Instant::now();
    if opts.load_textures && opts.load_lightmaps {
        imp.bake_lightmaps();
    }
    imp.stats.timings.push(("lightmaps", t.elapsed().as_secs_f32()));
    if level_slots > 0 {
        imp.import_environment(0);
    }
    // Doors and usable objects: their skeletal mesh components (usable_props.rs).
    if opts.other_actors {
        let t = std::time::Instant::now();
        let (props, channels): (Vec<_>, Vec<_>) = crate::usable_props::read_props_with_channels(content, packages).into_iter().unzip();
        imp.map.skeletal_props = props;
        // Their light environments: the levels' static lights and precomputed light volumes.
        let volumes: Vec<Option<Vec<crate::light_env::VolumeSample>>> = imp.pkgs[..level_slots].iter().map(|p| crate::light_env::read_light_volume(p)).collect();
        let lights: Vec<crate::light_env::EnvLight> = imp.pkgs[..level_slots].iter().flat_map(|p| crate::light_env::read_static_lights(p)).collect();
        crate::light_env::light_props(content, &mut imp.map, &instance_level, &volumes, &lights, &channels);
        imp.stats.timings.push(("light environments", t.elapsed().as_secs_f32()));
    }
    imp.stats.unresolved_paths.sort();
    imp.stats.unresolved_paths.dedup();
    imp.stats.material_chain_breaks.sort();
    imp.stats.material_chain_breaks.dedup();
    imp.stats.instances = imp.map.instances.len();
    imp.stats.meshes = imp.map.meshes.len();
    imp.stats.materials = imp.map.materials.len();
    imp.stats.textures = imp.map.textures.len();
    Ok((imp.map, imp.stats))
}

fn vec3_or(props: &[Property], name: &str, d: Vec3) -> Vec3 {
    find(props, name).and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(d)
}
fn float_or(props: &[Property], name: &str, d: f32) -> f32 {
    find(props, name).and_then(|p| p.as_float()).unwrap_or(d)
}
fn bool_or(props: &[Property], name: &str, d: bool) -> bool {
    match find(props, name).map(|p| &p.value) {
        Some(Value::Bool(b)) => *b,
        _ => d,
    }
}
/// A `RenderingChannelContainer` property as a bit set (`dishonored_ir::reflect_channel`): the
/// struct's serialized bools override `base`.
pub(crate) fn channel_mask(props: &[Property], name: &str, base: u32) -> u32 {
    let Some(Value::Struct(_, s)) = find(props, name).map(|p| &p.value) else { return base };
    let mut m = base;
    for (i, n) in dishonored_ir::reflect_channel::NAMES.iter().enumerate() {
        if let Some(Value::Bool(b)) = find(s, n).map(|p| &p.value) {
            if *b {
                m |= 1 << i;
            } else {
                m &= !(1 << i);
            }
        }
    }
    m
}
fn rot_matrix(props: &[Property], name: &str) -> Mat4 {
    match find(props, name).and_then(|p| p.as_rotator()) {
        Some([p, y, r]) => rotator_matrix(p, y, r),
        None => Mat4::IDENTITY,
    }
}

/// UE3 `Actor::LocalToWorld` = Scale(DrawScale*DrawScale3D) * Rot * Trans (row-vector),
/// i.e. `T * R * S` for column vectors.
pub(crate) fn actor_transform(props: &[Property]) -> Mat4 {
    let loc = vec3_or(props, "Location", Vec3::ZERO);
    let scale = float_or(props, "DrawScale", 1.0) * vec3_or(props, "DrawScale3D", Vec3::ONE);
    Mat4::from_translation(loc) * rot_matrix(props, "Rotation") * Mat4::from_scale(scale)
}

/// Primitive component local transform (Translation, Rotation, Scale*Scale3D).
pub(crate) fn component_transform(props: &[Property]) -> Mat4 {
    let t = vec3_or(props, "Translation", Vec3::ZERO);
    let s = float_or(props, "Scale", 1.0) * vec3_or(props, "Scale3D", Vec3::ONE);
    Mat4::from_translation(t) * rot_matrix(props, "Rotation") * Mat4::from_scale(s)
}

/// Actor classes whose blocking static mesh components collide where the map places them
/// (research/dishonored/COLLISION.md, source table): `StaticMeshActor` subclasses that are world
/// geometry, and the Boyle actors that block pawns without being world geometry.
const STATIC_BLOCKER_CLASSES: &[&str] = &[
    "DisStaticMeshActor",
    "DisStaticCollisionActor",
    "DishonoredMovable",
    "DishonoredBreakableNavBlock",
    "KActor",
    "DisAlarmBell",
    "DisSpeaker_PA",
    "DisWaterSource",
    "DisWhaleOilBattery",
    "DishonoredUsableObject",
    "DishonoredDynamicUsableObject",
    "DisDoor",
    "DisWallOfLight",
];

impl Importer<'_> {
    fn is_in_persistent_level(&self, pkg: &Package, i: usize) -> bool {
        // Placed actors live in TheWorld.PersistentLevel; other outers are archetypes/prefabs.
        let outer = ObjRef::from_index(pkg.exports[i].outer);
        pkg.obj_name(outer) == "PersistentLevel"
    }

    fn import_actors(&mut self, slot: usize) -> Result<()> {
        let pkg = self.pkgs[slot].clone();
        self.map.environment.light_shafts.extend(read_light_shafts(&pkg));
        // Placed StaticMeshComponents owned by other actor classes (InterpActor, KActor,
        // DishonoredMovable, DisDoor, pickups, usable objects...): outer -> components.
        let mut owned: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..pkg.exports.len() {
            if matches!(pkg.export_class(i).as_str(), "StaticMeshComponent" | "SplineMeshComponent") {
                if let ObjRef::Export(o) = ObjRef::from_index(pkg.exports[i].outer) {
                    owned.entry(o).or_default().push(i);
                }
            }
        }
        self.import_function_lights(slot);
        for i in 0..pkg.exports.len() {
            let class = pkg.export_class(i);
            if !matches!(class.as_str(), "StaticMeshActor" | "StaticMeshCollectionActor")
                && self.opts.other_actors
                && owned.contains_key(&i)
                && self.is_in_persistent_level(&pkg, i)
            {
                if matches!(class.as_str(), "SplineLoftActor" | "DisSplineLoftActor") {
                    self.stats.spline_loft_actors += 1;
                }
                if let Err(e) = self.import_owned_components(slot, i, &owned[&i]) {
                    self.stats.errors.push(format!("{}: {e:#}", pkg.obj_path(ObjRef::Export(i))));
                }
                continue;
            }
            if class == "SceneCaptureReflectActor" && self.is_in_persistent_level(&pkg, i) {
                self.import_reflect_capture(slot, i);
            }
            match class.as_str() {
                "StaticMeshActor" if self.is_in_persistent_level(&pkg, i) => {
                    if let Err(e) = self.import_static_mesh_actor(slot, i) {
                        self.stats.errors.push(format!("{}: {e:#}", pkg.obj_path(ObjRef::Export(i))));
                    }
                }
                "StaticMeshCollectionActor" if self.is_in_persistent_level(&pkg, i) => {
                    if let Err(e) = self.import_collection(slot, i) {
                        self.stats.errors.push(format!("{}: {e:#}", pkg.obj_path(ObjRef::Export(i))));
                    }
                }
                "DisFog" if self.is_in_persistent_level(&pkg, i) => self.import_fog(slot, i),
                "Emitter" if self.opts.particles && self.opts.load_textures && self.is_in_persistent_level(&pkg, i) => self.import_emitter(slot, i),
                c if (c.ends_with("Light") || c.contains("LightToggleable") || c.contains("LightMovable")) && self.is_in_persistent_level(&pkg, i) => {
                    self.import_light(slot, i, c);
                }
                "PlayerStart" | "DishonoredPlayerStart" if self.is_in_persistent_level(&pkg, i) => {
                    if let Ok(h) = pkg.read_object(i) {
                        let yaw = find(&h.properties, "Rotation")
                            .and_then(|p| p.as_rotator())
                            .map(|r| r[1] as f32 * 360.0 / 65536.0)
                            .unwrap_or(0.0);
                        self.map.spawn_candidates.push(SpawnCandidate {
                            position: vec3_or(&h.properties, "Location", Vec3::ZERO),
                            yaw_deg: yaw,
                            source: format!("{}:{}", pkg.name, pkg.obj_path(ObjRef::Export(i))),
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A placed `SceneCaptureReflectActor`: its component's render target, channels and update
    /// settings, and the mirror plane (normal = the actor's X axis, through its location;
    /// `SceneCaptureReflectComponent` vtable slot 88, Dishonored.exe 0x006E8200). Defaults are
    /// the classes' UnrealScript defaults (`SceneCaptureComponent`, `SceneCaptureReflectComponent`,
    /// `Default__SceneCaptureReflectActor`'s component).
    fn import_reflect_capture(&mut self, slot: usize, actor: usize) {
        let pkg = self.pkgs[slot].clone();
        let Ok(h) = pkg.read_object(actor) else { return };
        let Some(ObjRef::Export(ci)) = find(&h.properties, "SceneCapture").and_then(|p| p.as_object()).map(ObjRef::from_index) else { return };
        let Ok(ch) = pkg.read_object(ci) else { return };
        let cp = &ch.properties;
        let Some(t) = find(cp, "TextureTarget").and_then(|p| p.as_object()).filter(|o| *o != 0) else { return };
        let Some(tk) = self.resolve(slot, ObjRef::from_index(t)) else { return };
        let tpkg = self.pkgs[tk.0].clone();
        let tp = tpkg.read_object(tk.1).map(|h| h.properties).unwrap_or_default();
        let resolution_type = match find(&tp, "m_ResolutionType").map(|p| &p.value) {
            Some(Value::Enum(e)) => match e.as_str() {
                "TRT_FULLSIZE" => 1,
                "TRT_HALFSIZE" => 2,
                "TRT_QUARTERSIZE" => 3,
                _ => 0,
            },
            _ => 0,
        };
        let int = |n: &str| match find(&tp, n).map(|p| &p.value) {
            Some(Value::Int(v)) => (*v).max(0) as u32,
            _ => 0,
        };
        let loc = vec3_or(&h.properties, "Location", Vec3::ZERO);
        let normal = rot_matrix(&h.properties, "Rotation").transform_vector3(Vec3::X).normalize_or(Vec3::Z);
        self.map.reflect_captures.push(dishonored_ir::ReflectCapture {
            source: format!("{}:{}", pkg.name, pkg.obj_path(ObjRef::Export(actor))),
            target: tpkg.obj_path(ObjRef::Export(tk.1)),
            resolution_type,
            size: [int("SizeX"), int("SizeY")],
            normal,
            d: normal.dot(loc),
            channels: channel_mask(cp, "ReflectionChannels", dishonored_ir::reflect_channel::CAPTURE_DEFAULT),
            frame_rate: float_or(cp, "FrameRate", 1000.0),
            enabled: bool_or(cp, "bEnabled", true),
            fog: bool_or(cp, "bEnableFog", false),
            post_process: bool_or(cp, "bEnablePostProcess", false),
            skip_if_unseen: bool_or(cp, "bSkipUpdateIfTextureUsersOccluded", true),
            max_update_dist: float_or(cp, "MaxUpdateDist", 0.0),
        });
    }

    /// Placed `Emitter`: its `ParticleSystemComponent`'s `Template` at the actor transform.
    fn import_emitter(&mut self, slot: usize, actor: usize) {
        let pkg = self.pkgs[slot].clone();
        let Ok(h) = pkg.read_object(actor) else { return };
        // Emitters attached to another actor (`Base`: pawns, movers) follow it; not placed.
        if find(&h.properties, "Base").and_then(|p| p.as_object()).is_some_and(|o| o != 0) {
            return;
        }
        let Some(ObjRef::Export(ci)) = find(&h.properties, "ParticleSystemComponent").and_then(|p| p.as_object()).map(ObjRef::from_index) else { return };
        let Ok(ch) = pkg.read_object(ci) else { return };
        let Some(t) = find(&ch.properties, "Template").and_then(|p| p.as_object()).filter(|o| *o != 0) else { return };
        let Some(k) = self.resolve(slot, ObjRef::from_index(t)) else { return };
        let Some(system) = self.particle_system(k) else { return };
        let transform = actor_transform(&h.properties) * component_transform(&ch.properties);
        self.map.emitters.push(dishonored_ir::particles::EmitterInstance {
            system,
            transform,
            source: format!("{}:{}", pkg.name, pkg.obj_path(ObjRef::Export(actor))),
            auto_activate: bool_or(&ch.properties, "bAutoActivate", true),
        });
    }

    fn particle_system(&mut self, k: Key) -> Option<usize> {
        if let Some(id) = self.ps_ids.get(&k) {
            return *id;
        }
        self.ps_ids.insert(k, None);
        let ps = crate::particles::decode(self, k);
        let id = (!ps.emitters.is_empty()).then(|| {
            self.map.particle_systems.push(ps);
            self.map.particle_systems.len() - 1
        });
        self.ps_ids.insert(k, id);
        id
    }

    /// Decode a ParticleSystem by object path (gameplay effects requested by the caller).
    fn particle_by_path(&mut self, path: &str) -> Option<usize> {
        let k = *self.index.get(&path.to_ascii_lowercase())?;
        self.particle_system(k)
    }

    /// Arkane `DisFog` layer (defaults from `Engine.Default__DisFogComponent`: Opacity 0.8,
    /// white, Height 20, NearPlane 2000, FarPlane 10000, Origin 0).
    fn import_fog(&mut self, slot: usize, actor: usize) {
        let pkg = self.pkgs[slot].clone();
        let Ok(h) = pkg.read_object(actor) else { return };
        let Some(ObjRef::Export(ci)) = find(&h.properties, "Component").and_then(|p| p.as_object()).map(ObjRef::from_index) else {
            return;
        };
        let Ok(ch) = pkg.read_object(ci) else { return };
        let c = &ch.properties;
        if !bool_or(c, "bEnabled", true) || !bool_or(&h.properties, "bEnabled", true) {
            return;
        }
        let color = match find(c, "LightColor").map(|p| &p.value) {
            Some(Value::Color(rgba)) => [0, 1, 2].map(|k| crate::lightmap::color_to_linear(rgba[k])),
            _ => [1.0; 3],
        };
        self.map.environment.fog.push(FogSettings {
            source: format!("{}:{}", pkg.name, pkg.obj_path(ObjRef::Export(ci))),
            color,
            opacity: float_or(c, "Opacity", 0.8),
            near: float_or(c, "NearPlane", 2000.0),
            far: float_or(c, "FarPlane", 10000.0),
            origin: float_or(c, "Origin", 0.0),
            height: float_or(c, "Height", 20.0),
            no_fog: float_or(c, "NoFogPlane", 400000.0),
            height_density: float_or(c, "HeightDensityFactor", 1.0),
            is_sun: bool_or(c, "bIsSun", false),
            sun_power: float_or(c, "SunPower", 16.0),
            lut: match find(c, "m_RawLut").map(|p| &p.value) {
                Some(Value::Array(bytes)) if bytes.len() >= 4 => {
                    // TArray<BYTE>: count, then the bytes.
                    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
                    bytes.get(4..4 + n).map(|b| b.to_vec()).unwrap_or_default()
                }
                _ => Vec::new(),
            },
            custom_transition: bool_or(c, "bCustomTransition", false).then(|| float_or(c, "fCustomTransitionHeight", 250.0)),
            interior: bool_or(c, "bInteriorFog", false),
            exclusive: bool_or(c, "m_IsExclusive", false),
        });
    }

    /// Static light components with a `LightFunction` (`UseDirectLightMap` is false for them, so their
    /// direct light is not in the lightmaps and the engine draws them dynamically with the function:
    /// LIKELY, stock `ULightComponent` behaviour, not confirmed in Dishonored.exe). Collected from
    /// `StaticLightCollectionActor`s and plain `PointLight` / `SpotLight` actors; dominant lights
    /// are imported by `import_light`, which attaches their function.
    fn import_function_lights(&mut self, slot: usize) {
        let pkg = self.pkgs[slot].clone();
        for ci in 0..pkg.exports.len() {
            let cclass = pkg.export_class(ci);
            if !matches!(cclass.as_str(), "PointLightComponent" | "SpotLightComponent") {
                continue;
            }
            let ObjRef::Export(ai) = ObjRef::from_index(pkg.exports[ci].outer) else { continue };
            if pkg.obj_name(ObjRef::from_index(pkg.exports[ai].outer)) != "PersistentLevel" {
                continue;
            }
            let aclass = pkg.export_class(ai);
            if !matches!(aclass.as_str(), "StaticLightCollectionActor" | "PointLight" | "SpotLight") {
                continue;
            }
            let Ok(ch) = pkg.read_object(ci) else { continue };
            let Some(fref) = find(&ch.properties, "Function").and_then(|p| p.as_object()) else { continue };
            let ObjRef::Export(fi) = ObjRef::from_index(fref) else { continue };
            let Some(function) = self.read_light_function(slot, fi) else { continue };
            let Ok(ah) = pkg.read_object(ai) else { continue };
            match light_from_component(&pkg, &ah.properties, ci, &aclass) {
                Ok(Some(mut l)) => {
                    l.function = Some(function);
                    self.stats.function_lights += 1;
                    self.map.lights.push(l);
                }
                Ok(None) => {}
                Err(e) => self.stats.errors.push(e),
            }
        }
    }

    /// A `LightFunction` object: its source material's parameters (the material's own expression
    /// objects, whose `DefaultValue`s are the parameter values; unset means 0).
    fn read_light_function(&mut self, slot: usize, fi: usize) -> Option<LightFunction> {
        let pkg = self.pkgs[slot].clone();
        let fh = pkg.read_object(fi).ok()?;
        let mref = ObjRef::from_index(find(&fh.properties, "SourceMaterial")?.as_object()?);
        let path = pkg.obj_path(mref);
        let mut params: Vec<(String, f32)> = Vec::new();
        let mut vec_params: Vec<(String, [f32; 3])> = Vec::new();
        if let Some((ms, mi)) = self.resolve(slot, mref) {
            let mp = self.pkgs[ms].clone();
            for e in 0..mp.exports.len() {
                if !matches!(ObjRef::from_index(mp.exports[e].outer), ObjRef::Export(o) if o == mi) {
                    continue;
                }
                let Ok(eh) = mp.read_object(e) else { continue };
                let Some(Value::Name(name)) = find(&eh.properties, "ParameterName").map(|p| &p.value) else { continue };
                match mp.export_class(e).as_str() {
                    "MaterialExpressionScalarParameter" => params.push((name.clone(), float_or(&eh.properties, "DefaultValue", 0.0))),
                    "MaterialExpressionVectorParameter" => {
                        if let Some(Value::LinearColor(c)) = find(&eh.properties, "DefaultValue").map(|p| &p.value) {
                            vec_params.push((name.clone(), [c[0], c[1], c[2]]));
                        }
                    }
                    _ => {}
                }
            }
        }
        let scalar = |n: &str| params.iter().find(|(k, _)| k == n).map(|(_, v)| *v);
        let lower = path.to_ascii_lowercase();
        if lower.contains("flickering_light_function") {
            // Parameter values as dumped from the three materials when the material is not loaded.
            let (d1, d2) = if lower.contains("woodstove") {
                (1.0, 0.25)
            } else if lower.contains("boiler") {
                (0.5, 0.0)
            } else {
                (5.0, 3.0)
            };
            Some(LightFunction::Flicker { speed1: scalar("F_Flickering_1_speed").unwrap_or(d1), speed2: scalar("F_Flickering_2_speed").unwrap_or(d2), material: path })
        } else if lower.contains("casutics_lightfunction") || lower.contains("caustic") {
            Some(LightFunction::Caustics {
                speed: scalar("Speed").unwrap_or(0.35),
                color: vec_params.iter().find(|(k, _)| k == "CausticColor").map(|(_, v)| *v).unwrap_or([0.859174; 3]),
                scale: find(&fh.properties, "Scale").and_then(|p| p.as_vector()).map(|v| v[0]).unwrap_or(1.0),
            })
        } else {
            self.stats.errors.push(format!("unknown light function material {path}"));
            None
        }
    }
    /// Lights that UE3 renders dynamically: dominant lights (their direct light is not in
    /// the lightmaps) and toggleable/movable lights (direct light kept out of the lightmaps
    /// as shadow maps). Static lights (StaticLightCollectionActor, plain PointLight/SpotLight)
    /// are already baked and are counted but not imported.
    fn import_light(&mut self, slot: usize, actor: usize, class: &str) {
        let pkg = self.pkgs[slot].clone();
        let dynamic = class.starts_with("Dominant") || class.contains("Toggleable") || class.contains("Movable");
        if !dynamic {
            self.stats.static_lights_skipped += 1;
            return;
        }
        match read_light(&pkg, actor, class) {
            Ok(Some(mut l)) => {
                if let Ok(h) = pkg.read_object(actor) {
                    if let Some(ObjRef::Export(ci)) = find(&h.properties, "LightComponent").and_then(|p| p.as_object()).map(ObjRef::from_index) {
                        if let Some(ObjRef::Export(fi)) = pkg.read_object(ci).ok().and_then(|ch| find(&ch.properties, "Function").and_then(|p| p.as_object())).map(ObjRef::from_index) {
                            l.function = self.read_light_function(slot, fi);
                        }
                    }
                }
                self.map.lights.push(l);
            }
            Ok(None) => {}
            Err(e) => self.stats.errors.push(e),
        }
    }
}

/// The world matrices of a `StaticLightCollectionActor`'s components, by component export index.
/// The actor's native data is its `LightComponents` array (count, then object references) and,
/// as the last `count x 64` bytes, one native `FMatrix` (rows X Y Z W, translation in row 3) per
/// component, in array order.
fn collection_matrices(pkg: &Package, actor: usize) -> HashMap<usize, glam::Mat4> {
    let mut out = HashMap::new();
    let Ok(data) = pkg.export_data(actor) else { return out };
    let u32_at = |p: usize| data.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let comps: Vec<usize> = (0..pkg.exports.len()).filter(|&c| pkg.exports[c].outer == actor as i32 + 1 && pkg.export_class(c).ends_with("LightComponent")).collect();
    if comps.is_empty() {
        return out;
    }
    // The array: a count `n` followed by n object references that are all exports of the actor
    // (a positive index).
    let n_max = data.len() / 68;
    for o in (0..data.len().saturating_sub(8)).step_by(4) {
        let Some(n) = u32_at(o).map(|v| v as usize) else { break };
        if n < comps.len() || n > n_max || o + 4 + n * 4 + n * 64 > data.len() {
            continue;
        }
        let refs: Vec<u32> = (0..n).filter_map(|k| u32_at(o + 4 + k * 4)).collect();
        if refs.len() != n || !refs.iter().all(|&r| r > 0 && (r as usize) <= pkg.exports.len()) || !comps.iter().all(|&c| refs.contains(&(c as u32 + 1))) {
            continue;
        }
        let base = data.len() - n * 64;
        for (k, r) in refs.iter().enumerate() {
            let f: Vec<f32> = (0..16).map(|j| f32::from_bits(u32_at(base + k * 64 + j * 4).unwrap_or(0))).collect();
            out.insert(*r as usize - 1, fmatrix_rows([[f[0], f[1], f[2], f[3]], [f[4], f[5], f[6], f[7]], [f[8], f[9], f[10], f[11]], [f[12], f[13], f[14], f[15]]]));
        }
        break;
    }
    out
}

/// The light components of `pkg` with `bRenderLightShafts` (UE3 light shafts; the retail ones are
/// `DirectionalLightComponent`s, many in a `StaticLightCollectionActor` with no direct light). The
/// light's position is `ULightComponent::GetPosition` (0x004e0440): `-direction x TraceDistance`
/// (default 100000).
pub fn read_light_shafts(pkg: &Package) -> Vec<dishonored_ir::LightShaft> {
    let mut out = Vec::new();
    let mut collections: HashMap<usize, HashMap<usize, glam::Mat4>> = HashMap::new();
    for ci in 0..pkg.exports.len() {
        if pkg.export_class(ci) != "DirectionalLightComponent" {
            continue;
        }
        let ObjRef::Export(ai) = ObjRef::from_index(pkg.exports[ci].outer) else { continue };
        if pkg.obj_name(ObjRef::from_index(pkg.exports[ai].outer)) != "PersistentLevel" {
            continue;
        }
        let Ok(ch) = pkg.read_object(ci) else { continue };
        let c = &ch.properties;
        if !bool_or(c, "bRenderLightShafts", false) || !bool_or(c, "bEnabled", true) {
            continue;
        }
        let world = match find(c, "CachedParentToWorld").map(|p| &p.value) {
            Some(Value::Raw(_, b)) if b.len() == 64 => {
                let f: Vec<f32> = b.chunks_exact(4).map(|x| f32::from_le_bytes([x[0], x[1], x[2], x[3]])).collect();
                let row = |i: usize| [f[4 * i + 1], f[4 * i + 2], f[4 * i + 3], f[4 * i]];
                fmatrix_rows([row(0), row(1), row(2), row(3)])
            }
            _ if pkg.export_class(ai) == "StaticLightCollectionActor" => collections.entry(ai).or_insert_with(|| collection_matrices(pkg, ai)).get(&ci).copied().unwrap_or(glam::Mat4::IDENTITY),
            _ => pkg.read_object(ai).map(|h| actor_transform(&h.properties)).unwrap_or(glam::Mat4::IDENTITY),
        } * component_transform(c);
        let direction = world.x_axis.truncate().normalize_or(Vec3::NEG_Z);
        let tint = match find(c, "BloomTint").map(|p| &p.value) {
            Some(Value::Color(rgba)) => [0, 1, 2].map(|k| crate::lightmap::color_to_linear(rgba[k])),
            _ => [1.0; 3],
        };
        out.push(dishonored_ir::LightShaft {
            source: format!("{}:{}", pkg.name, pkg.obj_path(ObjRef::Export(ci))),
            position: -direction * float_or(c, "TraceDistance", 100000.0),
            // `Default__LightComponent`.
            occlusion_depth_range: float_or(c, "OcclusionDepthRange", 20000.0),
            bloom_scale: float_or(c, "BloomScale", 2.0),
            bloom_threshold: float_or(c, "BloomThreshold", 0.0),
            bloom_screen_blend_threshold: float_or(c, "BloomScreenBlendThreshold", 1.0),
            bloom_tint: if find(c, "BloomTint").is_some() { tint } else { [1.0; 3] },
            radial_blur_percent: float_or(c, "RadialBlurPercent", 100.0),
            occlusion_mask_darkness: float_or(c, "OcclusionMaskDarkness", 0.3),
        });
    }
    out
}

/// A placed light actor of `pkg` (`PointLight`, `SpotLightToggleable`, `DominantDirectionalLight`,
/// ...): `None` when it has no component or is disabled. Shared by the level import and the
/// AI light probe data.
pub fn read_light(pkg: &Package, actor: usize, class: &str) -> std::result::Result<Option<Light>, String> {
    {
        let Ok(h) = pkg.read_object(actor) else { return Ok(None) };
        let Some(ObjRef::Export(ci)) = find(&h.properties, "LightComponent").and_then(|p| p.as_object()).map(ObjRef::from_index) else {
            return Ok(None);
        };
        return light_from_component(pkg, &h.properties, ci, class);
    }
}

/// The light of component export ci, owned by an actor with properties ctor of class class.
pub fn light_from_component(pkg: &Package, actor: &[Property], ci: usize, class: &str) -> std::result::Result<Option<Light>, String> {
    {
        let Ok(ch) = pkg.read_object(ci) else {
            return Err(format!("light component {}", pkg.obj_path(ObjRef::Export(ci))));
        };
        let c = &ch.properties;
        if !bool_or(c, "bEnabled", true) {
            return Ok(None);
        }
        let cclass = pkg.export_class(ci);
        // Prefer the component's cooked CachedParentToWorld: spot light actors rely on a
        // class-default Rotation (pointing down) that is not serialized.
        let cached = match find(c, "CachedParentToWorld").map(|p| &p.value) {
            Some(Value::Raw(_, b)) if b.len() == 64 => {
                let f: Vec<f32> = b.chunks_exact(4).map(|x| f32::from_le_bytes([x[0], x[1], x[2], x[3]])).collect();
                // As a *tagged* Matrix property each row is stored (W, X, Y, Z), unlike the
                // native FMatrix after StaticMeshCollectionActor properties (observed on
                // SpotLightToggleable_0: rows (0 | 0 0 -3), (0 | 0 3 0), (0 | 3 0 0), (1 | loc)).
                let row = |i: usize| [f[4 * i + 1], f[4 * i + 2], f[4 * i + 3], f[4 * i]];
                Some(fmatrix_rows([row(0), row(1), row(2), row(3)]))
            }
            _ => None,
        };
        let world = cached.unwrap_or_else(|| actor_transform(actor)) * component_transform(c);
        let position = world.w_axis.truncate();
        let direction = world.x_axis.truncate().normalize_or(Vec3::NEG_Z);
        let color = match find(c, "LightColor").map(|p| &p.value) {
            Some(Value::Color(rgba)) => [0, 1, 2].map(|k| crate::lightmap::color_to_linear(rgba[k])),
            _ => [1.0; 3],
        };
        let kind = if cclass.contains("Directional") {
            LightKind::Directional
        } else if cclass.contains("Spot") {
            LightKind::Spot { inner_cone_deg: float_or(c, "InnerConeAngle", 0.0), outer_cone_deg: float_or(c, "OuterConeAngle", 44.0) }
        } else {
            LightKind::Point
        };
        Ok(Some(Light {
            kind,
            position,
            direction,
            color,
            brightness: float_or(c, "Brightness", 1.0),
            radius: float_or(c, "Radius", 1024.0),
            source: format!("{}:{}", pkg.name, pkg.obj_path(ObjRef::Export(ci))),
            falloff_exponent: float_or(c, "FalloffExponent", 2.0),
            dominant: class.starts_with("Dominant"),
            casts_shadows: bool_or(c, "CastShadows", true) && bool_or(c, "CastDynamicShadows", true),
            guid: match find(c, "LightGuid").map(|v| &v.value) {
                Some(Value::Guid(g)) => *g,
                _ => [0; 4],
            },
            function: None,
        }))
    }
}

impl Importer<'_> {
    /// Level-wide settings from the persistent level's WorldInfo: the reflection cubemap
    /// (`mSceneReflection` -> TextureCube faces) and Arkane's post settings
    /// (`m_ArkDefaultPpSettings`, flattened to dotted names).
    fn import_environment(&mut self, slot: usize) {
        let pkg = self.pkgs[slot].clone();
        let Some(wi) = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "WorldInfo" && self.is_in_persistent_level(&pkg, i)) else {
            return;
        };
        let Ok(h) = pkg.read_object(wi) else { return };
        if let Some(cube) = find(&h.properties, "mSceneReflection").and_then(|p| p.as_object()) {
            if let Some(ck) = self.resolve(slot, ObjRef::from_index(cube)) {
                let cpkg = self.pkgs[ck.0].clone();
                if let Ok(ch) = cpkg.read_object(ck.1) {
                    let mut faces = Vec::new();
                    for f in ["FacePosX", "FaceNegX", "FacePosY", "FaceNegY", "FacePosZ", "FaceNegZ"] {
                        let t = find(&ch.properties, f)
                            .and_then(|p| p.as_object())
                            .and_then(|r| self.resolve(ck.0, ObjRef::from_index(r)))
                            .and_then(|k| self.texture(k));
                        faces.extend(t);
                    }
                    if faces.len() == 6 {
                        self.map.environment.cubemap_faces = faces;
                    }
                }
            }
        }
        if let Some(Value::Struct(_, s)) = find(&h.properties, "m_ArkDefaultPpSettings").map(|p| &p.value) {
            let mut post = PostSettings { source: pkg.obj_path(ObjRef::Export(wi)), ..Default::default() };
            fn flatten(prefix: &str, props: &[Property], post: &mut PostSettings) {
                for p in props {
                    let name = if prefix.is_empty() { p.name.clone() } else { format!("{prefix}.{}", p.name) };
                    match &p.value {
                        Value::Struct(_, s) => flatten(&name, s, post),
                        Value::Float(v) => post.values.push((name, *v)),
                        Value::Int(v) => post.values.push((name, *v as f32)),
                        Value::Bool(b) => post.values.push((name, if *b { 1.0 } else { 0.0 })),
                        Value::Vector(v) => post.vectors.push((name, [v[0], v[1], v[2], 0.0])),
                        Value::LinearColor(c) => post.vectors.push((name, *c)),
                        _ => {}
                    }
                }
            }
            flatten("", s, &mut post);
            self.map.environment.post = Some(post);
        }
    }

    /// Static mesh components of non-static actors, imported at their initial transform as
    /// render-only instances (`collides = false`, so gameplay collision is unchanged:
    /// doors, movables and pickups are dynamic in the original).
    fn import_owned_components(&mut self, slot: usize, actor: usize, comps: &[usize]) -> Result<()> {
        let pkg = self.pkgs[slot].clone();
        let props = pkg.read_object(actor)?.properties;
        let hidden = bool_or(&props, "bHidden", false);
        let world = actor_transform(&props);
        let class = pkg.export_class(actor);
        // An `InterpActor` blocks pawns where it is at run time: its colliding components
        // (effective flags, `component_collision`) become mover collision.
        let mover = (class == "InterpActor").then(|| dishonored_ir::MoverRef {
            actor: format!("{}:{}", pkg.name, pkg.obj_name(ObjRef::Export(actor))),
            actor_to_world: Mat4::from_translation(vec3_or(&props, "Location", Vec3::ZERO)) * rot_matrix(&props, "Rotation"),
        });
        // Other blocking actors with static mesh components (physics props, breakable planks,
        // the static parts of usable objects, ...) collide where they are placed: the state the
        // map starts in (they can move or break later in the original, U-650). Pickups do not
        // block (`bBlockActors` false) and answer the vision and crosshair checks only.
        let static_blocker = mover.is_none() && (STATIC_BLOCKER_CLASSES.contains(&class.as_str()) || crate::collision_audit::is_kactor(&class));
        let rigid = static_blocker && self.simulated_kactor(&pkg, actor, &class);
        for &ci in comps {
            let before = self.map.instances.len();
            let src = format!("{} [{class}]", pkg.obj_path(ObjRef::Export(ci)));
            if let Err(e) = self.add_component(slot, ci, world, hidden, src) {
                self.stats.errors.push(format!("{}: {e:#}", pkg.obj_path(ObjRef::Export(ci))));
            }
            for inst in &mut self.map.instances[before..] {
                if inst.collides && !static_blocker {
                    inst.mover = mover.clone();
                    inst.collides = false;
                }
                // A simulated physics prop (`PHYS_RigidBody`) fails `IsBlockedBy`, so only the
                // checks without `TRACE_Blocking` (the vision line and the crosshair) meet it;
                // measured: movement and pistol checks pass through Boyle's buckets and ropes,
                // the vision check stops on them.
                if rigid {
                    inst.trace_mask &= crate::collision_audit::mask::VISION | crate::collision_audit::mask::CROSSHAIR;
                }
            }
            self.stats.other_actor_components += self.map.instances.len() - before;
        }
        Ok(())
    }

    fn import_static_mesh_actor(&mut self, slot: usize, i: usize) -> Result<()> {
        let pkg = self.pkgs[slot].clone();
        let props = pkg.read_object(i)?.properties;
        let Some(comp) = find(&props, "StaticMeshComponent").and_then(|p| p.as_object()) else {
            return Ok(());
        };
        let ObjRef::Export(ci) = ObjRef::from_index(comp) else {
            return Ok(());
        };
        // A StaticMeshActor the cooker kept (still referenced) whose component it moved into a
        // `StaticMeshCollectionActor` (`StaticMeshCollectionActor.StaticMeshActor_SMC_<n>`):
        // that collection places the component (its cached matrix carries the actor's
        // transform, and the component's `Scale3D` the actor's scale); the actor itself adds
        // nothing. Placing it again from the actor applied the actor's transform on top of the
        // component's (Ovrsr's `StaticMeshActor_2223`, Tower's `StaticMeshActor_2`: a second copy
        // scaled twice, far from where the original has it).
        if pkg.exports[ci].outer != i as i32 + 1 && pkg.export_class(match ObjRef::from_index(pkg.exports[ci].outer) {
            ObjRef::Export(o) => o,
            _ => i,
        }) == "StaticMeshCollectionActor"
        {
            return Ok(());
        }
        self.stats.static_mesh_actors += 1;
        let hidden = bool_or(&props, "bHidden", false);
        let world = actor_transform(&props);
        self.add_component(slot, ci, world, hidden, pkg.obj_path(ObjRef::Export(i)))
    }

    fn import_collection(&mut self, slot: usize, i: usize) -> Result<()> {
        let pkg = self.pkgs[slot].clone();
        let hdr = pkg.read_object(i)?;
        let comps = match find(&hdr.properties, "StaticMeshComponents") {
            Some(p) => p.array_objects()?,
            None => return Ok(()),
        };
        // Native tail: one FMatrix (CachedParentToWorld) per component entry.
        let data = pkg.export_data(i)?;
        let mut r = Reader::at(data, hdr.native_offset);
        for (n, c) in comps.iter().enumerate() {
            let mut rows = [[0f32; 4]; 4];
            for row in rows.iter_mut() {
                for v in row.iter_mut() {
                    *v = r.f32()?;
                }
            }
            let ObjRef::Export(ci) = ObjRef::from_index(*c) else {
                continue;
            };
            self.stats.collection_components += 1;
            let parent = fmatrix_rows(rows);
            let src = format!("{}[{n}]", pkg.obj_path(ObjRef::Export(i)));
            if let Err(e) = self.add_component(slot, ci, parent, false, src) {
                self.stats.errors.push(format!("{}: {e:#}", pkg.obj_path(ObjRef::Export(ci))));
            }
        }
        Ok(())
    }

    fn add_component(
        &mut self,
        slot: usize,
        ci: usize,
        parent: Mat4,
        actor_hidden: bool,
        source: String,
    ) -> Result<()> {
        let pkg = self.pkgs[slot].clone();
        let hdr = pkg.read_object(ci)?;
        let native_offset = hdr.native_offset;
        let props = hdr.properties;
        let key = match find(&props, "StaticMesh").and_then(|p| p.as_object()) {
            Some(mesh_ref) => self.resolve(slot, ObjRef::from_index(mesh_ref)),
            // The mesh of the component's archetype (`DisStaticCollisionActor`'s class default
            // `Collision_model`), when a map package carries it.
            None => self.resolver.inherited_object_path(&pkg, ci, "StaticMesh").and_then(|p| self.index.get(&p.to_ascii_lowercase()).copied()),
        };
        let Some(mut mesh) = key.and_then(|k| self.mesh(k)) else {
            return Ok(());
        };
        let is_spline = pkg.export_class(ci) == "SplineMeshComponent";
        if is_spline {
            if let Some(sp) = spline_mesh::read_params(&props) {
                let deformed = spline_mesh::deform(&self.map.meshes[mesh], &sp);
                self.map.meshes.push(deformed);
                mesh = self.map.meshes.len() - 1;
                self.stats.spline_meshes += 1;
            }
        }
        let mut materials = Vec::new();
        if let Some(p) = find(&props, "Materials") {
            for m in p.array_objects().unwrap_or_default() {
                let r = ObjRef::from_index(m);
                materials.push(if r == ObjRef::Null {
                    None
                } else {
                    self.resolve(slot, r).and_then(|k| self.material(k))
                });
            }
        }
        let hidden = actor_hidden || bool_or(&props, "HiddenGame", false);
        let (collides, trace_mask) = self.component_collision(&pkg, ci);
        let stealth_shadow = self.component_stealth_shadow(&pkg, ci);
        if self.opts.load_textures && self.opts.load_lightmaps {
            match pkg.export_data(ci).and_then(|d| read_static_mesh_component_lods(d, native_offset)) {
                Ok(lods) => match lods.into_iter().next().map(|l| (l.lightmap, l.shadow_maps)) {
                    Some((LightMapRef::Texture(lm), shadow_maps)) => {
                        self.stats.lightmaps_texture += 1;
                        let shadows = shadow_maps.into_iter().filter_map(|s| self.distance_field_shadow_map(slot, s)).collect();
                        self.pending_lightmaps.push(PendingLightmap { instance: self.map.instances.len(), slot, lm, shadows });
                    }
                    Some((LightMapRef::Vertex { .. }, _)) => self.stats.lightmaps_vertex += 1,
                    _ => self.stats.lightmaps_none += 1,
                },
                Err(_) if is_spline => self.stats.lightmaps_none += 1,
                Err(e) => {
                    self.stats.lightmap_parse_failures += 1;
                    self.stats.errors.push(format!("lightmap {}: {e:#}", pkg.obj_path(ObjRef::Export(ci))));
                }
            }
        }
        self.map.instances.push(MeshInstance {
            mesh,
            transform: parent * component_transform(&props),
            materials,
            hidden,
            collides,
            source,
            level: pkg.name.clone(),
            lightmap: None,
            mover: None,
            trace_mask,
            reflect_channels: channel_mask(&props, "ReflectionChannels", 0),
            stealth_shadow,
        });
        Ok(())
    }

    /// A KActor-family prop the physics simulates at run time: `PHYS_RigidBody` (the actor's
    /// `Physics`, archetypes resolved) unless its tweak fixes it in place (`m_bFixed` of its
    /// `DisTweaks_StaticBreakable` / `DisTweaks_Movable`: `PHYS_None`, e.g. Boyle's piano).
    fn simulated_kactor(&mut self, pkg: &Arc<Package>, actor: usize, class: &str) -> bool {
        if !crate::collision_audit::is_kactor(class) {
            return false;
        }
        let ap = self.resolver.effective(pkg, actor);
        if !matches!(find(&ap, "Physics").map(|p| &p.value), Some(Value::Enum(e)) if e == "PHYS_RigidBody") {
            return false;
        }
        // A whale oil battery placed in a receptacle (its `m_pBattery`) starts attached to it,
        // held in place: measured, Boyle's basement battery stops the movement and pistol checks.
        if class == "DisWhaleOilBattery"
            && (0..pkg.exports.len()).any(|r| {
                pkg.export_class(r) == "DisWhaleOilReceptacle"
                    && pkg.read_object(r).is_ok_and(|h| find(&h.properties, "m_pBattery").and_then(|p| p.as_object()) == Some(actor as i32 + 1))
            })
        {
            return false;
        }
        let tweak = ["m_pMovableTweaks", "m_pBreakableTweaks"].iter().find_map(|n| self.resolver.inherited_object_path(pkg, actor, n));
        let Some(tweak) = tweak else { return true };
        if self.tweaks.is_none() {
            let names: Vec<String> = self.pkgs.iter().map(|p| p.name.clone()).collect();
            self.tweaks = Some(crate::tweaks::TweakDb::load_with(self.content, &names).ok());
        }
        let fixed = self.tweaks.as_ref().and_then(|t| t.as_ref()).and_then(|t| t.try_resolve(&tweak)).is_some_and(|t| matches!(t.get("m_bFixed"), Some(crate::tweaks::TVal::B(true))));
        !fixed
    }

    /// Whether the stealth light's line check hits a placed component
    /// ([`crate::collision_audit::blocks_stealth_light`]): its effective flags and its actor's.
    fn component_stealth_shadow(&mut self, pkg: &Arc<Package>, ci: usize) -> bool {
        let cp = self.resolver.effective(pkg, ci);
        let ap = match ObjRef::from_index(pkg.exports[ci].outer) {
            ObjRef::Export(a) => Some(self.resolver.effective(pkg, a)),
            _ => None,
        };
        crate::collision_audit::blocks_stealth_light(&cp, ap.as_deref().map(|a| a.as_slice()))
    }

    /// Whether a placed component blocks the player's checks at all, and its trace types
    /// (research/dishonored/COLLISION.md): it collides (`CollideActors` and its actor's
    /// `bCollideActors`, archetypes resolved) and either its actor is world geometry
    /// (`ShouldTrace` accepts every level-geometry check) or both block (`IsBlockedBy`:
    /// the actor's `bBlockActors` and the component's `BlockActors`).
    fn component_collision(&mut self, pkg: &Arc<Package>, ci: usize) -> (bool, u8) {
        use crate::collision_audit::{flag, trace_mask};
        let cp = self.resolver.effective(pkg, ci);
        let ap = match ObjRef::from_index(pkg.exports[ci].outer) {
            ObjRef::Export(a) => Some(self.resolver.effective(pkg, a)),
            _ => None,
        };
        let collide = flag(&cp, "CollideActors") && ap.as_ref().is_none_or(|a| flag(a, "bCollideActors"));
        let world = ap.as_ref().is_some_and(|a| flag(a, "bWorldGeometry"));
        let block = flag(&cp, "BlockActors") && ap.as_ref().is_some_and(|a| flag(a, "bBlockActors"));
        let mut mask = trace_mask(&cp);
        // The vision line of sight (flags 0x08023086) checks level geometry and movers only
        // (no `TRACE_Others`): an actor that is neither (a physics prop, a usable object's
        // static part) does not block it whatever its trace types.
        let class = match ObjRef::from_index(pkg.exports[ci].outer) {
            ObjRef::Export(a) => pkg.export_class(a),
            _ => String::new(),
        };
        use crate::collision_audit::mask as m;
        // A Wall of Light (`DisWallOfLight::ShouldTrace`, 0x00A21B30) stops the player's
        // projectile and melee checks while it is powered, whatever its blocking flags; the
        // player walks into it (and is killed by its own logic).
        if class == "DisWallOfLight" {
            return (collide, mask & (m::PROJECTILE | m::MELEE));
        }
        // KActors (physics props, breakables) answer like movers (`TRACE_Movers`), so the vision
        // check sees them; other actors that are not world geometry only answer checks with
        // `TRACE_Others`, which the vision check has not.
        let kactor = crate::collision_audit::is_kactor(&class);
        // A usable object (`DisSkeletalBreakable::ShouldTrace`, 0x00A28580) answers the vision
        // check as level geometry when its tweak says `m_bShouldTraceAsWorldGeometry` and it is
        // not transparent for vision (Boyle's lockers and bins); a door (`DisDoor::ShouldTrace`,
        // 0x00A2C760) unless transparent (the sewer grate doors).
        let usable_seen = !world
            && crate::usable_props::is_usable_class(&class)
            && match (ObjRef::from_index(pkg.exports[ci].outer), ap.as_ref()) {
                (ObjRef::Export(a), Some(ap)) => self.breakable_seen_by_vision(pkg, a, ap, class == "DisDoor"),
                _ => false,
            };
        if !world && !kactor && !usable_seen && class != "InterpActor" {
            mask &= !m::VISION;
        }
        // A KActor that does not block (pickups: `bBlockActors` false) still answers the checks
        // without `TRACE_Blocking` (`AKActor::ShouldTrace`): measured, the vision line stops on
        // Boyle's paintings, hemlock and urn pickups.
        if kactor && collide && !block {
            return (true, mask & (m::VISION | m::CROSSHAIR));
        }
        (collide && (world || block), mask)
    }

    /// Whether a `DisSkeletalBreakable`'s vision answer is that of level geometry: its tweak's
    /// `m_bShouldTraceAsWorldGeometry` (a door: always), unless transparent for vision (the actor's
    /// `m_bTransparentForVisionChecks` when it sets `m_bOverrideDefaultVisionTransparency`, else
    /// the tweak's).
    fn breakable_seen_by_vision(&mut self, pkg: &Arc<Package>, actor: usize, ap: &[ue3_package::properties::Property], door: bool) -> bool {
        use crate::collision_audit::flag;
        let Some(tweak) = ["m_pUsableObjTweaks", "m_pBreakableTweaks", "m_pDoorTweaks"].iter().find_map(|n| self.resolver.inherited_object_path(pkg, actor, n)) else { return door };
        if self.tweaks.is_none() {
            let names: Vec<String> = self.pkgs.iter().map(|p| p.name.clone()).collect();
            self.tweaks = Some(crate::tweaks::TweakDb::load_with(self.content, &names).ok());
        }
        let Some(t) = self.tweaks.as_ref().and_then(|t| t.as_ref()).and_then(|t| t.try_resolve(&tweak)) else { return door };
        let tflag = |n: &str| matches!(t.get(n), Some(crate::tweaks::TVal::B(true)));
        let transparent = if flag(ap, "m_bOverrideDefaultVisionTransparency") { flag(ap, "m_bTransparentForVisionChecks") } else { tflag("m_bTransparentForVisionChecks") };
        !transparent && (door || tflag("m_bShouldTraceAsWorldGeometry"))
    }

    /// Decode every referenced lightmap texture pair once and bake each component's
    /// region (with its own coefficient scales) into a linear irradiance atlas that keeps
    /// the original atlas layout, so the component's UV rect is unchanged.
    fn bake_lightmaps(&mut self) {
        let pending = std::mem::take(&mut self.pending_lightmaps);
        let mut groups: HashMap<(Key, Key), Vec<usize>> = HashMap::new();
        for (i, p) in pending.iter().enumerate() {
            let avg = self.resolve(p.slot, ObjRef::from_index(p.lm.textures[0]));
            let dir = self.resolve(p.slot, ObjRef::from_index(p.lm.textures[1]));
            if let (Some(a), Some(d)) = (avg, dir) {
                groups.entry((a, d)).or_default().push(i);
            }
        }
        let mut keys: Vec<_> = groups.keys().copied().collect();
        keys.sort();
        // Distance-field shadow maps of the dominant directional light, decoded once (PF_G8).
        let dominant = self.dominant_directional_guids();
        // ... and of the dominant point / spot lights (U-403): light GUID -> index in map.lights.
        let local: Vec<([u32; 4], usize)> = self
            .map
            .lights
            .iter()
            .enumerate()
            .filter(|(_, l)| l.dominant && !matches!(l.kind, dishonored_ir::LightKind::Directional) && l.guid != [0; 4])
            .map(|(i, l)| (l.guid, i))
            .collect();
        // The on-disk cache (lightmap_cache.rs): same inputs, same pages.
        let sig = {
            let mut s = crate::lightmap_cache::Signature::default();
            s.u64(crate::lightmap_cache::FORMAT as u64);
            for p in &self.pkgs {
                s.str(&p.name);
                s.u64(p.exports.len() as u64);
                s.u64(p.names.len() as u64);
            }
            for g in dominant.iter().chain(local.iter().map(|l| &l.0)) {
                g.iter().for_each(|x| s.u64(*x as u64));
            }
            local.iter().for_each(|l| s.u64(l.1 as u64));
            for p in &pending {
                s.u64(p.instance as u64);
                for t in &p.lm.textures[..2] {
                    let k = self.resolve(p.slot, ObjRef::from_index(*t));
                    s.u64(k.map(|k| ((k.0 as u64) << 32) | k.1 as u64).unwrap_or(u64::MAX));
                }
                p.lm.scales[..2].iter().flatten().chain(p.lm.coordinate_scale.iter()).chain(p.lm.coordinate_bias.iter()).for_each(|v| s.f32(*v));
                for sm in &p.shadows {
                    sm.light_guid.iter().for_each(|x| s.u64(*x as u64));
                    s.u64(((sm.texture.0 as u64) << 32) | sm.texture.1 as u64);
                    sm.scale.iter().chain(sm.bias.iter()).for_each(|v| s.f32(*v));
                }
            }
            s
        };
        if let Some(b) = self.opts.lightmap_cache.as_deref().and_then(|p| crate::lightmap_cache::load(p, sig)) {
            for (i, l) in b.assignments {
                if let Some(inst) = self.map.instances.get_mut(i) {
                    inst.lightmap = Some(l);
                }
            }
            self.map.lightmaps = b.pages;
            self.stats.lightmap_atlases = b.atlases;
            self.stats.lightmap_pages = self.map.lightmaps.len();
            self.stats.shadow_map_components = b.shadow_map_components;
            self.stats.local_shadow_map_components = b.local_shadow_map_components;
            self.stats.texture_failures += b.texture_failures;
            self.stats.errors.extend(b.errors);
            self.stats.lightmaps_cached = true;
            return;
        }
        let (failures0, errors0) = (self.stats.texture_failures, self.stats.errors.len());
        let mut shadow_textures: HashMap<Key, Option<(usize, usize, Vec<u8>)>> = HashMap::new();
        for p in &pending {
            for s in p.shadows.iter().filter(|s| dominant.contains(&s.light_guid) || local.iter().any(|l| l.0 == s.light_guid)) {
                if shadow_textures.contains_key(&s.texture) {
                    continue;
                }
                let t = read_texture(self.content, &self.pkgs[s.texture.0], s.texture.1, 4096).ok().filter(|t| t.format == TextureFormat::R8);
                if t.is_none() {
                    self.stats.texture_failures += 1;
                    self.stats.errors.push(format!("shadow map {:?}: not decoded as PF_G8", s.texture));
                }
                shadow_textures.insert(s.texture, t.map(|t| (t.width as usize, t.height as usize, t.mips[0].clone())));
            }
        }
        for (ka, kd) in keys {
            let decode = |s: &Self, k: Key| -> Result<(u32, u32, Vec<[u8; 3]>)> {
                let pkg = &s.pkgs[k.0];
                let t = read_texture(s.content, pkg, k.1, 4096)?;
                anyhow::ensure!(t.format == TextureFormat::Bc1, "{}: lightmap format {:?}", t.source, t.format);
                Ok((t.width, t.height, decode_bc1(&t.mips[0], t.width, t.height)))
            };
            let (a, d) = match (decode(self, ka), decode(self, kd)) {
                (Ok(a), Ok(d)) if (a.0, a.1) == (d.0, d.1) => (a, d),
                (a, d) => {
                    self.stats.texture_failures += 1;
                    self.stats.errors.push(format!("lightmap pair {ka:?}/{kd:?}: {:?} {:?}", a.err(), d.err()));
                    continue;
                }
            };
            let (w, h) = (a.0 as usize, a.1 as usize);
            let mut out = vec![[0f32; 3]; w * h];
            let mut out_dir = vec![[0f32; 3]; w * h];
            // 0 = empty, 1 = padding, 2 = inside a component's rect.
            let mut mask = vec![0u8; w * h];
            let members = &groups[&(ka, kd)];
            // The dominant light's distance field resampled into this atlas's layout (255 where
            // no member has one): atlas texel -> UV1 -> shadow map UV.
            let member_shadow = |m: usize| {
                pending[m].shadows.iter().filter(|s| dominant.contains(&s.light_guid)).find_map(|s| shadow_textures.get(&s.texture)?.as_ref().map(|t| (s, t)))
            };
            let mut shadow = if members.iter().any(|&m| member_shadow(m).is_some()) { vec![255u8; w * h] } else { Vec::new() };
            // The component's first dominant point / spot light field (light index, map, texture).
            let member_local = |m: usize| {
                pending[m].shadows.iter().find_map(|s| {
                    let li = local.iter().find(|l| l.0 == s.light_guid)?.1;
                    shadow_textures.get(&s.texture)?.as_ref().map(|t| (li, s, t))
                })
            };
            let mut shadow_local = if members.iter().any(|&m| member_local(m).is_some()) { vec![255u8; w * h] } else { Vec::new() };
            for pass in [2u8, 1u8] {
                let pad = if pass == 2 { 0 } else { 1 };
                for &m in members {
                    let lm = &pending[m].lm;
                    let sm = member_shadow(m);
                    let sl = member_local(m);
                    let rx0 = (lm.coordinate_bias[0] * w as f32).floor() as i64 - pad;
                    let ry0 = (lm.coordinate_bias[1] * h as f32).floor() as i64 - pad;
                    let rx1 = ((lm.coordinate_bias[0] + lm.coordinate_scale[0]) * w as f32).ceil() as i64 + pad;
                    let ry1 = ((lm.coordinate_bias[1] + lm.coordinate_scale[1]) * h as f32).ceil() as i64 + pad;
                    for y in ry0.max(0)..ry1.min(h as i64) {
                        for x in rx0.max(0)..rx1.min(w as i64) {
                            let i = y as usize * w + x as usize;
                            if mask[i] >= pass {
                                continue;
                            }
                            mask[i] = pass;
                            let avg = a.2[i].map(srgb_to_linear);
                            let dir = d.2[i].map(srgb_to_linear);
                            out[i] = [0, 1, 2].map(|k| avg[k] * lm.scales[0][k]);
                            out_dir[i] = [0, 1, 2].map(|k| dir[k] * lm.scales[1][k]);
                            let uv = Vec2::new((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                            let uv1 = (uv - Vec2::from(lm.coordinate_bias)) / Vec2::from(lm.coordinate_scale).max(Vec2::splat(1e-9));
                            if let Some((s, t)) = sm {
                                shadow[i] = crate::lightmap::sample_r8_bilinear(&t.2, t.0, t.1, uv1 * Vec2::from(s.scale) + Vec2::from(s.bias));
                            }
                            if let Some((_, s, t)) = sl {
                                shadow_local[i] = crate::lightmap::sample_r8_bilinear(&t.2, t.0, t.1, uv1 * Vec2::from(s.scale) + Vec2::from(s.bias));
                            }
                        }
                    }
                }
            }
            let atlas = self.map.lightmaps.len();
            self.stats.shadow_map_components += members.iter().filter(|&&m| member_shadow(m).is_some()).count();
            self.stats.local_shadow_map_components += members.iter().filter(|&&m| member_local(m).is_some()).count();
            for &m in members {
                let lm = &pending[m].lm;
                let min = Vec2::from(lm.coordinate_bias);
                self.map.instances[pending[m].instance].lightmap = Some(InstanceLightmap {
                    atlas,
                    uv_min: min,
                    uv_max: min + Vec2::from(lm.coordinate_scale),
                    df_shadow: member_shadow(m).is_some(),
                    df_light: member_local(m).map(|l| l.0),
                });
            }
            self.map.lightmaps.push(LightmapAtlas {
                source: self.pkgs[ka.0].obj_path(ObjRef::Export(ka.1)),
                width: w as u32,
                height: h as u32,
                texels: out.into_iter().map(pack_rgb9e5).collect(),
                directional: out_dir.into_iter().map(pack_rgb9e5).collect(),
                shadow,
                shadow_local,
            });
        }
        self.stats.lightmap_atlases = self.map.lightmaps.len();
        self.pack_lightmap_pages(4096);
        self.stats.lightmap_pages = self.map.lightmaps.len();
        if let Some(path) = self.opts.lightmap_cache.as_deref() {
            let b = crate::lightmap_cache::Baked {
                pages: std::mem::take(&mut self.map.lightmaps),
                assignments: pending.iter().filter_map(|p| Some((p.instance, self.map.instances[p.instance].lightmap?))).collect(),
                atlases: self.stats.lightmap_atlases,
                shadow_map_components: self.stats.shadow_map_components,
                local_shadow_map_components: self.stats.local_shadow_map_components,
                texture_failures: self.stats.texture_failures - failures0,
                errors: self.stats.errors[errors0..].to_vec(),
            };
            if let Err(e) = crate::lightmap_cache::save(path, sig, &b) {
                self.stats.errors.push(format!("lightmap cache {}: {e}", path.display()));
            }
            self.map.lightmaps = b.pages;
        }
    }

    /// Shelf-pack the baked atlases (each kept intact) into a few large pages. Every distinct
    /// lightmap image breaks Bevy's draw batching; 196 atlases cost ~4x frame time on Boyle Ext.
    fn pack_lightmap_pages(&mut self, page: u32) {
        let atlases = std::mem::take(&mut self.map.lightmaps);
        let any_shadow = atlases.iter().any(|a| !a.shadow.is_empty());
        let any_local = atlases.iter().any(|a| !a.shadow_local.is_empty());
        let mut order: Vec<usize> = (0..atlases.len()).collect();
        order.sort_by_key(|&i| (std::cmp::Reverse(atlases[i].height), std::cmp::Reverse(atlases[i].width)));
        // atlas -> (page, x, y)
        let mut place = vec![(0usize, 0u32, 0u32); atlases.len()];
        let mut pages: Vec<LightmapAtlas> = Vec::new();
        let (mut x, mut y, mut shelf_h) = (0u32, 0u32, 0u32);
        for &i in &order {
            let a = &atlases[i];
            let (w, h) = (a.width.min(page), a.height.min(page));
            if pages.is_empty() || x + w > page {
                x = 0;
                y += shelf_h;
                shelf_h = 0;
            }
            if pages.is_empty() || y + h > page {
                pages.push(LightmapAtlas {
                    source: format!("lightmap page {}", pages.len()),
                    width: page,
                    height: page,
                    texels: vec![0; (page * page) as usize],
                    directional: vec![0; (page * page) as usize],
                    shadow: if any_shadow { vec![255; (page * page) as usize] } else { Vec::new() },
                    shadow_local: if any_local { vec![255; (page * page) as usize] } else { Vec::new() },
                });
                x = 0;
                y = 0;
                shelf_h = 0;
            }
            let p = pages.len() - 1;
            for row in 0..h {
                let src = (row * a.width) as usize;
                let dst = ((y + row) * page + x) as usize;
                pages[p].texels[dst..dst + w as usize].copy_from_slice(&a.texels[src..src + w as usize]);
                if a.directional.len() == a.texels.len() {
                    pages[p].directional[dst..dst + w as usize].copy_from_slice(&a.directional[src..src + w as usize]);
                }
                if a.shadow.len() == a.texels.len() {
                    pages[p].shadow[dst..dst + w as usize].copy_from_slice(&a.shadow[src..src + w as usize]);
                }
                if a.shadow_local.len() == a.texels.len() {
                    pages[p].shadow_local[dst..dst + w as usize].copy_from_slice(&a.shadow_local[src..src + w as usize]);
                }
            }
            place[i] = (p, x, y);
            x += w;
            shelf_h = shelf_h.max(h);
        }
        let pf = page as f32;
        for inst in &mut self.map.instances {
            if let Some(lm) = inst.lightmap.as_mut() {
                let a = &atlases[lm.atlas];
                let (p, ox, oy) = place[lm.atlas];
                let map_uv = |uv: Vec2| Vec2::new((ox as f32 + uv.x * a.width as f32) / pf, (oy as f32 + uv.y * a.height as f32) / pf);
                lm.atlas = p;
                lm.uv_min = map_uv(lm.uv_min);
                lm.uv_max = map_uv(lm.uv_max);
            }
        }
        self.map.lightmaps = pages;
    }

    /// A component's `ShadowMap2D` (tagged properties `Texture`, `CoordinateScale`,
    /// `CoordinateBias`, `LightGuid`, `bIsShadowFactorTexture`) when it holds a signed distance
    /// field (`bIsShadowFactorTexture` false; `ShadowMapTexture2D`, PF_G8).
    fn distance_field_shadow_map(&mut self, slot: usize, obj: i32) -> Option<ShadowMapRef> {
        let k = self.resolve(slot, ObjRef::from_index(obj))?;
        let pkg = self.pkgs[k.0].clone();
        let h = pkg.read_object(k.1).ok()?;
        let p = &h.properties;
        if bool_or(p, "bIsShadowFactorTexture", false) {
            return None;
        }
        let v2 = |n: &str| match find(p, n).map(|v| &v.value) {
            Some(Value::Vector2D(v)) => Some(*v),
            _ => None,
        };
        let light_guid = match find(p, "LightGuid").map(|v| &v.value) {
            Some(Value::Guid(g)) => *g,
            _ => return None,
        };
        let texture = self.resolve(k.0, ObjRef::from_index(find(p, "Texture")?.as_object()?))?;
        Some(ShadowMapRef { light_guid, texture, scale: v2("CoordinateScale")?, bias: v2("CoordinateBias")? })
    }

    /// `LightGuid`s of the levels' `DominantDirectionalLightComponent`s.
    fn dominant_directional_guids(&self) -> Vec<[u32; 4]> {
        let mut out = Vec::new();
        for pkg in &self.pkgs {
            for i in 0..pkg.exports.len() {
                if pkg.export_class(i) != "DominantDirectionalLightComponent" || pkg.obj_path(ObjRef::Export(i)).starts_with("Default__") {
                    continue;
                }
                if let Ok(h) = pkg.read_object(i) {
                    if let Some(Value::Guid(g)) = find(&h.properties, "LightGuid").map(|v| &v.value) {
                        out.push(*g);
                    }
                }
            }
        }
        out
    }

    fn resolve(&mut self, slot: usize, r: ObjRef) -> Option<Key> {
        let pkg = &self.pkgs[slot];
        match r {
            ObjRef::Null => None,
            ObjRef::Export(i) => Some((slot, i)),
            ObjRef::Import(_) => {
                let path = pkg.obj_path(r).to_ascii_lowercase();
                let k = self.index.get(&path).copied();
                if k.is_none() {
                    self.stats.unresolved_refs += 1;
                    self.stats.unresolved_paths.push(format!("{} ({})", pkg.obj_path(r), pkg.class_name(r)));
                }
                k
            }
        }
    }

    fn mesh(&mut self, k: Key) -> Option<MeshId> {
        if let Some(id) = self.mesh_ids.get(&k) {
            return *id;
        }
        let pkg = self.pkgs[k.0].clone();
        let id = match read_static_mesh(&pkg, k.1) {
            Ok(raw) => {
                let mut sections = Vec::new();
                for s in &raw.sections {
                    let mat = self.resolve(k.0, ObjRef::from_index(s.material)).and_then(|mk| self.material(mk));
                    sections.push(MeshSection {
                        first_index: s.first_index,
                        num_triangles: s.num_triangles,
                        material: mat,
                        enable_collision: s.enable_collision,
                    });
                }
                // Lightmap UVs: the set named by `LightMapCoordinateIndex` (serialized when
                // non-zero, e.g. 1 on air_duct_10; absent means the UE3 default 0).
                let lmi = pkg
                    .read_object(k.1)
                    .ok()
                    .and_then(|h| find(&h.properties, "LightMapCoordinateIndex").and_then(|p| p.as_float()))
                    .map(|v| v as usize)
                    .unwrap_or(0);
                let (simple_line, simple_box) = pkg.read_object(k.1).map(|h| crate::simple_collision::simple_flags(&h.properties)).unwrap_or((true, true));
                let asset = StaticMeshAsset {
                    source: pkg.obj_path(ObjRef::Export(k.1)),
                    normals: raw.normals,
                    tangents: raw.tangent_x,
                    uv0: raw.uvs.first().cloned().unwrap_or_else(|| vec![Vec2::ZERO; raw.positions.len()]),
                    uv1: raw.uvs.get(lmi).or(raw.uvs.get(1)).cloned().unwrap_or_default(),
                    // Sky domes sample their `Storm` texture at texture coordinate 1 (U-580).
                    uv_set1: if lmi != 1 { raw.uvs.get(1).cloned().unwrap_or_default() } else { Vec::new() },
                    colors: raw.colors,
                    indices: raw.indices.iter().map(|&i| i as u32).collect(),
                    sections,
                    collision_triangles: raw
                        .kdop_triangles
                        .iter()
                        .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
                        .collect(),
                    collision_materials: raw.kdop_triangles.iter().map(|t| t[3]).collect(),
                    simple_line,
                    simple_box,
                    simple_collision: match ObjRef::from_index(raw.body_setup) {
                        ObjRef::Export(b) => Some(crate::simple_collision::agg_geom_triangles(&pkg, b)),
                        _ => None,
                    },
                    bounds_min: raw.bounds_origin - raw.bounds_extent,
                    bounds_max: raw.bounds_origin + raw.bounds_extent,
                    positions: raw.positions,
                };
                self.map.meshes.push(asset);
                Some(self.map.meshes.len() - 1)
            }
            Err(e) => {
                self.stats.mesh_failures += 1;
                self.stats.errors.push(format!("{e:#}"));
                None
            }
        };
        self.mesh_ids.insert(k, id);
        id
    }

    /// `DisContactType_*` class of a `PhysicalMaterial`: its `PhysicalMaterialProperty`
    /// (`DisContactPhysicalMaterial.m_pContactType`, default `DisContactType_Environment`),
    /// else its `Parent`'s.
    fn contact_type(&mut self, mut k: Key) -> Option<String> {
        for _ in 0..8 {
            let pkg = self.pkgs[k.0].clone();
            let props = pkg.read_object(k.1).ok()?.properties;
            if let Some(pp) = find(&props, "PhysicalMaterialProperty").and_then(|p| p.as_object()).filter(|o| *o != 0) {
                let r = ObjRef::from_index(pp);
                let ct = match self.resolve(k.0, r) {
                    Some(pk) => {
                        let ppkg = self.pkgs[pk.0].clone();
                        let pprops = ppkg.read_object(pk.1).ok().map(|h| h.properties).unwrap_or_default();
                        find(&pprops, "m_pContactType").and_then(|p| p.as_object()).filter(|o| *o != 0).map(|c| ppkg.obj_name(ObjRef::from_index(c)))
                    }
                    None => None,
                };
                return Some(ct.unwrap_or_else(|| "DisContactType_Environment".to_string()));
            }
            let parent = find(&props, "Parent").and_then(|p| p.as_object()).filter(|o| *o != 0)?;
            k = self.resolve(k.0, ObjRef::from_index(parent))?;
        }
        None
    }

    fn material(&mut self, k: Key) -> Option<MaterialId> {
        if let Some(id) = self.mat_ids.get(&k) {
            return *id;
        }
        let pkg = self.pkgs[k.0].clone();
        let pkgs = self.pkgs.clone();
        let chain = material_chain::walk(&pkgs, k, &mut |slot, r| self.resolve(slot, r));
        if let Some(b) = &chain.broken_at {
            self.stats.material_chain_breaks.push(b.clone());
        }
        // Arkane slot convention first, then the base material's wired inputs, then names.
        let named = |subs: &[&str], excl: &[&str]| {
            chain
                .textures
                .iter()
                .find(|(n, _)| {
                    let l = n.to_ascii_lowercase();
                    subs.iter().any(|s| l.contains(s)) && !excl.iter().any(|x| l.contains(x))
                })
                .map(|(_, k)| *k)
        };
        let diffuse_key = material_chain::slot(&chain, "d_", &["mask"])
            .or(chain.diffuse_input)
            .or_else(|| named(&["diffuse", "albedo", "diff"], &["mask", "variation", "d2_"]))
            // Blockout greybox materials carry a single checker texture.
            .or_else(|| named(&["blockout"], &[]))
            .or_else(|| material_chain::slot_in(&chain.default_textures, "d_", &["mask"]));
        let normal_key = material_chain::slot(&chain, "n_", &[])
            .or(chain.normal_input)
            .or_else(|| named(&["normal"], &["n2_", "dn_", "detail", "variation", "wave"]))
            .or_else(|| material_chain::slot_in(&chain.default_textures, "n_", &[]));
        // Static switches of the compiled permutation (U-027) gate the Arkane slots: a slot
        // whose `*_Enable_*` switch is off is not in the original's shader even when the
        // instance (or a parent) still names a texture for it.
        let off = |s: &str| chain.switch(s) == Some(false);
        // The glow materials' fog noise (`F_Fog_Texture`, e.g. `wall_lamp_Inst`'s `Cloud_64_d`)
        // is only sampled with `F_Use_With_Fog`; with it off the texture is not a colour source.
        let fog_tex = chain.textures.iter().chain(chain.default_textures.iter()).find(|(n, _)| n.eq_ignore_ascii_case("F_Fog_Texture")).map(|t| t.1);
        let diffuse_key = diffuse_key.filter(|k| !(off("F_Use_With_Fog") && fog_tex == Some(*k)));
        let normal_key = normal_key.filter(|_| !off("N_Enable_NormalMap"));
        let spec_key = material_chain::slot(&chain, "s_", &["mask"]).filter(|_| !off("S_Enable_Specular"));
        let emissive_key = material_chain::slot(&chain, "e_", &["mask"]).filter(|_| !off("E_Enable_Emissive"));
        let opacity_key = material_chain::slot(&chain, "o_", &[]).or_else(|| named(&["opacity", "alpha"], &[]));
        let detail_key = material_chain::slot(&chain, "dn_", &["mask"]).filter(|_| !off("DN_Use_detailNormal"));
        let source = pkg.obj_path(ObjRef::Export(k.1));
        let lname = source.to_ascii_lowercase();
        let fx_card = ["fog", "smoke", "lightray", "light_ray", "godray", "mist"]
            .iter()
            .any(|k| lname.contains(k));
        let (blend, blend_known) = match chain.blend_mode.as_deref() {
            Some("BLEND_Masked") => (MaterialBlend::Masked, true),
            Some("BLEND_Translucent") | Some("BLEND_AlphaComposite") => (MaterialBlend::Translucent, true),
            Some("BLEND_Additive") => (MaterialBlend::Additive, true),
            Some("BLEND_Modulate") | Some("BLEND_ModulateAndAdd") => (MaterialBlend::Modulate, true),
            Some(_) => (MaterialBlend::Opaque, true),
            // BlendMode not serialized on a reached base material = class default (opaque).
            None if chain.base_material.is_some() => (MaterialBlend::Opaque, true),
            // Base material unreachable: volumetric cards are translucent in the
            // original and the naming is consistent enough to rely on.
            None if fx_card => (MaterialBlend::Translucent, false),
            None if opacity_key.is_some() => (MaterialBlend::Masked, false),
            None => (MaterialBlend::Opaque, false),
        };
        let sky = chain.base_material.as_deref().is_some_and(|b| b.starts_with("Skies.") || b.contains("Moon_PMAT") || b.contains("Sun_PMAT"));
        // Sky shaders sample their own textures, e.g. Moon_PMAT's moon disc (`Tex#0`, an
        // unnamed expression texture).
        let diffuse_key = diffuse_key.or_else(|| if sky { chain.expression_textures.first().copied().or(chain.textures.first().or(chain.default_textures.first()).map(|t| t.1)) } else { None });
        let mut diffuse = diffuse_key.and_then(|key| self.texture(key));
        let normal = normal_key.and_then(|key| self.texture(key));
        let opacity = opacity_key.and_then(|key| self.texture(key));
        if blend != MaterialBlend::Opaque {
            if let Some(o) = opacity {
                let sc = |n: &str, d: f32| chain.scalars.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or(d);
                let (lo, hi) = if blend == MaterialBlend::Masked { (0.0, 1.0) } else { (sc("O_Minimum_Opacity", 0.0), sc("O_Maximum_Opacity", 1.0)) };
                let ch = chain.mask_channel("O_Opacity_Mask_Channel").unwrap_or(0);
                diffuse = self.merged_opacity(diffuse, o, ch, lo, hi).or(diffuse);
            }
        }
        // Particle sprites (EFFECTS.md "Particle materials"): the base material's decoded shader
        // (`particle_material::recipe`) names the colour and opacity sources; the diffuse slot
        // is replaced by the colour source with the opacity channel in alpha, and the constants
        // go to `MaterialParams::particle`. The textures come from the instance chain or the base
        // material's defaults (e.g. `M_TB_CrouchSteam_INST` sets none).
        let particle = self.particle_sprite(&chain, &mut diffuse);
        // Channel-packed masks: the static component masks name the channel (U-027). Without
        // one, an emissive slot that reuses the opacity texture reads G (vista cards: opacity
        // in R, lit windows in G).
        let emissive = match (emissive_key, chain.mask_channel("E_Emissive_Channel")) {
            (Some(e), Some(ch)) => self.texture(e).and_then(|t| self.channel_texture(t, ch)),
            (Some(e), None) if Some(e) == opacity_key => self.texture(e).and_then(|t| self.channel_texture(t, 1)),
            (Some(e), _) => self.texture(e),
            (None, _) => None,
        };
        let specular = match (spec_key.and_then(|key| self.texture(key)), chain.mask_channel("S_Specular_Mask_Channel")) {
            (Some(t), Some(ch)) => self.channel_texture(t, ch),
            (t, _) => t,
        };
        let params = MaterialParams {
            specular,
            emissive,
            opacity,
            detail_normal: detail_key.and_then(|key| self.texture(key)),
            uses_cubemap: chain.switch("C_Enable_Cubemap").unwrap_or_else(|| chain.textures.iter().any(|(n, _)| n.to_ascii_lowercase().starts_with("c_cubemap_texture"))),
            blend,
            blend_known,
            two_sided: chain.two_sided.unwrap_or(false),
            unlit: chain.lighting_model.as_deref() == Some("MLM_Unlit"),
            opacity_clip: chain.opacity_clip.unwrap_or(0.3333),
            scalars: chain.scalars.clone(),
            vectors: chain.vectors.clone(),
            sky,
            particle,
            render_targets: chain.textures.iter().chain(chain.default_textures.iter()).filter(|(_, k)| self.pkgs[k.0].export_class(k.1) == "TextureRenderTarget2D").map(|(n, k)| (n.clone(), self.pkgs[k.0].obj_path(ObjRef::Export(k.1)))).collect(),
            base_material: chain.base_material.clone(),
            static_switches: {
                let mut names: Vec<String> = chain.switch_defaults.iter().map(|(n, _)| n.clone()).collect();
                if let Some(s) = &chain.static_params {
                    for (n, _, _) in &s.switches {
                        if !names.iter().any(|m| m.eq_ignore_ascii_case(n)) {
                            names.push(n.clone());
                        }
                    }
                }
                names.into_iter().filter_map(|n| chain.switch(&n).map(|v| (n, v))).collect()
            },
            mask_channels: MASK_CHANNELS.iter().filter_map(|n| chain.mask_channel(n).map(|c| (n.to_string(), c as u8))).collect(),
        };
        if diffuse.is_none() {
            self.stats.materials_without_diffuse.push(format!(
                "{source} (params: {})",
                chain.textures.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(",")
            ));
        }
        let mut texture_params: Vec<(String, TextureId)> = [("diffuse", diffuse), ("normal", normal), ("specular", params.specular), ("emissive", params.emissive), ("opacity", params.opacity)]
            .into_iter()
            .filter_map(|(n, t)| Some((n.to_string(), t?)))
            .collect();
        // Distortion materials: the raw textures the distortion pass samples (U-360); sky domes
        // (Skies_PMAT): the textures its shader samples.
        let raw: &[&str] = if chain.switch("Dist_Enable_Distortion") == Some(true) {
            &["Dist_Distortion", "O_Opacity", "UV_Distortion"]
        } else if chain.base_material.as_deref().is_some_and(|b| b.eq_ignore_ascii_case("Materials_ref.water_materials.river_main_PMAT")) {
            // Water (river_main_PMAT, the runtime's water.rs): the textures its base pass samples.
            &["W_Waves_normalmap", "D_diffuse_mask", "D_diffuse_texture"]
        } else if let Some(t) = chain.base_material.as_deref().and_then(base_raw_textures) {
            // Other base materials the runtime shades with their decoded shaders.
            t
        } else if blend == MaterialBlend::Translucent && chain.switch("C_Enable_CubemapMask") == Some(true) {
            // Translucent decals (generic_translucent_PMAT): the scene reflection's mask.
            &["C_Cubemap_Mask"]
        } else if sky {
            &["D_Diffuse", "C_Clouds_Texture", "C2_Clouds_2_Texture", "Storm", "Cloud"]
        } else {
            &[]
        };
        {
            for &n in raw {
                let key = chain.textures.iter().chain(chain.default_textures.iter()).find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1);
                if let Some(t) = key.and_then(|k| self.texture(k)) {
                    texture_params.push((n.to_string(), t));
                }
            }
            // generic_PMAT's diffuse variation (`D2_Enable_Diffuse_Variation`): its texture and mask.
            if !sky && chain.switch("D2_Enable_Diffuse_Variation") == Some(true) {
                for n in ["D2_diffuse_variation_texture", "D2_Diffuse_variation_mask"] {
                    let key = chain.textures.iter().chain(chain.default_textures.iter()).find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1);
                    if let Some(t) = key.and_then(|k| self.texture(k)) {
                        texture_params.push((n.to_string(), t));
                    }
                }
            }
            // A material's own cubemaps, face by face, as `<name>.<face>` (faces +X, -X, +Y,
            // -Y, +Z, -Z).
            if let Some(cubes) = chain.base_material.as_deref().and_then(base_cube_textures) {
                for &n in cubes {
                    let key = chain.textures.iter().chain(chain.default_textures.iter()).find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1);
                    if let Some(faces) = key.and_then(|k| self.cube_faces(k)) {
                        for (i, t) in faces.into_iter().enumerate() {
                            texture_params.push((format!("{n}.{i}"), t));
                        }
                    }
                }
            }
            // The base material's unnamed textures (its shaders' `Tex#i`), as `Tex<i>`.
            if sky || chain.base_material.as_deref().is_some_and(base_uses_expression_textures) {
                for (i, k) in chain.expression_textures.iter().enumerate() {
                    if let Some(t) = self.texture(*k) {
                        texture_params.push((format!("Tex{i}"), t));
                    }
                }
            }
        }
        let asset = MaterialAsset {
            source,
            diffuse,
            normal,
            blend: match blend {
                MaterialBlend::Opaque => BlendHint::Opaque,
                MaterialBlend::Masked => BlendHint::Masked,
                _ => BlendHint::Translucent,
            },
            texture_params,
            contact_type: chain.phys_material.and_then(|pm| self.contact_type(pm)),
        };
        self.map.materials.push(asset);
        let id = self.map.materials.len() - 1;
        self.map.material_params.resize(id, MaterialParams::default());
        self.map.material_params.push(params);
        self.mat_ids.insert(k, Some(id));
        Some(id)
    }

    /// Particle sprite materials (`particle_material::recipe`, EFFECTS.md "Particle
    /// materials"): bake the colour source with the opacity channel in alpha into `diffuse` and
    /// return the shader's constants. Bases without a recipe that are translucent, additive or
    /// masked take the first conventional opacity parameter (`OPACITY_PARAMS`) when the generic
    /// path found none, so no sprite falls back to an opaque diffuse alpha.
    fn particle_sprite(&mut self, chain: &material_chain::MaterialChain, diffuse: &mut Option<TextureId>) -> Option<ParticleShading> {
        use crate::particle_material::{AlphaUse, Colour, OPACITY_PARAMS, Opacity, recipe};
        let base = chain.base_material.as_deref()?;
        let tex_key = |n: &str| chain.textures.iter().chain(chain.default_textures.iter()).find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1);
        let switch = |s: &str| chain.switch(s) == Some(true);
        let mask = |s: &str| chain.mask_channel(s);
        let scalar = |n: &str, d: f32| chain.scalars.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or(d);
        let Some(r) = recipe(base, &switch, &mask) else {
            let blend = chain.blend_mode.as_deref().unwrap_or("BLEND_Opaque");
            if matches!(blend, "BLEND_Translucent" | "BLEND_Additive" | "BLEND_Masked") {
                let already = material_chain::slot(chain, "o_", &[]).is_some() || chain.textures.iter().any(|(n, _)| {
                    let l = n.to_ascii_lowercase();
                    l.contains("opacity") || l.contains("alpha")
                });
                if !already {
                    if let Some((o, ch)) = OPACITY_PARAMS.iter().find_map(|(n, m)| Some((tex_key(n)?, m.and_then(|m| chain.mask_channel(m)).unwrap_or(0)))) {
                        if let Some(o) = self.texture(o) {
                            *diffuse = self.merged_opacity(*diffuse, o, ch, 0.0, 1.0).or(*diffuse);
                        }
                    }
                }
            }
            return None;
        };
        let colour = match &r.colour {
            Colour::Texture { param, channel } => {
                let t = tex_key(param).and_then(|k| self.texture(k));
                match (t, channel) {
                    (Some(t), Some(ch)) => self.channel_texture(t, *ch),
                    (t, _) => t,
                }
            }
            Colour::Expression(i) => chain.expression_textures.get(*i).copied().and_then(|k| self.texture(k)),
            Colour::White => None,
        };
        let mut shading = ParticleShading { tint: [1.0; 3], opacity: 1.0, erode: r.erode, vertex_alpha_only: false, opacity_missing: false, radial_power: None };
        match &r.opacity {
            Opacity::Texture { param, channel } => {
                // Eroding materials: the multiplier sharpens the map (alpha = saturate(H x M) at
                // vertex alpha 0); the runtime fades by the vertex alpha (APPROXIMATION).
                let hi = if r.erode { r.opacity_scale.map(|(n, d)| scalar(n, d)).unwrap_or(1.0) } else { 1.0 };
                let o = tex_key(param).and_then(|k| self.texture(k));
                match o {
                    Some(o) => *diffuse = self.merged_opacity(colour, o, *channel, 0.0, hi).or(colour),
                    None => {
                        // The opacity texture did not resolve (textures off, or a missing
                        // package): the colour with an opaque alpha, opacity from the vertex.
                        *diffuse = colour.and_then(|c| self.merged_opacity(Some(c), c, 0, 1.0, 1.0)).or(colour);
                        shading.vertex_alpha_only = true;
                        shading.opacity_missing = true;
                    }
                }
            }
            Opacity::VertexOnly => {
                // The shader ignores the texture's alpha (`mov oC0.w, v1.w`).
                *diffuse = colour.and_then(|c| self.merged_opacity(Some(c), c, 0, 1.0, 1.0)).or(colour);
                shading.vertex_alpha_only = true;
            }
            Opacity::Radial => {
                *diffuse = None;
                shading.radial_power = Some(scalar("G_GlowPower", 10.0));
            }
        }
        if !r.erode {
            if let Some((n, d)) = r.opacity_scale {
                shading.opacity *= scalar(n, d);
            }
        }
        if let Some((n, d)) = r.brightness {
            let b = scalar(n, d);
            shading.tint = shading.tint.map(|c| c * b);
        }
        if let Some((n, d, use_alpha)) = r.tint {
            let v = chain.vectors.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or(d);
            let k = if use_alpha == AlphaUse::Colour { v[3] } else { 1.0 };
            for c in 0..3 {
                shading.tint[c] *= v[c] * k;
            }
            if use_alpha == AlphaUse::Opacity {
                shading.opacity *= v[3];
            }
        }
        Some(shading)
    }

    /// Greyscale BGRA8 copy of one channel of a texture (all mips).
    fn channel_texture(&mut self, t: TextureId, ch: usize) -> Option<TextureId> {
        let key = (t, usize::MAX - 1, ch as u32, 0);
        if let Some(id) = self.merged_ids.get(&key) {
            return *id;
        }
        let src = self.map.textures[t].clone();
        let mips: Vec<Vec<u8>> = (0..src.mips.len())
            .map_while(|m| decode_rgba8(&src, m))
            .map(|(_, _, px)| px.iter().flat_map(|p| [p[ch], p[ch], p[ch], 255]).collect())
            .collect();
        let id = (!mips.is_empty()).then(|| {
            self.map.textures.push(TextureAsset {
                source: format!("{}[{}]", src.source, ["r", "g", "b", "a"][ch]),
                format: TextureFormat::Bgra8,
                width: src.width,
                height: src.height,
                mips,
                srgb: src.srgb,
            });
            self.map.textures.len() - 1
        });
        self.merged_ids.insert(key, id);
        id
    }

    /// Diffuse (or white) with the opacity mask's red channel remapped to `[lo, hi]` as
    /// alpha (RGBA8, full mip chain), so masked/translucent materials alpha-test or blend
    /// with the original mask (`O_Minimum_Opacity` / `O_Maximum_Opacity`).
    fn merged_opacity(&mut self, d: Option<TextureId>, o: TextureId, ch: usize, lo: f32, hi: f32) -> Option<TextureId> {
        // The channel is folded into the key's last field (channel 0 keeps the plain key).
        let key = (d.unwrap_or(usize::MAX), o, lo.to_bits(), hi.to_bits() ^ ((ch as u32) << 29));
        if let Some(id) = self.merged_ids.get(&key) {
            return *id;
        }
        let ot = &self.map.textures[o];
        let osrc = ot.source.clone();
        let (ow, oh, opx) = decode_rgba8(ot, 0)?;
        let dt = match d {
            Some(d) => self.map.textures[d].clone(),
            None => TextureAsset {
                source: "white".into(),
                format: TextureFormat::Bgra8,
                width: ow,
                height: oh,
                mips: vec![vec![255u8; (ow * oh * 4) as usize]],
                srgb: true,
            },
        };
        let dt = &dt;
        let mut mips = Vec::new();
        for m in 0..dt.mips.len() {
            let Some((w, h, mut px)) = decode_rgba8(dt, m) else { break };
            for y in 0..h {
                for x in 0..w {
                    let sx = (x as u64 * ow as u64 / w as u64) as usize;
                    let sy = (y as u64 * oh as u64 / h as u64) as usize;
                    let a = opx[sy * ow as usize + sx][ch.min(3)] as f32 / 255.0;
                    px[(y * w + x) as usize][3] = ((lo + (hi - lo) * a).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
            mips.push(px.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect::<Vec<u8>>());
        }
        let id = (!mips.is_empty()).then(|| {
            let t = TextureAsset {
                source: format!("{}+{}", dt.source, osrc),
                format: TextureFormat::Bgra8,
                width: dt.width,
                height: dt.height,
                mips,
                srgb: dt.srgb,
            };
            self.map.textures.push(t);
            self.map.textures.len() - 1
        });
        self.merged_ids.insert(key, id);
        id
    }

    /// The six faces (+X, -X, +Y, -Y, +Z, -Z) of a `TextureCube`, as level textures (a
    /// material's own cubemap, e.g. water_spillway_translu_PMAT's `C_Cubemap_texture`).
    fn cube_faces(&mut self, ck: Key) -> Option<Vec<TextureId>> {
        let cpkg = self.pkgs[ck.0].clone();
        if cpkg.export_class(ck.1) != "TextureCube" {
            return None;
        }
        let ch = cpkg.read_object(ck.1).ok()?;
        let mut faces = Vec::new();
        for f in ["FacePosX", "FaceNegX", "FacePosY", "FaceNegY", "FacePosZ", "FaceNegZ"] {
            let r = find(&ch.properties, f).and_then(|p| p.as_object())?;
            let k = self.resolve(ck.0, ObjRef::from_index(r))?;
            faces.push(self.texture(k)?);
        }
        Some(faces)
    }

    fn texture(&mut self, k: Key) -> Option<TextureId> {
        if !self.opts.load_textures {
            return None;
        }
        if let Some(id) = self.tex_ids.get(&k) {
            return *id;
        }
        let pkg = self.pkgs[k.0].clone();
        let id = match read_texture(self.content, &pkg, k.1, self.opts.max_texture_size) {
            Ok(t) => {
                self.map.textures.push(t);
                Some(self.map.textures.len() - 1)
            }
            Err(e) => {
                self.stats.texture_failures += 1;
                self.stats.errors.push(format!("{e:#}"));
                None
            }
        };
        self.tex_ids.insert(k, id);
        id
    }
}

impl crate::particles::ParticleCtx for Importer<'_> {
    fn package(&self, slot: usize) -> Arc<Package> {
        self.pkgs[slot].clone()
    }
    fn resolve_ref(&mut self, slot: usize, r: ObjRef) -> Option<Key> {
        self.resolve(slot, r)
    }
    fn import_material(&mut self, k: Key) -> Option<MaterialId> {
        self.material(k)
    }
    fn import_mesh(&mut self, k: Key) -> Option<usize> {
        self.mesh(k)
    }
    fn class_default(&mut self, class: &str) -> Vec<Property> {
        if let Some(p) = self.class_defaults.get(class) {
            return p.clone();
        }
        let props = (|| {
            let engine = self.content.package("Engine").ok()?;
            let want = format!("Default__{class}");
            let i = (0..engine.exports.len()).find(|&i| engine.obj_name(ObjRef::Export(i)) == want)?;
            engine.read_object(i).ok().map(|h| h.properties)
        })()
        .unwrap_or_default();
        self.class_defaults.insert(class.to_string(), props.clone());
        props
    }
    fn material_blend(&self, id: MaterialId) -> dishonored_ir::particles::ParticleBlend {
        use dishonored_ir::particles::ParticleBlend as B;
        match self.map.material_params.get(id).map(|p| p.blend) {
            Some(MaterialBlend::Additive) => B::Additive,
            Some(MaterialBlend::Masked) => B::Masked,
            Some(MaterialBlend::Opaque) => B::Opaque,
            Some(MaterialBlend::Modulate) => B::Modulate,
            _ => B::Translucent,
        }
    }
}

// [foliage-survey] Placed skeletal scenery (see `SKELETAL_PROP_CLASSES`).
impl Importer<'_> {
    /// Every placed `SkeletalMeshComponent` of a `SKELETAL_PROP_CLASSES` actor in `slot`, as a
    /// render-only instance of its mesh in the reference pose. UE3 places a skeletal mesh at
    /// `actor x component x Rot(RotOrigin) x Translate(Origin)` (as the character and climbable
    /// loaders do); the reference pose's skinned vertices are the stored positions.
    fn import_skeletal_props(&mut self, slot: usize) {
        let pkg = self.pkgs[slot].clone();
        // `SkeletalMeshActor`s that Kismet names (matinee props, items attached to pawns) are the
        // cutscene player's; the others (Boyle's sewer gate and bridge) are scenery.
        let mut scripted: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) == "SeqVar_Object" {
                if let Some(ObjRef::Export(o)) = pkg.read_object(i).ok().and_then(|h| find(&h.properties, "ObjValue").and_then(|p| p.as_object())).map(ObjRef::from_index) {
                    scripted.insert(o);
                }
            }
        }
        for ci in 0..pkg.exports.len() {
            if pkg.export_class(ci) != "SkeletalMeshComponent" {
                continue;
            }
            let ObjRef::Export(actor) = ObjRef::from_index(pkg.exports[ci].outer) else { continue };
            let class = pkg.export_class(actor);
            // [anim-parity] A SkeletalMeshActor looping its own animation (Boyle Int's clock) is an
            // animated prop (`usable_props::playing_loop`), drawn by the runtime's level props.
            let scenery = SKELETAL_PROP_CLASSES.contains(&class.as_str()) || (class == "SkeletalMeshActor" && !scripted.contains(&actor) && crate::usable_props::playing_loop(&pkg, ci).is_none());
            if !scenery || !self.is_in_persistent_level(&pkg, actor) {
                continue;
            }
            if let Err(e) = self.import_skeletal_component(slot, actor, ci, &class) {
                self.stats.errors.push(format!("{}: {e:#}", pkg.obj_path(ObjRef::Export(ci))));
            }
        }
    }

    fn import_skeletal_component(&mut self, slot: usize, actor: usize, ci: usize, class: &str) -> Result<()> {
        let pkg = self.pkgs[slot].clone();
        let props = pkg.read_object(actor)?.properties;
        let cprops = pkg.read_object(ci)?.properties;
        let Some(mesh_ref) = find(&cprops, "SkeletalMesh").and_then(|p| p.as_object()).filter(|o| *o != 0) else { return Ok(()) };
        let Some(k) = self.resolve(slot, ObjRef::from_index(mesh_ref)) else { return Ok(()) };
        let Some((mesh, mesh_to_component, mesh_materials, slots)) = self.skeletal_mesh(k) else { return Ok(()) };
        let overrides = find(&cprops, "Materials").and_then(|p| p.array_objects().ok()).unwrap_or_default();
        let mut by_slot: Vec<Option<MaterialId>> = Vec::new();
        for (i, m) in mesh_materials.iter().enumerate() {
            by_slot.push(match overrides.get(i).copied().filter(|o| *o != 0) {
                Some(o) => self.resolve(slot, ObjRef::from_index(o)).and_then(|mk| self.material(mk)),
                None => *m,
            });
        }
        // Instance materials are per section; sections name their mesh material slot.
        let materials = slots.iter().map(|&s| by_slot.get(s).copied().flatten()).collect();
        let hidden = bool_or(&props, "bHidden", false) || bool_or(&cprops, "HiddenGame", false);
        self.map.instances.push(MeshInstance {
            mesh,
            transform: actor_transform(&props) * component_transform(&cprops) * mesh_to_component,
            materials,
            hidden,
            collides: false,
            source: format!("{} [{class}]", pkg.obj_path(ObjRef::Export(ci))),
            level: pkg.name.clone(),
            lightmap: None,
            mover: None,
            trace_mask: dishonored_ir::all_trace_types(),
            reflect_channels: 0,
            // A render-only copy of a skeletal prop; skeletal components are not modelled as
            // stealth shadow casters (their flags were not decoded).
            stealth_shadow: false,
        });
        self.stats.skeletal_props += 1;
        Ok(())
    }

    /// LOD0 of a skeletal mesh as a static mesh asset: (mesh, mesh to component, material per
    /// mesh slot, mesh slot per section).
    fn skeletal_mesh(&mut self, k: Key) -> Option<SkeletalProp> {
        if let Some(v) = self.skel_meshes.get(&k) {
            return v.clone();
        }
        self.skel_meshes.insert(k, None);
        let pkg = self.pkgs[k.0].clone();
        let raw = match crate::skeletal_mesh::read_skeletal_mesh(&pkg, k.1) {
            Ok(r) => r,
            Err(e) => {
                self.stats.mesh_failures += 1;
                self.stats.errors.push(format!("{e:#}"));
                return None;
            }
        };
        let mesh_to_component = rotator_matrix(raw.rot_origin[0], raw.rot_origin[1], raw.rot_origin[2]) * Mat4::from_translation(raw.mesh_origin);
        let mesh_materials: Vec<Option<MaterialId>> = raw.materials.iter().map(|&m| self.resolve(k.0, ObjRef::from_index(m)).and_then(|mk| self.material(mk))).collect();
        let (lo, hi) = raw.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        let slots: Vec<usize> = raw.sections.iter().map(|s| s.material_index.max(0) as usize).collect();
        let n = raw.positions.len();
        let asset = StaticMeshAsset {
            source: pkg.obj_path(ObjRef::Export(k.1)),
            normals: raw.normals,
            tangents: raw.tangents,
            uv0: if raw.uvs.len() == n { raw.uvs } else { vec![Vec2::ZERO; n] },
            uv1: Vec::new(),
            uv_set1: Vec::new(),
            colors: Vec::new(),
            indices: raw.indices,
            sections: slots
                .iter()
                .zip(&raw.sections)
                .map(|(&s, sec)| MeshSection { first_index: sec.first_index, num_triangles: sec.num_triangles, material: mesh_materials.get(s).copied().flatten(), enable_collision: false })
                .collect(),
            collision_triangles: Vec::new(),
            collision_materials: Vec::new(),
            simple_line: false,
            simple_box: false,
            simple_collision: None,
            bounds_min: lo,
            bounds_max: hi,
            positions: raw.positions,
        };
        self.map.meshes.push(asset);
        let v = Some((self.map.meshes.len() - 1, mesh_to_component, mesh_materials, slots));
        self.skel_meshes.insert(k, v.clone());
        v
    }
}

/// (mesh, mesh to component, material per mesh slot, mesh slot per section).
type SkeletalProp = (MeshId, Mat4, Vec<Option<MaterialId>>, Vec<usize>);
