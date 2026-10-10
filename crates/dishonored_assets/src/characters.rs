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

/// The Boyle party pawn tweak each built-in archetype stands for (same index as
/// [`ARCHETYPES`]). Its interactable subobject's localized `m_Name` is the pawn's display name
/// (`Twk_Pawn_Boyle_Party.int`: "Party Guest", "Watch Officer").
pub const ARCHETYPE_TWEAKS: &[&str] = &[
    "Twk_Pawn_Boyle_Party.Pwn_AristoParty_MTall_1Pwn_AristoParty_MTall_1",
    "Twk_Pawn_Boyle_Party.Pwn_EliteGuard_MTall_0_Party",
    "Twk_Pawn_Boyle_Party.Pwn_AristoParty_MTall_3Pwn_AristoParty_MTall_3",
    "Twk_Pawn_Boyle_Party.Twk_Pawn_Boyle_AristoF1",
];

/// The AnimSets of each built-in archetype (same index as [`ARCHETYPES`]: the elite guard
/// set list for archetype 1, the aristocrats' for 0 and 2, the female civilian's for 3; later
/// sets win a sequence name) and their locomotion configs, as the runtime's animation bank and
/// the AI's head bake read them.
pub const GUARD_SETS: &[&str] = &[
    "Npc_EliteGuard_Generic_as.Npc_EliteGuard_Generic_as",
    "Npc_EliteGuard_Tracking_as.Npc_EliteGuard_HeadTracking_as",
    "Npc_EliteGuard_Tracking_as.Npc_EliteGuard_TorsoTracking_as",
    "Npc_EliteGuard_empty_as.Npc_EliteGuard_empty_as",
    "Npc_EliteGuard_Sword_as.Npc_EliteGuard_Sword_as",
    "Npc_EliteGuard_Face_as.Npc_EliteGuard_Face_as",
    "Npc_EliteGuard_Gun_as.Npc_EliteGuard_Gun_as",
    "Npc_EliteGuard_Corpses_as.Npc_EliteGuard_Corpses_as",
    "Npc_EliteGuard_Distractions_as.Npc_EliteGuard_Distractions_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_Gestures_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesLeftArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesNoArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesRightArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_Postures_as",
    "Npc_EliteGuard_Blink_as.Npc_EliteGuard_Blink_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesHead_as",
];
/// `Pwn_AristoParty_*` (Twk_Pawn_Boyle_Party): the guard sets without sword and gun, then the
/// tall-civilian and aristocrat empty sets.
pub const ARISTO_SETS: &[&str] = &[
    "Npc_EliteGuard_Generic_as.Npc_EliteGuard_Generic_as",
    "Npc_EliteGuard_Tracking_as.Npc_EliteGuard_HeadTracking_as",
    "Npc_EliteGuard_Tracking_as.Npc_EliteGuard_TorsoTracking_as",
    "Npc_EliteGuard_empty_as.Npc_EliteGuard_empty_as",
    "Npc_EliteGuard_Face_as.Npc_EliteGuard_Face_as",
    "Npc_CivTall_Empty_as.Npc_CivTall_Empty_as",
    "Npc_EliteGuard_Corpses_as.Npc_EliteGuard_Corpses_as",
    "Npc_EliteGuard_Distractions_as.Npc_EliteGuard_Distractions_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_Gestures_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesLeftArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesNoArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesRightArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_Postures_as",
    "Npc_EliteGuard_Blink_as.Npc_EliteGuard_Blink_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesHead_as",
    "Npc_Aristo_Empty_as.Npc_Aristo_Empty_as",
];
/// `Twk_Pawn_Boyle_AristoF*`: the civilian female sets.
pub const FEMALE_SETS: &[&str] = &[
    "Npc_CivFemale_Generic_as.Npc_CivFemale_Generic_as",
    "Npc_CivFemale_Tracking_as.Npc_CivFemale_HeadTracking_as",
    "Npc_CivFemale_Tracking_as.Npc_CivFemale_TorsoTracking_as",
    "Npc_CivFemale_Empty_as.Npc_CivFemale_Empty_as",
    "Npc_CivFemale_Corpses_as.Npc_CivFemale_Corpses_as",
    "Npc_CivFemale_Distractions_as.Npc_CivFemale_Distraction_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_Gestures_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_GesturesLeftArm_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_GesturesNoArm_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_GesturesRightArm_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_Postures_as",
    "Npc_Female_Blink_as.Npc_Female_Blink_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_GesturesHead_as",
    "Npc_CivFemale_Face_as.Npc_CivFemale_Face_as",
];
pub const GUARD_LOCO: &str = "DisLocomotionConfig.EliteGuardLocomotionConfig";
pub const FEMALE_LOCO: &str = "DisLocomotionConfig.FemaleLocomotionConfig";

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
                Some(mi) => *mat_ids.entry((mpkg_name.clone(), mi)).or_insert_with(|| push_material(content, &mpkg, mi, max_tex, &mut model, &mut tex_ids)),
                _ => None,
            };
            sections.push(MeshSection { first_index: s.first_index, num_triangles: s.num_triangles, material, enable_collision: false, material_index: s.material_index.max(0) as u32 });
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

