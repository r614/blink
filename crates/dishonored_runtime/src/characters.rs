//! Skinned Dishonored character models for player bodies.
//!
//! Each part is uploaded once; every player gets a joint hierarchy (flat, children of the
//! body root) driven by decoded Edge animation clips (locomotion chosen from simulation
//! state); `procedural_pose` remains only as a fallback when a clip is unavailable.

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

pub struct PartGpu {
    pub sections: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    /// Bind-pose joint transforms in Bevy space relative to the body root.
    pub bind: Vec<Mat4>,
    pub bones: Vec<dishonored_ir::BoneAsset>,
    pub mesh_to_pawn: glam::Mat4,
}

pub struct CharacterGpu {
    pub name: String,
    pub parts: Vec<PartGpu>,
}

#[derive(Resource, Default)]
pub struct CharacterLibrary {
    pub models: Vec<CharacterGpu>,
}

/// C: Dishonored pawn space (Z-up, uu) -> Bevy (Y-up, scaled).
pub fn to_bevy(m: glam::Mat4) -> Mat4 {
    coords::transform(m)
}

pub fn upload(model: &CharacterModel, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>, binds: &mut Assets<SkinnedMeshInverseBindposes>) -> CharacterGpu {
    let tex: Vec<Handle<Image>> = model.textures.iter().map(|t| images.add(to_image(t))).collect();
    let materials: Vec<Handle<StandardMaterial>> = model
        .materials
        .iter()
        .map(|m| {
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
        for s in &p.sections {
            let start = s.first_index as usize;
            let end = (start + s.num_triangles as usize * 3).min(p.indices.len());
            let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, nrm.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(joints.clone()))
                .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights.clone())
                .with_inserted_indices(Indices::U32(p.indices[start..end].to_vec()));
            let mat = s.material.map(|m| materials[m].clone()).unwrap_or_else(|| fallback.clone());
            sections.push((meshes.add(mesh), mat));
        }
        parts.push(PartGpu { sections, inverse_bindposes: ib, bind, bones: p.bones.clone(), mesh_to_pawn: m2p });
    }
    CharacterGpu { name: model.name.clone(), parts }
}

pub fn load_library(
    mut commands: Commands,
    session: Res<crate::session::Session>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let mut lib = CharacterLibrary::default();
    let hh = session.content.sim.gameplay.movement.collision_half_height;
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    for (name, _) in ARCHETYPES {
        match load_character(&content, name, hh, 1024) {
            Ok(m) => lib.models.push(upload(&m, &mut meshes, &mut images, &mut mats, &mut binds)),
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
                ChildOf(root),
            ));
        }
    }
}

