//! First-person view: Corvo's rig (Engine `Ply_Player.Skm_Player`) with the real sword,
//! pistol and crossbow on the hand sockets, drawn in the world camera (scaled about the eye)
//! and driven by decoded player AnimSets chosen from simulation state.
//!
//! Layering follows the data: the sword sets animate the whole rig (camera, spine, both
//! arms), the gun/power sets only the left arm, which overrides it. The rig is placed so
//! that the animated `camera_jnt` coincides with the game camera, so authored camera
//! motion becomes arm motion relative to the view. Presentation only.

use crate::characters::{CharacterGpu, to_bevy, upload};
use crate::players::GameCamera;
use crate::session::SessionView;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use dishonored_assets::characters::{bind_globals, load_anim_set, load_model};
use dishonored_assets::edge_anim::BakedClip;
use dishonored_ir::BoneAsset;
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::{LeftPhase, MoveMode, PlayerState, SwordPhase};
use std::collections::HashMap;

/// The rig is drawn in the world camera, shrunk about the eye: uniform scaling about the
/// projection centre leaves the image unchanged, keeps the arms within a few centimetres of
/// the eye (so they rarely intersect walls), and avoids a second render view (which cost a
/// full extra pass including shadow cascades).
const VIEW_SCALE: f32 = 0.2;
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
    "Ply_Sword_Choke_as",
    "Ply_Empty_CarryCorpse_as",
    "Ply_Gadgets_as",
    "Ply_Gadgets_Grenade_as",
    "Ply_Gadgets_SpringRazor_as",
];

