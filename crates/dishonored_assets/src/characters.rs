//! Character archetypes for multiplayer bodies, built from Dishonored NPC skeletal meshes.

use crate::content::Content;
use crate::material::resolve_material;
use crate::skeletal_mesh::read_skeletal_mesh;
use crate::texture::read_texture;
use crate::util::rotator_matrix;
use anyhow::{Context, Result};
use dishonored_ir::*;
use glam::{Mat4, Vec3};
use std::collections::HashMap;
use ue3_package::ObjRef;

/// (archetype name, parts as (package, object path)). All parts are skinned to
/// compatible skeletons and share the first part's mesh->pawn transform (attached meshes
/// such as masks are authored in the body's mesh space with MeshOrigin 0).
pub const ARCHETYPES: &[(&str, &[(&str, &str)])] = &[
    // 0: masked male aristocrat (party guest)
    ("aristocrat", &[
        ("L_BoyleStreet_Ext_Script", "Npc_Aristo.Mesh.skm_MaleAristo02_Body"),
        ("L_BoyleStreet_Ext_Script", "Npc_Aristo.Mesh.skm_MaleAristo02_Head"),
        ("L_BoyleStreet_Ext_Script", "Npc_Masks.metalface01.skm_metalface01"),
    ]),
    // 1: elite guard
    ("elite_guard", &[
        ("L_Boyle_Ext_Script", "Npc_EliteGuard.Mesh.Skm_EliteGuard_Body"),
        ("L_Boyle_Ext_Script", "Npc_EliteGuard.Mesh.Skm_EliteGuard_Head"),
    ]),
    // 2: masked male aristocrat B
    ("aristocrat_b", &[
        ("L_Boyle_Int_Script", "Npc_Aristo.Mesh.skm_MaleAristo01_Body"),
        ("L_Brothel_Script", "Npc_Aristo.Mesh.skm_MaleAristo01_Head"),
        ("L_BoyleStreet_Ext_Script", "Npc_Masks.metalface02.skm_metalface02"),
    ]),
    // 3: masked female aristocrat
    ("female_aristocrat", &[
        ("L_BoyleStreet_Ext_Script", "Npc_FemaleAristo.Mesh.skm_FemaleAristo02_Body"),
        ("L_BoyleStreet_Ext_Script", "Npc_FemaleAristo.Mesh.skm_FemaleAristo02_Head"),
        ("L_BoyleStreet_Ext_Script", "Npc_Female_Masks.black_mask.Female_BlackMask"),
    ]),
];

