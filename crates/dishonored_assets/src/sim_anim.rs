//! The sequences the simulation plays (`GameplayConfig::sequences`) and its NPC animation
//! profile (`GameplayConfig::npc_anim`), decoded from the AnimSets: per sequence its
//! `SequenceLength`, `RateScale` and every `AnimNotifyEvent` (`Time`, `Duration`, the notify
//! object's class and properties over its class defaults), in the cooked order.
//!
//! The sets are the player's (`Startup` `Ply_*_as`: what the `Twk_Pawn_Corvo` animation states
//! play, research/dishonored/ANIMATION.md and `viewmodel.rs`), the crossbow's own set (its
//! `DishonoredNotify_FireProjectile`, ranged.md F14) and the elite guard's generic and sword
//! sets (its attacks, hit reactions, deaths and throws), whose `DisNPCAnimDefinitions_EliteGuard`
//! chains the simulation's NPC rules pick sequences from.

use crate::content::Content;
use crate::tweaks::{TMap, TVal, TweakDb, merge, props_to_map};
use anyhow::{Context, Result};
use dishonored_ir::gameplay::{NotifyInfo, NpcAnimConfig, RootMotion, SequenceInfo, sequence_key};
use std::collections::BTreeMap;
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

/// The player's sets in `Startup` (package path `<group>.<set>`).
pub const PLAYER_SETS: &[&str] = &[
    "Ply_Sword_Attack_as.Ply_Sword_Attack_as",
    "Ply_Sword_Versus_as.Ply_Sword_Versus_as",
    "Ply_Sword_Hit_as.Ply_Sword_Hit_as",
    "Ply_Sword_Assassination_as.Ply_Sword_Assassination_as",
    "Ply_Sword_Fatality_as.Ply_Sword_Fatality_as",
    "Ply_Sword_Choke_as.Ply_Sword_Choke_as",
    "Ply_Empty_CarryCorpse_as.Ply_Empty_CarryCorpse_as",
    "Ply_Pistol_as.Ply_Pistol_as",
    "Ply_Crossbow_as.Ply_Crossbow_as",
    "Ply_Guns_as.Ply_Guns_as",
    "Ply_Powers_as.Ply_Powers_as",
    "Ply_Gadgets_as.Ply_Gadgets_as",
    "Ply_Gadgets_Grenade_as.Ply_Gadgets_Grenade_as",
    "Ply_Gadgets_SpringRazor_as.Ply_Gadgets_SpringRazor_as",
];

/// The held weapons' own sets in `Startup` (the crossbow mesh plays `Crossbow_Fire` with the arms;
/// its sequence carries the shot's `DishonoredNotify_FireProjectile`, 0x00A96890).
pub const WEAPON_SETS: &[&str] = &["Crossbows_as.Crossbow_as"];

/// The elite guard pawn tweak whose animation profile the simulation's NPC rules use (the
/// scenarios' Boyle doorman: `GUARD_SETS`, `DisNPCAnimDefinitions_EliteGuard`, `Sword_` /
/// `Empty_`).
pub const NPC_TWEAK: &str = "Twk_Pawn_Boyle_Party.Pwn_EliteGuard_MTall_2_Doorman";

/// The elite guard's sets whose sequences the simulation plays: attacks, dodges, reactions,
/// deaths (generic and sword), the pistol's aim chains (gun) and the empty-handed locomotion's
/// start, stop and turn sequences (empty).
pub const NPC_SETS: &[&str] = &[
    "Npc_EliteGuard_Generic_as.Npc_EliteGuard_Generic_as",
    "Npc_EliteGuard_empty_as.Npc_EliteGuard_empty_as",
    "Npc_EliteGuard_Sword_as.Npc_EliteGuard_Sword_as",
    "Npc_EliteGuard_Gun_as.Npc_EliteGuard_Gun_as",
];

