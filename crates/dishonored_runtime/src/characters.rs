//! Skinned Dishonored character models for player bodies.
//!
//! Each part is uploaded once; every player gets a joint hierarchy (flat, children of the
//! body root) driven by decoded Edge animation selected the way the original does
//! (`crate::npc_anim`).

use crate::coords;
use crate::map_render::to_image;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use dishonored_assets::characters::{ARCHETYPES, bind_globals, load_character};
use dishonored_ir::CharacterModel;
use dishonored_sim::state::{MoveMode, PlayerState, SwordPhase};
use std::collections::HashMap;

#[derive(Clone)]
pub struct PartGpu {
    /// Skeletal mesh object path.
    pub source: String,
    pub sections: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    /// Index into `CharacterGpu::material_params` of each section's material.
    pub section_materials: Vec<Option<usize>>,
    /// Each section's material slot (`MeshSection::material_index`).
    pub section_slots: Vec<u32>,
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    /// Bind-pose joint transforms in Bevy space relative to the body root.
    pub bind: Vec<Mat4>,
    pub bones: Vec<dishonored_ir::BoneAsset>,
    pub mesh_to_pawn: glam::Mat4,
    /// Culling box relative to the body root: the bind-pose vertices grown by
    /// `ANIMATED_BOUNDS_PAD` (see `animated_bounds`).
    pub bounds: bevy::camera::primitives::Aabb,
}

/// Margin (uu) added on every side of a skinned part's bind-pose box. Bevy culls a skinned
/// mesh by the box of its bind-pose vertices, which does not follow the animated pose: a big
/// swing, a fall or a body drawn at its carrier's root would pop out at the screen edges.
const ANIMATED_BOUNDS_PAD: f32 = 250.0;

/// The culling box of a skinned part (Bevy space, relative to the body root), spawned with
/// `NoAutoAabb` so Bevy keeps it instead of recomputing the bind-pose box.
pub fn animated_bounds(pos: &[[f32; 3]]) -> bevy::camera::primitives::Aabb {
    let pad = Vec3::splat(ANIMATED_BOUNDS_PAD * coords::UU_TO_BEVY);
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in pos {
        lo = lo.min(Vec3::from_array(*p));
        hi = hi.max(Vec3::from_array(*p));
    }
    if pos.is_empty() {
        (lo, hi) = (Vec3::ZERO, Vec3::ZERO);
    }
    bevy::camera::primitives::Aabb::from_min_max(lo - pad, hi + pad)
}

#[derive(Clone)]
pub struct CharacterGpu {
    pub name: String,
    pub parts: Vec<PartGpu>,
    /// The model's textures and Arkane material parameters (`CharacterModel`), for materials
    /// built on top of the uploaded ones (the level props' light environments).
    pub textures: Vec<Handle<Image>>,
    pub material_params: Vec<dishonored_ir::MaterialParams>,
    /// The bind pose's bounding sphere in pawn space (uu): the centre of its box and the farthest
    /// vertex from it, a light environment's bounds (`char_light.rs`).
    pub sphere: (glam::Vec3, f32),
}

#[derive(Resource, Default)]
pub struct CharacterLibrary {
    pub models: Vec<CharacterGpu>,
    /// Material object path (lowercase) -> package cooking it, over the session's map packages.
    material_index: Option<HashMap<String, String>>,
    /// Entries of `MatchView::appearances` loaded so far (model `APPEARANCE_BASE + i`).
    pub looks: usize,
    /// Per model: hats and masks on sockets (`DisPawnAccessoryMesh`, U-144).
    pub accessories: HashMap<usize, Vec<Accessory>>,
    /// Static meshes loaded for accessories, by (package, path).
    accessory_meshes: HashMap<(String, String), Option<CharacterGpu>>,
    /// Per model: its skeletal meshes' sockets (weapons, accessories).
    pub sockets: HashMap<usize, Vec<ModelSocket>>,
}

/// A static mesh attached to a socket of a model's skeleton.
#[derive(Clone)]
pub struct Accessory {
    pub gpu: CharacterGpu,
    /// Part and bone of the socket.
    pub part: usize,
    pub bone: usize,
    /// Socket transform relative to the bone (`RelativeLocation`, `RelativeRotation`).
    pub local: glam::Mat4,
}

#[derive(Component)]
pub struct AccessoryJoint(pub usize);

/// A `SkeletalMeshSocket` of one of a model's parts, bound to the model's bone.
#[derive(Clone, Debug)]
pub struct ModelSocket {
    pub name: String,
    pub part: usize,
    pub bone: usize,
    /// Socket transform relative to the bone.
    pub local: glam::Mat4,
}

/// The sockets of every part of a model (parts given as (package, skeletal mesh path)).
fn model_sockets(content: &dishonored_assets::Content, parts: &[(String, String)], gpu: &CharacterGpu) -> Vec<ModelSocket> {
    let mut out = Vec::new();
    for (pi, (pkg, path)) in parts.iter().enumerate() {
        let Some(part) = gpu.parts.get(pi) else { continue };
        for s in dishonored_assets::characters::mesh_sockets(content, pkg, path).unwrap_or_default() {
            let Some(bone) = part.bones.iter().position(|b| b.name.eq_ignore_ascii_case(&s.bone)) else { continue };
            let local = glam::Mat4::from_translation(s.location) * dishonored_assets::util::rotator_matrix(s.rotation[0], s.rotation[1], s.rotation[2]);
            out.push(ModelSocket { name: s.name, part: pi, bone, local });
        }
    }
    out
}

/// C: Dishonored pawn space (Z-up, uu) -> Bevy (Y-up, scaled).
pub fn to_bevy(m: glam::Mat4) -> Mat4 {
    coords::transform(m)
}

