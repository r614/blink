//! First-person view: Corvo's rig (Engine `Ply_Player.Skm_Player`) with the real sword,
//! pistol and crossbow on the hand sockets, rendered by an overlay camera (own FOV and
//! depth) and driven by decoded player AnimSets chosen from simulation state.
//!
//! Layering follows the data: the sword sets animate the whole rig (camera, spine, both
//! arms), the gun/power sets only the left arm, which overrides it. The rig is placed so
//! that the animated `camera_jnt` coincides with the game camera, so authored camera
//! motion becomes arm motion relative to the view. Presentation only.

use crate::characters::{CharacterGpu, to_bevy, upload};
use crate::players::GameCamera;
use crate::session::SessionView;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use dishonored_assets::characters::{bind_globals, load_anim_set, load_model};
use dishonored_assets::edge_anim::BakedClip;
use dishonored_ir::BoneAsset;
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::{LeftPhase, MoveMode, PlayerState, SwordPhase};
use std::collections::HashMap;

/// Render layer of the first-person overlay.
pub const VIEW_LAYER: usize = 1;
/// Horizontal field of view (`DefaultCamera.ini` m_fDefaultFOV=75, UE3 FOV is horizontal).
/// The first-person rig uses the same FOV (UE3 foreground depth group, no separate weapon FOV
/// found in the INIs).
pub const HORIZONTAL_FOV_DEG: f32 = 75.0;
/// Crossfade between clips (seconds).
const FADE: f32 = 0.12;

const SETS: &[&str] = &[
    "Ply_Sword_Idle_as",
    "Ply_Sword_Locomotion_as",
    "Ply_Sword_Attack_as",
    "Ply_Sword_Versus_as",
    "Ply_Sword_Hit_as",
    "Ply_Sword_Assassination_as",
    "Ply_Pistol_as",
    "Ply_Crossbow_as",
    "Ply_Guns_as",
    "Ply_Powers_as",
];

const CLIPS: &[&str] = &[
    "Sword_Ready_Idle", "Sword_Sneak_Idle", "Sword_Ready_Walk", "Sword_Sneak_Walk", "Sword_Ready_Run", "Sword_Sneak_Run", "Sword_Ready_Sprint",
    "Sword_Ready_Jump", "Sword_Ready_JumpLandSmall", "Sword_Ready_MantleMedium", "Sword_Ready_Equip",
    "Sword_Ready_AttackRight_A_Small", "Sword_Ready_AttackLeft_A_Small", "Sword_Ready_AttackRight_B_Small", "Sword_Ready_AttackLeft_B_Small", "Sword_Sneak_Attack_Small",
    "Sword_Ready_ParryIdle_In", "Sword_Ready_ParryIdle", "Sword_Ready_Parry_A", "Sword_Stun_In", "Sword_BigHit_Front", "Sword_Ready_Assassination_Back",
    "Pistol_Fire", "Pistol_FastReload", "Crossbow_Fire", "Crossbow_RapidReload",
    "Guns_Idle", "Guns_Walk", "Guns_Sprint", "Guns_Ready_Equip",
    "Powers_Idle", "Powers_Walk", "Powers_Sprint", "Powers_Ready_Equip", "Powers_Cast_Blink_In", "Powers_Cast_Blink_Loop", "Powers_Cast_Blink_Out", "Powers_Cast_Windblast", "Powers_Switch",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    Sword,
    Pistol,
    Crossbow,
}

const WEAPONS: &[(Held, &str, &str)] = &[
    (Held::Sword, "Wpn_PlySwords.Wpn_PlySword01", "handAttachment_R_jnt"),
    (Held::Pistol, "Wpn_PlyGuns.Wpn_PlyGunElite", "handAttachment_L_jnt"),
    (Held::Crossbow, "Crossbows.crossbow_01", "handAttachment_L_jnt"),
];

struct WeaponGpu {
    held: Held,
    gpu: CharacterGpu,
    socket: usize,
    /// Weapon bind globals in weapon mesh space.
    bind: Vec<glam::Mat4>,
    m2p: glam::Mat4,
}