/// A notify object's properties over its class defaults: numbers and bools into `payload`
/// (nested structs and arrays flattened as `crate::gameplay::flatten` does), the top-level
/// vectors as `<name>.X` / `.Y` / `.Z` (`DisNotify_DropItem.m_DropVelocity`), names, enums,
/// strings and object paths into `names`.
fn notify_props(m: &TMap) -> (BTreeMap<String, f32>, BTreeMap<String, String>) {
    let mut payload = crate::gameplay::flatten(m);
    for (k, v) in m {
        if let TVal::Vec3(v) = v {
            for (axis, c) in ["X", "Y", "Z"].iter().zip(v) {
                payload.insert(format!("{k}.{axis}"), *c);
            }
        }
    }
    let names = m
        .iter()
        .filter_map(|(k, v)| match v {
            TVal::Name(s) | TVal::Str(s) | TVal::Obj(s) => Some((k.clone(), s.clone())),
            _ => None,
        })
        .collect();
    (payload, names)
}

/// Every sequence of a set (`<group>.<set>` in `package`), keyed by [`sequence_key`] with the
/// set's object name.
pub fn set_sequences(content: &Content, db: &TweakDb, package: &str, set_path: &str, root_motion: &BTreeMap<String, RootMotion>) -> Result<BTreeMap<String, SequenceInfo>> {
    let pkg = content.package(package)?;
    let set = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(set_path)).with_context(|| format!("{set_path} not in {package}"))?;
    let set_name = pkg.obj_name(ObjRef::Export(set));
    let sh = pkg.read_object(set)?;
    let mut defaults: BTreeMap<String, TMap> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for s in find(&sh.properties, "Sequences").context("Sequences")?.array_objects()? {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let h = pkg.read_object(e)?;
        let Some(Value::Name(name)) = find(&h.properties, "SequenceName").map(|p| &p.value) else { continue };
        let duration = find(&h.properties, "SequenceLength").and_then(|p| p.as_float()).unwrap_or(0.0);
        // `UAnimSequence.RateScale` defaults to 1 (an unset property).
        let rate_scale = find(&h.properties, "RateScale").and_then(|p| p.as_float()).unwrap_or(1.0);
        let mut notifies = Vec::new();
        if let Some(list) = find(&h.properties, "Notifies") {
            for ev in list.array_structs(&pkg).unwrap_or_default() {
                let time = find(&ev, "Time").and_then(|p| p.as_float()).unwrap_or(0.0);
                let duration = find(&ev, "Duration").and_then(|p| p.as_float()).unwrap_or(0.0);
                let (class, m) = match find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index) {
                    Some(ObjRef::Export(ni)) => {
                        let class = pkg.export_class(ni);
                        let base = defaults.entry(class.clone()).or_insert_with(|| db.class_defaults(&class).unwrap_or_default());
                        let mut m = base.clone();
                        merge(&mut m, &props_to_map(&pkg, &pkg.read_object(ni)?.properties));
                        (class, m)
                    }
                    _ => ("None".to_string(), TMap::new()),
                };
                let (payload, names) = notify_props(&m);
                notifies.push(NotifyInfo { class, time, duration, payload, names });
            }
        }
        let root = root_motion.contains_key(name.as_str()).then(|| name.clone());
        out.insert(sequence_key(&set_name, name), SequenceInfo { duration, rate_scale, notifies, root_motion: root });
    }
    Ok(out)
}

/// The simulation's sequence table: the player's and weapons' sets from `Startup`, the elite
/// guard's from the first NPC package that cooks each.
pub fn load_sequences(content: &Content, db: &TweakDb, root_motion: &BTreeMap<String, RootMotion>, prov: &mut BTreeMap<String, String>) -> Result<BTreeMap<String, SequenceInfo>> {
    let mut out = BTreeMap::new();
    for set in PLAYER_SETS.iter().chain(WEAPON_SETS) {
        let seqs = set_sequences(content, db, "Startup", set, root_motion)?;
        prov.insert(format!("sequences.{set}"), format!("Startup.upk {set}: {} sequences (SequenceLength, RateScale, Notifies over the notify classes' defaults)", seqs.len()));
        out.extend(seqs);
    }
    for set in NPC_SETS {
        let (pkg, seqs) = crate::ai_config::NPC_ANIM_PACKAGES
            .iter()
            .find_map(|p| content.has_package(p).then(|| set_sequences(content, db, p, set, root_motion).ok().map(|s| (*p, s))).flatten())
            .with_context(|| format!("{set} not in the NPC packages"))?;
        prov.insert(format!("sequences.{set}"), format!("{pkg}.upk {set}: {} sequences", seqs.len()));
        out.extend(seqs);
    }
    Ok(out)
}

