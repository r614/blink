//! First-person view: Corvo's rig (Engine `Ply_Player.Skm_Player`) with the real sword,
//! pistol and crossbow on the hand sockets, drawn in the world camera (scaled about the eye)
//! and driven by decoded player AnimSets chosen from simulation state.
//!
//! Selection and layering follow the player's AnimTree (`Startup` `Ply_Player_at`) and the
//! animation states of `Twk_Pawn_Corvo` (`pFullBodyTweaks`, `pUpperBodyTweaks`,
//! `pSpecialTweaks`: sequence, item prefix, `m_fBlendTime`, `m_bBlocksChildren`):
//! - the full-body picker (`Nav_Default_Ready` / `Nav_Sprinting_Ready` / `Nav_Crouched`, mantle,
//!   stunned, choke, assassination) poses the whole rig;
//! - `UpperBodyFilter` (`DisAnimNodeBlendPerBone`, branches `spine_3_jnt`, `Collarbone_R_Jnt`,
//!   `Collarbone_L_Jnt`) lays the upper-body picker over it (melee attacks, block, the gunshot
//!   reaction, jump in / small land, crouch in / out, equip);
//! - `LeftArmFilter` (branch `Collarbone_L_Jnt`) lays the left-arm picker over that (the left
//!   item's locomotion `Ply_Nav_LeftHand_at` and its actions: equip, switch, fire, reload, casts,
//!   gadgets, the sword-slash and jump reactions).
//!
//! Both filters cook `LocalToCompReqBones`, so they blend in component space (not
//! `bForceLocalSpaceBlend`): a filtered branch keeps the pose its own sequence gives it relative
//! to the root. `Collarbone_L_Jnt` hangs off bone 1 (the left filter's required bones are 0, 1
//! and the collarbone), so a local-space blend made the gun arm inherit the pelvis twist of the
//! full-body walk and run cycles.
//!
//! The weapon meshes play their own sets (`CorvoGun_as`, `Crossbow_as`, `PlySword_Anim_as`)
//! with the hand that holds them.
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
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::{ChokePhase, LeftPhase, MoveMode, PlayerState, PossessStage, SwordPhase};
use dishonored_sim::tuning::CombatTuning;
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

/// `m_fBlendTime` of the animation states (`Twk_Pawn_Corvo_Release` animation tweaks).
/// Navigation states (`Nav_Default_Ready`, `Nav_Sprinting_Ready`, `Nav_Crouched`).
const BLEND_NAV: f32 = 0.2;
/// `DishonoredAnimNodeBlendBySpeed` `BlendUpTime` / `BlendDownTime` (idle / walk / run inside a
/// navigation tree, and the left hand's `WalkingSpeedBlend`).
const BLEND_SPEED: f32 = 0.4;
/// `Mantle`.
const BLEND_MANTLE: f32 = 0.25;
/// `Choke` (0: the choke masters cut in).
const BLEND_CHOKE: f32 = 0.0;
/// Simple actions: equip / unequip, crouch in / out, small land, gunshot reaction,
/// assassination, stunned, grenade and Spring Razor deploys, the left hand's sword-slash and
/// jump reactions.
const BLEND_ACTION: f32 = 0.2;
/// Melee attacks, `AnimMeleeBlock`, `Upper_Jump_In`.
const BLEND_MELEE: f32 = 0.1;
/// Gun fire, reload in / out, power casts (`CastBlink`, `CastPossess`, `CastWindBlast`, ...),
/// `AnimSwitchPower`.
const BLEND_CAST: f32 = 0.1;
/// Weapon mesh clip changes (the weapons' own trees are not decoded).
const WEAPON_FADE: f32 = 0.1;
/// A finished swing keeps the upper body until this far into its clip (the port's
/// interruptible point; the original exits on `DishonoredNotify_AnimStateExit`).
const SWING_TAIL: f32 = 0.6;

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