#[derive(Resource)]
pub struct ViewRig {
    arms: CharacterGpu,
    bones: Vec<BoneAsset>,
    m2p: glam::Mat4,
    weapons: Vec<WeaponGpu>,
    /// Per AnimSet: track index -> bone index.
    sets: Vec<Vec<Option<usize>>>,
    clips: HashMap<String, (usize, BakedClip)>,
    left_arm: Vec<bool>,
    cam_bone: usize,
}

#[derive(Component)]
pub struct ViewRoot;
#[derive(Component)]
pub struct ViewCamera;
#[derive(Component)]
pub struct ViewJoint(usize);
#[derive(Component)]
pub struct WeaponJoint {
    weapon: usize,
    bone: usize,
}
#[derive(Component)]
pub struct WeaponMesh(usize);

pub fn load_view_rig(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    let arms = match load_model(&content, "fp_arms", &[("Engine", "Ply_Player.Skm_Player")], None, 2048) {
        Ok(m) => m,
        Err(e) => return warn!("first-person rig: {e:#}"),
    };
    let bones = arms.parts[0].bones.clone();
    let m2p = arms.parts[0].mesh_to_pawn;
    let find = |n: &str| bones.iter().position(|b| b.name.eq_ignore_ascii_case(n));
    let Some(cam_bone) = find("camera_jnt") else { return warn!("first-person rig: no camera_jnt") };
    let collar_l = find("Collarbone_L_Jnt");
    let mut left_arm = vec![false; bones.len()];
    for i in 0..bones.len() {
        let mut c = i as i32;
        while c >= 0 {
            if Some(c as usize) == collar_l {
                left_arm[i] = true;
                break;
            }
            let p = bones[c as usize].parent;
            c = if p == c || c == 0 { -1 } else { p };
        }
    }
    let mut weapons = Vec::new();
    for (wi, (held, path, socket)) in WEAPONS.iter().enumerate() {
        match load_model(&content, path, &[("Startup", path)], None, 1024) {
            Ok(w) => {
                let Some(socket) = find(socket) else { continue };
                let part = &w.parts[0];
                weapons.push(WeaponGpu { held: *held, socket, bind: bind_globals(&part.bones), m2p: part.mesh_to_pawn, gpu: upload(&w, &mut meshes, &mut images, &mut mats, &mut binds) });
                let _ = wi;
            }
            Err(e) => warn!("weapon {path}: {e:#}"),
        }
    }
    let mut sets = Vec::new();
    let mut clips = HashMap::new();
    for (si, set) in SETS.iter().enumerate() {
        match load_anim_set(&content, "Startup", &format!("{set}.{set}"), CLIPS) {
            Ok(lib) => {
                let mut lib = lib;
                lib.edge_order(&bones);
                sets.push(lib.tracks.iter().map(|t| find(t)).collect());
                for (name, clip) in lib.clips {
                    clips.insert(name, (si, clip));
                }
            }
            Err(e) => {
                warn!("{set}: {e:#}");
                sets.push(Vec::new());
            }
        }
    }
    info!("first-person rig: {} bones, {} weapons, {} clips", bones.len(), weapons.len(), clips.len());
    let arms_gpu = upload(&arms, &mut meshes, &mut images, &mut mats, &mut binds);
    commands.insert_resource(ViewRig { arms: arms_gpu, bones, m2p, weapons, sets, clips, left_arm, cam_bone });
}