/// The animation of the simulation's NPC trees (with the install, `dishonored_content`'s
/// `AnimSource`): the trees, the profile's body skeleton, its mesh rotation, its sets bound to the
/// skeleton with clips decoded on first use, its locomotion config.
pub struct NpcTreeData {
    /// `Npc_Common_at.NPC_Human_at` and `Npc_Common_at.NPC_Dead_at` (`Startup`).
    pub human: dishonored_ir::anim_tree::AnimTreeAsset,
    pub dead: dishonored_ir::anim_tree::AnimTreeAsset,
    pub skeleton: Vec<dishonored_ir::BoneAsset>,
    /// Mesh space -> the pawn's frame as the presentation places the body (`RotOrigin`, then
    /// turned to face +X: `characters::load_model`; the mesh component's own rotation is not
    /// decoded).
    pub mesh_rotation: glam::Quat,
    pub clips: crate::characters::LazyAnimSets,
    pub loco: dishonored_ir::npc_anim::LocoConfig,
}

/// The root motion of the NPC sequences the simulation plays (the table's sequences of
/// [`NPC_SETS`], keyed by their [`sequence_key`]): the root bone's (skeleton bone 0) offset from
/// the clip's first frame and its turn since that frame, sampled at
/// [`crate::fatality::ROOT_MOTION_RATE`] in the pawn's frame as the NPC tree evaluates it
/// (`NpcTreeData::mesh_rotation`; `dishonored_anim::TreeState::root_delta` takes the same track
/// through the same rotation). Only the key a name resolves to from the profile's sets is baked
/// (`profile_sets` searched from the last, as the simulation's `SeqTable::find` and UE3's
/// `FindAnimSequence`); a sequence whose root never leaves its first frame is left out.
pub fn load_npc_root_motion(data: &NpcTreeData, profile_sets: &[String], sequences: &BTreeMap<String, SequenceInfo>) -> BTreeMap<String, RootMotion> {
    use dishonored_anim::Clips;
    let rate = crate::fatality::ROOT_MOTION_RATE;
    let sets: Vec<String> = NPC_SETS.iter().map(|s| s.rsplit('.').next().unwrap_or(s).to_string()).collect();
    let mut out = BTreeMap::new();
    for key in sequences.keys() {
        let Some((set, name)) = key.split_once('.') else { continue };
        if !sets.iter().any(|s| s == set) || profile_sets.iter().rev().find(|s| sequences.contains_key(&sequence_key(s, name))).is_none_or(|s| s != set) {
            continue;
        }
        let Some(c) = data.clips.find(name) else { continue };
        let Some(j) = c.bones.iter().position(|b| *b == Some(0)) else { continue };
        let len = c.clip.duration.max(0.0);
        let at = |t: f32| {
            let (q, p) = c.clip.sample_held(t.clamp(0.0, len), j);
            (q.unwrap_or(glam::Quat::IDENTITY), p.unwrap_or(glam::Vec3::ZERO))
        };
        let (q0, p0) = at(0.0);
        let m = data.mesh_rotation;
        let n = (len * rate).ceil() as usize + 1;
        let (mut keys, mut yaw): (Vec<[f32; 2]>, Vec<f32>) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let mut moves = false;
        for i in 0..n {
            let (q, p) = at(i as f32 / rate);
            let d = m * (p - p0);
            // The root's turn since the first frame seen in the pawn's frame: where it takes the
            // pawn's forward axis (UE3 yaw: x forward, y right).
            let f = m * ((q * q0.inverse()) * (m.inverse() * glam::Vec3::X));
            // Unwrapped across +-180 so that a half turn's curve is continuous.
            let mut y = f.y.atan2(f.x).to_degrees();
            if let Some(&last) = yaw.last() {
                y = last + (y - last + 180.0f32).rem_euclid(360.0) - 180.0;
            }
            moves |= d.truncate().length() > 1e-3 || y.abs() > 1e-3;
            keys.push([d.x, d.y]);
            yaw.push(y);
        }
        if moves {
            out.insert(key.clone(), RootMotion { rate, keys, yaw });
        }
    }
    out
}