/// Resolve one character material (instance chain into DishonoredGame / Startup for the
/// specular and emissive slots, scalars and blend state, as for level materials), adding it and
/// its textures to `model`. Returns its index.
fn push_material(content: &Content, pkg: &std::sync::Arc<ue3_package::Package>, mi: usize, max_tex: u32, model: &mut CharacterModel, tex_ids: &mut HashMap<(String, usize), Option<usize>>) -> Option<usize> {
    let res = resolve_material(pkg, ObjRef::Export(mi)).unwrap_or_default();
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
    // Static switches of the compiled permutation gate the Arkane slots, as for level materials
    // (`level.rs`): a slot whose `*_Enable_*` switch is off is not sampled by the original's
    // shader even when the instance still names a texture for it. E.g. the doors'
    // `Doors.doors_poor_interior_01_inst` keeps `E_Emissive_texture = obsolete_door_d` (an
    // editor placeholder reading "I'M OBSOLETE, REPLACE ME BY DISTWEAKS_DOOR ...") with
    // `E_Enable_Emissive` false: drawn as emissive, the text showed on every such door.
    let off = |s: &str| chain.switch(s) == Some(false);
    let diffuse = tex_key(crate::material_chain::slot(&chain, "d_", &["mask"]).or(chain.diffuse_input)).or_else(|| tex_key(local(res.diffuse)));
    let normal = if off("N_Enable_NormalMap") { None } else { tex_key(crate::material_chain::slot(&chain, "n_", &[]).or(chain.normal_input)).or_else(|| tex_key(local(res.normal))) };
    let specular = if off("S_Enable_Specular") { None } else { tex_key(crate::material_chain::slot(&chain, "s_", &["mask"])) };
    let emissive = if off("E_Enable_Emissive") { None } else { tex_key(crate::material_chain::slot(&chain, "e_", &["mask"])) };
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
    model.materials.push(MaterialAsset { source: pkg.obj_path(ObjRef::Export(mi)), diffuse, normal, blend: if masked { BlendHint::Masked } else { BlendHint::Opaque }, texture_params: Vec::new(), contact_type: None });
    Some(model.materials.len() - 1)
}

/// A `StaticMesh` (accessories: hats, masks) as a one-bone model in its own mesh space, so it
/// renders through the same skinned path as weapons and follows a socket.
pub fn load_static_model(content: &Content, pkg_name: &str, path: &str, max_tex: u32) -> Result<CharacterModel> {
    let pkg = content.package(pkg_name)?;
    let idx = (0..pkg.exports.len())
        .find(|&i| pkg.export_class(i) == "StaticMesh" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path))
        .with_context(|| format!("{path} not in {pkg_name}"))?;
    let raw = crate::static_mesh::read_static_mesh(&pkg, idx)?;
    let mut model = CharacterModel { name: path.to_string(), parts: Vec::new(), materials: Vec::new(), material_params: Vec::new(), textures: Vec::new() };
    let mut tex_ids = HashMap::new();
    let mut mat_ids: HashMap<usize, Option<usize>> = HashMap::new();
    let mut sections = Vec::new();
    for (si, s) in raw.sections.iter().enumerate() {
        let material = match ObjRef::from_index(s.material) {
            ObjRef::Export(mi) => *mat_ids.entry(mi).or_insert_with(|| push_material(content, &pkg, mi, max_tex, &mut model, &mut tex_ids)),
            _ => None,
        };
        sections.push(MeshSection { first_index: s.first_index, num_triangles: s.num_triangles, material, enable_collision: false, material_index: si as u32 });
    }
    let n = raw.positions.len();
    model.parts.push(SkeletalMeshAsset {
        source: path.to_string(),
        positions: raw.positions,
        normals: raw.normals,
        uv0: raw.uvs.first().cloned().unwrap_or_else(|| vec![glam::Vec2::ZERO; n]),
        joints: vec![[0; 4]; n],
        weights: vec![[1.0, 0.0, 0.0, 0.0]; n],
        indices: raw.indices.iter().map(|i| *i as u32).collect(),
        sections,
        bones: vec![BoneAsset { name: "root".into(), parent: -1, rotation: glam::Quat::IDENTITY, translation: Vec3::ZERO }],
        mesh_to_pawn: Mat4::IDENTITY,
    });
    Ok(model)
}