pub fn upload(model: &CharacterModel, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>, binds: &mut Assets<SkinnedMeshInverseBindposes>, features: &crate::map_render::RenderFeatures) -> CharacterGpu {
    let tex: Vec<Handle<Image>> = model.textures.iter().map(|t| images.add(to_image(t))).collect();
    let materials: Vec<Handle<StandardMaterial>> = model
        .materials
        .iter()
        .enumerate()
        .map(|(i, m)| {
            // Same Arkane material mapping as the level (normal, specular, emissive, blend,
            // two-sidedness, specular power) when the chain was resolved.
            if let Some(p) = model.material_params.get(i).filter(|_| features.materials) {
                return mats.add(crate::map_render::build_material(i, m, p, &tex, features));
            }
            mats.add(StandardMaterial {
                base_color_texture: m.diffuse.map(|t| tex[t].clone()),
                base_color: if m.diffuse.is_some() { Color::WHITE } else { Color::srgb(0.5, 0.45, 0.4) },
                perceptual_roughness: 0.8,
                cull_mode: None,
                double_sided: true,
                ..default()
            })
        })
        .collect();
    let fallback = mats.add(StandardMaterial { base_color: Color::srgb(0.45, 0.4, 0.35), cull_mode: None, ..default() });
    let mut parts = Vec::new();
    for p in &model.parts {
        let m2p = p.mesh_to_pawn;
        let pos: Vec<[f32; 3]> = p.positions.iter().map(|v| coords::pos(m2p.transform_point3(*v)).to_array()).collect();
        let nrm: Vec<[f32; 3]> = p.normals.iter().map(|n| coords::dir(m2p.transform_vector3(*n)).normalize_or_zero().to_array()).collect();
        let uv: Vec<[f32; 2]> = p.uv0.iter().map(|u| u.to_array()).collect();
        let joints: Vec<[u16; 4]> = p.joints.clone();
        let weights: Vec<[f32; 4]> = p.weights.clone();
        let globals = bind_globals(&p.bones);
        let bind: Vec<Mat4> = globals.iter().map(|g| to_bevy(m2p * *g)).collect();
        let inverse: Vec<Mat4> = bind.iter().map(|b| b.inverse()).collect();
        let ib = binds.add(SkinnedMeshInverseBindposes::from(inverse));
        let mut sections = Vec::new();
        let mut section_materials = Vec::new();
        let section_slots: Vec<u32> = p.sections.iter().map(|s| s.material_index).collect();
        for s in &p.sections {
            let start = s.first_index as usize;
            let end = (start + s.num_triangles as usize * 3).min(p.indices.len());
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, nrm.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(joints.clone()))
                .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights.clone())
                .with_inserted_indices(Indices::U32(p.indices[start..end].to_vec()));
            if features.materials {
                let _ = mesh.generate_tangents();
            }
            let mat = s.material.map(|m| materials[m].clone()).unwrap_or_else(|| fallback.clone());
            sections.push((meshes.add(mesh), mat));
            section_materials.push(s.material);
        }
        let bounds = animated_bounds(&pos);
        parts.push(PartGpu { source: p.source.clone(), sections, section_materials, section_slots, inverse_bindposes: ib, bind, bones: p.bones.clone(), mesh_to_pawn: m2p, bounds });
    }
    let all: Vec<glam::Vec3> = model.parts.iter().flat_map(|p| p.positions.iter().map(|v| p.mesh_to_pawn.transform_point3(*v))).collect();
    let (lo, hi) = all.iter().fold((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
    let sphere = if all.is_empty() { (glam::Vec3::ZERO, 0.0) } else { let c = (lo + hi) * 0.5; (c, all.iter().map(|v| v.distance(c)).fold(0.0, f32::max)) };
    CharacterGpu { name: model.name.clone(), parts, textures: tex, material_params: model.material_params.clone(), sphere }
}

pub fn load_library(
    mut commands: Commands,
    features: Option<Res<crate::map_render::RenderFeatures>>,
    session: Res<crate::session::Session>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let feat = features.map(|f| f.clone()).unwrap_or_else(|| crate::map_render::RenderFeatures::from_flags(&|_| false));
    let mut lib = CharacterLibrary::default();
    let hh = session.content.sim.gameplay.movement.collision_half_height;
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    for (name, parts) in ARCHETYPES {
        match load_character(&content, name, hh, 1024) {
            Ok(m) => {
                let gpu = upload(&m, &mut meshes, &mut images, &mut mats, &mut binds, &feat);
                let owned: Vec<(String, String)> = parts.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
                let i = lib.models.len();
                lib.sockets.insert(i, model_sockets(&content, &owned, &gpu));
                lib.models.push(gpu);
            }
            Err(e) => warn!("character {name}: {e:#}"),
        }
    }
    info!("loaded {} character archetypes", lib.models.len());
    commands.insert_resource(lib);
}

#[derive(Component)]
pub struct Joint {
    pub part: usize,
    pub bone: usize,
}

#[derive(Component)]
pub struct CharacterRoot {
    pub archetype: usize,
}

/// Spawn a character under `root` (an existing entity that carries the pawn transform).
pub fn spawn_character(commands: &mut Commands, root: Entity, lib: &CharacterLibrary, archetype: usize) {
    let Some(model) = lib.models.get(archetype % lib.models.len().max(1)) else { return };
    commands.entity(root).insert(CharacterRoot { archetype: archetype % lib.models.len() });
    for (pi, part) in model.parts.iter().enumerate() {
        let joints: Vec<Entity> = part
            .bind
            .iter()
            .enumerate()
            .map(|(bi, b)| commands.spawn((Transform::from_matrix(*b), Joint { part: pi, bone: bi }, ChildOf(root))).id())
            .collect();
        for (mesh, mat) in &part.sections {
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: joints.clone() },
                Transform::default(),
                part.bounds,
                bevy::camera::visibility::NoAutoAabb,
                crate::ash_bodies::BodyPartMesh,
                ChildOf(root),
            ));
        }
    }
    // Accessories: one joint each, placed on its socket every frame (`animate_characters`).
    for (ai, acc) in lib.accessories.get(&(archetype % lib.models.len())).into_iter().flatten().enumerate() {
        let Some(part) = acc.gpu.parts.first() else { continue };
        let joint = commands.spawn((Transform::default(), AccessoryJoint(ai), ChildOf(root))).id();
        for (mesh, mat) in &part.sections {
            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: vec![joint] }, Transform::default(), bevy::camera::visibility::NoFrustumCulling, ChildOf(root)));
        }
    }
}

/// Third-person weapon models (shared by all characters; NPC skeletons have the same
/// `handAttachment_R/L_jnt` sockets as the player rig).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TpWeapon {
    PlayerSword,
    GuardSword,
    Pistol,
    Crossbow,
}

/// (weapon, skeletal mesh, hand bone, blade, the item tweak it stands for: what it drops).
const TP_WEAPONS: &[(TpWeapon, &str, &str, bool, &str)] = &[
    (TpWeapon::PlayerSword, "Wpn_PlySwords.Wpn_PlySword01", "handAttachment_R_jnt", true, "Twk_Inv_PlayerSpecific.Twk_Inv_SwordCorvo"),
    (TpWeapon::GuardSword, "Wpn_EliteSword.Wpn_EliteSword", "handAttachment_R_jnt", true, "Twk_Inv_EliteGuard.Twk_Inv_SwordEliteGuard"),
    (TpWeapon::Pistol, "Wpn_PlyGuns.Wpn_PlyGunElite", "handAttachment_L_jnt", false, "Twk_Inv_PlayerSpecific.Twk_Inv_PistolCorvo"),
    (TpWeapon::Crossbow, "Crossbows.crossbow_01", "handAttachment_L_jnt", false, "Twk_Inv_PlayerSpecific.Twk_Inv_CrossbowCorvo"),
];

/// The pickup a weapon leaves when its holder drops it (`dropped_weapons`): its static mesh, the
/// mesh's box (mesh space) and the drop's chance.
pub struct DropPickupGpu {
    pub gpu: CharacterGpu,
    pub lo: glam::Vec3,
    pub hi: glam::Vec3,
    pub chance: f32,
}

