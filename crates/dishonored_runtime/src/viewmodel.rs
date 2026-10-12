//! First-person view: Corvo's rig (Engine `Ply_Player.Skm_Player`) with the real sword,
//! pistol and crossbow on the hand sockets, drawn in the world camera (scaled about the eye)
//! and posed from the simulation's animation layers (`dishonored_sim::anim`: the sequences its
//! states play and their playheads; research/ARCHITECTURE.md step 7, the runtime picks none).
//!
//! The layering follows the player's AnimTree (`Startup` `Ply_Player_at`), whose state pickers
//! are the simulation's layers:
//! - the full-body layer (the paired moves' masters) poses the whole rig; with nothing on it the
//!   rig stands in `Sword_Ready_Idle`'s first frame, the navigation idle (`Nav_Default_Ready`),
//!   since the simulation requests no navigation state yet;
//! - `UpperBodyFilter` (`DisAnimNodeBlendPerBone`, branches `spine_3_jnt`, `Collarbone_R_Jnt`,
//!   `Collarbone_L_Jnt`) lays the upper-body layer over it (the swings, the block);
//! - `LeftArmFilter` (branch `Collarbone_L_Jnt`) lays the left-arm layer over that (fire,
//!   reload, casts, gadget deploys).
//!
//! A layer blends in over the requesting state's `m_fBlendTime` (`SeqPlay::blend`), counted on
//! the layer's own playhead, and out over the same time from its
//! `DishonoredNotify_AnimStateExit` or, without one, ending with the sequence, except the block
//! and the paired masters, which hold their last frame until the simulation stops them
//! (`layer_weights`); a sequence replacing another on a layer crossfades from the pose the layer
//! last showed. Nothing here advances a clock.
//!
//! Both filters cook `LocalToCompReqBones`, so they blend in component space (not
//! `bForceLocalSpaceBlend`): a filtered branch keeps the pose its own sequence gives it relative
//! to the root. `Collarbone_L_Jnt` hangs off bone 1 (the left filter's required bones are 0, 1
//! and the collarbone), so a local-space blend made the gun arm inherit the pelvis twist of the
//! full-body walk and run cycles.
//!
//! The weapon meshes play their own sets (`CorvoGun_as`, `Crossbow_as`, `PlySword_Anim_as`)
//! with the hand that holds them; the crossbow's fire is the simulation's weapon layer.
//!
//! The view follows the original's player camera (ANIMATION.md section 11, "View and arms"):
//! - the eye is at `camera_jnt`;
//! - the view looks along the controller's rotation plus 70 % (`m_RollAmount`) of that bone's
//!   animated rotation;
//! - the look controls of `Ply_Player_at` pitch both collarbones (plus each item's arm pitch
//!   offset) and `spine_3_jnt` with the look.
//!
//! The rig is placed relative to that view, so authored camera motion shows partly as arm
//! motion. Presentation only.

use crate::characters::{CharacterGpu, to_bevy, upload};
use crate::players::GameCamera;
use crate::session::SessionView;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use dishonored_assets::characters::{anim_set_path, bind_globals, load_anim_set, load_model};
use dishonored_assets::edge_anim::BakedClip;
use dishonored_ir::BoneAsset;
use dishonored_ir::gameplay::{GameplayConfig, SequenceInfo};
use dishonored_sim::anim::{SeqId, SeqPlay, SeqTable};
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::{ChokePhase, LeftPhase, PlayerState, SwordPhase};
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

/// The navigation idle the rig stands in while the simulation plays nothing on its full body
/// (`Nav_Default_Ready`'s idle with the sword's item prefix), at its first frame.
const STAND_IN: &str = "Sword_Ready_Idle";

/// `UpperBodyFilter.BranchStartBoneName`.
const UPPER_BODY_BRANCHES: &[&str] = &["spine_3_jnt", "Collarbone_R_Jnt", "Collarbone_L_Jnt"];
/// `LeftArmFilter.BranchStartBoneName`.
const LEFT_ARM_BRANCHES: &[&str] = &["Collarbone_L_Jnt"];

const SETS: &[&str] = &[
    "Ply_Sword_Idle_as",
    "Ply_Sword_Locomotion_as",
    "Ply_Sword_Attack_as",
    "Ply_Sword_Versus_as",
    "Ply_Sword_Hit_as",
    "Ply_Sword_Assassination_as",
    "Ply_Sword_Fatality_as",
    "Ply_Pistol_as",
    "Ply_Crossbow_as",
    "Ply_Guns_as",
    "Ply_Powers_as",
    "Ply_Sword_Choke_as",
    "Ply_Empty_CarryCorpse_as",
    "Ply_Gadgets_as",
    "Ply_Gadgets_Grenade_as",
    "Ply_Gadgets_SpringRazor_as",
    "Ply_Head_Locomotion_as",
    "Ply_Empty_Locomotion_as",
];

/// The rig's own sequences besides those the simulation's layers name (every sequence of the
/// table's player sets, `load_view_rig`): the stand-in idle and the additives.
const RIG_CLIPS: &[&str] = &[
    STAND_IN,
    // The held gun's aim (additive, `Ply_Guns_as`).
    "ADD_Guns_Aim_UpLeft", "ADD_Guns_Aim_CenterLeft", "ADD_Guns_Aim_DownLeft", "ADD_Guns_Aim_Up", "ADD_Guns_Aim_Center", "ADD_Guns_Aim_Down", "ADD_Guns_Aim_UpRight", "ADD_Guns_Aim_CenterRight", "ADD_Guns_Aim_DownRight",
    // Head bob (additive, `Ply_Head_Locomotion_as`).
    "ADD_Head_WalkN", "ADD_Head_WalkS", "ADD_Head_WalkW", "ADD_Head_WalkE", "ADD_Head_RunN", "ADD_Head_RunS", "ADD_Head_RunW", "ADD_Head_RunE",
    "ADD_Head_SprintN", "ADD_Head_SprintS", "ADD_Head_SprintW", "ADD_Head_SprintE", "ADD_Head_SneakN", "ADD_Head_SneakS", "ADD_Head_SneakW", "ADD_Head_SneakE",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    Sword,
    Pistol,
    Crossbow,
    /// Grenades and sticky grenades (`Grenade.Grenade`, `Grenade_at`).
    Grenade,
    SpringRazor,
}

/// (held, mesh, attach bone, the mesh's own AnimSets).
const WEAPONS: &[(Held, &str, &str, &[&str])] = &[
    (Held::Sword, "Wpn_PlySwords.Wpn_PlySword01", "handAttachment_R_jnt", &["PlySword_Anim_as"]),
    (Held::Pistol, "Wpn_PlyGuns.Wpn_PlyGunElite", "handAttachment_L_jnt", &["CorvoGun_as"]),
    (Held::Crossbow, "Crossbows.crossbow_01", "handAttachment_L_jnt", &["Crossbow_as"]),
    (Held::Grenade, "Grenade.Grenade", "handAttachment_L_jnt", &["Grenade_as"]),
    (Held::SpringRazor, "SpringRazor.SpringRazor", "handAttachment_L_jnt", &["SpringRazor_as"]),
];

/// The bolt on the crossbow's `Bolt_Socket` per ammunition (`Crossbows.bolt_01`, `Bolt_Sleep`,
/// `Bolt_Flare`).
const BOLTS: &[(LeftHand, &str)] = &[(LeftHand::Crossbow, "Crossbows.bolt_01"), (LeftHand::SleepDart, "Crossbows.Bolt_Sleep"), (LeftHand::IncendiaryBolt, "Crossbows.Bolt_Flare")];

/// Local bone transforms (rotation, translation).
pub type Pose = Vec<(Quat, Vec3)>;

/// Decoded sequences of some AnimSets, mapped to a skeleton.
#[derive(Default)]
pub(crate) struct ClipLib {
    /// Per AnimSet: track index -> bone index.
    sets: Vec<Vec<Option<usize>>>,
    clips: HashMap<String, (usize, BakedClip)>,
}

/// UE3 additive blending (`AnimNodeAdditiveBlending`). Edge stores additive sequences as deltas
/// from the reference pose (identity rotations and zero translations where nothing moves, seen
/// on `ADD_Head_WalkN` and `ADD_Guns_Aim_Up`); at weight `w` a delta turns a bone by
/// `slerp(identity, delta, w)` before its own rotation and moves it by `w` times its offset.
///
/// Additive children blended by weight (`AnimNodeBlendDirectional` over additive sequences,
/// `AnimNodeSequenceBlendByAim`): the deltas are summed by weight (rotations as a weighted
/// quaternion sum from identity, renormalised), then added once at `w`.
pub(crate) fn add_additive_blend(lib: &ClipLib, items: &[(String, f32, f32)], looped: bool, w: f32, pose: &mut [(Quat, Vec3)]) {
    let items: Vec<&(String, f32, f32)> = items.iter().filter(|i| i.2 > 0.0 && lib.has(&i.0)).collect();
    if w <= 0.0 || items.is_empty() {
        return;
    }
    let mut acc: Vec<(glam::Vec4, Vec3)> = vec![(glam::Vec4::ZERO, Vec3::ZERO); pose.len()];
    let mut total = 0.0;
    for (clip, time, iw) in items {
        let mut d: Pose = vec![(Quat::IDENTITY, Vec3::ZERO); pose.len()];
        lib.sample(clip, *time, looped, &mut d);
        for (a, (dq, dt)) in acc.iter_mut().zip(d) {
            let q = if dq.w < 0.0 { -dq } else { dq };
            a.0 += glam::Vec4::from(q) * *iw;
            a.1 += dt * *iw;
        }
        total += *iw;
    }
    // Weights short of 1 blend toward the identity delta.
    let rest = (1.0 - total).max(0.0);
    for (p, (q, t)) in pose.iter_mut().zip(acc) {
        let dq = Quat::from_vec4(q + glam::Vec4::new(0.0, 0.0, 0.0, rest)).normalize();
        p.0 = (Quat::IDENTITY.slerp(dq, w.min(1.0)) * p.0).normalize();
        p.1 += t * w.min(1.0);
    }
}