/// Overlay camera (child of the game camera) plus the rig entities.
pub fn spawn_view_rig(
    mut commands: Commands,
    rig: Option<Res<ViewRig>>,
    cams: Query<(Entity, Option<&bevy::core_pipeline::tonemapping::Tonemapping>, Option<&bevy::render::view::ColorGrading>, Has<bevy::camera::Hdr>), With<GameCamera>>,
    existing: Query<(), With<ViewRoot>>,
) {
    let Some(rig) = rig else { return };
    if !existing.is_empty() {
        return;
    }
    let Some((game_cam, tonemap, grading, hdr)) = cams.iter().next() else { return };
    let layer = RenderLayers::layer(VIEW_LAYER);
    let cam = commands
        .spawn((
            Camera3d::default(),
            Camera { order: 1, clear_color: ClearColorConfig::None, ..default() },
            Projection::Perspective(PerspectiveProjection { fov: 47f32.to_radians(), near: 0.01, far: 50.0, ..default() }),
            Transform::default(),
            layer.clone(),
            ViewCamera,
            ChildOf(game_cam),
        ))
        .id();
    // Same output format and grading as the world camera so the overlay composites on the
    // same target with matching brightness.
    if hdr {
        commands.entity(cam).insert(bevy::camera::Hdr);
    }
    if let Some(t) = tonemap {
        commands.entity(cam).insert(*t);
    }
    if let Some(g) = grading {
        commands.entity(cam).insert(g.clone());
    }
    // The rig has no lightmap; a fill ambient stands in for the light environment that
    // lights dynamic objects in the original (only dominant/toggleable lights are dynamic).
    commands.entity(cam).insert(AmbientLight { color: Color::srgb(0.75, 0.82, 0.9), brightness: 350.0, ..default() });
    let root = commands.spawn((Transform::default(), Visibility::Hidden, ViewRoot, ChildOf(cam))).id();
    let part = &rig.arms.parts[0];
    let joints: Vec<Entity> = part.bind.iter().enumerate().map(|(i, b)| commands.spawn((Transform::from_matrix(*b), ViewJoint(i), ChildOf(root))).id()).collect();
    for (mesh, mat) in &part.sections {
        commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: joints.clone() }, Transform::default(), layer.clone(), NoFrustumCulling, ChildOf(root)));
    }
    for (wi, w) in rig.weapons.iter().enumerate() {
        let wp = &w.gpu.parts[0];
        let wroot = commands.spawn((Transform::default(), Visibility::Inherited, WeaponMesh(wi), ChildOf(root))).id();
        let joints: Vec<Entity> = wp.bind.iter().enumerate().map(|(i, b)| commands.spawn((Transform::from_matrix(*b), WeaponJoint { weapon: wi, bone: i }, ChildOf(root))).id()).collect();
        for (mesh, mat) in &wp.sections {
            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), SkinnedMesh { inverse_bindposes: wp.inverse_bindposes.clone(), joints: joints.clone() }, Transform::default(), layer.clone(), NoFrustumCulling, ChildOf(wroot)));
        }
    }
}

#[derive(Default)]
struct Layer {
    key: String,
    clip: String,
    time: f32,
    rate: f32,
    looped: bool,
    from: Option<Vec<(Quat, Vec3)>>,
    fade: f32,
}

impl Layer {
    fn set(&mut self, key: String, clip: &str, looped: bool, rate: f32, current: Option<Vec<(Quat, Vec3)>>) {
        self.rate = rate;
        if self.key == key {
            return;
        }
        self.key = key;
        self.clip = clip.to_string();
        self.time = 0.0;
        self.looped = looped;
        self.from = current;
        self.fade = 0.0;
    }
}

/// First-person animation state (local view only).
#[derive(Resource, Default)]
pub struct ViewAnim {
    right: Layer,
    left: Layer,
    swing: u32,
    prev_sword: SwordPhase,
    prev_left: LeftPhase,
    left_action: u32,
    prev_locals: Vec<(Quat, Vec3)>,
}