pub struct TpWeaponGpu {
    pub kind: TpWeapon,
    pub gpu: CharacterGpu,
    pub socket_name: &'static str,
    /// Weapon mesh space -> socket space (U-025: blades leave the thumb side of the fist).
    pub to_socket: glam::Mat4,
    pub bind: Vec<glam::Mat4>,
    /// Weapon mesh Origin / RotOrigin (applied after the socket, as the original attaches).
    pub m2p: glam::Mat4,
    /// Socket of the character mesh the weapon attaches to.
    pub socket: &'static str,
    /// The `Spitfire` socket of the weapon mesh (`DisTweaks_FirePistol.m_MuzzleSocketName`):
    /// (bone, location in the bone).
    pub muzzle: Option<(usize, glam::Vec3)>,
    pub pickup: Option<DropPickupGpu>,
    /// The swing trail (`melee_trail`): its template and the extent's two corner sockets (bone,
    /// location).
    pub trail: Option<(String, [(usize, glam::Vec3); 2])>,
}

/// A `SkeletalMeshSocket` of a mesh: (bone name, relative location).
fn mesh_socket(pkg: &ue3_package::Package, mesh_path: &str, socket: &str) -> Option<(String, glam::Vec3)> {
    use ue3_package::properties::{Value, find};
    let prefix = format!("{}.", mesh_path.to_ascii_lowercase());
    (0..pkg.exports.len()).filter(|i| pkg.export_class(*i) == "SkeletalMeshSocket").find_map(|i| {
        if !pkg.obj_path(ue3_package::ObjRef::Export(i)).to_ascii_lowercase().starts_with(&prefix) {
            return None;
        }
        let h = pkg.read_object(i).ok()?;
        let name = |n: &str| match find(&h.properties, n).map(|x| &x.value) {
            Some(Value::Name(s)) => Some(s.clone()),
            _ => None,
        };
        if !name("SocketName")?.eq_ignore_ascii_case(socket) {
            return None;
        }
        Some((name("BoneName")?, glam::Vec3::from(find(&h.properties, "RelativeLocation").and_then(|x| x.as_vector()).unwrap_or([0.0; 3]))))
    })
}

#[derive(Resource, Default)]
pub struct WeaponLibrary {
    pub weapons: Vec<TpWeaponGpu>,
}

pub fn load_weapons(
    mut commands: Commands,
    features: Option<Res<crate::map_render::RenderFeatures>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let feat = features.map(|f| f.clone()).unwrap_or_else(|| crate::map_render::RenderFeatures::from_flags(&|_| false));
    let mut lib = WeaponLibrary::default();
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    // The item tweaks: the Boyle guard's sword is cooked in its script package.
    let tweak_pkgs = vec!["L_Boyle_Ext_Script".to_string()];
    let db = dishonored_assets::tweaks::TweakDb::load_with(&content, &tweak_pkgs).map_err(|e| warn!("weapon tweaks: {e:#}")).ok();
    for (kind, path, socket, blade, item) in TP_WEAPONS {
        match dishonored_assets::characters::load_model(&content, path, &[("Startup", path)], None, 512) {
            Ok(w) => {
                let part = &w.parts[0];
                let inv = part.mesh_to_pawn.inverse();
                let to_socket = if *blade { glam::Mat4::from_rotation_z(std::f32::consts::PI) * inv } else { inv };
                let sock = if *blade { "RightHandWpn" } else { "LeftHandWpn" };
                let bone_socket = |name: &str| content.package("Startup").ok().and_then(|pkg| mesh_socket(&pkg, path, name)).and_then(|(bone, loc)| part.bones.iter().position(|b| b.name.eq_ignore_ascii_case(&bone)).map(|bi| (bi, loc)));
                let muzzle = bone_socket("Spitfire");
                let trail = db.as_ref().and_then(|db| dishonored_assets::melee_trail::melee_trail_in(db, item).map_err(|e| warn!("weapon {item} trail: {e:#}")).ok().flatten()).and_then(|t| Some((t.template, [bone_socket(&t.sockets[0])?, bone_socket(&t.sockets[1])?])));
                // What it drops (`m_pPickupToDrop`).
                let drop = db.as_ref().and_then(|db| dishonored_assets::pickups::dropped_pickup_in(&content, db, &tweak_pkgs, item).map_err(|e| warn!("weapon {item} pickup: {e:#}")).ok());
                let pickup = drop.and_then(|d| match dishonored_assets::characters::load_static_model(&content, &d.mesh.0, &d.mesh.1, 512) {
                    Ok(m) => {
                        let (lo, hi) = m.parts[0].positions.iter().fold((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
                        Some(DropPickupGpu { gpu: upload(&m, &mut meshes, &mut images, &mut mats, &mut binds, &feat), lo, hi, chance: d.chance })
                    }
                    Err(e) => {
                        warn!("pickup mesh {}: {e:#}", d.mesh.1);
                        None
                    }
                });
                lib.weapons.push(TpWeaponGpu { kind: *kind, socket_name: socket, to_socket, bind: bind_globals(&part.bones), m2p: part.mesh_to_pawn, socket: sock, muzzle, pickup, trail, gpu: upload(&w, &mut meshes, &mut images, &mut mats, &mut binds, &feat) });
            }
            Err(e) => warn!("weapon {path}: {e:#}"),
        }
    }
    commands.insert_resource(lib);
}

#[derive(Component)]
pub struct TpWeaponJoint {
    pub weapon: usize,
    pub bone: usize,
}
#[derive(Component)]
pub struct TpWeaponMesh(pub usize);

/// Spawn hidden weapon meshes under a character root (shown per frame from state).
pub fn spawn_weapons(commands: &mut Commands, root: Entity, weapons: &WeaponLibrary, kind: dishonored_sim::state::PawnKind) {
    use dishonored_sim::state::PawnKind;
    for (wi, w) in weapons.weapons.iter().enumerate() {
        // Only what this kind can ever show (civilians carry nothing).
        let usable = match kind {
            PawnKind::Civilian | PawnKind::Rat => false,
            PawnKind::Guard => w.kind == TpWeapon::GuardSword,
            PawnKind::Human => w.kind != TpWeapon::GuardSword,
        };
        if !usable {
            continue;
        }
        let part = &w.gpu.parts[0];
        let holder = commands.spawn((Transform::default(), Visibility::Hidden, TpWeaponMesh(wi), ChildOf(root))).id();
        let joints: Vec<Entity> = part.bind.iter().enumerate().map(|(i, b)| commands.spawn((Transform::from_matrix(*b), TpWeaponJoint { weapon: wi, bone: i }, ChildOf(root))).id()).collect();
        for (mesh, mat) in &part.sections {
            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: joints.clone() }, Transform::default(), bevy::camera::visibility::NoFrustumCulling, ChildOf(holder)));
        }
    }
}