/// `AnimNodeSequenceBlendByAim` weights over its 3x3 grid for a normalised aim (x right, y
/// up, each in -1..1): bilinear between the centre and the side the aim leans to.
/// Order: LU, LC, LD, CU, CC, CD, RU, RC, RD.
pub fn aim_weights(x: f32, y: f32) -> [f32; 9] {
    let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
    let cols = [(-x).max(0.0), 1.0 - x.abs(), x.max(0.0)];
    let rows = [y.max(0.0), 1.0 - y.abs(), (-y).max(0.0)];
    let mut w = [0.0; 9];
    for (c, cw) in cols.iter().enumerate() {
        for (r, rw) in rows.iter().enumerate() {
            w[c * 3 + r] = cw * rw;
        }
    }
    w
}

/// The grid's sequence suffixes in `aim_weights` order (`m_bAutoCompleteAnimNames` builds
/// `ADD_` + the item prefix + `Aim_` + these).
const AIM_SUFFIXES: [&str; 9] = ["UpLeft", "CenterLeft", "DownLeft", "Up", "Center", "Down", "UpRight", "CenterRight", "DownRight"];
/// `DishonoredSeqBlendByAim` of `Ply_Player_at`: `m_fMaxLeftDegrees` = right = up = down = 10.
const AIM_MAX_DEGREES: f32 = 10.0;
/// `DisTweaks_InventoryItem.m_fItemAimSpeed` (3 for Corvo's pistol, crossbow and sword, and the
/// class default).
const ITEM_AIM_SPEED: f32 = 3.0;
/// `AnimNodeBlendDirectional` weights (forward, backward, left, right) for the movement
/// direction relative to the facing (radians, positive to the right): linear between
/// neighbouring quadrants (the cooked `Ply_Player_Nav_Default_at` snapshot, DirAngle 0.959,
/// weighs forward 0.389 and right 0.611).
pub fn directional_weights(a: f32) -> [f32; 4] {
    use std::f32::consts::{FRAC_PI_2, PI};
    if a < -FRAC_PI_2 {
        let t = (a + PI) / FRAC_PI_2;
        [0.0, 1.0 - t, t, 0.0]
    } else if a < 0.0 {
        let t = (a + FRAC_PI_2) / FRAC_PI_2;
        [t, 0.0, 1.0 - t, 0.0]
    } else if a < FRAC_PI_2 {
        let t = a / FRAC_PI_2;
        [1.0 - t, 0.0, 0.0, t]
    } else {
        let t = (a - FRAC_PI_2) / FRAC_PI_2;
        [0.0, t, 0.0, 1.0 - t]
    }
}

/// `DishonoredPlayerCamera.m_RollAmount` (`DefaultCamera.ini` 0.7): the share of the camera
/// bone's animated rotation (relative to the pawn) that the view takes. The pawn's view rotation
/// (`DishonoredPlayerPawn` vfunc +0x3E8, 0x00AABB70) is the controller's plus this share of the
/// bone's pitch (minus the camera look control's own pitch), yaw and roll.
pub const ROLL_AMOUNT: f32 = 0.7;

/// `DisTweaks_InventoryItem_PlayerSpecific.m_ArmPitch_OffsetKeys` (view pitch, arm pitch
/// difference, both degrees): Corvo's sword (`Twk_Inv_SwordCorvo`), the powers
/// (`Twk_Inv_Powers`), grenades (`Twk_Inv_GrenadeCorvo`) and the Spring Razor
/// (`Twk_Inv_SpringRazorCorvo`).
pub const ARM_PITCH_KEYS_DEFAULT: [(f32, f32); 4] = [(-90.0, 10.0), (-10.0, 0.0), (10.0, 0.0), (90.0, -15.0)];
/// The same for the pistol (`Twk_Inv_PistolCorvo`) and the crossbow (`Twk_Inv_CrossbowCorvo`).
pub const ARM_PITCH_KEYS_GUN: [(f32, f32); 4] = [(-90.0, 2.0), (-10.0, 0.0), (10.0, 0.0), (90.0, -3.0)];

