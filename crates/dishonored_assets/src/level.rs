//! Level import: StaticMeshActors and StaticMeshCollectionActors from a set of
//! level packages into a normalized [`DishonoredMap`].

use crate::content::Content;
use crate::material::resolve_material;
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
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            max_texture_size: 1024,
            load_textures: true,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ImportStats {
    pub static_mesh_actors: usize,
    pub collection_components: usize,
    pub instances: usize,
    pub meshes: usize,
    pub mesh_failures: usize,
    pub materials: usize,
    pub textures: usize,
    pub texture_failures: usize,
    pub unresolved_refs: usize,
    pub errors: Vec<String>,
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
    stats: ImportStats,
}

const RESOURCE_CLASSES: &[&str] = &[
    "StaticMesh",
    "Texture2D",
    "Material",
    "MaterialInstanceConstant",
];

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
        stats: ImportStats::default(),
    };
    for slot in 0..imp.pkgs.len() {
        imp.import_actors(slot)?;
    }
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
fn actor_transform(props: &[Property]) -> Mat4 {
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
        for i in 0..pkg.exports.len() {
            let class = pkg.export_class(i);
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
        let props = pkg.read_object(ci)?.properties;
        let Some(mesh_ref) = find(&props, "StaticMesh").and_then(|p| p.as_object()) else {
            return Ok(());
        };
        let Some(mesh) = self.resolve(slot, ObjRef::from_index(mesh_ref)).and_then(|k| self.mesh(k)) else {
            return Ok(());
        };
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
        self.map.instances.push(MeshInstance {
            mesh,
            transform: parent * component_transform(&props),
            materials,
            hidden,
            collides,
            source,
        });
        Ok(())
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
                    uv1: raw.uvs.get(1).cloned().unwrap_or_default(),
                    indices: raw.indices.iter().map(|&i| i as u32).collect(),
                    sections,
                    collision_triangles: raw
                        .kdop_triangles
                        .iter()
                        .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
                        .collect(),
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

    fn material(&mut self, k: Key) -> Option<MaterialId> {
        if let Some(id) = self.mat_ids.get(&k) {
            return *id;
        }
        let pkg = self.pkgs[k.0].clone();
        let resolved = resolve_material(&pkg, ObjRef::Export(k.1)).unwrap_or_default();
        let tex = |s: &mut Self, r: Option<ObjRef>| -> Option<TextureId> {
            let key = s.resolve(k.0, r?)?;
            s.texture(key)
        };
        let diffuse = tex(self, resolved.diffuse);
        let normal = tex(self, resolved.normal);
        let asset = MaterialAsset {
            source: pkg.obj_path(ObjRef::Export(k.1)),
            diffuse,
            normal,
            blend: if resolved.translucent {
                BlendHint::Translucent
            } else if resolved.masked {
                BlendHint::Masked
            } else {
                BlendHint::Opaque
            },
            texture_params: Vec::new(),
        };
        self.map.materials.push(asset);
        let id = Some(self.map.materials.len() - 1);
        self.mat_ids.insert(k, id);
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