/// Which weapons a pawn shows. In Contracts, players keep weapons hidden until they draw
/// (attack, block, aim, fire) so their silhouette matches the civilians.
pub fn shown_weapons(p: &PlayerState, contracts: bool) -> (Option<TpWeapon>, Option<TpWeapon>) {
    use dishonored_sim::input::LeftHand;
    use dishonored_sim::state::{LeftPhase, PawnKind};
    match p.kind {
        PawnKind::Civilian | PawnKind::Rat => (None, None),
        PawnKind::Guard => (Some(TpWeapon::GuardSword), None),
        PawnKind::Human => {
            let drawn_sword = !contracts || p.sword.phase != SwordPhase::Idle;
            let gun_out = !contracts || p.left.phase != LeftPhase::Idle;
            let left = match p.left.item {
                LeftHand::Pistol if gun_out => Some(TpWeapon::Pistol),
                LeftHand::Crossbow if gun_out => Some(TpWeapon::Crossbow),
                _ => None,
            };
            (drawn_sword.then_some(TpWeapon::PlayerSword), left)
        }
    }
}

/// Profile used for a pawn: armed players use the elite-guard sets (the only sword, gun and
/// parry sequences of the Boyle NPC sets); everyone else their own model's profile.
fn pawn_profile(bank: &mut crate::npc_anim::AnimBank, model: usize, p: &PlayerState, armed: bool) -> usize {
    use dishonored_sim::state::PawnKind;
    let guard = *bank.model_profile.get(&1).unwrap_or(&0);
    if p.kind == PawnKind::Human && armed {
        return guard;
    }
    bank.model_profile.get(&model).copied().unwrap_or(guard)
}

/// Clip and time of the full-body and upper-body chains (for the sound notifies).
fn chain_clips(st: &crate::npc_anim::PawnAnim) -> Vec<(String, f32)> {
    [st.full.as_ref(), st.upper.as_ref()].into_iter().flatten().filter_map(|c| c.steps.get(c.i).map(|s| (s.0.clone(), c.t))).collect()
}

/// What an NPC looks at while it speaks a line (APPROXIMATION, U-281: the conversation's
/// look-at requests were not decoded): the nearest living human player within 600 uu. The
/// AI's attention look-at (HeadTrack) comes from the host (`MatchView::npc_heads`, the vision
/// head's cursor), so alert pawns no longer use this.
fn look_target(p: &PlayerState, _stance: Option<dishonored_ir::npc_anim::NpcStance>, talking: bool, all: &[&PlayerState]) -> Option<glam::Vec3> {
    use dishonored_sim::state::PawnKind;
    if p.kind == PawnKind::Human || !p.alive || p.incapacitated() {
        return None;
    }
    let range = if talking { 600.0 } else { return None };
    all.iter()
        .filter(|o| o.kind == PawnKind::Human && o.alive && o.id != p.id)
        .map(|o| (o.position.distance(p.position), o))
        .filter(|(d, _)| *d < range)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, o)| o.position + glam::Vec3::Z * 60.0)
}

/// Pawns speaking a conversation line right now (mission NPC ids resolved to pawns).
fn talking_pawns(view: &crate::session::SessionView) -> Vec<u32> {
    let Some(m) = view.matchv.mission.as_ref() else { return Vec::new() };
    m.dialogue
        .iter()
        .filter(|l| l.remaining > 0.0)
        .filter_map(|l| l.npc.as_ref())
        .filter_map(|n| m.npc_ids.iter().find(|x| x.0 == *n).map(|x| x.1))
        .collect()
}

/// Lines being spoken by pawns: (pawn, line id, blurb GUID, seconds into the line).
fn speaking_lines(view: &crate::session::SessionView) -> Vec<(u32, u32, String, f32)> {
    let Some(m) = view.matchv.mission.as_ref() else { return Vec::new() };
    m.dialogue
        .iter()
        .filter(|l| l.remaining > 0.0)
        .filter_map(|l| {
            let pawn = m.npc_ids.iter().find(|x| Some(&x.0) == l.npc.as_ref())?.1;
            Some((pawn, l.id, l.blurb.clone()?, l.duration - l.remaining))
        })
        .collect()
}

/// FaceFX bone offsets per speaking pawn this frame (ANIMATION.md §12): the speaker's head
/// actor driven by the blurb's animation at the time since the line started.
fn face_frames(bank: &mut crate::npc_anim::AnimBank, lib: &CharacterLibrary, view: &crate::session::SessionView, roots: &[(u32, usize)], now: f64, cached: &mut Option<(String, Vec<String>)>) -> HashMap<u32, Vec<crate::facefx::FaceBone>> {
    let lines = speaking_lines(view);
    let live: std::collections::HashSet<u32> = lines.iter().map(|l| l.1).collect();
    bank.facefx.retain_lines(&live);
    let mut out = HashMap::new();
    if lines.is_empty() {
        return out;
    }
    let content = bank.content();
    let map = &view.matchv.map;
    // The map's packages (read from its WorldInfo and the install), once per map rather than
    // every frame a line plays.
    if cached.as_ref().is_none_or(|c| &c.0 != map) {
        let mut packages = if map.is_empty() { Vec::new() } else { dishonored_assets::map_packages(&content, map) };
        for p in ["L_Boyle_Int_Script", "L_Boyle_Ext_Script", "L_BoyleStreet_Ext_Script"] {
            if !packages.iter().any(|x| x.eq_ignore_ascii_case(p)) {
                packages.push(p.to_string());
            }
        }
        *cached = Some((map.clone(), packages));
    }
    let packages = &cached.as_ref().expect("set above").1;
    bank.facefx.index(&content, packages);
    let base = dishonored_multiplayer::mission::APPEARANCE_BASE as usize;
    for (pawn, line, blurb, elapsed) in lines {
        let Some(&(_, model)) = roots.iter().find(|r| r.0 == pawn) else { continue };
        let Some(anim) = bank.facefx.anim(&blurb) else { continue };
        let mut pk: Vec<String> = model.checked_sub(base).and_then(|k| view.matchv.appearances.get(k)).map(|l| l.parts.iter().map(|p| p.0.clone()).collect()).unwrap_or_default();
        pk.extend(packages.iter().cloned());
        let Some(b) = bank.facefx.binding(&content, model, &lib.models[model].parts, &pk) else { continue };
        let t = (now - bank.facefx.line_start(line, now, elapsed)) as f32;
        out.insert(pawn, crate::facefx::face_bones(&b, &anim, t, &lib.models[model].parts));
    }
    out
}

/// `DISH_ANIM_DEBUG=<pawn id>`, read once (not per pawn per frame); `DISH_ANIM_DUMP=<pawn
/// id>:<file>` appends that pawn's bone transforms every frame (`dump_bones`).
pub struct AnimDebug(Option<u32>, Option<(u32, std::path::PathBuf)>);

impl Default for AnimDebug {
    fn default() -> Self {
        let dump = std::env::var("DISH_ANIM_DUMP").ok().and_then(|v| {
            let (id, file) = v.split_once(':').filter(|(_, f)| !f.is_empty())?;
            Some((id.parse::<u32>().ok()?, std::path::PathBuf::from(file)))
        });
        Self(std::env::var("DISH_ANIM_DEBUG").ok().and_then(|v| v.parse::<u32>().ok()), dump)
    }
}