/// A `SkeletalMeshSocket` of a skeletal mesh: bone, offset and rotation relative to the bone.
#[derive(Debug, Clone)]
pub struct MeshSocket {
    pub name: String,
    pub bone: String,
    pub location: Vec3,
    pub rotation: [i32; 3],
}

/// The `Sockets` of a skeletal mesh (package, object path).
pub fn mesh_sockets(content: &Content, pkg_name: &str, path: &str) -> Result<Vec<MeshSocket>> {
    use ue3_package::properties::{Value, find};
    let pkg = content.package(pkg_name)?;
    let idx = (0..pkg.exports.len())
        .find(|&i| pkg.export_class(i) == "SkeletalMesh" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path))
        .with_context(|| format!("{path} not in {pkg_name}"))?;
    let h = pkg.read_object(idx)?;
    let mut out = Vec::new();
    for s in find(&h.properties, "Sockets").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let Ok(sh) = pkg.read_object(e) else { continue };
        let name = |n: &str| match find(&sh.properties, n).map(|p| &p.value) {
            Some(Value::Name(s)) => s.clone(),
            _ => String::new(),
        };
        let location = find(&sh.properties, "RelativeLocation").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
        let rotation = find(&sh.properties, "RelativeRotation").and_then(|p| p.as_rotator()).unwrap_or([0; 3]);
        out.push(MeshSocket { name: name("SocketName"), bone: name("BoneName"), location, rotation });
    }
    Ok(out)
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

/// An `AnimNotify_PlayParticleEffect` of a sequence: the system played at `time` on a socket
/// or bone of the mesh, attached (follows it) or not.
#[derive(Debug, Clone)]
pub struct ParticleNotify {
    pub sequence: String,
    pub time: f32,
    pub template: String,
    pub socket: String,
    pub bone: String,
    pub attach: bool,
}

/// The particle notifies of an AnimSet's sequences.
pub fn particle_notifies(content: &Content, pkg_name: &str, set_path: &str) -> Result<Vec<ParticleNotify>> {
    use ue3_package::properties::{Value, find};
    let pkg = content.package(pkg_name)?;
    let set = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(set_path)).with_context(|| format!("{set_path} not in {pkg_name}"))?;
    let sh = pkg.read_object(set)?;
    let mut out = Vec::new();
    for s in find(&sh.properties, "Sequences").context("Sequences")?.array_objects()? {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let h = pkg.read_object(e)?;
        let Some(Value::Name(sequence)) = find(&h.properties, "SequenceName").map(|p| &p.value) else { continue };
        let Some(n) = find(&h.properties, "Notifies") else { continue };
        for ev in n.array_structs(&pkg).unwrap_or_default() {
            let time = find(&ev, "Time").and_then(|p| p.as_float()).unwrap_or(0.0);
            let Some(ObjRef::Export(ni)) = find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
            if pkg.export_class(ni) != "AnimNotify_PlayParticleEffect" {
                continue;
            }
            let props = pkg.read_object(ni)?.properties;
            let name = |k: &str| match find(&props, k).map(|p| &p.value) {
                Some(Value::Name(v)) if v != "None" => v.clone(),
                _ => String::new(),
            };
            let template = match find(&props, "PSTemplate").and_then(|p| p.as_object()).map(ObjRef::from_index) {
                Some(r @ (ObjRef::Export(_) | ObjRef::Import(_))) => pkg.obj_path(r),
                _ => continue,
            };
            let attach = matches!(find(&props, "bAttach").map(|p| &p.value), Some(Value::Bool(true)));
            out.push(ParticleNotify { sequence: sequence.clone(), time, template, socket: name("SocketName"), bone: name("BoneName"), attach });
        }
    }
    Ok(out)
}