/// Sequences the states below name (item prefix applied).
const CLIPS: &[&str] = &[
    // Full body: navigation, air, mantle, stunned, assassination.
    "Sword_Ready_Idle", "Sword_Ready_Walk", "Sword_Ready_Run", "Sword_Ready_Sprint", "Sword_Ready_Jump",
    "Sword_Sneak_Idle", "Sword_Sneak_Walk", "Sword_Sneak_Jump",
    "Sword_Ready_MantleLow", "Sword_Ready_MantleMedium", "Sword_Ready_MantleHigh", "Sword_Sneak_MantleLow", "Sword_Sneak_MantleMedium", "Sword_Sneak_MantleHigh",
    "Sword_Stun_In", "Sword_Stun_Loop", "Sword_Ready_Assassination_Back_Master", "Sword_Ready_Assassination_Generic",
    "Sword_Choke_In_Master", "Sword_Choke_Loop_Master", "Sword_Choke_Win_Master", "Sword_Choke_Lose_Master", "Sword_Choke_Cancel_Master", "Sword_Choke_CarryCorpse_Master",
    "Head_Rope_Climb", "Empty_SwimIdle", "Empty_SwimN",
    // The held gun's aim (additive, `Ply_Guns_as`).
    "ADD_Guns_Aim_UpLeft", "ADD_Guns_Aim_CenterLeft", "ADD_Guns_Aim_DownLeft", "ADD_Guns_Aim_Up", "ADD_Guns_Aim_Center", "ADD_Guns_Aim_Down", "ADD_Guns_Aim_UpRight", "ADD_Guns_Aim_CenterRight", "ADD_Guns_Aim_DownRight",
    // Head bob and the step-up's head (additive, `Ply_Head_Locomotion_as`).
    "ADD_Head_WalkN", "ADD_Head_WalkS", "ADD_Head_WalkW", "ADD_Head_WalkE", "ADD_Head_RunN", "ADD_Head_RunS", "ADD_Head_RunW", "ADD_Head_RunE",
    "ADD_Head_SprintN", "ADD_Head_SprintS", "ADD_Head_SprintW", "ADD_Head_SprintE", "ADD_Head_SneakN", "ADD_Head_SneakS", "ADD_Head_SneakW", "ADD_Head_SneakE", "ADD_Head_Mantle_Low",
    "Empty_CarryCorpse_In_Master", "Empty_CarryCorpse_Idle_Master", "Empty_CarryCorpse_Walk_Master", "Empty_CarryCorpse_DropLow_Master",
    // Upper body.
    "Sword_Ready_AttackRight_A_Small", "Sword_Ready_AttackLeft_A_Small", "Sword_Ready_AttackRight_B_Small", "Sword_Ready_AttackLeft_B_Small", "Sword_Sneak_Attack_Small",
    "Sword_Ready_Parry_A", "Sword_Ready_Parry_B", "Sword_Ready_Parry_C", "Sword_SlideIn", "Sword_SlideLoop", "Sword_SlideOutSneak", "Sword_SlideOutSprint",
    "Sword_Ready_ParryIdle_In", "Sword_Sneak_ParryIdle_In", "Sword_Ready_ParryIdle", "Sword_Ready_ParryIdle_Out", "Sword_Sneak_ParryIdle_Out",
    "Sword_Ready_GunshotReaction", "Sword_Sneak_GunshotReaction", "Sword_Ready_JumpIn", "Sword_Sneak_JumpIn", "Sword_Ready_JumpLandSmall", "Sword_Sneak_JumpLandSmall",
    "Sword_Sneak_In", "Sword_Sneak_Out", "Sword_Ready_Equip",
    // Left arm: locomotion and reactions per item prefix.
    "Guns_Idle", "Guns_Walk", "Guns_Sprint", "Guns_Jump", "Guns_JumpIn", "Guns_JumpLandSmall", "Guns_Ready_Equip", "Guns_SwordSlash_Left", "Guns_SwordSlash_Right",
    "Powers_Idle", "Powers_Walk", "Powers_Sprint", "Powers_Jump", "Powers_JumpIn", "Powers_JumpLandSmall", "Powers_Ready_Equip", "Powers_SwordSlash_Left", "Powers_SwordSlash_Right", "Powers_Switch",
    "Gadgets_Idle", "Gadgets_Walk", "Gadgets_Sprint", "Gadgets_Jump", "Gadgets_JumpIn", "Gadgets_JumpLandSmall", "Gadgets_Ready_Equip", "Gadgets_SwordSlash_Left", "Gadgets_SwordSlash_Right", "Gadgets_Switch",
    // Left arm: actions.
    "Pistol_Fire", "Pistol_Reload_In", "Pistol_Reload_Out", "Pistol_FastReload",
    "Crossbow_Fire", "Crossbow_Fire_FastReload", "Crossbow_Reload_In", "Crossbow_Reload_Out", "Crossbow_RapidReload",
    "Powers_Cast_Blink_In", "Powers_Cast_Blink_Loop", "Powers_Cast_Blink_Out", "Powers_Cast_Windblast", "Powers_Cast_BendTime", "Powers_Cast_Swarm",
    "Powers_Cast_Possession_In", "Powers_Cast_Possession_Loop", "Powers_Cast_Possession_Out", "Powers_Cast_Possession_Cancel",
    "Gadgets_Grenade_Deploy", "Gadgets_SpringRazor_Deploy",
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
pub(crate) fn add_additive(lib: &ClipLib, clip: &str, time: f32, looped: bool, w: f32, pose: &mut [(Quat, Vec3)]) {
    if w <= 0.0 || !lib.has(clip) {
        return;
    }
    let mut d: Pose = vec![(Quat::IDENTITY, Vec3::ZERO); pose.len()];
    lib.sample(clip, time, looped, &mut d);
    for (p, (dq, dt)) in pose.iter_mut().zip(d) {
        p.0 = (Quat::IDENTITY.slerp(dq, w.min(1.0)) * p.0).normalize();
        p.1 += dt * w.min(1.0);
    }
}

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
    /// Particle notifies per sequence: (time, system, socket or bone, attached).
    pnotifies: HashMap<String, Vec<(f32, String, String, bool)>>,
    /// The rig's sockets: name -> (bone, transform relative to it).
    sockets: HashMap<String, (usize, glam::Mat4)>,
    /// Sockets and bones the hand effects are placed on (`effects::HandEffects`).
    effect_points: Vec<String>,
    upper_body: Vec<bool>,
    left_arm: Vec<bool>,
    cam_bone: usize,
    /// The bones of `Ply_Player_at`'s look controls: `LookAtControl_RightHand`
    /// (`Collarbone_R_Jnt`), `LookAtControl_LeftHand` (`Collarbone_L_Jnt`) and
    /// `LookAtControl_Camera` (`spine_3_jnt`).
    look_bones: [Option<usize>; 3],
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
    let set_paths: Vec<String> = SETS.iter().map(|s| format!("{s}.{s}")).collect();
    let lib = ClipLib::load(&content, &bones, &set_paths, CLIPS);
    let missing: Vec<&&str> = CLIPS.iter().filter(|c| !lib.has(c)).collect();
    if !missing.is_empty() {
        warn!("first-person rig: sequences not found: {missing:?}");
    }
    info!("first-person rig: {} bones, {} weapons, {} clips", bones.len(), weapons.len(), lib.clips.len());
    // The rig's sockets, and the particle notifies of its sets on them.
    let rig_sockets: HashMap<String, (usize, glam::Mat4)> = sockets
        .iter()
        .filter_map(|s| find(&s.bone).map(|b| (s.name.clone(), (b, glam::Mat4::from_translation(s.location) * dishonored_assets::util::rotator_matrix(s.rotation[0], s.rotation[1], s.rotation[2])))))
        .collect();
    let mut pnotifies: HashMap<String, Vec<(f32, String, String, bool)>> = HashMap::new();
    let mut effect_points: Vec<String> = vec![crate::effects::HAND_CAST_SOCKET.to_string()];
    for path in &set_paths {
        match dishonored_assets::characters::particle_notifies(&content, "Startup", path) {
            Ok(list) => {
                for n in list {
                    let point = if !n.socket.is_empty() { n.socket } else { n.bone };
                    if point.is_empty() || (!rig_sockets.contains_key(&point) && find(&point).is_none()) {
                        continue;
                    }
                    if !effect_points.contains(&point) {
                        effect_points.push(point.clone());
                    }
                    pnotifies.entry(n.sequence).or_default().push((n.time, n.template, point, n.attach));
                }
            }
            Err(e) => warn!("{path} notifies: {e:#}"),
        }
    }
    info!("first-person rig: particle notifies on {} sequences", pnotifies.len());
    let arms_gpu = upload(&arms, &mut meshes, &mut images, &mut mats, &mut binds, &feat);
    let bind_pose = bones.iter().map(|b| (b.rotation, b.translation)).collect();
    let look_bones = [find("Collarbone_R_Jnt"), find("Collarbone_L_Jnt"), find("spine_3_jnt")];
    commands.insert_resource(ViewRig { arms: arms_gpu, parents: parents(&bones), bones, bind_pose, m2p, weapons, bolts, lib, pnotifies, sockets: rig_sockets, effect_points, upper_body, left_arm, cam_bone, look_bones });
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

/// A sequence request for a layer.
#[derive(Clone, Debug)]
struct Play {
    /// Identity of the request: a new key restarts the layer and crossfades into it.
    key: String,
    clip: String,
    looped: bool,
    rate: f32,
    /// Blend-in time (the state's `m_fBlendTime`, or the tree node's own).
    blend: f32,
    /// Sound notifies fire (the left hand's locomotion players have `bNoNotifies`).
    notifies: bool,
}

fn once(key: impl Into<String>, clip: &str, blend: f32) -> Play {
    Play { key: key.into(), clip: clip.to_string(), looped: false, rate: 1.0, blend, notifies: true }
}

fn looping(key: impl Into<String>, clip: &str, rate: f32, blend: f32) -> Play {
    Play { key: key.into(), clip: clip.to_string(), looped: true, rate, blend, notifies: true }
}

/// One picker of the tree: the playing sequence, the crossfade from the previous one and the
/// filter weight it enters the final pose with.
#[derive(Default)]
struct Layer {
    key: String,
    clip: String,
    time: f32,
    rate: f32,
    looped: bool,
    notifies: bool,
    blend: f32,
    fade: f32,
    from: Option<Pose>,
    last: Pose,
    /// Filter weight (`Child2Weight`): 1 = the layer's branch is fully shown.
    weight: f32,
}

impl Layer {
    fn set(&mut self, p: &Play) {
        self.rate = p.rate;
        if self.key == p.key {
            return;
        }
        // Crossfade from what the layer showed, unless it was not shown at all.
        self.from = (self.weight > 0.0 && !self.last.is_empty()).then(|| self.last.clone());
        self.key = p.key.clone();
        self.clip = p.clip.clone();
        self.looped = p.looped;
        self.notifies = p.notifies;
        self.blend = p.blend;
        self.time = 0.0;
        self.fade = 0.0;
    }

    /// Move the filter weight toward `target` over `time` seconds.
    fn approach(&mut self, target: f32, time: f32, dt: f32) {
        self.weight = if time <= 0.0 {
            target
        } else if self.weight < target {
            (self.weight + dt / time).min(target)
        } else {
            (self.weight - dt / time).max(target)
        };
    }

    fn pose(&mut self, lib: &ClipLib, bind: &[(Quat, Vec3)]) -> Pose {
        // Bones the sequence does not drive keep the reference pose (a sequence player's
        // default; the gun and power sets drive only bones 0 and 1 and the left arm).
        let mut pose = bind.to_vec();
        lib.sample(&self.clip, self.time, self.looped, &mut pose);
        let w = if self.blend > 0.0 { (self.fade / self.blend).clamp(0.0, 1.0) } else { 1.0 };
        let done = match &self.from {
            Some(from) if w < 1.0 && from.len() == pose.len() => {
                for (p, f) in pose.iter_mut().zip(from) {
                    *p = mix(*f, *p, w);
                }
                false
            }
            _ => true,
        };
        if done {
            self.from = None;
        }
        self.last.clone_from(&pose);
        pose
    }
}

/// Advance a layer; returns the clip-time window it crossed.
fn advance_layer(l: &mut Layer, dt: f32, sounds: Option<&mut crate::audio::AnimSounds>, id: dishonored_sim::state::PlayerId) -> (f32, f32) {
    let from = l.time;
    l.time += dt * l.rate;
    l.fade += dt;
    // Sound notifies (AnimNotify_AkEvent, footfalls) of the advanced window.
    if l.notifies && l.weight > 0.0 {
        if let Some(s) = sounds {
            s.advance(id, &l.clip, from, l.time, l.looped);
        }
    }
    (from, l.time)
}

/// Whether a notify at `t` lies in the clip-time window [`from`, `to`) (wrapping for loops,
/// clamped at the end otherwise).
pub fn crossed(t: f32, from: f32, to: f32, len: f32, looped: bool) -> bool {
    if to <= from {
        return false;
    }
    if !looped || len <= 0.0 {
        return t >= from && t < to && (len <= 0.0 || t <= len);
    }
    if to - from >= len {
        return true;
    }
    let (a, b) = (from.rem_euclid(len), to.rem_euclid(len));
    if a <= b { t >= a && t < b } else { t >= a || t < b }
}

/// The step-up's head sequence and its blend (`DisAnimNodeBlendByStepUpMantle.m_fBlendTime`,
/// `m_fLowMantleStepUpBlendTime` 0.2).
const STEP_UP_HEAD: &str = "ADD_Head_Mantle_Low";
const STEP_UP_BLEND: f32 = 0.2;

/// Head bob of the full body's navigation: the directional additive of the walk / run / sprint /
/// sneak cycle (`ADD_Head_{Walk,Run,Sprint,Sneak}{N,S,W,E}`, synced to the cycle) weighted by
/// `AnimNodeBlendDirectional` over the movement direction. `Nav_Default` and `Nav_Sprinting`
/// pass it through `DisAnimNodeBlendByHeadBob`: its tick (0x00A88C30) weighs "Head Bob On" by
/// `DishonoredPlayerCamera.m_BobAmount` and "Off" by the rest (1 without a player camera);
/// the camera's begin play (0x00D06080) sets `m_BobAmount` from the options' head bob amount
/// (`PSI_Gameplay_HeadBobAmount`) unless a camera tweak overrides it. `Nav_Crouched` has no such
/// node.
fn head_bob(p: &PlayerState, fb: &Layer, lib: &ClipLib, amount: f32) -> (Vec<(String, f32, f32)>, f32) {
    let (kind, gated) = match fb.key.as_str() {
        "nav_default:walk" => ("Walk", true),
        "nav_default:run" => ("Run", true),
        "nav_sprint:sprint" => ("Sprint", true),
        "nav_crouch:walk" => ("Sneak", false),
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

/// The weapon mesh's own clip, crossfaded on changes.
#[derive(Default)]
struct WeaponAnim {
    key: String,
    from: Option<Pose>,
    fade: f32,
    last: Pose,
}

/// What the previous frame showed, for the transitions that start one-shot actions.
#[derive(Clone, Copy)]
struct Prev {
    mode: MoveMode,
    crouched: bool,
    alive: bool,
    swings: u8,
    sword: SwordPhase,
    item: LeftHand,
    left: LeftPhase,
    possess: PossessStage,
    step_ups: u8,
    parries: u8,
}

impl Prev {
    fn of(p: &PlayerState) -> Self {
        Prev { mode: p.mode, crouched: p.crouched, alive: p.alive, swings: p.sword.swings, sword: p.sword.phase, item: p.left.item, left: p.left.phase, possess: p.possess.stage, step_ups: p.traversal.step_ups, parries: p.sword.parries }
    }
}

/// First-person animation state (local view only).
#[derive(Resource, Default)]
pub struct ViewAnim {
    /// `FullBodyStatePicker`.
    fb: Layer,
    /// `UpperBodyStatePicker` through `UpperBodyFilter`.
    ub: Layer,
    /// `LeftArmStatePicker` through `LeftArmFilter`.
    la: Layer,
    /// One-shot upper-body / left-arm actions started by a transition (played to their end).
    ub_shot: Option<Play>,
    la_shot: Option<Play>,
    shots: u32,
    /// Left-hand phase changes (keys of back-to-back actions differ).
    left_action: u32,
    prev: Option<Prev>,
    weapons: Vec<WeaponAnim>,
    /// The rig's pawn space (the master root of synced sequences) relative to the game camera,
    /// unscaled, while the rig is shown.
    master_local: Option<Mat4>,
    /// Seconds into the step-up's head sequence, while it plays.
    step_up: Option<f32>,
    /// A won parry's sequence (`Blocking Parry`), played over the block.
    parry: Option<Play>,
    /// The view yaw last frame (degrees) and the held item's `m_CurItemAim` (degrees).
    prev_yaw: Option<f32>,
    item_aim: [f32; 2],
}

/// Attack clips in `SwordState::swing_clip` order (`Attack_Seq` of the melee states
/// `RightAttackA`, `LeftAttackA`, `RightAttackB`, `LeftAttackB`, `SneakAttackA`).
const SWING_CLIPS: [&str; 5] = ["Sword_Ready_AttackRight_A_Small", "Sword_Ready_AttackLeft_A_Small", "Sword_Ready_AttackRight_B_Small", "Sword_Ready_AttackLeft_B_Small", "Sword_Sneak_Attack_Small"];

fn swing_clip(p: &PlayerState) -> &'static str {
    SWING_CLIPS[(p.sword.swing_clip as usize).min(4)]
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

fn stance(p: &PlayerState) -> &'static str {
    if p.crouched { "Sneak" } else { "Ready" }
}

/// A one-shot action with a fresh key, if the clip was decoded.
fn shot(a: &mut ViewAnim, lib: &ClipLib, clip: &str, blend: f32) -> Option<Play> {
    if !lib.has(clip) {
        return None;
    }
    a.shots += 1;
    Some(once(format!("{clip}#s{}", a.shots), clip, blend))
}

/// Transitions since the last frame that start one-shot actions.
fn transitions(p: &PlayerState, a: &mut ViewAnim, lib: &ClipLib) {
    let Some(prev) = a.prev else { return };
    let st = stance(p);
    let pre = left_prefix(p.left.item);
    if p.left.phase != prev.left && p.left.phase != LeftPhase::Idle {
        a.left_action += 1;
    }
    // A won parry: `Blocking Parry`, an `AnimNodeRandom` of Sword_Ready_Parry_A / B / C at
    // chances 0.33 / 0.33 / 0.34 (`Ply_Player_MeleeBlock_at`).
    if p.sword.parries != prev.parries {
        let r = (p.sword.parries as u32).wrapping_mul(2654435761) % 100;
        let clip = if r < 33 { "Sword_Ready_Parry_A" } else if r < 66 { "Sword_Ready_Parry_B" } else { "Sword_Ready_Parry_C" };
        a.parry = shot(a, lib, clip, BLEND_MELEE);
    }
    // A step-up (0x00AD35E0): the `StepUpMantle` head.
    if p.traversal.step_ups != prev.step_ups {
        a.step_up = Some(0.0);
    }
    // Respawn: `EquipItem_Ready` (Sword_Ready_Equip).
    if p.alive && !prev.alive {
        a.ub_shot = shot(a, lib, "Sword_Ready_Equip", BLEND_ACTION);
    }
    // `Upper_GoTo_Crouch` / `Upper_Leave_Crouch` (Sneak_In / Sneak_Out).
    if p.crouched != prev.crouched && p.mode == MoveMode::Walking {
        a.ub_shot = shot(a, lib, if p.crouched { "Sword_Sneak_In" } else { "Sword_Sneak_Out" }, BLEND_ACTION);
    }
    // `Upper_Jump_In` / `Upper_Jump_In_LeftHand`.
    if prev.mode == MoveMode::Walking && p.mode == MoveMode::Falling && p.velocity.z > 0.0 {
        a.ub_shot = shot(a, lib, &format!("Sword_{st}_JumpIn"), BLEND_MELEE);
        if let Some(pre) = pre {
            a.la_shot = shot(a, lib, &format!("{pre}JumpIn"), BLEND_ACTION);
        }
    }
    // `Upper_Jump_Land_Small` / `Upper_Jump_Land_Small_LeftHand`.
    if prev.mode == MoveMode::Falling && p.mode == MoveMode::Walking {
        a.ub_shot = shot(a, lib, &format!("Sword_{st}_JumpLandSmall"), BLEND_ACTION);
        if let Some(pre) = pre {
            a.la_shot = shot(a, lib, &format!("{pre}JumpLandSmall"), BLEND_ACTION);
        }
    }
    // A new swing: the left hand's `SwordSlash_Left` / `SwordSlash_Right`.
    if p.sword.swings != prev.swings && matches!(p.sword.phase, SwordPhase::Startup | SwordPhase::Active | SwordPhase::Recovery) {
        if let Some(pre) = pre {
            let side = if matches!(p.sword.swing_clip, 1 | 3) { "Left" } else { "Right" };
            a.la_shot = shot(a, lib, &format!("{pre}SwordSlash_{side}"), BLEND_ACTION);
        }
    }
    // A shot: `PistolReaction` / `CrossbowReaction` (Ready_GunshotReaction) moves the sword arm.
    let gun = p.left.item == LeftHand::Pistol || p.left.item.is_crossbow();
    if gun && p.left.phase == LeftPhase::Recovering && prev.left != LeftPhase::Recovering {
        a.ub_shot = shot(a, lib, &format!("Sword_{st}_GunshotReaction"), BLEND_ACTION);
    }
    // Item change: `AnimSwitchPower` between items of one kind, else `EquipItem_Ready`.
    if p.left.item != prev.item {
        if let Some(pre) = pre {
            let switch = format!("{pre}Switch");
            a.la_shot = if left_prefix(prev.item) == Some(pre) && lib.has(&switch) { shot(a, lib, &switch, BLEND_CAST) } else { shot(a, lib, &format!("{pre}Ready_Equip"), BLEND_ACTION) };
        }
    }
    // Cancels: `CastPossess` Cancel, `CastBlink` Cancel (= Powers_Cast_Blink_Out).
    if prev.possess == PossessStage::Targeting && p.possess.stage == PossessStage::Inactive && p.left.item == LeftHand::Possession {
        a.la_shot = shot(a, lib, "Powers_Cast_Possession_Cancel", BLEND_CAST);
    }
    if p.left.item == LeftHand::Blink && prev.item == LeftHand::Blink && prev.left == LeftPhase::Aiming && p.left.phase == LeftPhase::Idle {
        a.la_shot = shot(a, lib, "Powers_Cast_Blink_Out", BLEND_CAST);
    }
    // Block released: `Block Canceled` (ParryIdle_Out).
    if prev.sword == SwordPhase::Blocking && p.sword.phase == SwordPhase::Idle {
        a.ub_shot = shot(a, lib, &format!("Sword_{st}_ParryIdle_Out"), BLEND_MELEE);
    }
}

/// The one-shot still playing (it ends a blend time before its clip does, so the layer has
/// faded out when the clip ends).
fn running_shot(slot: &mut Option<Play>, layer: &Layer, lib: &ClipLib) -> Option<Play> {
    let s = slot.as_ref()?;
    let finished = layer.key == s.key && layer.time + s.blend * s.rate >= lib.length(&s.clip);
    if finished {
        *slot = None;
        return None;
    }
    Some(s.clone())
}

/// The first decoded clip of `candidates`.
fn pick<'a>(lib: &ClipLib, candidates: &[&'a str]) -> Option<&'a str> {
    candidates.iter().copied().find(|c| lib.has(c))
}