/// One JSON line per frame for the animation diff against the original's per-tick bone capture
/// (fixtures/oracle/capture_queue.txt, `session bones`): the sim tick, the pawn's location and
/// yaw, the slots playing, and every bone of the body in component space (the skeletal mesh's
/// own space, before its `Origin` / `RotOrigin`, like UE3's `SpaceBases`): rotation (x, y, z,
/// w) and translation, Dishonored units.
pub fn dump_bones(file: &std::path::Path, tick: u64, p: &PlayerState, st: Option<&crate::npc_anim::PawnAnim>, part: &PartGpu, globals: &[glam::Mat4]) {
    use std::io::Write;
    let inv = part.mesh_to_pawn.inverse();
    let bones: Vec<serde_json::Value> = part
        .bones
        .iter()
        .zip(globals)
        .map(|(b, g)| {
            let (_, q, t) = (inv * *g).to_scale_rotation_translation();
            serde_json::json!({"name": b.name, "q": [q.x, q.y, q.z, q.w], "t": [t.x, t.y, t.z]})
        })
        .collect();
    let slot = |c: Option<&crate::npc_anim::Chain>| c.map(|c| serde_json::json!({"key": c.key, "seq": c.steps[c.i].0, "t": c.t, "w": c.weight}));
    let line = serde_json::json!({
        "tick": tick, "id": p.id, "pos": [p.position.x, p.position.y, p.position.z], "yaw": p.yaw,
        "modifier": st.map(|s| s.modifier.clone()), "vel": st.map(|s| [s.vel.0, s.vel.1]), "phase": st.map(|s| s.phase),
        "full": slot(st.and_then(|s| s.full.as_ref())), "upper": slot(st.and_then(|s| s.upper.as_ref())), "ragdoll": st.is_some_and(|s| s.ragdoll.is_some()),
        "bones": bones,
    });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{line}");
    }
}