/// The arm pitch offset of an item for a view pitch (0x00ACB8D0): between the bracketing keys
/// with a cosine ease, the first / last key's difference outside them.
pub fn arm_pitch_offset(keys: &[(f32, f32)], pitch: f32) -> f32 {
    let Some(next) = keys.iter().position(|k| k.0 > pitch) else {
        return keys.last().map(|k| k.1).unwrap_or(0.0);
    };
    if next == 0 {
        return keys[0].1;
    }
    let (a, b) = (keys[next - 1], keys[next]);
    let t = ((pitch - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    (b.1 - a.1) * (1.0 - (t * std::f32::consts::PI).cos()) * 0.5 + a.1
}

/// A UE3 rotator (pitch, yaw, roll in degrees) as a rotation of pawn space (X forward, Y right,
/// Z up): roll about X, then pitch, then yaw (`FRotationMatrix`; positive pitch lifts X toward
/// Z, positive roll lowers Y).
pub fn rotator_quat(pitch: f32, yaw: f32, roll: f32) -> Quat {
    Quat::from_rotation_z(yaw.to_radians()) * Quat::from_rotation_y(-pitch.to_radians()) * Quat::from_rotation_x(-roll.to_radians())
}

/// The rotator of a frame given by its forward, right and up axes in pawn space (the matrix to
/// rotator conversion of 0x0041D720).
pub fn frame_rotator(fwd: Vec3, right: Vec3, up: Vec3) -> (f32, f32, f32) {
    let pitch = fwd.z.atan2(fwd.truncate().length());
    let yaw = fwd.y.atan2(fwd.x);
    let y0 = Vec3::new(-yaw.sin(), yaw.cos(), 0.0);
    let roll = up.dot(y0).atan2(right.dot(y0));
    (pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees())
}

fn wrap_deg(a: f32) -> f32 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// The view's rotation relative to the pawn from the camera bone's frame (after the camera look
/// control) and the look pitch (0x00AABB70): the look pitch plus `ROLL_AMOUNT` of what the
/// animation adds to it.
pub fn view_rotation(fwd: Vec3, right: Vec3, up: Vec3, look_pitch: f32) -> Quat {
    let (p, y, r) = frame_rotator(fwd, right, up);
    rotator_quat(look_pitch + ROLL_AMOUNT * wrap_deg(p - look_pitch), ROLL_AMOUNT * wrap_deg(y), ROLL_AMOUNT * wrap_deg(r))
}

/// `SkelControlSingleBone` with `bApplyRotation`, `bAddRotation` and `BoneRotationSpace =
/// BCS_ActorSpace` (the look controls of `Ply_Player_at`): each listed bone turns by a pitch
/// (degrees, positive up) about its own origin in actor (pawn) space, its children following.
/// `mesh_to_pawn` is the mesh's Origin / RotOrigin transform.
pub fn apply_look_controls(parents: &[Option<usize>], local: &mut [(Quat, Vec3)], mesh_to_pawn: glam::Mat4, controls: &[(usize, f32)]) {
    let comp = to_component(parents, local);
    let (_, m2p_rot, _) = mesh_to_pawn.to_scale_rotation_translation();
    for &(bone, pitch) in controls {
        if bone >= local.len() || pitch == 0.0 {
            continue;
        }
        let actor = Quat::from_rotation_y(-pitch.to_radians());
        let turn = m2p_rot.inverse() * actor * m2p_rot;
        let new_rot = (turn * comp[bone].0).normalize();
        let parent_rot = parents.get(bone).copied().flatten().map(|p| comp[p].0).unwrap_or(Quat::IDENTITY);
        local[bone].0 = (parent_rot.inverse() * new_rot).normalize();
    }
}

/// Playable length: the clip's duration, at most its last frame.
fn clip_len(c: &BakedClip) -> f32 {
    let frames = c.num_frames.saturating_sub(1) as f32 / c.sample_rate.max(1.0);
    if frames > 0.0 { c.duration.min(frames) } else { c.duration }
}

impl ClipLib {
    fn load(content: &dishonored_assets::Content, bones: &[BoneAsset], paths: &[String], wanted: &[&str]) -> Self {
        Self::load_from(content, "Startup", bones, paths, wanted)
    }

    /// Load the named sequences (all when `wanted` is empty) of AnimSets of a package.
    pub(crate) fn load_from(content: &dishonored_assets::Content, pkg: &str, bones: &[BoneAsset], paths: &[String], wanted: &[&str]) -> Self {
        let mut lib = ClipLib::default();
        for path in paths {
            match load_anim_set(content, pkg, path, wanted) {
                Ok(mut set) => {
                    set.edge_order(bones);
                    let si = lib.sets.len();
                    lib.sets.push(set.tracks.iter().map(|t| bones.iter().position(|b| b.name.eq_ignore_ascii_case(t))).collect());
                    for (name, clip) in set.clips {
                        lib.clips.insert(name, (si, clip));
                    }
                }
                Err(e) => warn!("{path}: {e:#}"),
            }
        }
        lib
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.clips.contains_key(name)
    }

    pub(crate) fn length(&self, name: &str) -> f32 {
        self.clips.get(name).map(|(_, c)| clip_len(c)).unwrap_or(0.0)
    }

    /// Overwrite the bones the clip drives with its pose at `time` (clamped unless looped).
    pub(crate) fn sample(&self, name: &str, time: f32, looped: bool, pose: &mut [(Quat, Vec3)]) {
        let Some((set, clip)) = self.clips.get(name) else { return };
        let t = if looped { time } else { time.clamp(0.0, (clip_len(clip) - 1e-3).max(0.0)) };
        for (track, bone) in self.sets[*set].iter().enumerate() {
            let Some(b) = *bone else { continue };
            if b >= pose.len() {
                continue;
            }
            let (r, p) = clip.sample(t, track);
            if let Some(r) = r {
                pose[b].0 = r;
            }
            if let Some(p) = p {
                pose[b].1 = p;
            }
        }
    }
}

struct WeaponGpu {
    held: Held,
    gpu: CharacterGpu,
    socket: usize,
    /// The weapon's skeleton and its bind pose.
    bones: Vec<BoneAsset>,
    bind_pose: Pose,
    /// Weapon bind globals in weapon mesh space.
    bind: Vec<glam::Mat4>,
    m2p: glam::Mat4,
    /// The rig socket the weapon attaches to (`RightHandWpn` / `LeftHandWpn` of
    /// `Skm_Player`): transform relative to its bone.
    socket_rel: glam::Mat4,
    lib: ClipLib,
}

/// A bolt mesh shown loaded on the crossbow.
struct BoltGpu {
    ammo: LeftHand,
    gpu: CharacterGpu,
    bind: Vec<glam::Mat4>,
    m2p: glam::Mat4,
    /// The crossbow (index in ViewRig::weapons), the bone of its Bolt_Socket and the socket
    /// transform relative to that bone.
    weapon: usize,
    bone: usize,
    socket_rel: glam::Mat4,
}

#[derive(Resource)]
pub struct ViewRig {
    arms: CharacterGpu,
    bones: Vec<BoneAsset>,
    parents: Vec<Option<usize>>,
    bind_pose: Pose,
    m2p: glam::Mat4,
    weapons: Vec<WeaponGpu>,
    bolts: Vec<BoltGpu>,
    lib: ClipLib,
    /// The sword's swing trail (`melee_trail` of `Twk_Inv_SwordCorvo`): its template and the
    /// extent's corner sockets on the sword mesh (bone, location).
    sword_trail: Option<(String, [(usize, glam::Vec3); 2])>,
    /// The rig's sockets: name -> (bone, transform relative to it).
    sockets: HashMap<String, (usize, glam::Mat4)>,
    /// Sockets and bones the hand effects are placed on (`effects::HandEffects`): the cast socket
    /// and the points of the particle notifies of the table's player sequences.
    effect_points: Vec<String>,
    upper_body: Vec<bool>,
    left_arm: Vec<bool>,
    cam_bone: usize,
    /// The bones of `Ply_Player_at`'s look controls: `LookAtControl_RightHand`
    /// (`Collarbone_R_Jnt`), `LookAtControl_LeftHand` (`Collarbone_L_Jnt`) and
    /// `LookAtControl_Camera` (`spine_3_jnt`).
    look_bones: [Option<usize>; 3],
}

/// The arms' materials that carry the power glow (`PlayerArms_Tatoo_inst`, EFFECTS.md "Arm glow").
#[derive(Resource, Default)]
pub struct ArmGlowMats(pub Vec<Handle<StandardMaterial>>);

/// `Shr_DefaultPlayer`'s power-hand term, disassembled from `TBasePassPixelShaderFNoLightMapPolicy`
/// (RefShaderCache shader 37254; the same term is in every lighting policy's pixel shader). With
/// `Use_Power` = U and the instance's `F_Firy_Amount` 0.996 (the firy terms vanish: `clamp(1 -
/// 3 x amount) = 0`), the emitted colour is `U x G(uv)`:
/// `G = (2 - Ag) x P + Ag x SP.z x |TG|^Sat`, with `P = (1 - Tex#4.y) x (1 - D.r)^Amount x
/// Power_Hand_Color` where `1 - D.r >= 1e-6` (else 0), D = `D_Diffuse` (PlayerArmsTatooed_D),
/// SP = `SP_SpecPower` (PlayerArms_m), TG = `Tattoo_Glow_Color` (1, 0.5, 0), Sat =
/// `Tattoo_Glow_Saturation` 2, Ag = `Tattoo_Glow_Amount` 0.2, `Amount` = `Power_Hand_Amount` 40,
/// `Power_Hand_Color` = the instance's (10, 5, 1). G depends only on textures, so it is baked once
/// into an HDR emissive texture and `Use_Power` scales the material's emissive colour.
/// LIKELY: the shader map names the mask `Tex#4`; it is taken to be `M_Mask` (`PlayerArms_M2`),
/// the only unused-by-name mask in the material.
const POWER_HAND_COLOR: [f32; 3] = [10.0, 5.0, 1.0];
const POWER_HAND_AMOUNT: f32 = 40.0;
const TATTOO_GLOW_COLOR: [f32; 3] = [1.0, 0.5, 0.0];
const TATTOO_GLOW_SATURATION: f32 = 2.0;
const TATTOO_GLOW_AMOUNT: f32 = 0.2;

fn arm_glow_materials(content: &dishonored_assets::Content, arms: &CharacterGpu, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>) -> Vec<Handle<StandardMaterial>> {
    use dishonored_assets::texture::{decode_rgba8, read_texture};
    use ue3_package::ObjRef;
    let lin = |v: u8, srgb: bool| {
        let c = v as f32 / 255.0;
        if srgb { if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } } else { c }
    };
    // (width, height, texels as linear 0..1 rgba)
    let load = |path: &str| -> Option<(u32, u32, Vec<[f32; 4]>)> {
        for pkg_name in ["Engine", "Startup"] {
            let Ok(pkg) = content.package(pkg_name) else { continue };
            let Some(i) = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Texture2D" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path)) else { continue };
            let t = read_texture(content, &pkg, i, 512).ok()?;
            let (w, h, px) = decode_rgba8(&t, 0)?;
            return Some((w, h, px.iter().map(|p| [lin(p[0], t.srgb), lin(p[1], t.srgb), lin(p[2], t.srgb), p[3] as f32 / 255.0]).collect()));
        }
        None
    };
    let (Some(d), Some(sp), Some(m)) = (load("Ply_Player.PlayerArmsTatooed_D"), load("Ply_Player.PlayerArms_m"), load("Ply_Player.PlayerArms_M2")) else {
        warn!("arm glow: PlayerArms textures not found");
        return Vec::new();
    };
    let at = |t: &(u32, u32, Vec<[f32; 4]>), u: f32, v: f32| {
        let x = ((u * t.0 as f32) as i64).clamp(0, t.0 as i64 - 1) as usize;
        let y = ((v * t.1 as f32) as i64).clamp(0, t.1 as i64 - 1) as usize;
        t.2[y * t.0 as usize + x]
    };
    let tg: Vec<f32> = TATTOO_GLOW_COLOR.iter().map(|c| c.abs().max(1.0e-6).powf(TATTOO_GLOW_SATURATION)).collect();
    let mut data: Vec<u8> = Vec::with_capacity(d.2.len() * 16);
    for y in 0..d.1 {
        for x in 0..d.0 {
            let (u, v) = ((x as f32 + 0.5) / d.0 as f32, (y as f32 + 0.5) / d.1 as f32);
            let dr = at(&d, u, v)[0];
            let spz = at(&sp, u, v)[2];
            let mask = at(&m, u, v)[1];
            let dark = 1.0 - dr;
            let p = if dark - 1.0e-6 >= 0.0 { (1.0 - mask) * dark.abs().powf(POWER_HAND_AMOUNT) } else { 0.0 };
            for c in 0..3 {
                let g = (2.0 - TATTOO_GLOW_AMOUNT) * p * POWER_HAND_COLOR[c] + TATTOO_GLOW_AMOUNT * spz * tg[c];
                data.extend_from_slice(&g.to_le_bytes());
            }
            data.extend_from_slice(&1.0f32.to_le_bytes());
        }
    }
    let img = images.add(Image::new(
        bevy::render::render_resource::Extent3d { width: d.0, height: d.1, depth_or_array_layers: 1 },
        bevy::render::render_resource::TextureDimension::D2,
        data,
        bevy::render::render_resource::TextureFormat::Rgba32Float,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    ));
    // The sections whose material is the tattoo instance (its `Power_Hand_Color` override).
    let mut out = Vec::new();
    for part in &arms.parts {
        for (si, (_, h)) in part.sections.iter().enumerate() {
            let Some(mi) = part.section_materials.get(si).copied().flatten() else { continue };
            if arms.material_params.get(mi).is_some_and(|p| p.vectors.iter().any(|(n, _)| n.eq_ignore_ascii_case("Power_Hand_Color"))) && !out.contains(h) {
                if let Some(mut m) = mats.get_mut(h) {
                    m.emissive_texture = Some(img.clone());
                    m.emissive = LinearRgba::BLACK;
                    out.push(h.clone());
                }
            }
        }
    }
    if out.is_empty() {
        warn!("arm glow: no arm section uses PlayerArms_Tatoo_inst (material params not resolved?)");
    }
    out
}

/// `Use_Power` (`power_pp::ArmGlow`) on the arms' emissive colour.
pub fn arm_glow_apply(glow: Res<crate::power_pp::ArmGlow>, handles: Option<Res<ArmGlowMats>>, mut mats: ResMut<Assets<StandardMaterial>>, mut light_mats: ResMut<Assets<crate::char_light::CharMaterial>>, mut last: Local<f32>) {
    let Some(handles) = handles else { return };
    if (*last - glow.value).abs() < 1.0e-4 || handles.0.is_empty() {
        return;
    }
    *last = glow.value;
    let glow_colour = LinearRgba::rgb(glow.value, glow.value, glow.value);
    let mut textures = Vec::new();
    for h in &handles.0 {
        if let Some(mut m) = mats.get_mut(h) {
            m.emissive = glow_colour;
            textures.extend(m.emissive_texture.clone());
        }
    }
    // The arms are lit by their light environment (char_light.rs), which draws copies of these
    // materials: the copies sharing the glow texture take the same colour.
    let lit: Vec<_> = light_mats.iter().filter(|(_, m)| m.base.emissive_texture.as_ref().is_some_and(|t| textures.contains(t))).map(|(id, _)| id).collect();
    for id in lit {
        if let Some(mut m) = light_mats.get_mut(id) {
            m.base.emissive = glow_colour;
        }
    }
}

impl ViewRig {
    /// The arms' uploaded model (`char_light.rs` lights it with the pawn's light environment).
    pub fn arms(&self) -> &CharacterGpu {
        &self.arms
    }
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
#[derive(Component)]
pub struct BoltJoint {
    bolt: usize,
    bone: usize,
}
#[derive(Component)]
pub struct BoltMesh(usize);

/// Parent of each bone (`None` for a root), read as `bind_globals` reads `BoneAsset::parent`.
pub fn parents(bones: &[BoneAsset]) -> Vec<Option<usize>> {
    bones.iter().enumerate().map(|(i, b)| (i != 0 && b.parent >= 0 && (b.parent as usize) < i).then_some(b.parent as usize)).collect()
}

/// Bones at or below any of `branches` (`AnimNodeBlendPerBone::BranchStartBoneName`, whose
/// cooked `Child2PerBoneWeight` is 1 over those subtrees).
pub fn branch_mask(bones: &[BoneAsset], branches: &[&str]) -> Vec<bool> {
    let par = parents(bones);
    let mut mask = vec![false; bones.len()];
    for (i, b) in bones.iter().enumerate() {
        let on = branches.iter().any(|n| b.name.eq_ignore_ascii_case(n)) || par[i].is_some_and(|p| mask[p]);
        mask[i] = on;
    }
    mask
}

fn compose(parent: (Quat, Vec3), local: (Quat, Vec3)) -> (Quat, Vec3) {
    ((parent.0 * local.0).normalize(), parent.1 + parent.0 * local.1)
}

fn relative(parent: (Quat, Vec3), comp: (Quat, Vec3)) -> (Quat, Vec3) {
    let inv = parent.0.inverse();
    ((inv * comp.0).normalize(), inv * (comp.1 - parent.1))
}

fn mix(a: (Quat, Vec3), b: (Quat, Vec3), w: f32) -> (Quat, Vec3) {
    let q = if a.0.dot(b.0) < 0.0 { -b.0 } else { b.0 };
    (a.0.lerp(q, w).normalize(), a.1.lerp(b.1, w))
}

/// Component-space (mesh-space) transforms of a local pose.
pub fn to_component(parents: &[Option<usize>], local: &[(Quat, Vec3)]) -> Pose {
    let mut c: Pose = Vec::with_capacity(local.len());
    for (i, l) in local.iter().enumerate() {
        let v = match parents.get(i).copied().flatten() {
            Some(p) => compose(c[p], *l),
            None => *l,
        };
        c.push(v);
    }
    c
}

/// `AnimNodeBlendPerBone` blending in component space: a masked bone takes the blend (by
/// `weight`) of the two children's component-space transforms, converted back to local space
/// under the result's parent; other bones keep the source's local transform.
pub fn blend_per_bone(parents: &[Option<usize>], source: &[(Quat, Vec3)], target: &[(Quat, Vec3)], mask: &[bool], weight: f32) -> Pose {
    if weight <= 0.0 || source.len() != target.len() {
        return source.to_vec();
    }
    let w = weight.min(1.0);
    let cs = to_component(parents, source);
    let ct = to_component(parents, target);
    let mut out = source.to_vec();
    let mut oc: Pose = Vec::with_capacity(source.len());
    for (i, src) in source.iter().enumerate() {
        let parent = parents.get(i).copied().flatten().map(|p| oc[p]);
        if mask.get(i).copied().unwrap_or(false) {
            let c = mix(cs[i], ct[i], w);
            out[i] = parent.map_or(c, |p| relative(p, c));
            oc.push(c);
        } else {
            oc.push(parent.map_or(*src, |p| compose(p, *src)));
        }
    }
    out
}

pub fn load_view_rig(
    mut commands: Commands,
    session: Res<crate::session::Session>,
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
    for n in UPPER_BODY_BRANCHES {
        if find(n).is_none() {
            warn!("first-person rig: no {n}");
        }
    }
    let upper_body = branch_mask(&bones, UPPER_BODY_BRANCHES);
    let left_arm = branch_mask(&bones, LEFT_ARM_BRANCHES);
    let mut weapons = Vec::new();
    let sockets = dishonored_assets::characters::mesh_sockets(&content, "Engine", "Ply_Player.Skm_Player").unwrap_or_default();
    for (held, path, socket, sets) in WEAPONS {
        match load_model(&content, path, &[("Startup", path)], None, 1024) {
            Ok(w) => {
                let Some(socket) = find(socket) else { continue };
                let part = &w.parts[0];
                let sock_name = if *held == Held::Sword { "RightHandWpn" } else { "LeftHandWpn" };
                let socket_rel = sockets.iter().find(|s| s.name.eq_ignore_ascii_case(sock_name)).map(|s| glam::Mat4::from_translation(s.location) * dishonored_assets::util::rotator_matrix(s.rotation[0], s.rotation[1], s.rotation[2])).unwrap_or(glam::Mat4::IDENTITY);
                let set_paths: Vec<String> = sets
                    .iter()
                    .filter_map(|s| match anim_set_path(&content, "Startup", s) {
                        Ok(p) => Some(p),
                        Err(e) => {
                            warn!("weapon {path}: {e:#}");
                            None
                        }
                    })
                    .collect();
                let lib = ClipLib::load(&content, &part.bones, &set_paths, &[]);
                let mut names: Vec<&String> = lib.clips.keys().collect();
                names.sort();
                info!("first-person weapon {path}: sets {set_paths:?}, clips {names:?}");
                weapons.push(WeaponGpu {
                    held: *held,
                    socket,
                    bones: part.bones.clone(),
                    bind_pose: part.bones.iter().map(|b| (b.rotation, b.translation)).collect(),
                    bind: bind_globals(&part.bones),
                    m2p: part.mesh_to_pawn,
                    socket_rel,
                    lib,
                    gpu: upload(&w, &mut meshes, &mut images, &mut mats, &mut binds, &feat),
                });
            }
            Err(e) => warn!("weapon {path}: {e:#}"),
        }
    }
    // Bolts on the crossbow's `Bolt_Socket` (`m_ArrowSocketName`).
    let mut bolts = Vec::new();
    if let Some(cw) = weapons.iter().position(|w| w.held == Held::Crossbow) {
        let cb_sockets = dishonored_assets::characters::mesh_sockets(&content, "Startup", "Crossbows.crossbow_01").unwrap_or_default();
        let sock = cb_sockets.iter().find(|s| s.name.eq_ignore_ascii_case("Bolt_Socket"));
        let bone = sock.and_then(|s| weapons[cw].bones.iter().position(|b| b.name.eq_ignore_ascii_case(&s.bone)));
        match (sock, bone) {
            (Some(s), Some(bone)) => {
                let socket_rel = glam::Mat4::from_translation(s.location) * dishonored_assets::util::rotator_matrix(s.rotation[0], s.rotation[1], s.rotation[2]);
                for (ammo, path) in BOLTS {
                    match load_model(&content, path, &[("Startup", path)], None, 512) {
                        Ok(m) => {
                            let part = &m.parts[0];
                            bolts.push(BoltGpu { ammo: *ammo, bind: bind_globals(&part.bones), m2p: part.mesh_to_pawn, weapon: cw, bone, socket_rel, gpu: upload(&m, &mut meshes, &mut images, &mut mats, &mut binds, &feat) });
                        }
                        Err(e) => warn!("bolt {path}: {e:#}"),
                    }
                }
            }
            _ => warn!("first-person rig: crossbow_01 has no Bolt_Socket"),
        }
    }
    // The rig's sets: every sequence the simulation's table lists for them (what its layers can
    // play) and the rig's own.
    let set_paths: Vec<String> = SETS.iter().map(|s| format!("{s}.{s}")).collect();
    let table = &session.content.sim.gameplay.sequences;
    let played: Vec<&str> = table.keys().filter_map(|k| k.split_once('.')).filter(|(set, _)| SETS.contains(set)).map(|(_, n)| n).collect();
    let wanted: Vec<&str> = RIG_CLIPS.iter().copied().chain(played.iter().copied()).collect();
    let lib = ClipLib::load(&content, &bones, &set_paths, &wanted);
    let missing: Vec<&&str> = wanted.iter().filter(|c| !lib.has(c)).collect();
    if !missing.is_empty() {
        warn!("first-person rig: sequences not found: {missing:?}");
    }
    info!("first-person rig: {} bones, {} weapons, {} clips", bones.len(), weapons.len(), lib.clips.len());
    // The rig's sockets, and the points of the particle notifies of its sequences on them.
    let rig_sockets: HashMap<String, (usize, glam::Mat4)> = sockets
        .iter()
        .filter_map(|s| find(&s.bone).map(|b| (s.name.clone(), (b, glam::Mat4::from_translation(s.location) * dishonored_assets::util::rotator_matrix(s.rotation[0], s.rotation[1], s.rotation[2])))))
        .collect();
    let mut effect_points: Vec<String> = vec![crate::effects::HAND_CAST_SOCKET.to_string()];
    for (key, info) in table {
        if !key.split_once('.').is_some_and(|(set, _)| SETS.contains(&set)) {
            continue;
        }
        for n in &info.notifies {
            let crate::anim_notify::Notify::Particle { point, .. } = crate::anim_notify::resolve(n) else { continue };
            if point.is_empty() || (!rig_sockets.contains_key(&point) && find(&point).is_none()) || effect_points.contains(&point) {
                continue;
            }
            effect_points.push(point);
        }
    }
    let sword_trail = weapons.iter().find(|w| w.held == Held::Sword).and_then(|w| {
        let db = dishonored_assets::tweaks::TweakDb::load(&content).ok()?;
        let t = dishonored_assets::melee_trail::melee_trail_in(&db, "Twk_Inv_PlayerSpecific.Twk_Inv_SwordCorvo").map_err(|e| warn!("sword trail: {e:#}")).ok()??;
        let sockets = dishonored_assets::characters::mesh_sockets(&content, "Startup", "Wpn_PlySwords.Wpn_PlySword01").unwrap_or_default();
        let corner = |name: &str| sockets.iter().find(|s| s.name.eq_ignore_ascii_case(name)).and_then(|s| w.bones.iter().position(|b| b.name.eq_ignore_ascii_case(&s.bone)).map(|bi| (bi, s.location)));
        Some((t.template, [corner(&t.sockets[0])?, corner(&t.sockets[1])?]))
    });
    let arms_gpu = upload(&arms, &mut meshes, &mut images, &mut mats, &mut binds, &feat);
    let glow = arm_glow_materials(&content, &arms_gpu, &mut images, &mut mats);
    commands.insert_resource(ArmGlowMats(glow));
    let bind_pose = bones.iter().map(|b| (b.rotation, b.translation)).collect();
    let look_bones = [find("Collarbone_R_Jnt"), find("Collarbone_L_Jnt"), find("spine_3_jnt")];
    commands.insert_resource(ViewRig { arms: arms_gpu, parents: parents(&bones), bones, bind_pose, m2p, weapons, bolts, lib, sword_trail, sockets: rig_sockets, effect_points, upper_body, left_arm, cam_bone, look_bones });
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
    for (bi, b) in rig.bolts.iter().enumerate() {
        let bp = &b.gpu.parts[0];
        let broot = commands.spawn((Transform::default(), Visibility::Hidden, BoltMesh(bi), ChildOf(root))).id();
        let joints: Vec<Entity> = bp.bind.iter().enumerate().map(|(i, m)| commands.spawn((Transform::from_matrix(*m), BoltJoint { bolt: bi, bone: i }, ChildOf(root))).id()).collect();
        for (mesh, mat) in &bp.sections {
            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), SkinnedMesh { inverse_bindposes: bp.inverse_bindposes.clone(), joints: joints.clone() }, Transform::default(), NoFrustumCulling, bevy::light::NotShadowCaster, ChildOf(broot)));
        }
    }
}