/// The `AnimNotify_CameraEffect`s of an AnimSet's sequences: (sequence, time, lens effect class
/// name, e.g. `DisEmitterCameraLensEffect_blood_medium`).
pub fn camera_effect_notifies(content: &Content, pkg_name: &str, set_path: &str) -> Result<Vec<(String, f32, String)>> {
    use ue3_package::properties::{Value, find};
    let pkg = content.package(pkg_name)?;
    let set = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(set_path)).with_context(|| format!("{set_path} not in {pkg_name}"))?;
    let sh = pkg.read_object(set)?;
    let mut out = Vec::new();
    for s in find(&sh.properties, "Sequences").context("Sequences")?.array_objects()? {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let h = pkg.read_object(e)?;
        let Some(Value::Name(sequence)) = find(&h.properties, "SequenceName").map(|p| &p.value) else { continue };
        let Some(n) = find(&h.properties, "Notifies") else { continue };
        for ev in n.array_structs(&pkg).unwrap_or_default() {
            let time = find(&ev, "Time").and_then(|p| p.as_float()).unwrap_or(0.0);
            let Some(ObjRef::Export(ni)) = find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
            if pkg.export_class(ni) != "AnimNotify_CameraEffect" {
                continue;
            }
            let props = pkg.read_object(ni)?.properties;
            let Some(r @ (ObjRef::Export(_) | ObjRef::Import(_))) = find(&props, "CameraLensEffect").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
            out.push((sequence.clone(), time, pkg.obj_name(r)));
        }
    }
    Ok(out)
}

/// The `DisNotify_Trails` windows of an AnimSet's sequences: (sequence, time, duration).
pub fn trail_notifies(content: &Content, pkg_name: &str, set_path: &str) -> Result<Vec<(String, f32, f32)>> {
    use ue3_package::properties::{Value, find};
    let pkg = content.package(pkg_name)?;
    let set = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(set_path)).with_context(|| format!("{set_path} not in {pkg_name}"))?;
    let sh = pkg.read_object(set)?;
    let mut out = Vec::new();
    for s in find(&sh.properties, "Sequences").context("Sequences")?.array_objects()? {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let h = pkg.read_object(e)?;
        let Some(Value::Name(sequence)) = find(&h.properties, "SequenceName").map(|p| &p.value) else { continue };
        let Some(n) = find(&h.properties, "Notifies") else { continue };
        for ev in n.array_structs(&pkg).unwrap_or_default() {
            let Some(ObjRef::Export(ni)) = find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
            if pkg.export_class(ni) != "DisNotify_Trails" {
                continue;
            }
            let time = find(&ev, "Time").and_then(|p| p.as_float()).unwrap_or(0.0);
            let duration = find(&ev, "Duration").and_then(|p| p.as_float()).unwrap_or(0.0);
            out.push((sequence.clone(), time, duration));
        }
    }
    Ok(out)
}

/// A camera lens effect class (`EmitterCameraLensEffectBase` subclass) as played by a notify:
/// `Default__<class>.PS_CameraEffect` (DishonoredGame), `DistFromCamera` / `BaseFOV` / `LifeSpan`
/// from the class or, when it does not set them, `Default__EmitterCameraLensEffectBase` (Engine:
/// 90, 80, 10 s).
pub fn lens_effect_class(content: &Content, class: &str) -> Option<dishonored_ir::LensEffectDef> {
    use ue3_package::properties::find;
    let defaults = |pkg_name: &str, name: &str| -> Option<(std::sync::Arc<ue3_package::Package>, Vec<ue3_package::properties::Property>)> {
        let pkg = content.package(pkg_name).ok()?;
        let want = format!("Default__{name}");
        let i = (0..pkg.exports.len()).find(|&e| pkg.obj_name(ObjRef::Export(e)) == want)?;
        let props = pkg.read_object(i).ok()?.properties;
        Some((pkg, props))
    };
    let (game, d) = defaults("DishonoredGame", class)?;
    let base = defaults("Engine", "EmitterCameraLensEffectBase").map(|b| b.1).unwrap_or_default();
    let system = find(&d, "PS_CameraEffect").and_then(|p| p.as_object()).map(ObjRef::from_index).filter(|r| *r != ObjRef::Null).map(|r| game.obj_path(r))?;
    let f = |n: &str, dflt: f32| find(&d, n).or_else(|| find(&base, n)).and_then(|p| p.as_float()).unwrap_or(dflt);
    Some(dishonored_ir::LensEffectDef { action: class.to_string(), system, distance: f("DistFromCamera", 90.0), base_fov: f("BaseFOV", 80.0), life_span: f("LifeSpan", 10.0) })
}

