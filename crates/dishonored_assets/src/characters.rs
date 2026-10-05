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
pub use dishonored_ir::spawners::Appearance;

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
    load_model(content, name, parts, Some(pawn_half_height), max_tex)
}

/// Load skeletal mesh parts sharing the first part's mesh->pawn transform. With
/// `pawn_half_height`, the model is turned to face +X and stood on the cylinder bottom
/// (pawn bodies); without it, mesh->pawn is just R(RotOrigin)*(v+MeshOrigin) (first-person
/// rig, weapons).
pub fn load_model(content: &Content, name: &str, parts: &[(&str, &str)], pawn_half_height: Option<f32>, max_tex: u32) -> Result<CharacterModel> {
    load_model_with(content, name, parts, &[], pawn_half_height, max_tex)
}

/// A material replacing one of a part's materials (`DisMaterialSetting`): part index, material
/// index, then (package, object path) of the material.
pub type MaterialOverride = (usize, usize, String, String);

/// [`load_model`] with material overrides (`DisSeqAct_NPCSetMaterials`, tweak variations).
pub fn load_model_with(content: &Content, name: &str, parts: &[(&str, &str)], overrides: &[MaterialOverride], pawn_half_height: Option<f32>, max_tex: u32) -> Result<CharacterModel> {
    let mut shared_m2p: Option<Mat4> = None;
    let mut model = CharacterModel { name: name.to_string(), parts: Vec::new(), materials: Vec::new(), material_params: Vec::new(), textures: Vec::new() };
    let mut mat_ids: HashMap<(String, usize), Option<usize>> = HashMap::new();
    let mut tex_ids: HashMap<(String, usize), Option<usize>> = HashMap::new();
    let mut feet_z = f32::MAX;
    for (part_i, (pkg_name, part)) in parts.iter().enumerate() {
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
            // An override material may live in another package than the mesh.
            let ovr = overrides.iter().find(|o| o.0 == part_i && o.1 == s.material_index.max(0) as usize).and_then(|o| {
                let p = content.package(&o.2).ok()?;
                let i = (0..p.exports.len()).find(|&i| p.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&o.3))?;
                Some((o.2.clone(), p, i))
            });
            let (mpkg_name, mpkg, target) = match ovr {
                Some((n, p, i)) => (n, p, Some(i)),
                None => (pkg_name.to_string(), pkg.clone(), match ObjRef::from_index(mref) {
                    ObjRef::Export(mi) => Some(mi),
                    _ => None,
                }),
            };
            let material = match target {
                Some(mi) => *mat_ids.entry((mpkg_name.clone(), mi)).or_insert_with(|| {
                    let pkg = &mpkg;
                    let res = resolve_material(pkg, ObjRef::Export(mi)).unwrap_or_default();
                    // Full Arkane chain (instance -> parents in DishonoredGame/Startup) for the
                    // specular/emissive slots, scalars and blend state, as for level materials.
                    let (pkgs, chain) = character_chain(content, pkg, mi);
                    let mut tex_key = |k: Option<(usize, usize)>| -> Option<usize> {
                        let (slot, ti) = k?;
                        let p = pkgs.get(slot)?.clone();
                        *tex_ids.entry((p.name.clone(), ti)).or_insert_with(|| match read_texture(content, &p, ti, max_tex) {
                            Ok(t) => {
                                model.textures.push(t);
                                Some(model.textures.len() - 1)
                            }
                            Err(_) => None,
                        })
                    };
                    let local = |r: Option<ObjRef>| match r {
                        Some(ObjRef::Export(ti)) => Some((0usize, ti)),
                        _ => None,
                    };
                    let diffuse = tex_key(crate::material_chain::slot(&chain, "d_", &["mask"]).or(chain.diffuse_input)).or_else(|| tex_key(local(res.diffuse)));
                    let normal = tex_key(crate::material_chain::slot(&chain, "n_", &[]).or(chain.normal_input)).or_else(|| tex_key(local(res.normal)));
                    let specular = tex_key(crate::material_chain::slot(&chain, "s_", &["mask"]));
                    let emissive = tex_key(crate::material_chain::slot(&chain, "e_", &["mask"]));
                    let masked = res.masked || chain.blend_mode.as_deref() == Some("BLEND_Masked");
                    let blend = match chain.blend_mode.as_deref() {
                        Some("BLEND_Translucent") | Some("BLEND_AlphaComposite") => MaterialBlend::Translucent,
                        Some("BLEND_Additive") => MaterialBlend::Additive,
                        _ if masked => MaterialBlend::Masked,
                        _ => MaterialBlend::Opaque,
                    };
                    model.material_params.push(MaterialParams {
                        specular,
                        emissive,
                        blend,
                        blend_known: chain.base_material.is_some(),
                        two_sided: chain.two_sided.unwrap_or(false),
                        unlit: chain.lighting_model.as_deref() == Some("MLM_Unlit"),
                        opacity_clip: chain.opacity_clip.unwrap_or(0.3333),
                        scalars: chain.scalars.clone(),
                        vectors: chain.vectors.clone(),
                        base_material: chain.base_material.clone(),
                        ..Default::default()
                    });
                    model.materials.push(MaterialAsset { source: pkg.obj_path(ObjRef::Export(mi)), diffuse, normal, blend: if masked { BlendHint::Masked } else { BlendHint::Opaque }, texture_params: Vec::new() });
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
    let Some(pawn_half_height) = pawn_half_height else { return Ok(model) };
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

impl AnimLibrary {
    /// Put `tracks` in Edge joint order: Edge clip joint `j` is the set's `j`-th track in
    /// the bone order of the skeleton the set was compressed against, not the order of
    /// `TrackBoneNames` (verified on Ply_Sword_* vs Engine Ply_Player.Skm_Player: every
    /// animated translation then matches its bone's bind length). Tracks missing from the
    /// skeleton keep their relative order at the end.
    pub fn edge_order(&mut self, skeleton: &[BoneAsset]) {
        let idx = |n: &String| skeleton.iter().position(|b| b.name.eq_ignore_ascii_case(n)).unwrap_or(usize::MAX);
        self.tracks.sort_by_key(idx);
    }
}

/// Path -> (library package, export) for the shared material libraries that character
/// material instances parent into (built once per process).
fn library_index(content: &Content) -> &'static (Vec<std::sync::Arc<ue3_package::Package>>, HashMap<String, (usize, usize)>) {
    static INDEX: std::sync::OnceLock<(Vec<std::sync::Arc<ue3_package::Package>>, HashMap<String, (usize, usize)>)> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| {
        let libs: Vec<_> = ["DishonoredGame", "Startup"].iter().filter_map(|n| content.package(n).ok()).collect();
        let mut map = HashMap::new();
        for (li, p) in libs.iter().enumerate() {
            for i in 0..p.exports.len() {
                map.entry(p.obj_path(ObjRef::Export(i)).to_ascii_lowercase()).or_insert((li, i));
            }
        }
        (libs, map)
    })
}

/// Walk a character material's chain. Returns the package slots used by the chain's keys
/// (slot 0 = the character package, then the libraries) and the chain.
fn character_chain(content: &Content, pkg: &std::sync::Arc<ue3_package::Package>, material: usize) -> (Vec<std::sync::Arc<ue3_package::Package>>, crate::material_chain::MaterialChain) {
    let (libs, index) = library_index(content);
    let mut pkgs = vec![pkg.clone()];
    pkgs.extend(libs.iter().cloned());
    let chain = {
        let pk = pkgs.clone();
        crate::material_chain::walk(&pkgs, (0, material), &mut |slot, r| match r {
            ObjRef::Null => None,
            ObjRef::Export(i) => Some((slot, i)),
            ObjRef::Import(_) => index.get(&pk[slot].obj_path(r).to_ascii_lowercase()).map(|(li, i)| (li + 1, *i)),
        })
    };
    (pkgs, chain)
}

/// Resolve pawn tweaks to appearances. `level_packages` are the map's packages (the meshes and
/// the `Twk_Pawn_*` level tweaks are cooked into them).
pub fn resolve_appearances(content: &Content, level_packages: &[String], tweaks: &[String]) -> Result<HashMap<String, Appearance>> {
    use crate::tweaks::{TVal, TweakDb};
    let db = TweakDb::load_with(content, level_packages)?;
    // Where each SkeletalMesh is cooked (first package wins).
    let mut meshes: HashMap<String, String> = HashMap::new();
    for p in level_packages {
        let Ok(pkg) = content.package(p) else { continue };
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) == "SkeletalMesh" {
                meshes.entry(pkg.obj_path(ObjRef::Export(i)).to_ascii_lowercase()).or_insert_with(|| p.clone());
            }
        }
    }
    let locate = |path: &str| meshes.get(&path.to_ascii_lowercase()).map(|p| (p.clone(), path.to_string()));
    let mut out = HashMap::new();
    for t in tweaks {
        if out.contains_key(t) {
            continue;
        }
        let Some(m) = db.try_resolve(t) else { continue };
        let mut a = Appearance::default();
        if let Some(TVal::Obj(o)) = m.get("m_pSkeletalMesh") {
            a.body = locate(o);
        }
        if let Some(TVal::List(heads)) = m.get("m_RandomHeadMeshes") {
            for h in heads {
                let chance = match h.get("m_fRandomChance") {
                    Some(TVal::F(f)) => *f,
                    _ => 1.0,
                };
                if let Some(TVal::Obj(o)) = h.get("m_pMesh") {
                    if let Some(l) = locate(o) {
                        a.heads.push((l, chance));
                    }
                }
            }
        }
        out.insert(t.clone(), a);
    }
    Ok(out)
}