/// Full-body state (`FullBodyStatePicker`) and whether it blocks the upper-body and left-arm
/// pickers (`m_bBlocksChildren`).
fn full_body(p: &PlayerState, a: &ViewAnim, lib: &ClipLib, gameplay: &GameplayConfig) -> (Play, bool) {
    let mv = &gameplay.movement;
    // Choke (`Ply_Sword_Choke_as` masters, by `eChokeState`) and carrying a body
    // (`Ply_Empty_CarryCorpse_as` masters): whole rig.
    let choke = match p.choke.phase {
        ChokePhase::In => Some(("choke_in", "Sword_Choke_In_Master", false)),
        ChokePhase::Loop => Some(("choke_loop", "Sword_Choke_Loop_Master", true)),
        ChokePhase::Win => Some(("choke_win", "Sword_Choke_Win_Master", false)),
        ChokePhase::Lose => Some(("choke_lose", "Sword_Choke_Lose_Master", false)),
        ChokePhase::Cancel => Some(("choke_cancel", "Sword_Choke_Cancel_Master", false)),
        ChokePhase::None => None,
    };
    if let Some((k, c, l)) = choke {
        return (Play { key: k.into(), clip: c.into(), looped: l, rate: 1.0, blend: BLEND_CHOKE, notifies: true }, true);
    }
    let speed = p.velocity.truncate().length();
    if p.carrying.is_some() {
        if p.carry_anim > 0.0 {
            // Lifted straight from a won choke, or picked up from the ground.
            let from_choke = a.fb.key.starts_with("choke") || a.fb.key == "carry_choke";
            return (if from_choke { once("carry_choke", "Sword_Choke_CarryCorpse_Master", BLEND_CHOKE) } else { once("carry_in", "Empty_CarryCorpse_In_Master", BLEND_ACTION) }, true);
        }
        let walking = p.mode == MoveMode::Walking && speed > 30.0;
        return (if walking { looping("carry_walk", "Empty_CarryCorpse_Walk_Master", (speed / 200.0).clamp(0.6, 1.4), BLEND_NAV) } else { looping("carry_idle", "Empty_CarryCorpse_Idle_Master", 1.0, BLEND_NAV) }, true);
    }
    // The low drop plays out over the action lock that follows it.
    if p.action_lock > 0.0 && a.fb.key.starts_with("carry") {
        return (once("carry_drop", "Empty_CarryCorpse_DropLow_Master", BLEND_ACTION), true);
    }
    match p.sword.phase {
        // `Assassinate_BackA` (KillSeq Ready_Assassination_Back_Master), else the upper-body
        // `Assassinate_GenericA`.
        SwordPhase::Assassinating => {
            if let Some(c) = pick(lib, &["Sword_Ready_Assassination_Back_Master", "Sword_Ready_Assassination_Generic"]) {
                return (once("assassinate", c, BLEND_ACTION), true);
            }
        }
        // `AnimStunned` (`Ply_Player_Stunned_at`: in, then loop).
        SwordPhase::Staggered => {
            let in_done = a.fb.key == "stun_loop" || (a.fb.key == "stun_in" && a.fb.time >= lib.length("Sword_Stun_In"));
            if in_done && lib.has("Sword_Stun_Loop") {
                return (looping("stun_loop", "Sword_Stun_Loop", 1.0, BLEND_ACTION), true);
            }
            if lib.has("Sword_Stun_In") {
                return (once("stun_in", "Sword_Stun_In", BLEND_ACTION), true);
            }
        }
        _ => {}
    }
    // `Mantle` (`Ply_Player_Mantle_at`): `DishonoredAnimNodeBlendByMantleType` picks low /
    // medium / high from the edge height against `m_LowMantleMaxEdgeHeight` (120) and
    // `m_MediumMantleMaxEdgeHeight` (170) of `pMantleTweaks` (the finder's own classes);
    // `DisAnimNodeBlendByPlayerSneak` / `...MantleCrouchRequired` take the sneaking sequences.
    // Stretched over the simulation's traversal.
    if p.mode == MoveMode::Mantling {
        let e = p.traversal.edge_height;
        let h = if e <= mv.mantle.low_max_edge_height {
            "Low"
        } else if e <= mv.mantle.medium_max_edge_height {
            "Medium"
        } else {
            "High"
        };
        let clip = format!("Sword_{}_Mantle{h}", stance(p));
        let clip = if lib.has(&clip) { clip } else { format!("Sword_Ready_Mantle{h}") };
        if lib.has(&clip) {
            let len = lib.length(&clip);
            let rate = if p.traversal.duration > 0.0 { (len / p.traversal.duration).clamp(0.5, 2.0) } else { 1.0 };
            return (Play { key: format!("mantle_{h}"), clip, looped: false, rate, blend: BLEND_MANTLE, notifies: true }, true);
        }
    }
    // `Slide` (`Ply_Player_Slide_at`, the `Sword_` item prefix, blend 0.1): `SlideIn`, then the
    // looping `SlideLoop`; when the slide ends, `SlideOutSprint` into a sprint, else
    // `SlideOutSneak` (`DisAnimNodeBlendByLocoType`: walking and running take it).
    if p.slide.active {
        let in_done = a.fb.key == "slide_loop" || (a.fb.key == "slide_in" && a.fb.time >= lib.length("Sword_SlideIn"));
        if in_done && lib.has("Sword_SlideLoop") {
            return (looping("slide_loop", "Sword_SlideLoop", 1.0, BLEND_MELEE), false);
        }
        if lib.has("Sword_SlideIn") {
            return (once("slide_in", "Sword_SlideIn", BLEND_MELEE), false);
        }
    }
    if a.fb.key.starts_with("slide_") && p.mode == MoveMode::Walking {
        let out = if a.fb.key.starts_with("slide_out") { a.fb.clip.clone() } else if p.sprinting { "Sword_SlideOutSprint".into() } else { "Sword_SlideOutSneak".into() };
        if lib.has(&out) && !(a.fb.key.starts_with("slide_out") && a.fb.time >= lib.length(&out)) {
            return (once(format!("slide_out:{out}"), &out, BLEND_MELEE), false);
        }
    }    // `Climb` (`Ply_Player_Climb_at`, blocks children, no item prefix): a speed blend between
    // an empty child (the reference pose) and `Head_Rope_Climb` while moving on the chain.
    if p.mode == MoveMode::Climbing {
        if p.velocity.length() > 5.0 && lib.has("Head_Rope_Climb") {
            return (looping("climb:move", "Head_Rope_Climb", 1.0, BLEND_NAV), true);
        }
        return (Play { key: "climb:ref".into(), clip: String::new(), looped: true, rate: 1.0, blend: BLEND_NAV, notifies: false }, true);
    }
    // `Nav_Swim` (blocks children, no item prefix): the swimming sequences of the empty-handed
    // locomotion set (`Empty_SwimIdle`, `Empty_SwimN`; LIKELY, the tree was not dumped).
    if p.mode == MoveMode::Swimming {
        if let Some(c) = pick(lib, &[if speed > 20.0 { "Empty_SwimN" } else { "Empty_SwimIdle" }, "Empty_SwimIdle"]) {
            return (looping(format!("swim:{c}"), c, 1.0, BLEND_NAV), true);
        }
    }
    // Navigation: the air child (`CustomAirIdle`, looping), else `WalkingSpeedBlend`.
    if p.mode == MoveMode::Falling {
        if let Some(c) = pick(lib, &[if p.crouched { "Sword_Sneak_Jump" } else { "Sword_Ready_Jump" }, "Sword_Ready_Jump"]) {
            return (looping(format!("nav_air:{c}"), c, 1.0, BLEND_NAV), false);
        }
    }
    let moving = speed > 20.0 && matches!(p.mode, MoveMode::Walking | MoveMode::Swimming);
    let rate = |nominal: f32| if nominal > 0.0 { (speed / nominal).clamp(0.5, 1.6) } else { 1.0 };
    let (tree, child, clip, r) = if p.crouched {
        // `Nav_Crouched`: idle or walk.
        if moving { ("crouch", "walk", "Sword_Sneak_Walk", rate(mv.crouch_speed)) } else { ("crouch", "idle", "Sword_Sneak_Idle", 1.0) }
    } else if p.sprinting && moving {
        ("sprint", "sprint", "Sword_Ready_Sprint", rate(mv.sprint_speed))
    } else if !moving {
        ("default", "idle", "Sword_Ready_Idle", 1.0)
    } else if speed < 0.45 * (mv.walk_speed + mv.run_speed) {
        ("default", "walk", "Sword_Ready_Walk", rate(mv.walk_speed))
    } else {
        ("default", "run", "Sword_Ready_Run", rate(mv.run_speed))
    };
    let clip = pick(lib, &[clip, "Sword_Ready_Idle"]).unwrap_or("Sword_Ready_Idle");
    let same_tree = a.fb.key.starts_with(&format!("nav_{tree}:"));
    (looping(format!("nav_{tree}:{child}"), clip, r, if same_tree { BLEND_SPEED } else { BLEND_NAV }), false)
}