/// The weight of one of the simulation's layers from its own playhead and the requesting state's
/// blend time (`SeqPlay::blend`): (in, out). In over the blend time from the sequence's start;
/// out over it from its `DishonoredNotify_AnimStateExit`, where the state leaves for the next one
/// (the player's state pickers were not decoded, U-972, so LIKELY). Without one, what the layer
/// plays for decides (`dishonored_sim::anim::token`): the block's sequence holds its last frame
/// while the state holds it (the simulation stops it with the block: `ParryIdle_In`) and so does a
/// paired move's master; the swings' and the left hand's sequences play out after their state
/// (`left_requests`: "its sequence keeps playing out on the arm"), so they leave ending with the
/// sequence (a slot's early blend-out, stock UE3 `AnimNodeSlot::TickAnim`). Times are play
/// times: the sequence's time over `RateScale x Rate`.
pub fn layer_weights(s: &SeqPlay, info: &SequenceInfo) -> (f32, f32) {
    use dishonored_sim::anim::token;
    let speed = (info.rate_scale_or_one() * s.rate).abs().max(1e-6);
    let ramp = |play: f32| if s.blend > 0.0 { (play / s.blend).clamp(0.0, 1.0) } else if play > 0.0 { 1.0 } else { 0.0 };
    let w_in = if s.blend > 0.0 { ramp(s.time / speed) } else { 1.0 };
    let w_exit = match info.notify("DishonoredNotify_AnimStateExit").map(|n| n.time) {
        Some(te) if s.time >= te => 1.0 - ramp((s.time - te) / speed),
        _ => 1.0,
    };
    let held = matches!(s.token & token::KIND, token::BLOCK | token::PAIRED);
    let w_end = if s.looping || held { 1.0 } else { ramp((info.duration - s.time) / speed) };
    (w_in, w_exit.min(w_end))
}

