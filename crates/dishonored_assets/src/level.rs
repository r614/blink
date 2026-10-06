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
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ImportStats {
    pub static_mesh_actors: usize,
    pub collection_components: usize,
    /// Instances from StaticMeshComponents of other actor classes (render-only).
    pub other_actor_components: usize,
    /// SplineMeshComponents baked into deformed mesh copies.
    pub spline_meshes: usize,
    /// Static light actors skipped because their light is baked into the lightmaps.
    pub static_lights_skipped: usize,
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
}

const RESOURCE_CLASSES: &[&str] = &[
    "StaticMesh",
    "Texture2D",
    "Material",
    "MaterialInstanceConstant",
    "TextureCube",
    "ParticleSystem",
];

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
    };
    for slot in 0..level_slots {
        imp.import_actors(slot)?;
    }
    if opts.particles && opts.load_textures {
        for p in &opts.extra_particles {
            imp.particle_by_path(p);
        }
    }
    if opts.load_textures && opts.load_lightmaps {
        imp.bake_lightmaps();
    }
    if level_slots > 0 {
        imp.import_environment(0);
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
fn component_transform(props: &[Property]) -> Mat4 {
    let t = vec3_or(props, "Translation", Vec3::ZERO);
    let s = float_or(props, "Scale", 1.0) * vec3_or(props, "Scale3D", Vec3::ONE);
    Mat4::from_translation(t) * rot_matrix(props, "Rotation") * Mat4::from_scale(s)
}

impl Importer<'_> {
    fn is_in_persistent_level(&self, pkg: &Package, i: usize) -> bool {
        // Placed actors live in TheWorld.PersistentLevel; other outers are archetypes/prefabs.
        let outer = ObjRef::from_index(pkg.exports[i].outer);
        pkg.obj_name(outer) == "PersistentLevel"
    }

    fn import_actors(&mut self, slot: usize) -> Result<()> {
        let pkg = self.pkgs[slot].clone();
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
        for i in 0..pkg.exports.len() {
            let class = pkg.export_class(i);
            if !matches!(class.as_str(), "StaticMeshActor" | "StaticMeshCollectionActor")
                && self.opts.other_actors
                && owned.contains_key(&i)
                && self.is_in_persistent_level(&pkg, i)
            {
                if let Err(e) = self.import_owned_components(slot, i, &owned[&i]) {
                    self.stats.errors.push(format!("{}: {e:#}", pkg.obj_path(ObjRef::Export(i))));
                }
                continue;
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
            Some(Value::Color(rgba)) => [0, 1, 2].map(|k| crate::lightmap::srgb_to_linear(rgba[k])),
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
            Ok(Some(l)) => self.map.lights.push(l),
            Ok(None) => {}
            Err(e) => self.stats.errors.push(e),
        }
    }
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
        let world = cached.unwrap_or_else(|| actor_transform(&h.properties)) * component_transform(c);
        let position = world.w_axis.truncate();
        let direction = world.x_axis.truncate().normalize_or(Vec3::NEG_Z);
        let color = match find(c, "LightColor").map(|p| &p.value) {
            Some(Value::Color(rgba)) => [0, 1, 2].map(|k| crate::lightmap::srgb_to_linear(rgba[k])),
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
        for &ci in comps {
            let before = self.map.instances.len();
            let src = format!("{} [{class}]", pkg.obj_path(ObjRef::Export(ci)));
            if let Err(e) = self.add_component(slot, ci, world, hidden, src) {
                self.stats.errors.push(format!("{}: {e:#}", pkg.obj_path(ObjRef::Export(ci))));
            }
            for inst in &mut self.map.instances[before..] {
                inst.collides = false;
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
        let Some(mesh_ref) = find(&props, "StaticMesh").and_then(|p| p.as_object()) else {
            return Ok(());
        };
        let Some(mut mesh) = self.resolve(slot, ObjRef::from_index(mesh_ref)).and_then(|k| self.mesh(k)) else {
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
        let collides = bool_or(&props, "CollideActors", true) && bool_or(&props, "BlockActors", true);
        if self.opts.load_textures && self.opts.load_lightmaps {
            match pkg.export_data(ci).and_then(|d| read_static_mesh_component_lods(d, native_offset)) {
                Ok(lods) => match lods.into_iter().next().map(|l| l.lightmap) {
                    Some(LightMapRef::Texture(lm)) => {
                        self.stats.lightmaps_texture += 1;
                        self.pending_lightmaps.push(PendingLightmap { instance: self.map.instances.len(), slot, lm });
                    }
                    Some(LightMapRef::Vertex { .. }) => self.stats.lightmaps_vertex += 1,
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
            lightmap: None,
        });
        Ok(())
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
            for pass in [2u8, 1u8] {
                let pad = if pass == 2 { 0 } else { 1 };
                for &m in members {
                    let lm = &pending[m].lm;
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
                        }
                    }
                }
            }
            let atlas = self.map.lightmaps.len();
            self.map.lightmaps.push(LightmapAtlas {
                source: self.pkgs[ka.0].obj_path(ObjRef::Export(ka.1)),
                width: w as u32,
                height: h as u32,
                texels: out.into_iter().map(pack_rgb9e5).collect(),
                directional: out_dir.into_iter().map(pack_rgb9e5).collect(),
            });
            for &m in members {
                let lm = &pending[m].lm;
                let min = Vec2::from(lm.coordinate_bias);
                self.map.instances[pending[m].instance].lightmap = Some(InstanceLightmap {
                    atlas,
                    uv_min: min,
                    uv_max: min + Vec2::from(lm.coordinate_scale),
                });
            }
        }
        self.stats.lightmap_atlases = self.map.lightmaps.len();
        self.pack_lightmap_pages(4096);
        self.stats.lightmap_pages = self.map.lightmaps.len();
    }

    /// Shelf-pack the baked atlases (each kept intact) into a few large pages. Every distinct
    /// lightmap image breaks Bevy's draw batching; 196 atlases cost ~4x frame time on Boyle Ext.
    fn pack_lightmap_pages(&mut self, page: u32) {
        let atlases = std::mem::take(&mut self.map.lightmaps);
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
                let asset = StaticMeshAsset {
                    source: pkg.obj_path(ObjRef::Export(k.1)),
                    normals: raw.normals,
                    tangents: raw.tangent_x,
                    uv0: raw.uvs.first().cloned().unwrap_or_else(|| vec![Vec2::ZERO; raw.positions.len()]),
                    // Lightmap UVs: the set named by `LightMapCoordinateIndex` (serialized when
                    // non-zero, e.g. 1 on air_duct_10; absent means the UE3 default 0).
                    uv1: {
                        let lmi = pkg
                            .read_object(k.1)
                            .ok()
                            .and_then(|h| find(&h.properties, "LightMapCoordinateIndex").and_then(|p| p.as_float()))
                            .map(|v| v as usize)
                            .unwrap_or(0);
                        raw.uvs.get(lmi).or(raw.uvs.get(1)).cloned().unwrap_or_default()
                    },
                    indices: raw.indices.iter().map(|&i| i as u32).collect(),
                    sections,
                    collision_triangles: raw
                        .kdop_triangles
                        .iter()
                        .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
                        .collect(),
                    collision_materials: raw.kdop_triangles.iter().map(|t| t[3]).collect(),
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
        // Sky shaders sample their own (default) textures, e.g. Moon_PMAT's moon disc.
        let diffuse_key = diffuse_key.or_else(|| if sky { chain.textures.first().or(chain.default_textures.first()).map(|t| t.1) } else { None });
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
        };
        if diffuse.is_none() {
            self.stats.materials_without_diffuse.push(format!(
                "{source} (params: {})",
                chain.textures.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(",")
            ));
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
            texture_params: [("diffuse", diffuse), ("normal", normal), ("specular", params.specular), ("emissive", params.emissive), ("opacity", params.opacity)]
                .into_iter()
                .filter_map(|(n, t)| Some((n.to_string(), t?)))
                .collect(),
            contact_type: chain.phys_material.and_then(|pm| self.contact_type(pm)),
        };
        self.map.materials.push(asset);
        let id = self.map.materials.len() - 1;
        self.map.material_params.resize(id, MaterialParams::default());
        self.map.material_params.push(params);
        self.mat_ids.insert(k, Some(id));
        Some(id)
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