/// Upper-body state (`UpperBodyStatePicker`), if any.
fn upper_body(p: &PlayerState, a: &mut ViewAnim, lib: &ClipLib) -> Option<Play> {
    match p.sword.phase {
        // The simulation picks the clip (its damage zone times the hit, `SwingTiming`).
        SwordPhase::Startup | SwordPhase::Active | SwordPhase::Recovery => {
            a.ub_shot = None;
            return Some(once(format!("swing{}", p.sword.swings), swing_clip(p), BLEND_MELEE));
        }
        // `AnimMeleeBlock` (`Ply_Player_MeleeBlock_at`): start block, then the loop.
        SwordPhase::Blocking => {
            a.ub_shot = None;
            if let Some(pl) = running_shot(&mut a.parry, &a.ub, lib) {
                return Some(pl);
            }
            let start = format!("Sword_{}_ParryIdle_In", stance(p));
            let in_done = a.ub.key == "block" || (a.ub.key == "block_in" && a.ub.time >= lib.length(&a.ub.clip));
            return Some(if in_done || !lib.has(&start) { looping("block", "Sword_Ready_ParryIdle", 1.0, BLEND_MELEE) } else { once("block_in", &start, BLEND_MELEE) });
        }
        _ => {}
    }
    // A finished swing plays out to its interruptible point.
    if a.ub.key.starts_with("swing") && a.ub.weight > 0.0 && a.ub.time < SWING_TAIL && a.ub_shot.is_none() {
        return Some(once(a.ub.key.clone(), &a.ub.clip, BLEND_MELEE));
    }
    running_shot(&mut a.ub_shot, &a.ub, lib)
}