/// Load one archetype. `pawn_half_height` aligns the feet with the collision cylinder.
pub fn load_character(content: &Content, name: &str, pawn_half_height: f32, max_tex: u32) -> Result<CharacterModel> {
    let (_, parts) = ARCHETYPES.iter().find(|a| a.0 == name).with_context(|| format!("unknown archetype {name}"))?;
    let mut shared_m2p: Option<Mat4> = None;
    let mut model = CharacterModel { name: name.to_string(), parts: Vec::new(), materials: Vec::new(), textures: Vec::new() };
    let mut mat_ids: HashMap<(String, usize), Option<usize>> = HashMap::new();
    let mut tex_ids: HashMap<(String, usize), Option<usize>> = HashMap::new();
    let mut feet_z = f32::MAX;
    for (pkg_name, part) in parts.iter() {
        let pkg = content.package(pkg_name)?;
        let idx = (0..pkg.exports.len())
            .find(|&i| pkg.export_class(i) == "SkeletalMesh" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(part))
            .with_context(|| format!("{part} not in {pkg_name}"))?;
        let raw = read_skeletal_mesh(&pkg, idx)?;
        // Mesh -> pawn: R(RotOrigin) * (v + MeshOrigin); verified numerically on
        // skm_MaleAristo02_Body (feet at -97, head top at +79 around the pawn centre).
        let r = rotator_matrix(raw.rot_origin[0], raw.rot_origin[1], raw.rot_origin[2]);
        let m2p = *shared_m2p.get_or_insert(r * Mat4::from_translation(raw.mesh_origin));
        for p in &raw.positions {
            feet_z = feet_z.min(m2p.transform_point3(*p).z);
        }
        let mut sections = Vec::new();
        for s in &raw.sections {
            let mref = raw.materials.get(s.material_index.max(0) as usize).copied().unwrap_or(0);
            let material = match ObjRef::from_index(mref) {
                ObjRef::Export(mi) => *mat_ids.entry((pkg_name.to_string(), mi)).or_insert_with(|| {
                    let res = resolve_material(&pkg, ObjRef::Export(mi)).unwrap_or_default();
                    let mut tex = |r: Option<ObjRef>| -> Option<usize> {
                        let ObjRef::Export(ti) = r? else { return None };
                        *tex_ids.entry((pkg_name.to_string(), ti)).or_insert_with(|| match read_texture(content, &pkg, ti, max_tex) {
                            Ok(t) => {
                                model.textures.push(t);
                                Some(model.textures.len() - 1)
                            }
                            Err(_) => None,
                        })
                    };
                    let diffuse = tex(res.diffuse);
                    let normal = tex(res.normal);
                    model.materials.push(MaterialAsset { source: pkg.obj_path(ObjRef::Export(mi)), diffuse, normal, blend: if res.masked { BlendHint::Masked } else { BlendHint::Opaque }, texture_params: Vec::new() });
                    Some(model.materials.len() - 1)
                }),
                _ => None,
            };
            sections.push(MeshSection { first_index: s.first_index, num_triangles: s.num_triangles, material, enable_collision: false });
        }
        model.parts.push(SkeletalMeshAsset {
            source: part.to_string(),
            positions: raw.positions,
            normals: raw.normals,
            uv0: raw.uvs,
            joints: raw.joints,
            weights: raw.weights,
            indices: raw.indices,
            sections,
            bones: raw.bones.iter().map(|b| BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect(),
            mesh_to_pawn: m2p,
        });
    }
    // Face +X in pawn space: the toes point along mesh +Z.
    if let Some(p0) = model.parts.first() {
        let f = p0.mesh_to_pawn.transform_vector3(Vec3::Z);
        let yaw = -f.y.atan2(f.x);
        let fix = Mat4::from_rotation_z(yaw);
        for p in model.parts.iter_mut() {
            p.mesh_to_pawn = fix * p.mesh_to_pawn;
        }
    }
    // Stand the model on the cylinder's bottom (presentation only).
    let lift = Mat4::from_translation(Vec3::Z * (-pawn_half_height - feet_z));
    for p in model.parts.iter_mut() {
        p.mesh_to_pawn = lift * p.mesh_to_pawn;
    }
    Ok(model)
}

/// Global (mesh-space) bind matrices from local bones (quaternions used as stored; this
/// convention places the toes inside the mesh bounds, the conjugate does not).
pub fn bind_globals(bones: &[BoneAsset]) -> Vec<Mat4> {
    let mut g: Vec<Mat4> = Vec::with_capacity(bones.len());
    for (i, b) in bones.iter().enumerate() {
        let local = Mat4::from_rotation_translation(b.rotation, b.translation);
        if i == 0 || b.parent < 0 || b.parent as usize >= i {
            g.push(local);
        } else {
            g.push(g[b.parent as usize] * local);
        }
    }
    g
}

/// Decoded locomotion clips plus their track (bone name) order.
pub struct AnimLibrary {
    pub tracks: Vec<String>,
    pub clips: HashMap<String, crate::edge_anim::BakedClip>,
}

/// Load and bake the named sequences of an AnimSet.
pub fn load_anim_set(content: &Content, pkg_name: &str, set_path: &str, wanted: &[&str]) -> Result<AnimLibrary> {
    use ue3_package::properties::{Value, find};
    use ue3_package::reader::Reader;
    let pkg = content.package(pkg_name)?;
    let set = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(set_path)).with_context(|| format!("{set_path} not in {pkg_name}"))?;
    let sh = pkg.read_object(set)?;
    let (n, raw) = find(&sh.properties, "TrackBoneNames").and_then(|p| p.array_raw()).context("TrackBoneNames")?;
    let mut r = Reader::new(raw);
    let tracks: Vec<String> = (0..n).map(|_| ue3_package::FName::read(&mut r).map(|f| pkg.name(f)).unwrap_or_default()).collect();
    let mut clips = HashMap::new();
    for s in find(&sh.properties, "Sequences").context("Sequences")?.array_objects()? {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let h = pkg.read_object(e)?;
        let name = match find(&h.properties, "SequenceName").map(|p| &p.value) {
            Some(Value::Name(n)) => n.clone(),
            _ => continue,
        };
        if !wanted.is_empty() && !wanted.iter().any(|w| w.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let data = pkg.export_data(e)?;
        let mut r = Reader::at(data, h.native_offset);
        let _raw_tracks = r.i32()?;
        let len = r.count(1)?;
        let clip = crate::edge_anim::EdgeClip::parse(r.bytes(len)?)?;
        match clip.bake() {
            Ok(b) => {
                clips.insert(name, b);
            }
            Err(e) => eprintln!("skipping {name}: {e:#}"),
        }
    }
    Ok(AnimLibrary { tracks, clips })
}