/// Right-hand (sword, full-body) clip for a player state: (key, clip, looped, rate).
fn right_clip(p: &PlayerState, a: &mut ViewAnim) -> (String, &'static str, bool, f32) {
    use SwordPhase::*;
    let attacking = matches!(p.sword.phase, Startup | Active | Recovery);
    if attacking && !matches!(a.prev_sword, Startup | Active | Recovery) {
        a.swing += 1;
    }
    a.prev_sword = p.sword.phase;
    let swing_clip = if p.crouched {
        "Sword_Sneak_Attack_Small"
    } else {
        ["Sword_Ready_AttackRight_A_Small", "Sword_Ready_AttackLeft_A_Small", "Sword_Ready_AttackRight_B_Small", "Sword_Ready_AttackLeft_B_Small"][a.swing as usize % 4]
    };
    let one = |k: &str, c: &'static str| (k.to_string(), c, false, 1.0);
    match p.sword.phase {
        Startup | Active | Recovery => return (format!("swing{}", a.swing), swing_clip, false, 1.0),
        Blocking => return if p.sword.block_time < 0.5 { one("block_in", "Sword_Ready_ParryIdle_In") } else { ("block".into(), "Sword_Ready_ParryIdle", true, 1.0) },
        Staggered => return one("stagger", "Sword_Stun_In"),
        Assassinating => return one("assassinate", "Sword_Ready_Assassination_Back"),
        Idle => {}
    }
    // Let a finished swing play out to its interruptible point before locomotion.
    if a.right.key.starts_with("swing") && a.right.time < 0.6 {
        return (a.right.key.clone(), swing_clip, false, 1.0);
    }
    let speed = p.velocity.truncate().length();
    match p.mode {
        MoveMode::Falling => return ("air".into(), "Sword_Ready_Jump", false, 1.0),
        MoveMode::Mantling => return one("mantle", "Sword_Ready_MantleMedium"),
        _ => {}
    }
    let sneak = p.crouched;
    if speed > 480.0 {
        ("sprint".into(), "Sword_Ready_Sprint", true, (speed / 600.0).clamp(0.6, 1.5))
    } else if speed > 260.0 {
        if sneak { ("run_s".into(), "Sword_Sneak_Run", true, (speed / 275.0).clamp(0.6, 1.5)) } else { ("run".into(), "Sword_Ready_Run", true, (speed / 400.0).clamp(0.6, 1.5)) }
    } else if speed > 30.0 {
        if sneak { ("walk_s".into(), "Sword_Sneak_Walk", true, (speed / 200.0).clamp(0.5, 1.6)) } else { ("walk".into(), "Sword_Ready_Walk", true, (speed / 200.0).clamp(0.5, 1.6)) }
    } else if sneak {
        ("idle_s".into(), "Sword_Sneak_Idle", true, 1.0)
    } else {
        ("idle".into(), "Sword_Ready_Idle", true, 1.0)
    }
}

/// Left-hand (gun/power, left arm only) clip.
fn left_clip(p: &PlayerState, a: &mut ViewAnim, tuning: &dishonored_sim::tuning::CombatTuning) -> Option<(String, &'static str, bool, f32)> {
    let item = p.left.item;
    if item == LeftHand::Empty {
        return None;
    }
    if p.left.phase != a.prev_left && p.left.phase != LeftPhase::Idle {
        a.left_action += 1;
    }
    a.prev_left = p.left.phase;
    let gun = matches!(item, LeftHand::Pistol | LeftHand::Crossbow);
    let n = a.left_action;
    let one = |c: &'static str, rate: f32| Some((format!("{c}{n}"), c, false, rate));
    match p.left.phase {
        LeftPhase::Equipping => return one(if gun { "Guns_Ready_Equip" } else { "Powers_Ready_Equip" }, 1.0),
        LeftPhase::Recovering => {
            return match item {
                LeftHand::Pistol => one("Pistol_Fire", 1.0),
                LeftHand::Crossbow => one("Crossbow_Fire", 1.0),
                LeftHand::Blink => one("Powers_Cast_Blink_Out", 1.0),
                LeftHand::Windblast => one("Powers_Cast_Windblast", 1.0),
                _ => one("Powers_Switch", 1.0),
            };
        }
        LeftPhase::Reloading => {
            // The reload clip is stretched to the simulation's reload time.
            return match item {
                LeftHand::Pistol => one("Pistol_FastReload", 0.8 / tuning.pistol_reload.max(0.1)),
                _ => one("Crossbow_RapidReload", 0.533 / tuning.crossbow_reload.max(0.1)),
            };
        }
        LeftPhase::Aiming if item == LeftHand::Blink => {
            return Some(if p.left.timer < 0.3 && !a.left.key.starts_with("blink_loop") { ("blink_in".into(), "Powers_Cast_Blink_In", false, 1.0) } else { ("blink_loop".into(), "Powers_Cast_Blink_Loop", true, 1.0) });
        }
        _ => {}
    }
    let speed = p.velocity.truncate().length();
    let (idle, walk, sprint) = if gun { ("Guns_Idle", "Guns_Walk", "Guns_Sprint") } else { ("Powers_Idle", "Powers_Walk", "Powers_Sprint") };
    Some(if p.mode == MoveMode::Walking && speed > 480.0 {
        (sprint.into(), sprint, true, 1.0)
    } else if p.mode == MoveMode::Walking && speed > 30.0 {
        (walk.into(), walk, true, (speed / 250.0).clamp(0.5, 1.6))
    } else {
        (idle.into(), idle, true, 1.0)
    })
}

