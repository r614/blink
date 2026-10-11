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

/// The elite guard's sets whose sequences the simulation plays (attacks, reactions, deaths).
pub const NPC_SETS: &[&str] = &["Npc_EliteGuard_Generic_as.Npc_EliteGuard_Generic_as", "Npc_EliteGuard_Sword_as.Npc_EliteGuard_Sword_as"];

/// A notify object's properties over its class defaults: numbers and bools into `payload`
/// (nested structs and arrays flattened as `crate::gameplay::flatten` does), names, enums,
/// strings and object paths into `names`.
fn notify_props(m: &TMap) -> (BTreeMap<String, f32>, BTreeMap<String, String>) {
    let payload = crate::gameplay::flatten(m);
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
    Ok(NpcAnimConfig { defs, sets, item_prefix: profile.item_prefix.clone(), empty_prefix: profile.empty_prefix.clone() })
}