/// A sequence of the rig's sets at a time over the reference pose (bones it does not drive keep
/// it: a sequence player's default; the gun and power sets drive only bones 0 and 1 and the
/// left arm). With `root_motion` the root bone stays as its first frame has it (`RMM_Translate`
/// / `RMRM_RotateActor` masters: their travel moves the pawn, `dishonored_sim::sync_motion`, and
/// their turn turns the view).
fn sequence_pose(lib: &ClipLib, bind: &[(Quat, Vec3)], name: &str, time: f32, looping: bool, root_motion: bool) -> Pose {
    let mut pose = bind.to_vec();
    lib.sample(name, time, looping, &mut pose);
    if root_motion && !pose.is_empty() {
        let mut first = bind.to_vec();
        lib.sample(name, 0.0, false, &mut first);
        pose[0] = first[0];
    }
    pose
}

/// One of the rig's pickers drawing a simulation layer: what it showed last, the crossfade source
/// when the simulation replaces the layer's sequence.
#[derive(Default)]
struct RigLayer {
    /// The layer's state (`SeqPlay::token`) and sequence last drawn.
    token: u32,
    seq: SeqId,
    last: Pose,
    last_weight: f32,
    /// The pose and weight the layer showed when its sequence changed.
    from: Option<(Pose, f32)>,
}

/// What a rig layer shows this frame.
struct Shown {
    name: String,
    time: f32,
    looping: bool,
    pose: Pose,
    weight: f32,
}

impl RigLayer {
    /// The layer's pose and weight from the simulation's layer (`None`: nothing on it, shown at
    /// once as nothing).
    fn show(&mut self, s: Option<&SeqPlay>, table: &SeqTable, lib: &ClipLib, bind: &[(Quat, Vec3)], gameplay: &GameplayConfig) -> Option<Shown> {
        let Some((s, info, name)) = s.and_then(|s| Some((s, table.info(s.seq)?, table.name(s.seq)?))) else {
            *self = RigLayer::default();
            return None;
        };
        if s.token != self.token || s.seq != self.seq {
            self.from = (self.last_weight > 0.0 && !self.last.is_empty()).then(|| (self.last.clone(), self.last_weight));
            self.token = s.token;
            self.seq = s.seq;
        }
        let (w_in, w_out) = layer_weights(s, info);
        let mut pose = sequence_pose(lib, bind, name, s.time, s.looping, gameplay.root_motion.contains_key(name));
        let mut weight = w_in;
        match self.from.as_ref() {
            Some((from, fw)) if w_in < 1.0 && from.len() == pose.len() => {
                for (p, f) in pose.iter_mut().zip(from) {
                    *p = mix(*f, *p, w_in);
                }
                weight = fw + (1.0 - fw) * w_in;
            }
            _ => self.from = None,
        }
        weight *= w_out;
        self.last.clone_from(&pose);
        self.last_weight = weight;
        Some(Shown { name: name.to_string(), time: s.time, looping: s.looping, pose, weight })
    }
}

/// Head bob of the full body's navigation: the directional additive of the walk / run / sprint /
/// sneak cycle (`ADD_Head_{Walk,Run,Sprint,Sneak}{N,S,W,E}`, synced to the cycle) weighted by
/// `AnimNodeBlendDirectional` over the movement direction. `Nav_Default` (`Sword_Ready_Walk`,
/// `Sword_Ready_Run`) and `Nav_Sprinting` (`Sword_Ready_Sprint`) pass it through
/// `DisAnimNodeBlendByHeadBob`: its tick (0x00A88C30) weighs "Head Bob On" by
/// `DishonoredPlayerCamera.m_BobAmount` and "Off" by the rest (1 without a player camera); the
/// camera's begin play (0x00D06080) sets `m_BobAmount` from the options' head bob amount
/// (`PSI_Gameplay_HeadBobAmount`) unless a camera tweak overrides it. `Nav_Crouched`
/// (`Sword_Sneak_Walk`) has no such node. It follows the full-body layer's cycle, so it bobs only
/// while the simulation plays one there.
fn head_bob(p: &PlayerState, cycle: &str, lib: &ClipLib, amount: f32) -> (Vec<(String, f32, f32)>, f32) {
    let (kind, gated) = match cycle {
        "Sword_Ready_Walk" => ("Walk", true),
        "Sword_Ready_Run" => ("Run", true),
        "Sword_Ready_Sprint" => ("Sprint", true),
        "Sword_Sneak_Walk" => ("Sneak", false),
        _ => return (Vec::new(), 0.0),
    };
    let w = if gated { amount.clamp(0.0, 1.0) } else { 1.0 };
    let v = p.velocity.truncate();
    let fwd = dishonored_sim::movement::forward_vector(p.yaw).truncate();
    let right = dishonored_sim::movement::right_vector(p.yaw).truncate();
    let dir = v.dot(right).atan2(v.dot(fwd));
    let items = directional_weights(dir)
        .iter()
        .zip(["N", "S", "W", "E"])
        .filter(|(dw, _)| **dw > 0.0)
        .map(|(dw, d)| (format!("ADD_Head_{kind}{d}"), 0.0, *dw))
        .filter(|(c, _, _)| lib.has(c))
        .collect();
    (items, w)
}

/// First-person animation state (local view only): what the pickers last drew, no clock.
#[derive(Resource, Default)]
pub struct ViewAnim {
    /// The full-body, upper-body and left-arm pickers (`FullBodyStatePicker`, `UpperBodyFilter`,
    /// `LeftArmFilter`) drawing the simulation's layers of the same index.
    layers: [RigLayer; 3],
    /// The rig's pawn space (the master root of synced sequences) relative to the game camera,
    /// unscaled, while the rig is shown.
    master_local: Option<Mat4>,
    /// The view yaw last frame (degrees), the held item's `m_CurItemAim` (degrees) and the tick
    /// it was eased to.
    prev_yaw: Option<f32>,
    item_aim: [f32; 2],
    aim_tick: Option<u64>,
}

/// Item prefix of the left hand (`m_ApplyItemEquipPrefix = EDisEquipUsage_Secondary`).
fn left_prefix(item: LeftHand) -> Option<&'static str> {
    match item {
        LeftHand::Empty => None,
        LeftHand::Pistol | LeftHand::Crossbow | LeftHand::SleepDart | LeftHand::IncendiaryBolt => Some("Guns_"),
        LeftHand::Grenade | LeftHand::StickyGrenade | LeftHand::SpringRazor => Some("Gadgets_"),
        _ => Some("Powers_"),
    }
}

/// The left item's arm pitch keys (none for an empty hand: 0x00ACB8D0 returns 0 without an item).
fn left_arm_pitch_keys(item: LeftHand) -> Option<&'static [(f32, f32)]> {
    match item {
        LeftHand::Empty => None,
        LeftHand::Pistol | LeftHand::Crossbow | LeftHand::SleepDart | LeftHand::IncendiaryBolt => Some(&ARM_PITCH_KEYS_GUN),
        _ => Some(&ARM_PITCH_KEYS_DEFAULT),
    }
}