fn sample_into(rig: &ViewRig, layer: &Layer, locals: &mut [(Quat, Vec3)], mask: Option<&[bool]>) {
    let Some((set, clip)) = rig.clips.get(&layer.clip) else { return };
    let t = if layer.looped { layer.time } else { layer.time.min(clip.duration - 1e-3).max(0.0) };
    let mut target = locals.to_vec();
    for (track, bone) in rig.sets[*set].iter().enumerate() {
        let Some(b) = *bone else { continue };
        if mask.is_some_and(|m| !m[b]) {
            continue;
        }
        let (r, p) = clip.sample(t, track);
        if let Some(r) = r {
            target[b].0 = r;
        }
        if let Some(p) = p {
            target[b].1 = p;
        }
    }
    let w = if layer.from.is_some() { (layer.fade / FADE).clamp(0.0, 1.0) } else { 1.0 };
    for (i, l) in locals.iter_mut().enumerate() {
        if mask.is_some_and(|m| !m[i]) {
            continue;
        }
        let from = layer.from.as_ref().and_then(|f| f.get(i)).copied().unwrap_or(*l);
        l.0 = from.0.slerp(target[i].0, w);
        l.1 = from.1.lerp(target[i].1, w);
    }
}

pub fn animate_view_rig(
    rig: Option<Res<ViewRig>>,
    mut anim: ResMut<ViewAnim>,
    view: Res<SessionView>,
    session: Res<crate::session::Session>,
    time: Res<Time>,
    cams: Query<&GameCamera>,
    mut roots: Query<(&mut Transform, &mut Visibility), (With<ViewRoot>, Without<ViewJoint>, Without<WeaponJoint>, Without<WeaponMesh>)>,
    mut joints: Query<(&ViewJoint, &mut Transform), (Without<ViewRoot>, Without<WeaponJoint>)>,
    mut wjoints: Query<(&WeaponJoint, &mut Transform), (Without<ViewRoot>, Without<ViewJoint>)>,
    mut wmeshes: Query<(&WeaponMesh, &mut Visibility), Without<ViewRoot>>,
) {
    let Some(rig) = rig else { return };
    let Ok((mut root_t, mut root_vis)) = roots.single_mut() else { return };
    let first_person = cams.iter().all(|c| !c.third_person && c.follow.is_none_or(|f| Some(f) == view.me.as_ref().map(|m| m.id)));
    let Some(me) = view.me.as_ref().filter(|m| m.alive && first_person && m.mode != MoveMode::Blinking) else {
        *root_vis = Visibility::Hidden;
        return;
    };
    *root_vis = Visibility::Inherited;
    let dt = time.delta_secs();
    let tuning = dishonored_sim::tuning::CombatTuning::from_config(&session.content.sim.gameplay);
    let a = &mut *anim;
    if a.prev_locals.len() != rig.bones.len() {
        a.prev_locals = rig.bones.iter().map(|b| (b.rotation, b.translation)).collect();
    }
    let (rk, rc, rl, rr) = right_clip(me, a);
    let snap = a.prev_locals.clone();
    a.right.set(rk, rc, rl, rr, Some(snap.clone()));
    match left_clip(me, a, &tuning) {
        Some((k, c, l, r)) => a.left.set(k, c, l, r, Some(snap)),
        None => a.left.set(String::new(), "", true, 1.0, Some(snap)),
    }
    for l in [&mut a.right, &mut a.left] {
        l.time += dt * l.rate;
        l.fade += dt;
    }
    let mut locals: Vec<(Quat, Vec3)> = rig.bones.iter().map(|b| (b.rotation, b.translation)).collect();
    sample_into(&rig, &a.right, &mut locals, None);
    if !a.left.clip.is_empty() {
        sample_into(&rig, &a.left, &mut locals, Some(&rig.left_arm));
    }
    a.prev_locals = locals.clone();
    let bones: Vec<BoneAsset> = rig.bones.iter().zip(&locals).map(|(b, (q, t))| BoneAsset { rotation: *q, translation: *t, ..b.clone() }).collect();
    let globals = bind_globals(&bones);
    if std::env::var_os("DISH_VM_DEBUG").is_some() {
        let cam = rig.m2p * globals[rig.cam_bone];
        let rel = |i: usize| cam.inverse().transform_point3((rig.m2p * globals[i]).w_axis.truncate());
        let names: Vec<String> = rig.weapons.iter().map(|w| format!("{}={:.0}", rig.bones[w.socket].name, rel(w.socket))).collect();
        info!("vm r={} l={} {}", a.right.clip, a.left.clip, names.join(" "));
    }
    // Camera bone frame: forward = -Z, right = +Y, up = +X (pawn space).
    let cam = rig.m2p * globals[rig.cam_bone];
    let v = glam::Mat4::from_cols(-cam.z_axis, cam.y_axis, cam.x_axis, cam.w_axis);
    // Pawn canonical (X fwd, Y right, Z up) in Bevy-converted space -> camera local.
    let a_rot = Mat4::from_quat(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    *root_t = Transform::from_matrix(a_rot * to_bevy(v).inverse());
    for (j, mut t) in &mut joints {
        *t = Transform::from_matrix(to_bevy(rig.m2p * globals[j.0]));
    }
    let attach = std::env::var("DISH_VM_ATTACH").unwrap_or_default();
    for (wj, mut t) in &mut wjoints {
        let w = &rig.weapons[wj.weapon];
        let base = rig.m2p * globals[w.socket];
        let m = match attach.as_str() {
            "A" => base * w.bind[wj.bone],
            "B" => base * w.m2p * w.bind[wj.bone],
            "C" => base * w.m2p.inverse() * w.bind[wj.bone],
            "E" => base * glam::Mat4::from_rotation_x(std::f32::consts::PI) * w.m2p.inverse() * w.bind[wj.bone],
            // Default: weapon mesh space -> socket through the inverse RotOrigin, turned half a
            // turn about the socket Z so the blade leaves the thumb side of the fist.
            _ if w.held == Held::Sword => base * glam::Mat4::from_rotation_z(std::f32::consts::PI) * w.m2p.inverse() * w.bind[wj.bone],
            // Guns: inverse RotOrigin alone puts the barrel forward in the fire/idle poses.
            _ => base * w.m2p.inverse() * w.bind[wj.bone],
        };
        *t = Transform::from_matrix(to_bevy(m));
    }
    for (wm, mut vis) in &mut wmeshes {
        let show = match rig.weapons[wm.0].held {
            Held::Sword => true,
            Held::Pistol => me.left.item == LeftHand::Pistol,
            Held::Crossbow => me.left.item == LeftHand::Crossbow,
        };
        *vis = if show { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Keep every perspective camera at the original's horizontal FOV for the current aspect
/// (Bevy's `fov` is vertical).
pub fn apply_fov(mut cams: Query<&mut Projection, With<Camera3d>>) {
    for mut p in &mut cams {
        if let Projection::Perspective(pp) = p.as_mut() {
            let v = 2.0 * ((HORIZONTAL_FOV_DEG.to_radians() * 0.5).tan() / pp.aspect_ratio.max(0.1)).atan();
            if (pp.fov - v).abs() > 1e-4 {
                pp.fov = v;
            }
        }
    }
}