const CLIPS: &[&str] = &[
    "Sword_Ready_Idle", "Sword_Sneak_Idle", "Sword_Ready_Walk", "Sword_Sneak_Walk", "Sword_Ready_Run", "Sword_Sneak_Run", "Sword_Ready_Sprint",
    "Sword_Ready_Jump", "Sword_Ready_JumpLandSmall", "Sword_Ready_MantleMedium", "Sword_Ready_Equip",
    "Sword_Ready_AttackRight_A_Small", "Sword_Ready_AttackLeft_A_Small", "Sword_Ready_AttackRight_B_Small", "Sword_Ready_AttackLeft_B_Small", "Sword_Sneak_Attack_Small",
    "Sword_Ready_ParryIdle_In", "Sword_Ready_ParryIdle", "Sword_Ready_Parry_A", "Sword_Stun_In", "Sword_BigHit_Front", "Sword_Ready_Assassination_Back",
    "Pistol_Fire", "Pistol_FastReload", "Crossbow_Fire", "Crossbow_RapidReload",
    "Guns_Idle", "Guns_Walk", "Guns_Sprint", "Guns_Ready_Equip",
    "Powers_Idle", "Powers_Walk", "Powers_Sprint", "Powers_Ready_Equip", "Powers_Cast_Blink_In", "Powers_Cast_Blink_Loop", "Powers_Cast_Blink_Out", "Powers_Cast_Windblast", "Powers_Switch",
    "Powers_Cast_BendTime", "Powers_Cast_Swarm", "Powers_Cast_Possession_In", "Powers_Cast_Possession_Loop", "Powers_Cast_Possession_Out", "Powers_Cast_Possession_Cancel", "Powers_OutOfMana",
    "Sword_Choke_In_Master", "Sword_Choke_Loop_Master", "Sword_Choke_Win_Master", "Sword_Choke_Lose_Master", "Sword_Choke_Cancel_Master", "Sword_Choke_CarryCorpse_Master",
    "Empty_CarryCorpse_In_Master", "Empty_CarryCorpse_Idle_Master", "Empty_CarryCorpse_Walk_Master", "Empty_CarryCorpse_DropLow_Master",
    "Gadgets_Idle", "Gadgets_Walk", "Gadgets_Sprint", "Gadgets_Ready_Equip", "Gadgets_Deploy", "Gadgets_Reload",
    "Gadgets_Grenade_Cooking_In", "Gadgets_Grenade_Cooking_Idle", "Gadgets_Grenade_Deploy", "Gadgets_SpringRazor_Deploy",
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
    /// The rig socket the weapon attaches to (`RightHandWpn` / `LeftHandWpn` of
    /// `Skm_Player`): transform relative to its bone.
    socket_rel: glam::Mat4,
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
    features: Option<Res<crate::map_render::RenderFeatures>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let feat = features.map(|f| f.clone()).unwrap_or_else(|| crate::map_render::RenderFeatures::from_flags(&|_| false));
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
    let sockets = dishonored_assets::characters::mesh_sockets(&content, "Engine", "Ply_Player.Skm_Player").unwrap_or_default();
    for (wi, (held, path, socket)) in WEAPONS.iter().enumerate() {
        match load_model(&content, path, &[("Startup", path)], None, 1024) {
            Ok(w) => {
                let Some(socket) = find(socket) else { continue };
                let part = &w.parts[0];
                let sock_name = if *held == Held::Sword { "RightHandWpn" } else { "LeftHandWpn" };
                let socket_rel = sockets.iter().find(|s| s.name.eq_ignore_ascii_case(sock_name)).map(|s| glam::Mat4::from_translation(s.location) * dishonored_assets::util::rotator_matrix(s.rotation[0], s.rotation[1], s.rotation[2])).unwrap_or(glam::Mat4::IDENTITY);
                weapons.push(WeaponGpu { held: *held, socket, bind: bind_globals(&part.bones), m2p: part.mesh_to_pawn, socket_rel, gpu: upload(&w, &mut meshes, &mut images, &mut mats, &mut binds, &feat) });
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
    let arms_gpu = upload(&arms, &mut meshes, &mut images, &mut mats, &mut binds, &feat);
    commands.insert_resource(ViewRig { arms: arms_gpu, bones, m2p, weapons, sets, clips, left_arm, cam_bone });
}

/// The rig, parented to the game camera and drawn in the main view (see `VIEW_SCALE`).
pub fn spawn_view_rig(
    mut commands: Commands,
    rig: Option<Res<ViewRig>>,
    cams: Query<Entity, With<GameCamera>>,
    existing: Query<(), With<ViewRoot>>,
) {
    let Some(rig) = rig else { return };
    if !existing.is_empty() {
        return;
    }
    if std::env::var_os("DISHONORED_NO_VIEWMODEL").is_some() {
        return;
    }
    let Some(game_cam) = cams.iter().next() else { return };
    let root = commands.spawn((Transform::default(), Visibility::Hidden, ViewRoot, ChildOf(game_cam))).id();
    let part = &rig.arms.parts[0];
    let joints: Vec<Entity> = part.bind.iter().enumerate().map(|(i, b)| commands.spawn((Transform::from_matrix(*b), ViewJoint(i), ChildOf(root))).id()).collect();
    for (mesh, mat) in &part.sections {
        commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: joints.clone() }, Transform::default(), NoFrustumCulling, bevy::light::NotShadowCaster, ChildOf(root)));
    }
    for (wi, w) in rig.weapons.iter().enumerate() {
        let wp = &w.gpu.parts[0];
        let wroot = commands.spawn((Transform::default(), Visibility::Inherited, WeaponMesh(wi), ChildOf(root))).id();
        let joints: Vec<Entity> = wp.bind.iter().enumerate().map(|(i, b)| commands.spawn((Transform::from_matrix(*b), WeaponJoint { weapon: wi, bone: i }, ChildOf(root))).id()).collect();
        for (mesh, mat) in &wp.sections {
            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), SkinnedMesh { inverse_bindposes: wp.inverse_bindposes.clone(), joints: joints.clone() }, Transform::default(), NoFrustumCulling, bevy::light::NotShadowCaster, ChildOf(wroot)));
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
    // Choke (`Ply_Sword_Choke_as` masters, by `eChokeState`) and carrying a body
    // (`Ply_Empty_CarryCorpse_as` masters): whole rig.
    {
        use dishonored_sim::state::ChokePhase as C;
        let choke = match p.choke.phase {
            C::In => Some(("choke_in", "Sword_Choke_In_Master", false)),
            C::Loop => Some(("choke_loop", "Sword_Choke_Loop_Master", true)),
            C::Win => Some(("choke_win", "Sword_Choke_Win_Master", false)),
            C::Lose => Some(("choke_lose", "Sword_Choke_Lose_Master", false)),
            C::Cancel => Some(("choke_cancel", "Sword_Choke_Cancel_Master", false)),
            C::None => None,
        };
        if let Some((k, c, l)) = choke {
            return (k.to_string(), c, l, 1.0);
        }
        if p.carrying.is_some() {
            if p.carry_anim > 0.0 {
                // Lifted straight from a won choke, or picked up from the ground.
                let from_choke = a.right.key.starts_with("choke") || a.right.key == "carry_choke";
                return if from_choke { one("carry_choke", "Sword_Choke_CarryCorpse_Master") } else { one("carry_in", "Empty_CarryCorpse_In_Master") };
            }
            let walking = p.mode == MoveMode::Walking && p.velocity.truncate().length() > 30.0;
            return if walking { ("carry_walk".into(), "Empty_CarryCorpse_Walk_Master", true, (p.velocity.truncate().length() / 200.0).clamp(0.6, 1.4)) } else { ("carry_idle".into(), "Empty_CarryCorpse_Idle_Master", true, 1.0) };
        }
        // The low drop plays out over the action lock that follows it.
        if p.action_lock > 0.0 && (a.right.key.starts_with("carry") || a.right.key == "carry_drop") {
            return one("carry_drop", "Empty_CarryCorpse_DropLow_Master");
        }
    }
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
    // Choking and carrying use both arms (whole-rig clips on the right layer).
    if item == LeftHand::Empty || p.carrying.is_some() || p.choke.phase != dishonored_sim::state::ChokePhase::None || a.right.key == "carry_drop" {
        return None;
    }
    let gadget = matches!(item, LeftHand::Grenade | LeftHand::StickyGrenade | LeftHand::SpringRazor);
    if p.left.phase != a.prev_left && p.left.phase != LeftPhase::Idle {
        a.left_action += 1;
    }
    a.prev_left = p.left.phase;
    let gun = matches!(item, LeftHand::Pistol | LeftHand::Crossbow) || item.is_crossbow();
    let n = a.left_action;
    let one = |c: &'static str, rate: f32| Some((format!("{c}{n}"), c, false, rate));
    // Possession: in while targeting, loop while possessing (`Ply_Powers_as`).
    {
        use dishonored_sim::state::PossessStage as S;
        if item == LeftHand::Possession {
            match p.possess.stage {
                S::Targeting => return Some(if p.left.timer < 0.3 && !a.left.key.starts_with("poss_loop") { ("poss_in".into(), "Powers_Cast_Possession_In", false, 1.0) } else { ("poss_loop".into(), "Powers_Cast_Possession_Loop", true, 1.0) }),
                S::PrePossess => return Some(("poss_out".into(), "Powers_Cast_Possession_Out", false, 1.0)),
                _ => {}
            }
        }
    }
    match p.left.phase {
        LeftPhase::Equipping => return one(if gun { "Guns_Ready_Equip" } else if gadget { "Gadgets_Ready_Equip" } else { "Powers_Ready_Equip" }, 1.0),
        LeftPhase::Recovering => {
            return match item {
                LeftHand::Pistol => one("Pistol_Fire", 1.0),
                LeftHand::Crossbow | LeftHand::SleepDart | LeftHand::IncendiaryBolt => one("Crossbow_Fire", 1.0),
                LeftHand::Blink => one("Powers_Cast_Blink_Out", 1.0),
                LeftHand::Windblast => one("Powers_Cast_Windblast", 1.0),
                LeftHand::BendTime => one("Powers_Cast_BendTime", 1.0),
                LeftHand::DevouringSwarm => one("Powers_Cast_Swarm", 1.0),
                LeftHand::Grenade | LeftHand::StickyGrenade => one("Gadgets_Grenade_Deploy", 1.0),
                LeftHand::SpringRazor => one("Gadgets_SpringRazor_Deploy", 1.0),
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
    let (idle, walk, sprint) = if gun {
        ("Guns_Idle", "Guns_Walk", "Guns_Sprint")
    } else if gadget {
        ("Gadgets_Idle", "Gadgets_Walk", "Gadgets_Sprint")
    } else {
        ("Powers_Idle", "Powers_Walk", "Powers_Sprint")
    };
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
    mut anim_sounds: Option<ResMut<crate::audio::AnimSounds>>,
    cutscenes: Option<Res<crate::cutscene::Cutscenes>>,
) {
    let Some(rig) = rig else { return };
    let Ok((mut root_t, mut root_vis)) = roots.single_mut() else { return };
    let first_person = cams.iter().all(|c| !c.third_person && c.follow.is_none_or(|f| Some(f) == view.me.as_ref().map(|m| m.id)));
    // [interp] A soiree's player group puts both hands' items away (`StatePlayerMasterSoiree`
    // enter, 0x00a66ff0: both hands blended to 1 through 0x00aa1cf0).
    let held = cutscenes.as_ref().is_some_and(|c| c.held);
    let Some(me) = view.me.as_ref().filter(|m| m.alive && first_person && m.mode != MoveMode::Blinking && !held) else {
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
        let from = l.time;
        l.time += dt * l.rate;
        // Sound notifies (AnimNotify_AkEvent, footfalls) of the advanced window.
        if let Some(s) = anim_sounds.as_mut() {
            s.advance(me.id, &l.clip, from, l.time, l.looped);
        }
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
    *root_t = Transform::from_matrix(Mat4::from_scale(Vec3::splat(VIEW_SCALE)) * a_rot * to_bevy(v).inverse());
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
            // The earlier visual choice for the sword (U-025): Rz(180) * inverse RotOrigin.
            "V" => base * glam::Mat4::from_rotation_z(std::f32::consts::PI) * w.m2p.inverse() * w.bind[wj.bone],
            // The original attach: the rig socket (`RightHandWpn` yaw +90 / `LeftHandWpn`
            // (0, -16384, 16384) on the hand attachment bones of `Skm_Player`), then the weapon
            // mesh's own Origin / RotOrigin. For the guns this equals the inverse RotOrigin used
            // before (the rotator is a 120-degree axis cycle); for the sword it keeps the blade
            // direction and fixes its roll.
            _ => base * w.socket_rel * w.m2p * w.bind[wj.bone],
        };
        *t = Transform::from_matrix(to_bevy(m));
    }
    for (wm, mut vis) in &mut wmeshes {
        // Carrying a body sheathes the sword and lowers the left hand.
        let carrying = me.carrying.is_some() || a.right.key == "carry_drop";
        let show = match rig.weapons[wm.0].held {
            Held::Sword => !carrying,
            Held::Pistol => me.left.item == LeftHand::Pistol && !carrying,
            Held::Crossbow => me.left.item.is_crossbow() && !carrying,
        };
        *vis = if show { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// Keep every perspective camera at the original's horizontal FOV for the current aspect
/// (Bevy's `fov` is vertical).
pub fn apply_fov(mut cams: Query<&mut Projection, With<Camera3d>>, settings: Option<Res<crate::settings::GameSettings>>) {
    // Options -> Graphics -> FOV (`PSI_GraphicsPC_FOV`, default 75 = HORIZONTAL_FOV_DEG).
    let fov = settings.map(|s| s.s.graphics.fov).unwrap_or(HORIZONTAL_FOV_DEG);
    for mut p in &mut cams {
        if let Projection::Perspective(pp) = p.as_mut() {
            let v = 2.0 * ((fov.to_radians() * 0.5).tan() / pp.aspect_ratio.max(0.1)).atan();
            if (pp.fov - v).abs() > 1e-4 {
                pp.fov = v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dishonored_assets::util::rotator_matrix;
    use glam::Mat4;

    fn close(a: Mat4, b: Mat4) -> bool {
        a.to_cols_array().iter().zip(b.to_cols_array()).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    /// The weapons' RotOrigin (0, -16384, 16384) is a 120-degree axis cycle, so the original's
    /// attach (socket `LeftHandWpn` = the same rotator, then RotOrigin) equals the inverse
    /// RotOrigin the guns use.
    #[test]
    fn gun_socket_equals_inverse_rot_origin() {
        let r = rotator_matrix(0, -16384, 16384);
        assert!(close(r * r, r.inverse()));
    }

    /// The sword's `RightHandWpn` socket (yaw +90) then RotOrigin puts the blade (mesh +Z) where
    /// the earlier visual choice Rz(180) * inverse(RotOrigin) did; the two differ by a roll
    /// about the blade.
    #[test]
    fn sword_socket_attach() {
        let r = rotator_matrix(0, -16384, 16384);
        let s = rotator_matrix(0, 16384, 0);
        let decoded = s * r;
        let chosen = Mat4::from_rotation_z(std::f32::consts::PI) * r.inverse();
        assert!((decoded.transform_vector3(glam::Vec3::Z) - chosen.transform_vector3(glam::Vec3::Z)).length() < 1e-4);
        assert!(!close(decoded, chosen));
    }
}