/// [`NpcTreeData`] of [`NPC_TWEAK`]'s profile.
pub fn load_npc_tree_data(content: &Content) -> Result<NpcTreeData> {
    let pkgs: Vec<String> = crate::ai_config::NPC_ANIM_PACKAGES.iter().map(|s| s.to_string()).collect();
    let looks = crate::characters::resolve_appearances(content, &pkgs, &[NPC_TWEAK.to_string()])?;
    let look = looks.get(NPC_TWEAK).with_context(|| format!("{NPC_TWEAK} not resolved"))?;
    let (body_pkg, body) = look.body.clone().with_context(|| format!("{NPC_TWEAK}: no m_pSkeletalMesh"))?;
    let skeleton = crate::characters::load_skeleton(content, &body_pkg, &body)?;
    let pkg = content.package(&body_pkg)?;
    let mesh = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SkeletalMesh" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&body)).with_context(|| format!("{body} not in {body_pkg}"))?;
    let raw = crate::skeletal_mesh::read_skeletal_mesh(&pkg, mesh)?;
    let r = crate::util::rotator_matrix(raw.rot_origin[0], raw.rot_origin[1], raw.rot_origin[2]);
    let f = r.transform_vector3(glam::Vec3::Z);
    let mesh_rotation = glam::Quat::from_mat4(&(glam::Mat4::from_rotation_z(-f.y.atan2(f.x)) * r));
    let clips = crate::characters::LazyAnimSets::bind(content, &look.anim.sets, &skeleton, &skeleton)?;
    let loco = crate::npc_anim::load_loco_config(content, look.anim.locomotion.as_ref().with_context(|| format!("{NPC_TWEAK}: no locomotion config"))?)?;
    Ok(NpcTreeData {
        human: crate::anim_tree::load_anim_tree(content, "Startup", "Npc_Common_at.NPC_Human_at")?,
        dead: crate::anim_tree::load_anim_tree(content, "Startup", "Npc_Common_at.NPC_Dead_at")?,
        skeleton,
        mesh_rotation,
        clips,
        loco,
    })
}

/// The NPC master state machine's transition logic (`DishonoredGame.upk`
/// `Default__DishonoredNPCPawn.MasterFSM.pMasterFSMTransitionLogic`: `m_Transitions` rows of
/// `m_pFromClass`, `m_pToClass`, `m_Result`) and the parents of the classes it names (their class
/// exports' super), up to `DishonoredNativeState`.
pub fn load_npc_fsm(content: &Content) -> Result<dishonored_ir::npc_anim::NpcFsm> {
    const LOGIC: &str = "Default__DishonoredNPCPawn.MasterFSM.pMasterFSMTransitionLogic";
    const RESULTS: &[&str] = &["EStateTransitionLogicResult_NotAllowed", "EStateTransitionLogicResult_Allowed", "EStateTransitionLogicResult_Call"];
    let pkg = content.package("DishonoredGame")?;
    let i = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(LOGIC)).with_context(|| format!("{LOGIC} not in DishonoredGame"))?;
    let h = pkg.read_object(i)?;
    let mut fsm = dishonored_ir::npc_anim::NpcFsm::default();
    let class = |r: Option<i32>| r.map(ObjRef::from_index).filter(|r| *r != ObjRef::Null).map(|r| pkg.obj_name(r)).unwrap_or_default();
    for row in find(&h.properties, "m_Transitions").context("m_Transitions")?.array_structs(&pkg)? {
        let from = class(find(&row, "m_pFromClass").and_then(|p| p.as_object()));
        let to = class(find(&row, "m_pToClass").and_then(|p| p.as_object()));
        let result = match find(&row, "m_Result").map(|p| &p.value) {
            Some(Value::Enum(s)) => RESULTS.iter().position(|r| r.eq_ignore_ascii_case(s)).unwrap_or(2) as u8,
            Some(Value::Byte(b)) => *b,
            _ => 0,
        };
        fsm.transitions.push((from, to, result));
    }
    // The parents of every class the rows name, from the class exports' super.
    let mut todo: Vec<String> = fsm.transitions.iter().flat_map(|r| [r.0.clone(), r.1.clone()]).collect();
    while let Some(c) = todo.pop() {
        if c.is_empty() || c == "DishonoredNativeState" || fsm.parents.contains_key(&c) {
            continue;
        }
        let Some(e) = (0..pkg.exports.len()).find(|&e| pkg.export_class(e) == "Class" && pkg.obj_name(ObjRef::Export(e)) == c) else { continue };
        let s = pkg.exports[e].super_;
        if s == 0 {
            continue;
        }
        let parent = pkg.obj_name(ObjRef::from_index(s));
        todo.push(parent.clone());
        fsm.parents.insert(c, parent);
    }
    Ok(fsm)
}