/// The weapon mesh's own sequence for what its hand plays (`arm`, the hand's layer sequence;
/// empty: none). The weapons' sets hold (Startup): `CorvoGun_as` Equip / FastReload /
/// Fire_Accuracy_1 / Fire_Hammer / Idle; `Crossbow_as` Equip / Fire / Fire_FastReload / Idle /
/// OutOfAmmo_Idle / RapidReload / Reload_In / Spyglass_Fire; `PlySword_Anim_as` the sword's
/// equips, unequips, FirstAttack and `Sword_Ready_Idle`; `Grenade_as` and `SpringRazor_as` only
/// their Idle (`Grenade_at`, `SpringRazor_at` play nothing else). The weapons' trees pick through
/// weapon action tweaks that were not dumped; the pairing below is by name (LIKELY). The
/// crossbow's fire is the simulation's own weapon layer.
fn weapon_clip(w: &WeaponGpu, arm: &str, p: &PlayerState) -> Option<String> {
    let pick = |names: &[&str]| names.iter().find(|n| w.lib.has(n)).map(|n| n.to_string());
    match w.held {
        Held::Sword => pick(&[arm, "Sword_Ready_Idle"]),
        Held::Pistol => match arm {
            "Pistol_Fire" => pick(&["CorvoGun_Fire_Hammer", "CorvoGun_Idle"]),
            "Pistol_Fire_Accurate" | "Pistol_FireAccuracy1" => pick(&["CorvoGun_Fire_Accuracy_1", "CorvoGun_Idle"]),
            "Pistol_FastReload" => pick(&["CorvoGun_FastReload", "CorvoGun_Idle"]),
            "Guns_Ready_Equip" => pick(&["CorvoGun_Equip", "CorvoGun_Idle"]),
            _ => pick(&["CorvoGun_Idle"]),
        },
        Held::Crossbow => {
            let idle = if p.left.bolts_loaded == 0 && p.left.item == LeftHand::Crossbow { "Crossbow_OutOfAmmo_Idle" } else { "Crossbow_Idle" };
            match arm {
                "Guns_Ready_Equip" => pick(&["Crossbow_Equip", idle]),
                "Crossbow_RapidReload" | "Crossbow_Reload_In" => pick(&[arm, idle]),
                _ => pick(&[idle, "Crossbow_Idle"]),
            }
        }
        Held::Grenade => pick(&["Grenade_Idle"]),
        Held::SpringRazor => pick(&["SpringRazor_Idle"]),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn animate_view_rig(
    rig: Option<Res<ViewRig>>,
    mut anim: ResMut<ViewAnim>,
    view: Res<SessionView>,
    session: Res<crate::session::Session>,
    mut cams: Query<(&GameCamera, &mut Transform), (Without<ViewRoot>, Without<ViewJoint>, Without<WeaponJoint>, Without<BoltJoint>)>,
    mut roots: Query<(&mut Transform, &mut Visibility), (With<ViewRoot>, Without<ViewJoint>, Without<WeaponJoint>, Without<WeaponMesh>)>,
    mut joints: Query<(&ViewJoint, &mut Transform), (Without<ViewRoot>, Without<WeaponJoint>)>,
    mut wjoints: Query<(&WeaponJoint, &mut Transform), (Without<ViewRoot>, Without<ViewJoint>)>,
    mut wmeshes: Query<(&WeaponMesh, &mut Visibility), Without<ViewRoot>>,
    mut bjoints: Query<(&BoltJoint, &mut Transform), (Without<ViewRoot>, Without<ViewJoint>, Without<WeaponJoint>)>,
    mut bmeshes: Query<(&BoltMesh, &mut Visibility), (Without<ViewRoot>, Without<WeaponMesh>)>,
    cutscenes: Option<Res<crate::cutscene::Cutscenes>>,
    (settings, mut hands, mut trails, mut notes): (Option<Res<crate::settings::GameSettings>>, ResMut<crate::effects::HandEffects>, Option<ResMut<crate::weapon_trails::WeaponTrails>>, ResMut<crate::anim_notify::AnimNotifies>),
) {
    use dishonored_sim::anim::layer;
    let Some(rig) = rig else { return };
    let Ok((mut root_t, mut root_vis)) = roots.single_mut() else { return };
    let first_person = cams.iter().all(|(c, _)| !c.third_person && c.follow.is_none_or(|f| Some(f) == view.me.as_ref().map(|m| m.id)));
    // [interp] A soiree's player group puts both hands' items away (`StatePlayerMasterSoiree`
    // enter, 0x00a66ff0: both hands blended to 1 through 0x00aa1cf0).
    let held = cutscenes.as_ref().is_some_and(|c| c.held);
    // The rig stays drawn through a Blink's travel: the cast plays `Powers_Cast_Blink_Out` on
    // the left arm as the move starts, and no Blink code hides the player's mesh (U-781).
    let shown = view.me.as_ref().filter(|m| m.alive && first_person && !held);
    let Some(me) = shown else {
        anim.master_local = None;
        anim.prev_yaw = None;
        anim.aim_tick = None;
        hands.sockets.clear();
        *root_vis = Visibility::Hidden;
        return;
    };
    *root_vis = Visibility::Inherited;
    // The view's yaw (Dishonored degrees) from the game camera, before this frame's bob.
    let view_yaw = cams.iter().next().map(|(_, t)| {
        let f = t.rotation * Vec3::NEG_Z;
        f.z.atan2(f.x).to_degrees()
    });
    // The controller's look pitch (degrees, positive up) from the game camera, before this
    // frame's view adjustment.
    let look_pitch = cams.iter().next().map(|(_, t)| (t.rotation * Vec3::NEG_Z).y.clamp(-1.0, 1.0).asin().to_degrees()).unwrap_or(0.0);
    let gameplay = &session.content.sim.gameplay;
    let table = session.content.sim.anim.clone();
    let lib = &rig.lib;
    let a = &mut *anim;
    // The simulation's layers of the local player (`dishonored_sim::anim::PawnAnim`).
    let sim = view.anim.get(&me.id);
    let sim_layer = |l: usize| sim.and_then(|s| s.layers[l].as_ref());
    let [fb_layer, ub_layer, la_layer] = &mut a.layers;
    let fb = fb_layer.show(sim_layer(layer::FULL_BODY), &table, lib, &rig.bind_pose, gameplay);
    let ub = ub_layer.show(sim_layer(layer::UPPER_BODY), &table, lib, &rig.bind_pose, gameplay);
    let la = la_layer.show(sim_layer(layer::LEFT_ARM), &table, lib, &rig.bind_pose, gameplay);
    let (ub_w, la_w) = (ub.as_ref().map_or(0.0, |s| s.weight), la.as_ref().map_or(0.0, |s| s.weight));

    // The full body: the layer over the stand-in idle by its weight. The same without the
    // additives gives the unbobbed camera.
    let stand_in = sequence_pose(lib, &rig.bind_pose, STAND_IN, 0.0, false, false);
    let fb_base: Pose = match fb.as_ref() {
        Some(s) => stand_in.iter().zip(&s.pose).map(|(a, b)| mix(*a, *b, s.weight.clamp(0.0, 1.0))).collect(),
        None => stand_in,
    };
    let fb_root = fb.as_ref().filter(|s| gameplay.root_motion.contains_key(&s.name));
    let ub_pose = ub.as_ref().filter(|s| s.weight > 0.0).map(|s| s.pose.clone());
    let mut la_pose = la.as_ref().filter(|s| s.weight > 0.0).map(|s| s.pose.clone());
    // The held item's aim (`DishonoredInventoryItem` tick 0x00C0B990): `m_CurItemAim` eases toward
    // the item's target at `clamp(dt x m_fItemAimSpeed, 0, 1)`, `dt` the simulation's ticks since
    // the last frame; the target (slot 102, 0x00C05CD0) is the camera's yaw last frame minus now,
    // in degrees, and no pitch (`CameraCache` vs `LastFrameCameraCache`). `DishonoredSeqBlendByAim`
    // (0x00A8FB20) divides it by its max degrees (10) and lays `ADD_<item prefix>Aim_*` over the
    // left arm (`DisAnimNodeBlendItemAimAdditive`).
    if let Some(yaw) = view_yaw {
        let ticks = a.aim_tick.map_or(0, |t| view.tick.saturating_sub(t));
        let dt = ticks as f32 * dishonored_sim::movement::DT * me.time_scale.max(0.0);
        let target = a.prev_yaw.map(|p| (p - yaw + 540.0).rem_euclid(360.0) - 180.0).unwrap_or(0.0);
        let k = (dt * ITEM_AIM_SPEED).clamp(0.0, 1.0);
        a.item_aim[0] += (target - a.item_aim[0]) * k;
        a.item_aim[1] += (0.0 - a.item_aim[1]) * k;
        a.prev_yaw = Some(yaw);
        a.aim_tick = Some(view.tick);
    }
    if let (Some(p), Some(pre)) = (la_pose.as_mut(), left_prefix(me.left.item)) {
        let w = aim_weights(a.item_aim[0] / AIM_MAX_DEGREES, a.item_aim[1] / AIM_MAX_DEGREES);
        let items: Vec<(String, f32, f32)> = AIM_SUFFIXES.iter().zip(w).map(|(s, w)| (format!("ADD_{pre}Aim_{s}"), 0.0, w)).collect();
        add_additive_blend(lib, &items, false, 1.0, p);
    }
    let filters = |fb: &[(Quat, Vec3)]| -> Pose {
        let mut l = fb.to_vec();
        if let Some(u) = &ub_pose {
            l = blend_per_bone(&rig.parents, &l, u, &rig.upper_body, ub_w);
        }
        if let Some(p) = &la_pose {
            l = blend_per_bone(&rig.parents, &l, p, &rig.left_arm, la_w);
        }
        l
    };
    let bob_amount = settings.as_ref().map(|s| s.s.gameplay.head_bob).unwrap_or(1.0);
    let mut fb_bob = fb_base.clone();
    if let Some(s) = fb.as_ref() {
        let phase = (s.time / lib.length(&s.name).max(1e-3)).rem_euclid(1.0);
        let (items, w): (Vec<(String, f32, f32)>, f32) = head_bob(me, &s.name, lib, bob_amount);
        let items: Vec<(String, f32, f32)> = items.into_iter().map(|(c, _, dw)| { let t = phase * lib.length(&c); (c, t, dw) }).collect();
        add_additive_blend(lib, &items, true, w * s.weight, &mut fb_bob);
    }
    let mut locals = filters(&fb_bob);
    // The look controls (`Ply_Player_at` SkelControlLists, driven by the pawn's view target
    // update 0x00AABDD0): both collarbones turn by the look pitch plus their item's arm pitch
    // offset (`m_ArmFollowOffset_Rot_Primary` / `_Secondary`, 0x00ACB8D0 from the view pitch,
    // eased by 1 - cos(pi x weight) / 2 at the full weight the player control sets), and
    // `spine_3_jnt` (the head and the camera bone) by the look pitch alone. The pawn itself
    // does not pitch while walking or falling.
    {
        let right = look_pitch + arm_pitch_offset(&ARM_PITCH_KEYS_DEFAULT, look_pitch);
        let left = look_pitch + left_arm_pitch_keys(me.left.item).map_or(0.0, |k| arm_pitch_offset(k, look_pitch));
        let controls: Vec<(usize, f32)> = rig.look_bones.iter().zip([right, left, look_pitch]).filter_map(|(&b, p)| b.map(|b| (b, p))).collect();
        apply_look_controls(&rig.parents, &mut locals, rig.m2p, &controls);
    }
    let bones: Vec<BoneAsset> = rig.bones.iter().zip(&locals).map(|(b, (q, t))| BoneAsset { rotation: *q, translation: *t, ..b.clone() }).collect();
    let globals = bind_globals(&bones);
    // The camera bone without the additives and the look controls: the simulation's eye stands
    // for it.
    let bones0: Vec<BoneAsset> = rig.bones.iter().zip(&filters(&fb_base)).map(|(b, (q, t))| BoneAsset { rotation: *q, translation: *t, ..b.clone() }).collect();
    let cam0 = rig.m2p * bind_globals(&bones0)[rig.cam_bone];

    // The weapons' own sequences, with the hand that holds them: the hand's sequence over the
    // weapon's idle by the hand layer's weight; the crossbow's fire is its own layer.
    let weapon_layer = sim_layer(layer::WEAPON).and_then(|s| Some((s, table.info(s.seq)?, table.name(s.seq)?)));
    let mut wglobals: Vec<Vec<glam::Mat4>> = Vec::with_capacity(rig.weapons.len());
    for w in &rig.weapons {
        let driver = match w.held {
            Held::Sword if ub_w > 0.5 => ub.as_ref(),
            Held::Sword => fb.as_ref(),
            Held::Pistol | Held::Crossbow | Held::Grenade | Held::SpringRazor => la.as_ref(),
        };
        let idle = weapon_clip(w, "", me);
        let mut pose = w.bind_pose.clone();
        if let Some(c) = &idle {
            w.lib.sample(c, 0.0, true, &mut pose);
        }
        let own = match (w.held, weapon_layer) {
            (Held::Crossbow, Some((s, info, name))) if w.lib.has(name) => Some((name.to_string(), s.time, s.looping, { let (i, o) = layer_weights(s, info); i.min(o) })),
            _ => driver.and_then(|d| weapon_clip(w, &d.name, me).filter(|c| Some(c) != idle.as_ref()).map(|c| (c, d.time, d.looping, d.weight))),
        };
        if let Some((c, time, looping, weight)) = own {
            let mut over = w.bind_pose.clone();
            w.lib.sample(&c, time, looping, &mut over);
            for (p, o) in pose.iter_mut().zip(&over) {
                *p = mix(*p, *o, weight.clamp(0.0, 1.0));
            }
        }
        let wb: Vec<BoneAsset> = w.bones.iter().zip(&pose).map(|(b, (q, t))| BoneAsset { rotation: *q, translation: *t, ..b.clone() }).collect();
        wglobals.push(bind_globals(&wb));
    }

    if std::env::var_os("DISH_VM_DEBUG").is_some() {
        let cam = rig.m2p * globals[rig.cam_bone];
        let rel = |i: usize| cam.inverse().transform_point3((rig.m2p * globals[i]).w_axis.truncate());
        let names: Vec<String> = rig.weapons.iter().map(|w| format!("{}={:.0}", rig.bones[w.socket].name, rel(w.socket))).collect();
        let show = |s: &Option<Shown>| s.as_ref().map(|s| format!("{}@{:.3}({:.2})", s.name, s.time, s.weight)).unwrap_or_default();
        info!("vm fb={} ub={} la={} {}", show(&fb), show(&ub), show(&la), names.join(" "));
    }
    // The view (0x00AABA60 / 0x00AABB70): at the camera bone (`m_CameraBoneName` camera_jnt,
    // its frame turned by `DisTweaks_Pawn_Body.m_fBone_AngleOffset_Pitch` -90, i.e. forward =
    // -Z, right = +Y, up = +X of the bone), looking along the controller's rotation plus
    // `ROLL_AMOUNT` of the bone's own animated rotation. The arms are drawn where the mesh is,
    // so they show the rest of the bone's rotation relative to the view.
    let cam = rig.m2p * globals[rig.cam_bone];
    let (fwd, right, up) = (-cam.z_axis.truncate(), cam.y_axis.truncate(), cam.x_axis.truncate());
    let view_rot = view_rotation(fwd, right, up, look_pitch);
    let v = glam::Mat4::from_rotation_translation(view_rot, cam.w_axis.truncate());
    // Pawn canonical (X fwd, Y right, Z up) in Bevy-converted space -> camera local.
    let a_rot = Mat4::from_quat(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    // The lean influence rolls the view (`m_fCameraTiltPercent`), but the arm look controls take
    // only pitch (0x00AABDD0), so the arms stay level: undo the camera's lean roll on the rig.
    let lean = dishonored_sim::movement::lean_roll(me, &gameplay.movement).to_radians();
    let master_local = Mat4::from_rotation_z(lean) * a_rot * to_bevy(v).inverse();
    // The game camera stands for the camera bone without the additives and the look controls;
    // the bone's motion from those (head bob, the look pivot about `spine_3_jnt`) moves it, and it
    // turns to the view rotation above (and the lean roll).
    let delta = cam.w_axis.truncate() - cam0.w_axis.truncate();
    // A root-motion master's turn (`RMRM_RotateActor`) turns the pawn and its view; the rig's root
    // keeps its first frame (`sequence_pose`).
    let turn = fb_root.and_then(|s| gameplay.root_motion.get(&s.name).map(|r| r.yaw_at(s.time))).unwrap_or(0.0);
    let mut cam_world = Mat4::IDENTITY;
    for (_, mut ct) in &mut cams {
        let f = ct.rotation * Vec3::NEG_Z;
        let fh = Quat::from_rotation_y(-turn.to_radians()) * Vec3::new(f.x, 0.0, f.z).try_normalize().unwrap_or(Vec3::NEG_Z);
        let rh = fh.cross(Vec3::Y);
        let map = |p: glam::Vec3| fh * p.x + rh * p.y + Vec3::Y * p.z;
        let (vf, vu) = (view_rot * glam::Vec3::X, view_rot * glam::Vec3::Z);
        let mut t = Transform::from_translation(ct.translation + map(delta) * crate::coords::UU_TO_BEVY).looking_to(map(vf), map(vu));
        t.rotate_local_z(-lean);
        *ct = t;
        cam_world = t.to_matrix();
    }
    a.master_local = Some(master_local);
    *root_t = Transform::from_matrix(Mat4::from_scale(Vec3::splat(VIEW_SCALE)) * master_local);
    // The rig's sockets and bones the hand effects use, in Dishonored world space (scaled with
    // the rig, so the particles keep their size relative to the hand).
    hands.sockets.clear();
    let root_world = cam_world * Mat4::from_scale(Vec3::splat(VIEW_SCALE)) * master_local;
    for name in &rig.effect_points {
        let local = match rig.sockets.get(name) {
            Some((bone, rel)) => rig.m2p * globals[*bone] * *rel,
            None => match rig.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name)) {
                Some(bone) => rig.m2p * globals[bone],
                None => continue,
            },
        };
        hands.sockets.insert(name.clone(), crate::cutscene::from_bevy(root_world * to_bevy(local)));
    }
    // The particle notifies the simulation fired for the local player (`AnimNotify_PlayParticleEffect`
    // of the player sets: the Mark of the Outsider's glows on `Tattoo`, the swimming splashes) on
    // the rig's sockets; the third-person body leaves them to the rig.
    notes.first_person = Some(me.id);
    let particles: Vec<(String, String, bool)> = notes
        .of(me.id)
        .filter_map(|n| match n {
            crate::anim_notify::Notify::Particle { template, point, attach } if rig.effect_points.contains(point) => Some((template.clone(), point.clone(), *attach)),
            _ => None,
        })
        .collect();
    hands.pending.extend(particles);
    // The sword's trail on its blade corners (in the rig's scaled frame, like the hand effects),
    // while a `DisNotify_Trails` window is open on the layer the sword follows.
    let sword_layer = if ub_w > 0.5 { layer::UPPER_BODY } else { layer::FULL_BODY };
    let trailing = sim.is_some_and(|s| s.window_open(sword_layer, &table, "DisNotify_Trails"));
    if let (true, Some((template, corners)), Some(tr)) = (trailing, rig.sword_trail.as_ref(), trails.as_mut()) {
        if let Some((wi, w)) = rig.weapons.iter().enumerate().find(|(_, w)| w.held == Held::Sword) {
            if me.carrying.is_none() {
                let corner = |k: usize| {
                    let wb = wglobals.get(wi).and_then(|g| g.get(corners[k].0)).copied().unwrap_or(w.bind[corners[k].0]);
                    let local = rig.m2p * globals[w.socket] * w.socket_rel * w.m2p * wb * glam::Mat4::from_translation(corners[k].1);
                    crate::cutscene::from_bevy(root_world * to_bevy(local)).w_axis.truncate()
                };
                tr.sources.push(((me.id, crate::weapon_trails::FIRST_PERSON), template.clone(), corner(0), corner(1)));
            }
        }
    }
    for (j, mut t) in &mut joints {
        *t = Transform::from_matrix(to_bevy(rig.m2p * globals[j.0]));
    }
    let attach = std::env::var("DISH_VM_ATTACH").unwrap_or_default();
    for (wj, mut t) in &mut wjoints {
        let w = &rig.weapons[wj.weapon];
        let base = rig.m2p * globals[w.socket];
        let wb = wglobals.get(wj.weapon).and_then(|g| g.get(wj.bone)).copied().unwrap_or(w.bind[wj.bone]);
        let m = match attach.as_str() {
            "A" => base * wb,
            "B" => base * w.m2p * wb,
            "C" => base * w.m2p.inverse() * wb,
            "E" => base * glam::Mat4::from_rotation_x(std::f32::consts::PI) * w.m2p.inverse() * wb,
            // The earlier visual choice for the sword (U-025): Rz(180) * inverse RotOrigin.
            "V" => base * glam::Mat4::from_rotation_z(std::f32::consts::PI) * w.m2p.inverse() * wb,
            // The original attach: the rig socket (`RightHandWpn` yaw +90 / `LeftHandWpn`
            // (0, -16384, 16384) on the hand attachment bones of `Skm_Player`), then the weapon
            // mesh's own Origin / RotOrigin. For the guns this equals the inverse RotOrigin used
            // before (the rotator is a 120-degree axis cycle); for the sword it keeps the blade
            // direction and fixes its roll.
            _ => base * w.socket_rel * w.m2p * wb,
        };
        *t = Transform::from_matrix(to_bevy(m));
    }
    // The loaded bolt rides the crossbow's `Bolt_Socket` (it leaves at the shot and is out
    // during the reload).
    for (bj, mut t) in &mut bjoints {
        let b = &rig.bolts[bj.bolt];
        let w = &rig.weapons[b.weapon];
        let wb = wglobals.get(b.weapon).and_then(|g| g.get(b.bone)).copied().unwrap_or(w.bind[b.bone]);
        let socket = rig.m2p * globals[w.socket] * w.socket_rel * w.m2p * wb * b.socket_rel;
        *t = Transform::from_matrix(to_bevy(socket * b.m2p * b.bind[bj.bone]));
    }
    for (bm, mut vis) in &mut bmeshes {
        let b = &rig.bolts[bm.0];
        let loaded = match b.ammo {
            LeftHand::Crossbow => me.left.bolts_loaded > 0,
            _ => true,
        };
        let show = me.left.item == b.ammo && loaded && !matches!(me.left.phase, LeftPhase::Recovering | LeftPhase::Reloading) && me.carrying.is_none();
        *vis = if show { Visibility::Inherited } else { Visibility::Hidden };
    }
    for (wm, mut vis) in &mut wmeshes {
        // Carrying a body sheathes the sword and lowers the left hand.
        let carrying = me.carrying.is_some();
        let show = match rig.weapons[wm.0].held {
            Held::Sword => !carrying,
            Held::Pistol => me.left.item == LeftHand::Pistol && !carrying,
            Held::Crossbow => me.left.item.is_crossbow() && !carrying,
            Held::Grenade => matches!(me.left.item, LeftHand::Grenade | LeftHand::StickyGrenade) && !carrying,
            Held::SpringRazor => me.left.item == LeftHand::SpringRazor && !carrying,
        };
        *vis = if show { Visibility::Inherited } else { Visibility::Hidden };
    }
}

/// The victim's yaw (degrees) in a paired move of `master`: the direction the move was picked
/// for (0x00BBA970: front, left, right, back) turned into the master's yaw at the start, so the
/// master stands exactly there (`m_bDoNotRotateSlave` off). Finisher actions name their direction;
/// the head chop and the knockdowns are frontal.
fn slave_yaw(master: &PlayerState) -> f32 {
    use dishonored_ir::gameplay::fatality_move as fm;
    let dir = match master.sword.assassination_move {
        m if m < 8 => (m % 4) as usize,
        m if m < 12 => 0,
        _ => match master.sword.fatality.map(|r| r.action as usize) {
            Some(fm::LEFT | fm::LEFT_FAST) => 1,
            Some(fm::RIGHT | fm::RIGHT_FAST) => 2,
            Some(fm::BACK | fm::BACK_FAST) => 3,
            _ => 0,
        },
    };
    // The master's direction from the victim is the reverse of its own facing.
    master.sword.root_yaw + [180.0, -90.0, 90.0, 0.0][dir]
}

/// A body left where its choke or carry ended (Dishonored-space root position and yaw), kept
/// until the simulation moves it.
#[derive(Clone, Copy, Debug)]
pub struct BodyAnchor {
    pub pos: glam::Vec3,
    pub yaw: f32,
    sim_pos: glam::Vec3,
}

/// Bodies drawn in another pawn's frame. Synced sequences (`Generic_Choke_*_Slave`,
/// `Corpses_CarryCorpse_*_Slave`, `Generic_Assassination_*_Slave`: every slave's root starts at
/// the origin with its master's) are authored at the master's root, so a choked, assassinated or carried
/// body is drawn there: for the local first-person master, the rig's own pawn space (whose
/// `camera_jnt` is the view, so the carried body hangs where the master sequence puts it,
/// over the shoulder), else the master's body root. A body released from a choke or a carry
/// stays in that frame (its anchor) while its slave sequence ends and its ragdoll falls.
#[derive(Resource, Default)]
pub struct BodyFrames {
    pub anchors: HashMap<dishonored_sim::state::PlayerId, BodyAnchor>,
    /// The (level) frame each held body had last frame.
    held: HashMap<dishonored_sim::state::PlayerId, Transform>,
}

/// Yaw-only version of a frame (pawn +X is the Bevy +X column; Y up).
fn level_frame(m: Mat4) -> Transform {
    let f = m.x_axis.truncate();
    let yaw = f.z.atan2(f.x);
    Transform { translation: m.w_axis.truncate(), rotation: Quat::from_rotation_y(-yaw), scale: Vec3::ONE }
}

fn anchor_of(t: &Transform, sim_pos: glam::Vec3) -> BodyAnchor {
    let p = t.translation / crate::coords::UU_TO_BEVY;
    let f = t.rotation * Vec3::X;
    BodyAnchor { pos: glam::Vec3::new(p.x, p.z, p.y), yaw: f.z.atan2(f.x).to_degrees(), sim_pos }
}

fn anchor_transform(a: &BodyAnchor) -> Transform {
    Transform { translation: crate::coords::pos(a.pos), rotation: Quat::from_rotation_y(-a.yaw.to_radians()), scale: Vec3::ONE }
}

/// Place choked, carried and just-released bodies in their master's frame (after
/// `players::sync_players`, before the characters are animated).
pub fn place_held_bodies(
    anim: Res<ViewAnim>,
    view: Res<SessionView>,
    mut frames: ResMut<BodyFrames>,
    cams: Query<&Transform, (With<GameCamera>, Without<crate::players::PlayerBody>)>,
    mut bodies: Query<(&crate::players::PlayerBody, &mut Transform), (With<crate::characters::CharacterRoot>, Without<GameCamera>)>,
) {
    use dishonored_sim::state::Condition;
    let all: Vec<&PlayerState> = view.others.iter().chain(view.me.iter()).collect();
    let me = view.me.as_ref().map(|m| m.id);
    // The local master's frames: as drawn (following the view) and level.
    let fp = cams.iter().next().zip(anim.master_local).map(|(cam, ml)| {
        let level_cam = {
            let f = cam.rotation * Vec3::NEG_Z;
            let yaw = (-f.x).atan2(-f.z);
            Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), cam.translation)
        };
        (Transform::from_matrix(cam.to_matrix() * ml), level_frame(level_cam * ml))
    });
    let roots: HashMap<u32, Transform> = bodies.iter().map(|(b, t)| (b.0, *t)).collect();
    // (shown, level) frame of a master.
    let frame_of = |id: u32| -> Option<(Transform, Transform)> {
        if Some(id) == me {
            if let Some(f) = fp {
                return Some(f);
            }
        }
        roots.get(&id).map(|t| (*t, *t))
    };
    let mut live: HashMap<u32, Transform> = HashMap::new();
    for p in &all {
        let held = if let Some(c) = p.carried_by {
            frame_of(c)
        } else {
            // In a choke, or playing its Win / Lose slave while the master is in that state; or the
            // victim of a running assassination (its slave synced to the master's sequence).
            let choker = p.condition_by.and_then(|c| all.iter().find(|q| q.id == c));
            let in_choke = choker.is_some_and(|c| {
                p.condition == Condition::BeingChoked || (c.choke.target == Some(p.id) && matches!(c.choke.phase, ChokePhase::Win | ChokePhase::Lose))
            });
            let assassin = all.iter().find(|q| q.sword.phase == SwordPhase::Assassinating && q.sword.assassination_target == Some(p.id));
            if in_choke {
                choker.and_then(|c| frame_of(c.id)).map(|(_, level)| (level, level))
            } else {
                // A paired move's victim plays its slave where it stood, turned to the move's
                // relation to the master's yaw at the start; the master reaches it by its root
                // motion (`dishonored_sim::sync_motion`).
                assassin.and_then(|a| roots.get(&p.id).map(|t| {
                    let t = Transform { rotation: Quat::from_rotation_y(-slave_yaw(a).to_radians()), ..*t };
                    (t, t)
                }))
            }
        };
        if let Some((shown, level)) = held {
            live.insert(p.id, shown);
            frames.held.insert(p.id, level);
            frames.anchors.remove(&p.id);
            continue;
        }
        let down = !p.alive || p.condition == Condition::Unconscious;
        if let Some(level) = frames.held.remove(&p.id) {
            if down {
                frames.anchors.insert(p.id, anchor_of(&level, p.position));
            }
        }
        if !down {
            frames.anchors.remove(&p.id);
            continue;
        }
        if let Some(a) = frames.anchors.get(&p.id).copied() {
            // The simulation moved the body (lifted elsewhere, pushed): drop the anchor.
            if (p.position - a.sim_pos).truncate().length() > 8.0 {
                frames.anchors.remove(&p.id);
            } else {
                live.insert(p.id, anchor_transform(&a));
            }
        }
    }
    frames.held.retain(|id, _| all.iter().any(|p| p.id == *id));
    frames.anchors.retain(|id, _| all.iter().any(|p| p.id == *id));
    for (b, mut t) in &mut bodies {
        if let Some(x) = live.get(&b.0) {
            *t = *x;
        }
    }
}

/// Keep every perspective camera at the original's horizontal FOV for the current aspect
/// (Bevy's `fov` is vertical).
pub fn apply_fov(mut cams: Query<&mut Projection, (With<Camera3d>, Without<crate::reflect::ReflectCamera>)>, settings: Option<Res<crate::settings::GameSettings>>, view: Option<Res<crate::session::SessionView>>) {
    // Options -> Graphics -> FOV (`PSI_GraphicsPC_FOV`, default 75 = HORIZONTAL_FOV_DEG).
    let fov = settings.map(|s| s.s.graphics.fov).unwrap_or(HORIZONTAL_FOV_DEG);
    // At a keyhole the peek's own field of view (`m_fKeyHoleFOV * (1 - magnification)`, 0x00A63400).
    let fov = view.as_deref().and_then(crate::tripwires::peek_view).map(|v| v.fov).unwrap_or(fov);
    for mut p in &mut cams {
        if let Projection::Perspective(pp) = p.as_mut() {
            let v = 2.0 * ((fov.to_radians() * 0.5).tan() / pp.aspect_ratio.max(0.1)).atan();
            if (pp.fov - v).abs() > 1e-4 {
                pp.fov = v;
            }
        }
    }
}