/// Object path of the `AnimSet` export of a package whose own name (the last path component)
/// is `name`, for sets whose outer is not known (the weapons' sets).
pub fn anim_set_path(content: &Content, pkg_name: &str, name: &str) -> Result<String> {
    export_path(content, pkg_name, "AnimSet", name)
}

/// Object path of the export of class `class` whose own name (the last path component) is
/// `name` (the first such export).
pub fn export_path(content: &Content, pkg_name: &str, class: &str, name: &str) -> Result<String> {
    let pkg = content.package(pkg_name)?;
    (0..pkg.exports.len())
        .filter(|&i| pkg.export_class(i) == class)
        .map(|i| pkg.obj_path(ObjRef::Export(i)))
        .find(|p| p.rsplit('.').next().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .with_context(|| format!("{class} {name} not in {pkg_name}"))
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
    // Where each SkeletalMesh / AnimSet / locomotion config is cooked (first package wins).
    let mut meshes: HashMap<String, String> = HashMap::new();
    for p in level_packages {
        let Ok(pkg) = content.package(p) else { continue };
        for i in 0..pkg.exports.len() {
            if matches!(pkg.export_class(i).as_str(), "SkeletalMesh" | "AnimSet" | "ArkComponentLocomotionConfig" | "PhysicsAsset" | "StaticMesh") {
                meshes.entry(pkg.obj_path(ObjRef::Export(i)).to_ascii_lowercase()).or_insert_with(|| p.clone());
            }
        }
    }
    let locate = |path: &str| meshes.get(&path.to_ascii_lowercase()).map(|p| (p.clone(), path.to_string()));
    // `DisMeshMaterialVariationList`: per material slot, the (material, chance) options.
    let variations = |v: Option<&TVal>| -> Vec<Vec<(String, f32)>> {
        let Some(TVal::Struct(s)) = v else { return Vec::new() };
        let Some(TVal::List(slots)) = s.get("m_VariationMaterialIndex") else { return Vec::new() };
        slots
            .iter()
            .map(|slot| match slot.get("m_MaterialVariation") {
                Some(TVal::List(opts)) => opts
                    .iter()
                    .filter_map(|o| match (o.get("m_pMaterial"), o.get("m_fRandomChance")) {
                        (Some(TVal::Obj(m)), c) if !m.is_empty() => Some((m.clone(), if let Some(TVal::F(f)) = c { *f } else { 0.0 })),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect()
    };
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
                        a.head_variations.push(variations(h.get("m_MaterialVariation")));
                    }
                }
            }
        }
        a.body_variations = variations(m.get("m_BodyVariationMaterials"));
        if let Some(TVal::List(sets)) = m.get("m_AnimSets") {
            a.anim.sets = sets.iter().filter_map(|s| match s.get("obj") {
                Some(TVal::Obj(o)) => locate(o),
                _ => None,
            }).collect();
        }
        if let Some(TVal::Obj(o)) = m.get("m_pComponentLocomotionConfig") {
            a.anim.locomotion = locate(o);
        }
        if let Some(TVal::Obj(o)) = m.get("m_pAnimDefClass") {
            a.anim.defs = o.rsplit('.').next().unwrap_or("").to_string();
        }
        a.anim.ragdoll_on_death = matches!(m.get("m_bRagdollOnDeath"), Some(TVal::B(true)));
        if let Some(TVal::Obj(o)) = m.get("m_pPhysicsAsset") {
            a.anim.physics = locate(o);
        }
        if let Some(TVal::Obj(o)) = m.get("m_pSeveredLimbInfo") {
            a.anim.severed_limbs = locate(o);
        }
        // Look-at configs are cooked into DishonoredGame (`DisLookatConfig.*`).
        if let Some(TVal::Obj(o)) = m.get("m_pComponentLookatConfig") {
            a.anim.lookat = locate(o).or_else(|| Some(("DishonoredGame".to_string(), o.clone())));
        }
        // The item prefixes of item-prefixed chain steps: the primary item's (the loadout's first
        // entry: the elite guard's sword, the Overseer's music box, the hound's bite, the
        // Tallboy's bow) and the empty hands' `m_ItemPrefix`.
        let item_prefix = |path: Option<&str>| -> String {
            match path.and_then(|p| db.try_resolve(p)).as_ref().and_then(|t| t.get("m_ItemPrefix").cloned()) {
                Some(TVal::Name(n)) | Some(TVal::Str(n)) if n != "None" => n,
                _ => String::new(),
            }
        };
        let first_item = match m.get("m_ContentInventoryLoadout") {
            Some(TVal::List(l)) => l.first().and_then(|e| match e.get("obj") {
                Some(TVal::Obj(o)) if !o.is_empty() => Some(o.clone()),
                _ => None,
            }),
            _ => None,
        };
        a.anim.item_prefix = item_prefix(first_item.as_deref());
        let empty = match m.get("m_pEmptyHandsItem") {
            Some(TVal::Obj(o)) if !o.is_empty() => Some(o.clone()),
            _ => None,
        };
        a.anim.empty_prefix = item_prefix(empty.as_deref());
        // Accessories: `m_RandomAccessoryInfo[Hat, Mask]` with `m_HatMeshes` / `m_MaskMeshes`.
        // A static array: element 0 under the plain name, element i under "name[i]".
        let info: Vec<f32> = ["m_RandomAccessoryInfo", "m_RandomAccessoryInfo[1]"]
            .iter()
            .map(|k| match m.get(*k) {
                Some(TVal::Struct(s)) => match s.get("m_fRandomChanceOfUse") {
                    Some(TVal::F(f)) => *f,
                    _ => 0.0,
                },
                _ => 0.0,
            })
            .collect();
        for (i, list) in ["m_HatMeshes", "m_MaskMeshes"].iter().enumerate() {
            let Some(TVal::List(l)) = m.get(*list) else { continue };
            let meshes: Vec<dishonored_ir::spawners::AccessoryMesh> = l
                .iter()
                .filter_map(|e| {
                    let Some(TVal::Obj(o)) = e.get("m_pMesh") else { return None };
                    let socket = match e.get("m_SocketName") {
                        Some(TVal::Name(n)) | Some(TVal::Str(n)) => n.clone(),
                        _ => String::new(),
                    };
                    let chance = match e.get("m_fRandomChance") {
                        Some(TVal::F(f)) => *f,
                        _ => 0.0,
                    };
                    Some(dishonored_ir::spawners::AccessoryMesh { mesh: locate(o)?, socket, chance })
                })
                .collect();
            if !meshes.is_empty() {
                while a.accessories.len() <= i {
                    a.accessories.push(Default::default());
                }
                a.accessories[i] = dishonored_ir::spawners::AccessorySlot { chance_of_use: info.get(i).copied().unwrap_or(0.0), meshes, helmet: false };
            }
        }
        // Armour (`m_PotentialNPCArmor`: `m_fPercentChance`, `DisTweaks_Armor`): the armour's
        // static mesh (its dropped-item movable tweak's `m_pStaticMesh`) on
        // `m_EquippedSocketName` (elite guard helmets on the "Helmet" socket).
        if let Some(TVal::List(armor)) = m.get("m_PotentialNPCArmor") {
            for e in armor {
                let chance = match e.get("m_fPercentChance") {
                    Some(TVal::F(f)) => *f,
                    _ => 0.0,
                };
                let Some(TVal::Obj(t)) = e.get("m_pArmor") else { continue };
                let Some(at) = db.try_resolve(t) else { continue };
                let socket = match at.get("m_EquippedSocketName") {
                    Some(TVal::Name(n)) | Some(TVal::Str(n)) => n.clone(),
                    _ => continue,
                };
                let mesh = match at.get("m_pDoppedItemTweaks") {
                    Some(TVal::Obj(mt)) => db.try_resolve(mt).and_then(|mv| match mv.get("m_pStaticMesh") {
                        Some(TVal::Obj(sm)) => locate(sm),
                        _ => None,
                    }),
                    _ => None,
                };
                // A head armour that hits can knock off (`m_ProtectedBodyRegion` DHR_HEAD with a
                // `m_DamageTypesThatKnockOff` list): the simulation's `PlayerState::helmet`.
                let helmet = matches!(at.get("m_ProtectedBodyRegion"), Some(TVal::Name(r)) if r == "DHR_HEAD") && matches!(at.get("m_DamageTypesThatKnockOff"), Some(TVal::List(l)) if !l.is_empty());
                let meshes = mesh.map(|mesh| vec![dishonored_ir::spawners::AccessoryMesh { mesh, socket, chance: 1.0 }]).unwrap_or_default();
                if !meshes.is_empty() || helmet {
                    a.accessories.push(dishonored_ir::spawners::AccessorySlot { chance_of_use: chance, meshes, helmet });
                }
            }
        }
        out.insert(t.clone(), a);
    }
    Ok(out)
}