/// The NPC profile of [`NPC_TWEAK`]: its `m_pAnimDefClass` chains, its `m_AnimSets` object names
/// in tweak order and its items' prefixes.
pub fn load_npc_anim(content: &Content, prov: &mut BTreeMap<String, String>) -> Result<NpcAnimConfig> {
    let pkgs: Vec<String> = crate::ai_config::NPC_ANIM_PACKAGES.iter().map(|s| s.to_string()).collect();
    let looks = crate::characters::resolve_appearances(content, &pkgs, &[NPC_TWEAK.to_string()])?;
    let look = looks.get(NPC_TWEAK).with_context(|| format!("{NPC_TWEAK} not resolved"))?;
    let profile = &look.anim;
    anyhow::ensure!(!profile.defs.is_empty(), "{NPC_TWEAK}: no m_pAnimDefClass");
    let defs = crate::npc_anim::load_anim_defs(content, &pkgs, &profile.defs)?;
    let sets: Vec<String> = profile.sets.iter().map(|(_, path)| path.rsplit('.').next().unwrap_or(path).to_string()).collect();
    prov.insert("npc_anim".into(), format!("{NPC_TWEAK}: m_pAnimDefClass {}, m_AnimSets {sets:?}, m_ItemPrefix {:?} / empty hands {:?}", profile.defs, profile.item_prefix, profile.empty_prefix));
    let fsm = load_npc_fsm(content)?;
    prov.insert("npc_anim.fsm".into(), format!("DishonoredGame.upk Default__DishonoredNPCPawn.MasterFSM.pMasterFSMTransitionLogic: {} rows, {} class parents", fsm.transitions.len(), fsm.parents.len()));
    let loco_node = load_loco_node(content)?;
    prov.insert("npc_anim.loco_node".into(), format!("Startup.upk Npc_Common_at.NPC_Human_at.ArkAnimNodeLocomotion_0 over Engine.upk Default__ArkAnimNodeLocomotion: {loco_node:?}"));
    Ok(NpcAnimConfig { defs, sets, item_prefix: profile.item_prefix.clone(), empty_prefix: profile.empty_prefix.clone(), fsm, loco_node })
}

/// The NPC tree's locomotion node tunables (`NPC_Human_at`'s node over the class defaults); every
/// one must be found.
pub fn load_loco_node(content: &Content) -> Result<dishonored_ir::npc_anim::LocoNode> {
    const NAMES: [&str; 10] = [
        "m_fVelocityMaxBlendSpeed",
        "m_fVelocityMaxDif",
        "m_fVelocityMinBlendSpeedRatio",
        "m_fModifierBlendDuration",
        "m_fSpeedMultiplier",
        "m_fTurnFastBlendDuration",
        "m_fTurnSlowBlendDuration",
        "m_fStartBlendDuration",
        "m_fStopBlendDuration",
        "m_fTransitionBlendDuration",
    ];
    let f = crate::npc_anim::loco_node_floats(content, &NAMES);
    let v = |k: usize| f[k].with_context(|| format!("ArkAnimNodeLocomotion.{} not found", NAMES[k]));
    Ok(dishonored_ir::npc_anim::LocoNode {
        velocity_rate: v(0)?,
        velocity_range: v(1)?,
        velocity_min_ratio: v(2)?,
        modifier_blend: v(3)?,
        speed_multiplier: v(4)?,
        turn_fast_blend: v(5)?,
        turn_slow_blend: v(6)?,
        start_blend: v(7)?,
        stop_blend: v(8)?,
        transition_blend: v(9)?,
    })
}