/// Left-arm state (`LeftArmStatePicker`), if the left hand holds something.
fn left_arm(p: &PlayerState, a: &mut ViewAnim, lib: &ClipLib, tuning: &CombatTuning, gameplay: &GameplayConfig) -> Option<Play> {
    let item = p.left.item;
    let pre = left_prefix(item)?;
    // `CastPossess` (`Ply_CastPower_at`): in, loop while targeting, out when possessing.
    if item == LeftHand::Possession {
        match p.possess.stage {
            PossessStage::Targeting => {
                a.la_shot = None;
                let in_done = a.la.key == "poss_loop" || (a.la.key == "poss_in" && a.la.time >= lib.length("Powers_Cast_Possession_In"));
                return Some(if in_done { looping("poss_loop", "Powers_Cast_Possession_Loop", 1.0, BLEND_CAST) } else { once("poss_in", "Powers_Cast_Possession_In", BLEND_CAST) });
            }
            PossessStage::PrePossess => {
                a.la_shot = None;
                return Some(once("poss_out", "Powers_Cast_Possession_Out", BLEND_CAST));
            }
            _ => {}
        }
    }
    let n = a.left_action;
    let act = |c: &str, rate: f32, blend: f32| Some(Play { key: format!("{c}#{n}"), clip: c.to_string(), looped: false, rate, blend, notifies: true });
    let action = match p.left.phase {
        LeftPhase::Recovering => match item {
            LeftHand::Pistol => act("Pistol_Fire", 1.0, BLEND_CAST),
            // `FireReloadFastCrossbow` (Crossbow_Fire_FastReload at play rate 1.5) with
            // `Attribute_FastReload`.
            i if i.is_crossbow() => {
                if dishonored_sim::left_hand::gun_reload(p, gameplay, tuning).fast > 0.0 { act("Crossbow_Fire_FastReload", 1.5, BLEND_CAST) } else { act("Crossbow_Fire", 1.0, BLEND_CAST) }
            }
            LeftHand::Blink => act("Powers_Cast_Blink_Out", 1.0, BLEND_CAST),
            LeftHand::Windblast => act("Powers_Cast_Windblast", 1.0, BLEND_CAST),
            LeftHand::BendTime => act("Powers_Cast_BendTime", 1.0, BLEND_CAST),
            LeftHand::DevouringSwarm => act("Powers_Cast_Swarm", 1.0, BLEND_CAST),
            LeftHand::Grenade | LeftHand::StickyGrenade => act("Gadgets_Grenade_Deploy", 1.0, BLEND_ACTION),
            LeftHand::SpringRazor => act("Gadgets_SpringRazor_Deploy", 1.0, BLEND_ACTION),
            _ => None,
        },
        // The reload context (`DisItemContext_ReloadGunPlayer`): ReloadIn, the timed stage
        // (ReloadIn holds its last frame), ReloadOut; or the fast reload stretched over its
        // stage. Crossbow actions play at rate 1.3 (`Twk_Inv_CrossbowCorvo`).
        LeftPhase::Reloading => {
            let g = dishonored_sim::left_hand::gun_reload(p, gameplay, tuning);
            let pistol = item == LeftHand::Pistol;
            if g.fast > 0.0 {
                let c = if pistol { "Pistol_FastReload" } else { "Crossbow_RapidReload" };
                let rate = if g.end > 0.0 { (lib.length(c) / g.end).clamp(0.25, 4.0) } else { 1.0 };
                act(c, rate, 0.0)
            } else {
                let (rin, rout, rate) = if pistol { ("Pistol_Reload_In", "Pistol_Reload_Out", 1.0) } else { ("Crossbow_Reload_In", "Crossbow_Reload_Out", 1.3) };
                act(if p.left.timer < g.load { rin } else { rout }, rate, BLEND_CAST)
            }
        }
        // `CastBlink`: in, then loop while aiming.
        LeftPhase::Aiming if item == LeftHand::Blink => {
            let in_done = a.la.key == "blink_loop" || (a.la.key == "blink_in" && a.la.time >= lib.length("Powers_Cast_Blink_In"));
            Some(if in_done { looping("blink_loop", "Powers_Cast_Blink_Loop", 1.0, BLEND_CAST) } else { once("blink_in", "Powers_Cast_Blink_In", BLEND_CAST) })
        }
        _ => None,
    };
    if let Some(play) = action.filter(|pl| lib.has(&pl.clip)) {
        a.la_shot = None;
        return Some(play);
    }
    if let Some(s) = running_shot(&mut a.la_shot, &a.la, lib) {
        return Some(s);
    }
    // `Nav_Default` of the left hand (`Ply_Nav_LeftHand_at`): idle below the speed blend,
    // `CustomWalk` walking or running, `CustomSprint` sprinting, `CustomAirIdle` falling.
    let speed = p.velocity.truncate().length();
    let moving = speed > 20.0;
    let (child, suffix) = if p.mode == MoveMode::Falling {
        ("air", "Jump")
    } else if !moving {
        ("idle", "Idle")
    } else if p.sprinting {
        ("sprint", "Sprint")
    } else {
        ("walk", "Walk")
    };
    let clip = format!("{pre}{suffix}");
    let clip = if lib.has(&clip) { clip } else { format!("{pre}Idle") };
    let blend = if a.la.key.starts_with("lnav:") && child != "air" && !a.la.key.starts_with("lnav:air") { BLEND_SPEED } else { BLEND_NAV };
    Some(Play { key: format!("lnav:{child}:{clip}"), clip, looped: true, rate: 1.0, blend, notifies: false })
}