/// Placeholder pose: arms lowered from the bind pose and a speed-driven leg/arm swing.
/// Presentation only; replaced by decoded Edge animation once available.
pub fn procedural_pose(part: &PartGpu, speed: f32, phase: f32, crouched: bool) -> Vec<Mat4> {
    let mut locals: Vec<(glam::Quat, glam::Vec3)> = part.bones.iter().map(|b| (b.rotation, b.translation)).collect();
    let swing = (speed / 400.0).clamp(0.0, 1.5) * 0.6 * phase.sin();
    for (i, b) in part.bones.iter().enumerate() {
        let n = b.name.to_ascii_lowercase();
        let extra = if n.starts_with("upperarm") || n.starts_with("shoulder") || n.contains("arm_l") && n.contains("upper") {
            0.0
        } else if n.contains("thigh") || n.contains("upleg") || n.starts_with("hip_") {
            if n.contains("_l") { swing } else { -swing }
        } else if n.contains("knee") || n.contains("calf") || n.contains("leg_l_jnt") || n.contains("leg_r_jnt") {
            if crouched { 0.9 } else { swing.abs() * 0.8 }
        } else {
            0.0
        };
        if extra != 0.0 {
            locals[i].0 = locals[i].0 * glam::Quat::from_rotation_z(extra);
        }
    }
    let bones: Vec<dishonored_ir::BoneAsset> = part.bones.iter().zip(&locals).map(|(b, (q, t))| dishonored_ir::BoneAsset { rotation: *q, translation: *t, ..b.clone() }).collect();
    bind_globals(&bones).iter().map(|g| to_bevy(part.mesh_to_pawn * *g)).collect()
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

const TP_WEAPONS: &[(TpWeapon, &str, &str, bool)] = &[
    (TpWeapon::PlayerSword, "Wpn_PlySwords.Wpn_PlySword01", "handAttachment_R_jnt", true),
    (TpWeapon::GuardSword, "Wpn_EliteSword.Wpn_EliteSword", "handAttachment_R_jnt", true),
    (TpWeapon::Pistol, "Wpn_PlyGuns.Wpn_PlyGunElite", "handAttachment_L_jnt", false),
    (TpWeapon::Crossbow, "Crossbows.crossbow_01", "handAttachment_L_jnt", false),
];

pub struct TpWeaponGpu {
    pub kind: TpWeapon,
    pub gpu: CharacterGpu,
    pub socket_name: &'static str,
    /// Weapon mesh space -> socket space (U-025: blades leave the thumb side of the fist).
    pub to_socket: glam::Mat4,
    pub bind: Vec<glam::Mat4>,
}

#[derive(Resource, Default)]
pub struct WeaponLibrary {
    pub weapons: Vec<TpWeaponGpu>,
}

pub fn load_weapons(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let mut lib = WeaponLibrary::default();
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    for (kind, path, socket, blade) in TP_WEAPONS {
        match dishonored_assets::characters::load_model(&content, path, &[("Startup", path)], None, 512) {
            Ok(w) => {
                let part = &w.parts[0];
                let inv = part.mesh_to_pawn.inverse();
                let to_socket = if *blade { glam::Mat4::from_rotation_z(std::f32::consts::PI) * inv } else { inv };
                lib.weapons.push(TpWeaponGpu { kind: *kind, socket_name: socket, to_socket, bind: bind_globals(&part.bones), gpu: upload(&w, &mut meshes, &mut images, &mut mats, &mut binds) });
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
            PawnKind::Civilian => false,
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
        PawnKind::Civilian => (None, None),
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

pub fn animate_characters(
    anims: Option<Res<Animations>>,
    mut commands: Commands,
    mut states: Query<&mut AnimState>,
    lib: Option<Res<CharacterLibrary>>,
    weapons: Option<Res<WeaponLibrary>>,
    view: Res<crate::session::SessionView>,
    time: Res<Time>,
    roots: Query<(Entity, &CharacterRoot, &crate::players::PlayerBody, &Children)>,
    mut joints: Query<(&Joint, &mut Transform), Without<TpWeaponJoint>>,
    mut wjoints: Query<(&TpWeaponJoint, &mut Transform), Without<Joint>>,
    mut wmeshes: Query<(&TpWeaponMesh, &mut Visibility)>,
) {
    let Some(lib) = lib else { return };
    let contracts = view.matchv.mode.eq_ignore_ascii_case("contracts");
    for (entity, root, body, children) in &roots {
        let Some(p) = view.others.iter().chain(view.me.iter()).find(|p| p.id == body.0) else { continue };
        let model = &lib.models[root.archetype];
        let speed = p.velocity.truncate().length();
        let phase = time.elapsed_secs() * (speed / 400.0).max(0.2) * 7.0 + p.id as f32;
        let (right_w, left_w) = shown_weapons(p, contracts);
        let globals: Vec<Vec<glam::Mat4>> = match anims.as_ref() {
            Some(a) => {
                let choice = choose_tp_clip(p, right_w.is_some() || left_w.is_some());
                let t = match states.get_mut(entity) {
                    Ok(mut s) => {
                        if s.key != choice.key {
                            s.key = choice.key.clone();
                            s.time = 0.0;
                        }
                        s.time += time.delta_secs() * choice.rate;
                        s.time
                    }
                    Err(_) => {
                        commands.entity(entity).insert(AnimState { time: 0.0, key: choice.key.clone() });
                        0.0
                    }
                };
                match a.clips.get(choice.clip) {
                    Some((set, clip)) => {
                        let t = if choice.looped { t + p.id as f32 * 0.37 } else { t.min(clip.duration - 1e-3) };
                        model.parts.iter().enumerate().map(|(pi, part)| decoded_globals(part, &a.track_to_bone[*set][root.archetype][pi], clip, t)).collect()
                    }
                    None => model.parts.iter().map(|part| procedural_globals(part, speed, phase, p.crouched)).collect(),
                }
            }
            None => model.parts.iter().map(|part| procedural_globals(part, speed, phase, p.crouched)).collect(),
        };
        for c in children.iter() {
            if let Ok((j, mut t)) = joints.get_mut(c) {
                if let Some(m) = globals.get(j.part).and_then(|v| v.get(j.bone)) {
                    *t = Transform::from_matrix(to_bevy(*m));
                }
            }
        }
        let Some(weapons) = weapons.as_ref() else { continue };
        let body_bones = &model.parts[0].bones;
        for c in children.iter() {
            if let Ok((wm, mut vis)) = wmeshes.get_mut(c) {
                let k = weapons.weapons[wm.0].kind;
                let show = p.alive && p.mode != MoveMode::Blinking && (Some(k) == right_w || Some(k) == left_w);
                *vis = if show { Visibility::Inherited } else { Visibility::Hidden };
            }
            if let Ok((wj, mut t)) = wjoints.get_mut(c) {
                let w = &weapons.weapons[wj.weapon];
                let Some(sock) = body_bones.iter().position(|b| b.name.eq_ignore_ascii_case(w.socket_name)) else { continue };
                *t = Transform::from_matrix(to_bevy(globals[0][sock] * w.to_socket * w.bind[wj.bone]));
            }
        }
    }
}

/// NPC AnimSets (all compressed against the elite guard skeleton) used for third person.
const TP_SETS: &[&str] = &["Npc_EliteGuard_empty_as", "Npc_EliteGuard_Sword_as", "Npc_EliteGuard_Generic_as", "Npc_EliteGuard_Gun_as"];

/// Clips used by the third-person state machine.
pub const TP_CLIPS: &[&str] = &[
    "Empty_Idle", "Empty_WalkN", "Empty_WalkS_Revert", "Empty_WalkE", "Empty_WalkW_Revert", "Empty_RunN", "Empty_RunS_Revert", "Empty_RunE", "Empty_RunW_Revert", "Empty_RunN_Panic1",
    "Sword_Idle", "Sword_WalkN", "Sword_WalkS_Revert", "Sword_WalkE", "Sword_WalkW_Revert", "Sword_RunN", "Sword_RunS_Revert", "Sword_RunE", "Sword_RunW_Revert",
    "Sword_Patrol_Idle", "Sword_Patrol_WalkN", "Sword_Patrol_WalkS_Revert", "Sword_Patrol_WalkE", "Sword_Patrol_WalkW_Revert",
    "Sword_Attack_ShortBigA", "Sword_Attack_ShortBigB", "Sword_Attack_ShortBigC", "Sword_Ready_ParryIdle", "Sword_Versus_Stun_In", "Sword_Attack_FastA", "Sword_Cover_Loop",
    "Sword_Gun_IdleHold", "Sword_Gun_Fire", "Sword_Gun_Reload", "Sword_Gun_Draw",
    "Generic_DeathFrontA", "Generic_DeathBackA", "Generic_WindBlast_Back",
];

#[derive(Resource)]
pub struct Animations {
    pub clips: HashMap<String, (usize, dishonored_assets::edge_anim::BakedClip)>,
    /// Per set, per model, per part: bone index for each Edge joint (None = bone missing).
    pub track_to_bone: Vec<Vec<Vec<Vec<Option<usize>>>>>,
}

pub fn load_animations(mut commands: Commands, lib: Option<Res<CharacterLibrary>>) {
    let Some(lib) = lib else { return };
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    // The sets belong to the elite guard skeleton (archetype 1); Edge joints follow its bone order.
    let Some(guard) = lib.models.get(1).and_then(|m| m.parts.first()) else { return };
    let mut clips = HashMap::new();
    let mut track_to_bone = Vec::new();
    for (si, set) in TP_SETS.iter().enumerate() {
        match dishonored_assets::characters::load_anim_set(&content, "L_Boyle_Ext_Script", &format!("{set}.{set}"), TP_CLIPS) {
            Ok(mut anims) => {
                anims.edge_order(&guard.bones);
                track_to_bone.push(lib.models.iter().map(|m| m.parts.iter().map(|p| anims.tracks.iter().map(|t| p.bones.iter().position(|b| b.name.eq_ignore_ascii_case(t))).collect()).collect()).collect());
                for (name, clip) in anims.clips {
                    clips.insert(name, (si, clip));
                }
            }
            Err(e) => {
                warn!("{set}: {e:#}");
                track_to_bone.push(Vec::new());
            }
        }
    }
    info!("decoded {} third-person Edge clips", clips.len());
    commands.insert_resource(Animations { clips, track_to_bone });
}

pub struct TpClip {
    pub key: String,
    pub clip: &'static str,
    pub looped: bool,
    pub rate: f32,
}

/// Directional locomotion clip from velocity relative to facing.
fn directional(p: &PlayerState, n: &'static str, s: &'static str, e: &'static str, w: &'static str) -> &'static str {
    let v = p.velocity.truncate();
    let fwd = dishonored_sim::movement::forward_vector(p.yaw).truncate();
    let right = dishonored_sim::movement::right_vector(p.yaw).truncate();
    let (f, r) = (v.dot(fwd), v.dot(right));
    if f.abs() >= r.abs() {
        if f >= 0.0 { n } else { s }
    } else if r >= 0.0 {
        e
    } else {
        w
    }
}

/// Third-person clip from simulation state (simulation owns motion and timing; animation
/// only presents it). `armed`: a weapon is visibly drawn.
pub fn choose_tp_clip(p: &PlayerState, armed: bool) -> TpClip {
    use dishonored_sim::input::LeftHand;
    use dishonored_sim::state::{LeftPhase, PawnKind};
    let one = |key: &str, clip: &'static str| TpClip { key: key.to_string(), clip, looped: false, rate: 1.0 };
    let lp = |clip: &'static str, rate: f32| TpClip { key: clip.to_string(), clip, looped: true, rate };
    let gun = matches!(p.left.item, LeftHand::Pistol | LeftHand::Crossbow);
    if !p.alive || p.mode == MoveMode::Dead {
        return one("dead", if p.id % 2 == 0 { "Generic_DeathFrontA" } else { "Generic_DeathBackA" });
    }
    match p.sword.phase {
        SwordPhase::Startup | SwordPhase::Active | SwordPhase::Recovery => return one("swing", ["Sword_Attack_ShortBigA", "Sword_Attack_ShortBigB", "Sword_Attack_ShortBigC"][p.id as usize % 3]),
        SwordPhase::Blocking => return lp("Sword_Ready_ParryIdle", 1.0),
        SwordPhase::Staggered => return one("stagger", "Sword_Versus_Stun_In"),
        SwordPhase::Assassinating => return one("assassinate", "Sword_Attack_FastA"),
        SwordPhase::Idle => {}
    }
    match p.left.phase {
        LeftPhase::Recovering if gun => return one("fire", "Sword_Gun_Fire"),
        LeftPhase::Reloading => return one("reload", "Sword_Gun_Reload"),
        LeftPhase::Equipping if gun && armed => return one("draw", "Sword_Gun_Draw"),
        LeftPhase::Aiming if gun => return lp("Sword_Gun_IdleHold", 1.0),
        _ => {}
    }
    // Launched sideways (Windblast); a plain fall keeps the locomotion pose.
    if p.mode == MoveMode::Falling && p.velocity.truncate().length() > 700.0 {
        return one("flung", "Generic_WindBlast_Back");
    }
    let speed = p.velocity.truncate().length();
    if p.mode != MoveMode::Walking || speed < 20.0 {
        return match (p.kind, armed) {
            (PawnKind::Guard, _) => lp("Sword_Patrol_Idle", 1.0),
            (_, true) if p.crouched => lp("Sword_Cover_Loop", 1.0),
            (_, true) => lp("Sword_Idle", 1.0),
            _ => lp("Empty_Idle", 1.0),
        };
    }
    let run = speed > 300.0;
    // Playback rate follows ground speed (walk clips authored around 200 uu/s, run ~400).
    let rate = if run { speed / 400.0 } else { speed / 200.0 }.clamp(0.5, 1.8);
    let clip = match (p.kind, armed, run) {
        (PawnKind::Civilian, _, true) => "Empty_RunN_Panic1",
        (PawnKind::Guard, _, false) => directional(p, "Sword_Patrol_WalkN", "Sword_Patrol_WalkS_Revert", "Sword_Patrol_WalkE", "Sword_Patrol_WalkW_Revert"),
        (PawnKind::Guard, _, true) | (_, true, true) => directional(p, "Sword_RunN", "Sword_RunS_Revert", "Sword_RunE", "Sword_RunW_Revert"),
        (_, true, false) => directional(p, "Sword_WalkN", "Sword_WalkS_Revert", "Sword_WalkE", "Sword_WalkW_Revert"),
        (_, false, true) => directional(p, "Empty_RunN", "Empty_RunS_Revert", "Empty_RunE", "Empty_RunW_Revert"),
        (_, false, false) => directional(p, "Empty_WalkN", "Empty_WalkS_Revert", "Empty_WalkE", "Empty_WalkW_Revert"),
    };
    lp(clip, rate)
}

#[derive(Component, Default)]
pub struct AnimState {
    pub time: f32,
    pub key: String,
}

/// Pawn-space bone globals for a decoded clip (rotation-only retargeting by bone name:
/// bind translations keep each body's proportions).
pub fn decoded_globals(part: &PartGpu, map: &[Option<usize>], clip: &dishonored_assets::edge_anim::BakedClip, time: f32) -> Vec<glam::Mat4> {
    let mut bones = part.bones.clone();
    for (t, bone) in map.iter().enumerate() {
        let Some(b) = bone else { continue };
        if let (Some(q), _) = clip.sample(time, t) {
            bones[*b].rotation = q;
        }
    }
    bind_globals(&bones).iter().map(|g| part.mesh_to_pawn * *g).collect()
}

fn procedural_globals(part: &PartGpu, speed: f32, phase: f32, crouched: bool) -> Vec<glam::Mat4> {
    procedural_pose(part, speed, phase, crouched).iter().map(|m| from_bevy(*m)).collect()
}

/// Inverse of [`to_bevy`].
fn from_bevy(m: Mat4) -> glam::Mat4 {
    let swap = glam::Mat4::from_cols(glam::Vec4::X, glam::Vec4::Z, glam::Vec4::Y, glam::Vec4::W);
    let c = glam::Mat4::from_scale(glam::Vec3::splat(coords::UU_TO_BEVY)) * swap;
    c.inverse() * m * c
}