#[allow(clippy::too_many_arguments)]
pub fn animate_characters(
    bank: Option<ResMut<crate::npc_anim::AnimBank>>,
    mut commands: Commands,
    mut states: Query<&mut crate::npc_anim::PawnAnim>,
    lib: Option<Res<CharacterLibrary>>,
    weapons: Option<Res<WeaponLibrary>>,
    mut view: ResMut<crate::session::SessionView>,
    // (Grouped: a system takes at most 16 parameters.)
    (time, mut face_packages, anim_debug, body_frames): (Res<Time>, Local<Option<(String, Vec<String>)>>, Local<AnimDebug>, Option<Res<crate::viewmodel::BodyFrames>>),
    roots: Query<(Entity, &CharacterRoot, &crate::players::PlayerBody, &Children, &GlobalTransform, Option<&InheritedVisibility>)>,
    mut joints: Query<(&Joint, &mut Transform), (Without<TpWeaponJoint>, Without<AccessoryJoint>)>,
    mut wjoints: Query<(&TpWeaponJoint, &mut Transform), (Without<Joint>, Without<AccessoryJoint>)>,
    mut ajoints: Query<(&AccessoryJoint, &mut Transform), (Without<Joint>, Without<TpWeaponJoint>)>,
    mut wmeshes: Query<(&TpWeaponMesh, &mut Visibility)>,
    mut anim_sounds: Option<ResMut<crate::audio::AnimSounds>>,
    session: Res<crate::session::Session>,
    mut sockets: Option<ResMut<crate::effects::EffectSockets>>,
    (mut pawn_points, mut pawn_fx, mut drops, mut trails, mut ash): (Option<ResMut<crate::cutscene::PawnPoints>>, Option<ResMut<crate::effects::PawnEffects>>, Option<ResMut<crate::dropped_weapons::DroppedWeapons>>, Option<ResMut<crate::weapon_trails::WeaponTrails>>, Option<ResMut<crate::ash_bodies::AshBodies>>),
) {
    let events = std::mem::take(&mut view.anim_events);
    // Bodies the sim removed, for their burn (`ash_bodies`).
    if let Some(ash) = ash.as_mut() {
        for e in &events {
            if let dishonored_sim::events::SimEvent::BodyRemoved { body, ash: burnt } = e {
                ash.removed.insert(*body, (*burnt, time.elapsed_secs_f64()));
            }
        }
    }
    let (Some(lib), Some(mut bank)) = (lib, bank) else { return };
    let contracts = view.matchv.mode.eq_ignore_ascii_case("contracts");
    let all: Vec<&PlayerState> = view.others.iter().chain(view.me.iter()).collect();
    let hits = crate::present_rules::collect_hits(&events, &all);
    let talking = talking_pawns(&view);
    let root_models: Vec<(u32, usize)> = roots.iter().map(|r| (r.2.0, r.1.archetype)).collect();
    let faces = face_frames(&mut bank, &lib, &view, &root_models, time.elapsed_secs_f64(), &mut face_packages);
    // Each pawn's chains tick with its dilated time (`PlayerState::time_scale`: Bend Time, a
    // finisher's slow motion).
    let dt = time.delta_secs().min(0.1);
    let mv = &session.content.sim.gameplay.movement;
    for (entity, root, body, children, root_gt, root_vis) in &roots {
        let Some(p) = all.iter().find(|p| p.id == body.0).copied() else { continue };
        let model = &lib.models[root.archetype];
        let (right_w, left_w) = shown_weapons(p, contracts);
        let armed = right_w.is_some() || left_w.is_some() || p.kind == dishonored_sim::state::PawnKind::Guard;
        let profile = pawn_profile(&mut bank, root.archetype, p, armed);
        let stance = view.matchv.npc_poses.iter().find(|x| x.0 == p.id).map(|x| x.1);
        // The level floor under a pawn-space point (pawn space: origin at the pawn, turned by its
        // yaw), for the ragdoll's floor grid.
        // A body left in its choker's or carrier's frame is drawn at that anchor
        // (`viewmodel::BodyFrames`): its ragdoll samples the level there.
        let (pos, yaw, hh) = match body_frames.as_ref().and_then(|f| f.anchors.get(&p.id)) {
            Some(a) => (a.pos, a.yaw.to_radians(), mv.collision_half_height),
            None => (p.position, p.yaw.to_radians(), mv.collision_half_height),
        };
        let world = &session.content.collision;
        let ground = move |x: f32, y: f32| -> f32 {
            use dishonored_world::WorldQuery;
            let (s, c) = yaw.sin_cos();
            let w = pos + glam::Vec3::new(x * c - y * s, x * s + y * c, 0.0);
            world.raycast(w + glam::Vec3::Z * 60.0, -glam::Vec3::Z, 400.0, dishonored_world::channel::PAWN).map(|h| h.point.z - pos.z).unwrap_or(-hh)
        };
        // The collision triangles around the pawn in pawn space, for the ragdoll's walls.
        let walls = move || -> Vec<[glam::Vec3; 3]> {
            let r = crate::ragdoll::WALL_REACH;
            let q = dishonored_world::geom::Aabb { min: pos - glam::Vec3::new(r, r, hh + 64.0), max: pos + glam::Vec3::new(r, r, 200.0) };
            let (s, c) = (-yaw).sin_cos();
            let to_pawn = |v: glam::Vec3| {
                let d = v - pos;
                glam::Vec3::new(d.x * c - d.y * s, d.x * s + d.y * c, d.z)
            };
            let mut ids = Vec::new();
            world.bvh.query_aabb(&q, |i| ids.push(i));
            ids.sort_unstable();
            ids.into_iter()
                .map(|i| &world.tris[i as usize])
                .filter(|t| t.channels & dishonored_world::channel::PAWN != 0)
                .map(|t| [to_pawn(t.a), to_pawn(t.b), to_pawn(t.c)])
                .collect()
        };
        let action = view.matchv.npc_actions.iter().find(|x| x.0 == p.id).map(|x| (x.1, x.2));
        let input = crate::npc_anim::AnimInput { p, stance, action, armed, talking: talking.contains(&p.id), dt: dt * p.time_scale.max(0.0), hits: hits.get(&p.id).map(|v| v.as_slice()).unwrap_or(&[]), look_target: look_target(p, stance, talking.contains(&p.id), &all), head: view.matchv.npc_heads.iter().find(|h| h.id == p.id).copied(), face: faces.get(&p.id).map(|v| v.as_slice()).unwrap_or(&[]), floor: -mv.collision_half_height, ground: Some(&ground), walls: Some(&walls), gravity: mv.gravity_z, carrier_speed: p.carried_by.and_then(|c| all.iter().find(|x| x.id == c)).map(|c| c.velocity.truncate().length()).unwrap_or(0.0) };
        // Particle notifies the chains crossed this frame: (system, socket or bone, attached).
        let mut fired: Vec<(String, String, bool)> = Vec::new();
        // A `DisNotify_DropItem` crossed this frame; the limp death started (the ragdoll took the
        // body) or the dead pawn plays nothing more: what drops its weapons.
        let mut drop_notify: Option<crate::npc_anim::DropNotify> = None;
        let mut limp = false;
        // A `DisNotify_Trails` window of a chain covers its time: the held swords trail.
        let mut trailing = false;
        let pose = match states.get_mut(entity) {
            Ok(mut st) => {
                let before = chain_clips(&st);
                let was_limp = st.ragdoll.is_some();
                let pose = crate::npc_anim::animate(&mut bank, &lib, root.archetype, profile, &mut st, &input);
                limp = (!was_limp && st.ragdoll.is_some()) || (st.full.is_none() && st.ragdoll.is_none());
                trailing = chain_clips(&st).iter().any(|(clip, t)| bank.trailing(profile, clip, *t));
                // `AnimNotify_PlayParticleEffect` of the full-body and upper-body chains: a clip
                // started this frame fires from its start (a notify at 0 too).
                for (clip, t1) in chain_clips(&st) {
                    let t0 = before.iter().find(|b| b.0 == clip).map(|b| b.1);
                    for (t, template, point, attach) in bank.particle_notifies(profile, &clip) {
                        let crossed = match t0 {
                            Some(t0) => t0 < t && t <= t1,
                            None => t <= t1,
                        };
                        if crossed {
                            fired.push((template, point, attach));
                        }
                    }
                    for d in bank.drop_notifies(profile, &clip) {
                        let crossed = match t0 {
                            Some(t0) => t0 < d.t && d.t <= t1,
                            None => d.t <= t1,
                        };
                        if crossed {
                            drop_notify = Some(d);
                        }
                    }
                }
                // Sound notifies (AnimNotify_AkEvent, footfalls) of the clip windows advanced.
                if let Some(snd) = anim_sounds.as_mut() {
                    for (clip, t1) in chain_clips(&st) {
                        if let Some((_, t0)) = before.iter().find(|b| b.0 == clip) {
                            snd.advance(p.id, &clip, *t0, t1, false);
                        }
                    }
                    // Locomotion feet touching down (`m_bFootOnGround`).
                    for (k, down) in st.feet_down.iter().enumerate() {
                        if *down {
                            snd.foot_down(p.id, k as i32);
                        }
                    }
                    // Locomotion: the sync group's master fires the notifies (footfalls).
                    // Reversed loops play from the end backwards.
                    if let Some((clip, rev, p0, p1)) = st.loco_master.clone() {
                        if let Some(d) = bank.duration(profile, &clip) {
                            let full_body = st.full.as_ref().is_some_and(|c| c.weight > 0.5);
                            if !full_body {
                                if rev { snd.advance(p.id, &clip, d * (1.0 - p1), d * (1.0 - p0), true) } else { snd.advance(p.id, &clip, p0 * d, p1 * d, true) }
                            }
                        }
                    }
                }
                pose
            }
            Err(_) => {
                let mut st = crate::npc_anim::PawnAnim::default();
                let pose = crate::npc_anim::animate(&mut bank, &lib, root.archetype, profile, &mut st, &input);
                commands.entity(entity).insert(st);
                pose
            }
        };
        let globals: Vec<Vec<glam::Mat4>> = model.parts.iter().zip(&pose).map(|(part, pp)| crate::npc_anim::globals(part, pp)).collect();
        // DISH_ANIM_DUMP=<pawn id>:<file>: the pawn's bones every frame (animation diff).
        if let Some((_, file)) = anim_debug.1.as_ref().filter(|d| d.0 == p.id) {
            dump_bones(file, view.tick, p, states.get(entity).ok(), &model.parts[0], &globals[0]);
        }
        // Diagnostics: DISH_ANIM_DEBUG=<pawn id> logs the pawn's slots and root.
        if anim_debug.0 == Some(p.id) {
            if let Ok(st) = states.get(entity) {
                let slot = |c: &Option<crate::npc_anim::Chain>| c.as_ref().map(|c| format!("{}:{}@{:.2}w{:.2}", c.key, c.steps[c.i].0, c.t, c.weight)).unwrap_or_default();
                info!("anim {} mod={} vel={:.0},{:.0} full={} upper={} ragdoll={} root={:?} pelvis={:?}", p.id, st.modifier, st.vel.0, st.vel.1, slot(&st.full), slot(&st.upper), st.ragdoll.is_some(), pose[0].first().copied().flatten().map(|k| k.t), globals[0].get(1).map(|m| m.w_axis.truncate()));
            }
        }
        // [interp] Bones and sockets matinee attachments hold objects on (a prop in the hand).
        if let Some(pp) = pawn_points.as_mut() {
            let wanted: Vec<String> = pp.wanted.iter().filter(|w| w.0 == p.id).map(|w| w.1.clone()).collect();
            for name in wanted {
                let local = model
                    .parts
                    .iter()
                    .enumerate()
                    .find_map(|(pi, part)| part.bones.iter().position(|b| b.name.eq_ignore_ascii_case(&name)).map(|bi| globals[pi][bi]))
                    .or_else(|| lib.sockets.get(&root.archetype).and_then(|v| v.iter().find(|s| s.name.eq_ignore_ascii_case(&name))).map(|s| globals[s.part][s.bone] * s.local));
                if let Some(l) = local {
                    pp.points.insert((p.id, name), crate::cutscene::from_bevy(root_gt.to_matrix() * to_bevy(l)));
                }
            }
        }
        // Sockets (else bones) of the particle notifies fired now and of the attached effects
        // still playing on this pawn (Dishonored world transforms).
        if let Some(fx) = pawn_fx.as_mut() {
            let point = |name: &str| -> Option<glam::Mat4> {
                let local = lib
                    .sockets
                    .get(&root.archetype)
                    .and_then(|v| v.iter().find(|s| s.name.eq_ignore_ascii_case(name)))
                    .map(|s| globals[s.part][s.bone] * s.local)
                    .or_else(|| model.parts.iter().enumerate().find_map(|(pi, part)| part.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name)).map(|bi| globals[pi][bi])))?;
                Some(crate::cutscene::from_bevy(root_gt.to_matrix() * to_bevy(local)))
            };
            for (template, name, attach) in fired {
                if let Some(m) = point(&name) {
                    fx.pending.push((template, p.id, name.clone(), m, attach));
                    fx.points.insert((p.id, name), m);
                }
            }
            for name in fx.wanted(p.id) {
                match point(&name) {
                    Some(m) => fx.points.insert((p.id, name), m),
                    None => fx.points.remove(&(p.id, name)),
                };
            }
        }
        // Bones the Shadow Kill ash attaches to (`m_ShadowKillFXs`), world space.
        if let Some(s) = sockets.as_mut() {
            let bones = &model.parts[0].bones;
            let uu = |b: Vec3| Vec3::new(b.x, b.z, b.y) / crate::coords::UU_TO_BEVY;
            let pts: Vec<Vec3> = crate::effects::SHADOW_KILL_BONES
                .iter()
                .filter_map(|n| bones.iter().position(|b| b.name.eq_ignore_ascii_case(n)))
                .map(|bi| uu(root_gt.transform_point(to_bevy(globals[0][bi]).w_axis.truncate())))
                .collect();
            s.bones.insert(p.id, pts);
            if let Some(bi) = bones.iter().position(|b| b.name.eq_ignore_ascii_case("head_jnt")) {
                s.heads.insert(p.id, uu(root_gt.transform_point(to_bevy(globals[0][bi]).w_axis.truncate())));
            }
        }
        for c in children.iter() {
            if let Ok((j, mut t)) = joints.get_mut(c) {
                if let Some(m) = globals.get(j.part).and_then(|v| v.get(j.bone)) {
                    *t = Transform::from_matrix(to_bevy(*m));
                }
            }
            if let Ok((a, mut t)) = ajoints.get_mut(c) {
                if let Some(acc) = lib.accessories.get(&root.archetype).and_then(|v| v.get(a.0)) {
                    if let Some(m) = globals.get(acc.part).and_then(|v| v.get(acc.bone)) {
                        *t = Transform::from_matrix(to_bevy(*m * acc.local));
                    }
                }
            }
        }
        let Some(weapons) = weapons.as_ref() else { continue };
        let body_bones = &model.parts[0].bones;
        // The weapon mesh's frame in pawn space: the character mesh's weapon socket, then the
        // weapon's Origin / RotOrigin (U-025); meshes without the socket keep the bone with the
        // earlier visual offset.
        let weapon_frame = |w: &TpWeaponGpu| -> Option<glam::Mat4> {
            if let Some(s) = lib.sockets.get(&root.archetype).and_then(|v| v.iter().find(|s| s.name.eq_ignore_ascii_case(w.socket))) {
                return Some(globals[s.part][s.bone] * s.local * w.m2p);
            }
            let sock = body_bones.iter().position(|b| b.name.eq_ignore_ascii_case(w.socket_name))?;
            Some(globals[0][sock] * w.to_socket)
        };
        let to_world = |m: glam::Mat4| crate::cutscene::from_bevy(root_gt.to_matrix() * to_bevy(m));
        for c in children.iter() {
            if let Ok((wm, mut vis)) = wmeshes.get_mut(c) {
                let w = &weapons.weapons[wm.0];
                let held = Some(w.kind) == right_w || Some(w.kind) == left_w;
                let key = (p.id, wm.0);
                let mut gone = false;
                if let Some(drops) = drops.as_mut() {
                    if p.alive {
                        drops.dropped.remove(&key);
                    }
                    gone = drops.dropped.contains(&key);
                    let frame = weapon_frame(w).map(to_world);
                    // A dead pawn lets go (pawn slot 348: health below 1): at its animation's drop
                    // notify (the notify's usage picks the hand), when it goes limp, or when it plays
                    // nothing more. A corpse never seen holding the weapon has nothing to drop.
                    if !p.alive && held && !gone {
                        let right_hand = w.socket == "RightHandWpn";
                        let notify = drop_notify.filter(|d| d.which != 2 || d.primary == right_hand);
                        if !drops.held.contains(&key) {
                            drops.dropped.insert(key);
                            gone = true;
                        } else if notify.is_some() || limp {
                            // `m_DropVelocity` turned by the pawn mesh's LocalToWorld.
                            let velocity = notify.map(|d| to_world(model.parts[0].mesh_to_pawn).transform_vector3(d.velocity)).unwrap_or(glam::Vec3::ZERO);
                            if let (Some(f), Some(pk)) = (frame, w.pickup.as_ref()) {
                                // The roll (0x00C0BC00: spawned when the chance is at least rand() / RAND_MAX).
                                let mut seed = ((p.id as u64) << 32) ^ wm.0 as u64;
                                if pk.chance >= crate::present_rules::next_rand(&mut seed) {
                                    drops.pending.push(crate::dropped_weapons::DropRequest { pawn: p.id, weapon: wm.0, frame: f * crate::dropped_weapons::pickup_turn(), velocity });
                                }
                            }
                            drops.dropped.insert(key);
                            gone = true;
                        }
                    }
                    if held && !gone && frame.is_some() && p.condition != dishonored_sim::state::Condition::Unconscious && p.carried_by.is_none() {
                        drops.held.insert(key);
                    } else {
                        drops.held.remove(&key);
                    }
                }
                let show = (p.alive || !gone) && p.condition != dishonored_sim::state::Condition::Unconscious && p.carried_by.is_none() && p.mode != MoveMode::Blinking && !dishonored_sim::teleport_spell::hidden(p) && held;
                *vis = if show { Visibility::Inherited } else { Visibility::Hidden };
                // The blade's corners feed its swing trail.
                // (Not from a body that is not drawn: the local player's own in first person.)
                if let (true, true, Some((template, sockets)), Some(f), Some(tr)) = (show && root_vis.is_none_or(|v| v.get()), trailing, w.trail.as_ref(), weapon_frame(w), trails.as_mut()) {
                    let corner = |k: usize| to_world(f * w.bind[sockets[k].0]).transform_point3(sockets[k].1);
                    tr.sources.push(((p.id, wm.0 as u32), template.clone(), corner(0), corner(1)));
                }
            }
            if let Ok((wj, mut t)) = wjoints.get_mut(c) {
                let w = &weapons.weapons[wj.weapon];
                if let Some(f) = weapon_frame(w) {
                    *t = Transform::from_matrix(to_bevy(f * w.bind[wj.bone]));
                }
            }
        }
        // Muzzle socket of the shown pistol (world space, Dishonored units) for the muzzle flash.
        if let (Some(s), Some(k)) = (sockets.as_mut(), left_w.filter(|k| *k == TpWeapon::Pistol)) {
            if let Some((w, (bone, loc))) = weapons.weapons.iter().find(|w| w.kind == k).and_then(|w| w.muzzle.map(|m| (w, m))) {
                if let Some(sock) = body_bones.iter().position(|b| b.name.eq_ignore_ascii_case(w.socket_name)) {
                    let m = globals[0][sock] * w.to_socket * w.bind[bone];
                    let local_b = to_bevy(m * glam::Mat4::from_translation(loc));
                    let fwd_b = to_bevy(m * glam::Mat4::from_translation(loc + glam::Vec3::Z * -10.0));
                    let pb = root_gt.transform_point(local_b.w_axis.truncate());
                    let fb = root_gt.transform_point(fwd_b.w_axis.truncate());
                    let uu = |b: Vec3| Vec3::new(b.x, b.z, b.y) / crate::coords::UU_TO_BEVY;
                    s.muzzle.insert(p.id, (uu(pb), (uu(fb) - uu(pb)).normalize_or_zero()));
                }
            }
        }
    }
}