/// Sync group of a full-body / left-arm locomotion key (`MasterIdleSync`, `MasterRunSync`,
/// `MasterSprintSync`; the full body's walk is in `WalkCycles`, which the left hand does not
/// join).
fn sync_group(key: &str) -> Option<&'static str> {
    if key.starts_with("nav_default:idle") || key.starts_with("nav_crouch:idle") || key.starts_with("lnav:idle:") {
        Some("idle")
    } else if key.starts_with("nav_default:run") || key.starts_with("lnav:walk:") {
        Some("run")
    } else if key.starts_with("nav_sprint:sprint") || key.starts_with("lnav:sprint:") {
        Some("sprint")
    } else {
        None
    }
}

/// The weapon mesh's own sequence for what its hand plays. The weapons' sets hold
/// (Startup): `CorvoGun_as` Equip / FastReload / Fire_Accuracy_1 / Fire_Hammer / Idle;
/// `Crossbow_as` Equip / Fire / Fire_FastReload / Idle / OutOfAmmo_Idle / RapidReload /
/// Reload_In / Spyglass_Fire; `PlySword_Anim_as` the sword's equips, unequips, FirstAttack and
/// `Sword_Ready_Idle`; `Grenade_as` and `SpringRazor_as` only their Idle (`Grenade_at`,
/// `SpringRazor_at` play nothing else). The weapons' trees pick through weapon action tweaks
/// that were not dumped; the pairing below is by name (LIKELY).
fn weapon_clip(w: &WeaponGpu, driver: &Layer, p: &PlayerState) -> Option<String> {
    let arm = if driver.weight > 0.0 { driver.clip.as_str() } else { "" };
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
                "Crossbow_Fire" | "Crossbow_Fire_FastReload" | "Crossbow_RapidReload" | "Crossbow_Reload_In" => pick(&[arm, idle]),
                _ => pick(&[idle, "Crossbow_Idle"]),
            }
        }
        Held::Grenade => pick(&["Grenade_Idle"]),
        Held::SpringRazor => pick(&["SpringRazor_Idle"]),
    }
}
pub fn animate_view_rig(
    rig: Option<Res<ViewRig>>,
    mut anim: ResMut<ViewAnim>,
    view: Res<SessionView>,
    session: Res<crate::session::Session>,
    time: Res<Time>,
    mut cams: Query<(&GameCamera, &mut Transform), (Without<ViewRoot>, Without<ViewJoint>, Without<WeaponJoint>, Without<BoltJoint>)>,
    mut roots: Query<(&mut Transform, &mut Visibility), (With<ViewRoot>, Without<ViewJoint>, Without<WeaponJoint>, Without<WeaponMesh>)>,
    mut joints: Query<(&ViewJoint, &mut Transform), (Without<ViewRoot>, Without<WeaponJoint>)>,
    mut wjoints: Query<(&WeaponJoint, &mut Transform), (Without<ViewRoot>, Without<ViewJoint>)>,
    mut wmeshes: Query<(&WeaponMesh, &mut Visibility), Without<ViewRoot>>,
    mut bjoints: Query<(&BoltJoint, &mut Transform), (Without<ViewRoot>, Without<ViewJoint>, Without<WeaponJoint>)>,
    mut bmeshes: Query<(&BoltMesh, &mut Visibility), (Without<ViewRoot>, Without<WeaponMesh>)>,
    mut anim_sounds: Option<ResMut<crate::audio::AnimSounds>>,
    cutscenes: Option<Res<crate::cutscene::Cutscenes>>,
    (settings, mut hands): (Option<Res<crate::settings::GameSettings>>, ResMut<crate::effects::HandEffects>),
) {
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
        if let Some(m) = view.me.as_ref() {
            anim.prev = Some(Prev::of(m));
        }
        anim.master_local = None;
        anim.prev_yaw = None;
        hands.sockets.clear();
        *root_vis = Visibility::Hidden;
        return;
    };
    *root_vis = Visibility::Inherited;
    let dt = time.delta_secs();
    // The view's yaw (Dishonored degrees) from the game camera, before this frame's bob.
    let view_yaw = cams.iter().next().map(|(_, t)| {
        let f = t.rotation * Vec3::NEG_Z;
        f.z.atan2(f.x).to_degrees()
    });
    // The controller's look pitch (degrees, positive up) from the game camera, before this
    // frame's view adjustment.
    let look_pitch = cams.iter().next().map(|(_, t)| (t.rotation * Vec3::NEG_Z).y.clamp(-1.0, 1.0).asin().to_degrees()).unwrap_or(0.0);
    let gameplay = &session.content.sim.gameplay;
    let tuning = CombatTuning::from_config(gameplay);
    let lib = &rig.lib;
    let a = &mut *anim;

    // Pick each layer's state.
    transitions(me, a, lib);
    let (fb_play, blocks) = full_body(me, a, lib, gameplay);
    a.fb.set(&fb_play);
    a.fb.weight = 1.0;
    if blocks {
        a.ub_shot = None;
        a.la_shot = None;
    }
    let ub_play = if blocks { None } else { upper_body(me, a, lib) };
    match ub_play {
        Some(pl) => {
            a.ub.set(&pl);
            a.ub.approach(1.0, pl.blend, dt);
        }
        None => {
            let b = a.ub.blend;
            a.ub.approach(0.0, b, dt);
        }
    }
    let la_play = if blocks { None } else { left_arm(me, a, lib, &tuning, gameplay) };
    match la_play {
        Some(pl) => {
            a.la.set(&pl);
            a.la.approach(1.0, pl.blend, dt);
        }
        None => {
            let b = a.la.blend;
            a.la.approach(0.0, b, dt);
        }
    }
    a.prev = Some(Prev::of(me));

    // Advance; the left hand's locomotion follows the full body's in a shared sync group.
    let mut windows: Vec<(String, f32, f32, bool)> = Vec::new();
    for l in [&mut a.fb, &mut a.ub, &mut a.la] {
        let (from, to) = advance_layer(l, dt, anim_sounds.as_deref_mut(), me.id);
        if l.notifies && l.weight > 0.0 {
            windows.push((l.clip.clone(), from, to, l.looped));
        }
    }
    // Particle notifies crossed this frame (`AnimNotify_PlayParticleEffect`).
    for (clip, from, to, looped) in &windows {
        let Some(list) = rig.pnotifies.get(clip) else { continue };
        let len = lib.length(clip);
        for (t, template, socket, attach) in list {
            if crossed(*t, *from, *to, len, *looped) {
                hands.pending.push((template.clone(), socket.clone(), *attach));
            }
        }
    }
    if let (Some(gf), Some(gl)) = (sync_group(&a.fb.key), sync_group(&a.la.key)) {
        let (lf, ll) = (lib.length(&a.fb.clip), lib.length(&a.la.clip));
        if gf == gl && lf > 0.0 && ll > 0.0 {
            a.la.time = (a.fb.time / lf).rem_euclid(1.0) * ll;
        }
    }

    // Compose: full body (with its head bob), then the filters in component space, then the
    // step-up's head. The same without the additives gives the unbobbed camera.
    let fb_base = a.fb.pose(lib, &rig.bind_pose);
    let ub_pose = (a.ub.weight > 0.0).then(|| a.ub.pose(lib, &rig.bind_pose));
    let mut la_pose = (a.la.weight > 0.0).then(|| a.la.pose(lib, &rig.bind_pose));
    // The held item's aim (`DishonoredInventoryItem` tick 0x00C0B990): `m_CurItemAim` eases toward
    // the item's target at `clamp(dt x m_fItemAimSpeed, 0, 1)`; the target (slot 102,
    // 0x00C05CD0) is the camera's yaw last frame minus now, in degrees, and no pitch
    // (`CameraCache` vs `LastFrameCameraCache`). `DishonoredSeqBlendByAim` (0x00A8FB20) divides
    // it by its max degrees (10) and lays `ADD_<item prefix>Aim_*` over the left arm
    // (`DisAnimNodeBlendItemAimAdditive`).
    if let Some(yaw) = view_yaw {
        let target = a.prev_yaw.map(|p| (p - yaw + 540.0).rem_euclid(360.0) - 180.0).unwrap_or(0.0);
        let k = (dt * ITEM_AIM_SPEED).clamp(0.0, 1.0);
        a.item_aim[0] += (target - a.item_aim[0]) * k;
        a.item_aim[1] += (0.0 - a.item_aim[1]) * k;
        a.prev_yaw = Some(yaw);
    }
    if let (Some(p), Some(pre)) = (la_pose.as_mut(), left_prefix(me.left.item)) {
        let w = aim_weights(a.item_aim[0] / AIM_MAX_DEGREES, a.item_aim[1] / AIM_MAX_DEGREES);
        let items: Vec<(String, f32, f32)> = AIM_SUFFIXES.iter().zip(w).map(|(s, w)| (format!("ADD_{pre}Aim_{s}"), 0.0, w)).collect();
        add_additive_blend(lib, &items, false, 1.0, p);
    }
    let (ub_w, la_w) = (a.ub.weight, a.la.weight);
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
    {
        let phase = (a.fb.time / lib.length(&a.fb.clip).max(1e-3)).rem_euclid(1.0);
        let (items, w): (Vec<(String, f32, f32)>, f32) = head_bob(me, &a.fb, lib, bob_amount);
        let items: Vec<(String, f32, f32)> = items.into_iter().map(|(c, _, dw)| { let t = phase * lib.length(&c); (c, t, dw) }).collect();
        add_additive_blend(lib, &items, true, w, &mut fb_bob);
    }
    let mut locals = filters(&fb_bob);
    // `StepUpMantle` (`DisAnimNodeBlendByStepUpMantle`, `m_fBlendTime` 0.2): `ADD_Head_Mantle_Low`
    // over the whole pose after a step-up, blended in and out.
    if let Some(t) = a.step_up {
        let len = lib.length(STEP_UP_HEAD);
        let w = (t / STEP_UP_BLEND).min(1.0).min(((len - t) / STEP_UP_BLEND).max(0.0));
        add_additive(lib, STEP_UP_HEAD, t, false, w, &mut locals);
        a.step_up = (t + dt < len).then_some(t + dt);
    }
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

    // The weapons' own sequences, with the hand that holds them.
    if a.weapons.len() != rig.weapons.len() {
        a.weapons = rig.weapons.iter().map(|_| WeaponAnim::default()).collect();
    }
    let mut wglobals: Vec<Vec<glam::Mat4>> = Vec::with_capacity(rig.weapons.len());
    for (w, wa) in rig.weapons.iter().zip(a.weapons.iter_mut()) {
        let driver = match w.held {
            Held::Sword if a.ub.weight > 0.5 => &a.ub,
            Held::Sword => &a.fb,
            Held::Pistol | Held::Crossbow | Held::Grenade | Held::SpringRazor => &a.la,
        };
        let clip = weapon_clip(w, driver, me);
        let mut pose = w.bind_pose.clone();
        if let Some(c) = &clip {
            w.lib.sample(c, driver.time, driver.looped, &mut pose);
        }
        let key = clip.unwrap_or_default();
        if wa.key != key {
            wa.from = (!wa.last.is_empty()).then(|| wa.last.clone());
            wa.key = key;
            wa.fade = 0.0;
        }
        wa.fade += dt;
        let fw = (wa.fade / WEAPON_FADE).clamp(0.0, 1.0);
        let done = match &wa.from {
            Some(from) if fw < 1.0 && from.len() == pose.len() => {
                for (p, f) in pose.iter_mut().zip(from) {
                    *p = mix(*f, *p, fw);
                }
                false
            }
            _ => true,
        };
        if done {
            wa.from = None;
        }
        wa.last.clone_from(&pose);
        let wb: Vec<BoneAsset> = w.bones.iter().zip(&pose).map(|(b, (q, t))| BoneAsset { rotation: *q, translation: *t, ..b.clone() }).collect();
        wglobals.push(bind_globals(&wb));
    }

    if std::env::var_os("DISH_VM_DEBUG").is_some() {
        let cam = rig.m2p * globals[rig.cam_bone];
        let rel = |i: usize| cam.inverse().transform_point3((rig.m2p * globals[i]).w_axis.truncate());
        let names: Vec<String> = rig.weapons.iter().map(|w| format!("{}={:.0}", rig.bones[w.socket].name, rel(w.socket))).collect();
        let wk: Vec<&str> = a.weapons.iter().map(|w| w.key.as_str()).collect();
        info!("vm fb={} ub={}({:.2}) la={}({:.2}) weapons={:?} {}", a.fb.clip, a.ub.clip, a.ub.weight, a.la.clip, a.la.weight, wk, names.join(" "));
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
    // the bone's motion from those (head bob, the step-up's head, the look pivot about
    // `spine_3_jnt`) moves it, and it turns to the view rotation above (and the lean roll).
    let delta = cam.w_axis.truncate() - cam0.w_axis.truncate();
    let mut cam_world = Mat4::IDENTITY;
    for (_, mut ct) in &mut cams {
        let f = ct.rotation * Vec3::NEG_Z;
        let fh = Vec3::new(f.x, 0.0, f.z).try_normalize().unwrap_or(Vec3::NEG_Z);
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
        let carrying = me.carrying.is_some() || a.fb.key == "carry_drop";
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

/// A body left where its choke or carry ended (Dishonored-space root position and yaw), kept
/// until the simulation moves it.
#[derive(Clone, Copy, Debug)]
pub struct BodyAnchor {
    pub pos: glam::Vec3,
    pub yaw: f32,
    sim_pos: glam::Vec3,
}

/// Bodies drawn in another pawn's frame. Synced sequences (`Generic_Choke_*_Slave`,
/// `Corpses_CarryCorpse_*_Slave`) are authored at the master's root, so a choked or carried
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
            // In a choke, or playing its Win / Lose slave while the master is in that state.
            let choker = p.condition_by.and_then(|c| all.iter().find(|q| q.id == c));
            let in_choke = choker.is_some_and(|c| {
                p.condition == Condition::BeingChoked || (c.choke.target == Some(p.id) && matches!(c.choke.phase, ChokePhase::Win | ChokePhase::Lose))
            });
            if in_choke { choker.and_then(|c| frame_of(c.id)).map(|(_, level)| (level, level)) } else { None }
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
pub fn apply_fov(mut cams: Query<&mut Projection, (With<Camera3d>, Without<crate::reflect::ReflectCamera>)>, settings: Option<Res<crate::settings::GameSettings>>) {
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