/// Set up the animation bank: the built-in archetypes get the profiles of the Boyle pawn
/// tweaks they stand for (`AnimBank::builtin`).
pub fn load_animations(mut commands: Commands, lib: Option<Res<CharacterLibrary>>) {
    let Some(lib) = lib else { return };
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    let mut bank = crate::npc_anim::AnimBank::new(std::sync::Arc::new(content));
    for m in 0..lib.models.len() {
        let p = bank.builtin(m);
        let i = bank.profile(&p);
        bank.model_profile.insert(m, i);
    }
    info!("animation profiles: {} ({} AnimSets)", bank.profiles.len(), bank.sets.len());
    commands.insert_resource(bank);
}

/// Load the NPC looks the host published (`MatchView::appearances`: each pawn tweak's body and
/// head meshes) as models `APPEARANCE_BASE + i`, and bind the third-person animation sets to
/// them. A look that fails to load falls back to the first built-in model so indices stay
/// aligned.
#[allow(clippy::too_many_arguments)]
pub fn load_appearances(
    view: Res<crate::session::SessionView>,
    session: Res<crate::session::Session>,
    features: Option<Res<crate::map_render::RenderFeatures>>,
    lib: Option<ResMut<CharacterLibrary>>,
    bank: Option<ResMut<crate::npc_anim::AnimBank>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let (Some(mut lib), Some(mut bank)) = (lib, bank) else { return };
    let base = dishonored_multiplayer::mission::APPEARANCE_BASE as usize;
    let table = &view.matchv.appearances;
    if lib.models.len() != base + lib.looks || lib.looks >= table.len() {
        return;
    }
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    let feat = features.map(|f| f.clone()).unwrap_or_else(|| crate::map_render::RenderFeatures::from_flags(&|_| false));
    let hh = session.content.sim.gameplay.movement.collision_half_height;
    // A few per frame so a large table does not stall one frame for long.
    for _ in 0..4 {
        let i = lib.looks;
        let Some(look) = table.get(i) else { break };
        let parts = &look.parts;
        let refs: Vec<(&str, &str)> = parts.iter().map(|(p, m)| (p.as_str(), m.as_str())).collect();
        // Material swaps (`DisSeqAct_NPCSetMaterials`): find the package cooking each material.
        let overrides: Vec<dishonored_assets::characters::MaterialOverride> = look
            .materials
            .iter()
            .filter_map(|(part, index, path)| {
                let pkg = lib.material_package(&content, &session, path)?;
                Some((*part as usize, *index as usize, pkg, path.clone()))
            })
            .collect();
        let name = parts.first().map(|p| p.1.rsplit('.').next().unwrap_or("npc").to_string()).unwrap_or_default();
        let gpu = match dishonored_assets::characters::load_model_with(&content, &name, &refs, &overrides, Some(hh), 1024) {
            Ok(m) => upload(&m, &mut meshes, &mut images, &mut mats, &mut binds, &feat),
            Err(e) => {
                warn!("NPC look {i} ({name}): {e:#}");
                lib.models[0].clone()
            }
        };
        // The pawn tweak's animation setup (`Look::anim`); without one, the guard sets.
        let profile = match look.anim.checked_sub(1).and_then(|k| view.matchv.anim_profiles.get(k as usize)) {
            Some(p) => bank.profile(p),
            None => *bank.model_profile.get(&1).unwrap_or(&0),
        };
        bank.model_profile.insert(lib.models.len(), profile);
        // Accessories rolled for this look: the static mesh on the named socket of one of the
        // look's skeletal meshes (body or head).
        let sockets = model_sockets(&content, parts, &gpu);
        let mut accs = Vec::new();
        for ((pkg, path), socket) in &look.accessories {
            let key = (pkg.clone(), path.clone());
            if !lib.accessory_meshes.contains_key(&key) {
                let g = match dishonored_assets::characters::load_static_model(&content, pkg, path, 512) {
                    Ok(m) => Some(upload(&m, &mut meshes, &mut images, &mut mats, &mut binds, &feat)),
                    Err(e) => {
                        warn!("accessory {path}: {e:#}");
                        None
                    }
                };
                lib.accessory_meshes.insert(key.clone(), g);
            }
            let Some(agpu) = lib.accessory_meshes.get(&key).cloned().flatten() else { continue };
            let found = sockets.iter().find(|s| s.name.eq_ignore_ascii_case(socket)).map(|s| (s.part, s.bone, s.local));
            match found {
                Some((part, bone, local)) => accs.push(Accessory { gpu: agpu, part, bone, local }),
                None => warn!("accessory {path}: socket {socket} not found"),
            }
        }
        if !accs.is_empty() {
            let mi = lib.models.len();
            lib.accessories.insert(mi, accs);
        }
        let mi = lib.models.len();
        lib.sockets.insert(mi, sockets);
        lib.models.push(gpu);
        lib.looks += 1;
    }
}

impl CharacterLibrary {
    /// The level package that cooks a material (index built once from the session's maps).
    fn material_package(&mut self, content: &dishonored_assets::Content, session: &crate::session::Session, path: &str) -> Option<String> {
        let index = self.material_index.get_or_insert_with(|| {
            let mut m = HashMap::new();
            for mc in &session.maps.maps {
                for p in dishonored_assets::map_packages(content, &mc.map_name) {
                    let Ok(pkg) = content.package(&p) else { continue };
                    for i in 0..pkg.exports.len() {
                        let c = pkg.export_class(i);
                        if c == "MaterialInstanceConstant" || c == "Material" {
                            m.entry(pkg.obj_path(ue3_package::ObjRef::Export(i)).to_ascii_lowercase()).or_insert_with(|| p.clone());
                        }
                    }
                }
            }
            m
        });
        index.get(&path.to_ascii_lowercase()).cloned()
    }
}
