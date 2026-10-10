//! The player sword's finisher kills (`DisTweaks_Fatality` of `Twk_Inv_SwordCorvo`; the rules are
//! in `dishonored_sim::fatality`): move set 0's chances and actions, each action's animation
//! states mapped to their master sequences through the pawn's full-body states (item prefix
//! applied), each master's `DishonoredNotify_AnimStateExit` and bend-time windows, and the
//! slave chain of the action.

use crate::content::Content;
use crate::tweaks::{TMap, TVal, TweakDb, get, get_f32};
use anyhow::{Context, Result};
use dishonored_ir::gameplay::{BendWindow, FatalityClip, FatalityConfig, RootMotion, fatality_move};
use dishonored_ir::npc_anim::NPC_ANIM_NAMES;
use std::collections::HashMap;
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

/// The sword's fatality context, the pawn's full-body animation states and the sword item.
pub const FATALITY: &str = "Twk_Inv_PlayerSpecific.Twk_Inv_SwordCorvo.DisTweaks_InventoryItem_PlayerSpecific_1.DisTweaks_Fatality_1";
/// The pawn's state objects: (path, upper body).
const ANIMATION: [(&str, bool); 3] = [
    ("Twk_Pawn_Corvo.Twk_Pawn_Corvo.DisTweaks_PlayerPawn_Animation_1.DisTweaks_PlayerPawn_FullBodyAnimation_1", false),
    ("Twk_Pawn_Corvo.Twk_Pawn_Corvo.DisTweaks_PlayerPawn_Animation_1.DisTweaks_PlayerPawn_SpecialAnimation_1", false),
    ("Twk_Pawn_Corvo.Twk_Pawn_Corvo.DisTweaks_PlayerPawn_Animation_1.DisTweaks_PlayerPawn_UpperBodyAnimation_1", true),
];
const SWORD: &str = "Twk_Inv_PlayerSpecific.Twk_Inv_SwordCorvo";
/// The masters' AnimSet (`m_PawnItemAnimSets` of the sword).
const SET: (&str, &str) = ("Startup", "Ply_Sword_Fatality_as.Ply_Sword_Fatality_as");
const BEND_CLASSES: [&str; 2] = ["DisNotify_BendTime_Ranged", "DisNotify_AdrenalineBendTime_Ranged"];

/// An `array<name>` property (8-byte name references).
fn names(pkg: &Package, p: &Property) -> Option<Vec<String>> {
    let Value::Array(b) = &p.value else { return None };
    let n = i32::from_le_bytes(b.get(..4)?.try_into().ok()?);
    if n < 0 || b.len() != 4 + 8 * n as usize {
        return None;
    }
    b[4..]
        .chunks_exact(8)
        .map(|c| {
            let (i, k) = (i32::from_le_bytes(c[..4].try_into().ok()?), i32::from_le_bytes(c[4..].try_into().ok()?));
            let base = pkg.names.get(usize::try_from(i).ok()?)?.name.clone();
            Some(if k > 0 { format!("{base}_{}", k - 1) } else { base })
        })
        .collect()
}

/// `m_FatalityMoveSets[0].<field>` (`.m_MainAction` for a linked action) `.m_AnimStates` from
/// the first object of the tweak's lineage that sets it.
fn action_states(lineage: &[(std::sync::Arc<Package>, usize)], field: &str) -> Vec<String> {
    for (pkg, e) in lineage {
        let Ok(h) = pkg.read_object(*e) else { continue };
        let Some(set) = h.properties.iter().find(|p| p.name.eq_ignore_ascii_case("m_FatalityMoveSets") && p.array_index == 0) else { continue };
        let Value::Struct(_, sp) = &set.value else { continue };
        let Some(Value::Struct(_, ap)) = find(sp, field).map(|p| &p.value) else { continue };
        let action = if field == "m_Generic" {
            ap
        } else {
            match find(ap, "m_MainAction").map(|p| &p.value) {
                Some(Value::Struct(_, m)) => m,
                _ => continue,
            }
        };
        if let Some(s) = find(action, "m_AnimStates") {
            return names(pkg, s).unwrap_or_default();
        }
    }
    Vec::new()
}

/// The pawn's states: state -> (sequence, item prefix applied, upper body).
fn state_table(db: &TweakDb) -> Result<HashMap<String, (String, bool, bool)>> {
    let mut out = HashMap::new();
    for (path, upper) in ANIMATION {
    let t = db.resolve(path)?;
    for (k, v) in &t {
        let TVal::List(states) = v else { continue };
        if !k.starts_with("m_AnimStates") {
            continue;
        }
        for s in states {
            let Some(TVal::Name(state)) = s.get("m_StateName") else { continue };
            let prefix = matches!(s.get("m_bAddItemPrefix"), Some(TVal::B(true)));
            let seq = match s.get("m_CustomSequences") {
                // A simple action's `CustomSequence`; a melee-attack state's `Attack_Seq`.
                Some(TVal::List(c)) => ["CustomSequence", "Attack_Seq"].iter().find_map(|alias| c.iter().find(|c| matches!(c.get("m_Alias"), Some(TVal::Name(a)) if a == alias))).and_then(|c| match c.get("m_Sequence") {
                    Some(TVal::Name(n)) => Some(n.clone()),
                    _ => None,
                }),
                _ => None,
            };
            if let Some(seq) = seq {
                out.entry(state.clone()).or_insert((seq, prefix, upper));
            }
        }
    }
    }
    Ok(out)
}

/// The bend-time windows of a sequence of the masters' set (play time: `Time / RateScale`,
/// `Duration / RateScale`), the notify's values over its class defaults.
fn bend_windows(content: &Content, db: &TweakDb, sequence: &str) -> Vec<BendWindow> {
    let Ok(pkg) = content.package(SET.0) else { return Vec::new() };
    let Some(set) = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(SET.1)) else { return Vec::new() };
    let Ok(h) = pkg.read_object(set) else { return Vec::new() };
    let defaults: HashMap<&str, TMap> = BEND_CLASSES.iter().map(|c| (*c, db.class_defaults(c).unwrap_or_default())).collect();
    let mut out = Vec::new();
    for s in find(&h.properties, "Sequences").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let Ok(sh) = pkg.read_object(e) else { continue };
        if !matches!(find(&sh.properties, "SequenceName").map(|p| &p.value), Some(Value::Name(n)) if n.eq_ignore_ascii_case(sequence)) {
            continue;
        }
        let rate = find(&sh.properties, "RateScale").and_then(|p| p.as_float()).filter(|r| *r > 0.0).unwrap_or(1.0);
        for ev in find(&sh.properties, "Notifies").and_then(|p| p.array_structs(&pkg).ok()).unwrap_or_default() {
            let Some(ObjRef::Export(ni)) = find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
            let class = pkg.export_class(ni);
            let Some(d) = defaults.get(class.as_str()) else { continue };
            let own = pkg.read_object(ni).map(|h| crate::tweaks::props_to_map(&pkg, &h.properties)).unwrap_or_default();
            let f = |k: &str| get_f32(&own, k).or_else(|| get_f32(d, k)).unwrap_or(1.0);
            out.push(BendWindow {
                start: find(&ev, "Time").and_then(|p| p.as_float()).unwrap_or(0.0) / rate,
                duration: find(&ev, "Duration").and_then(|p| p.as_float()).unwrap_or(0.0) / rate,
                world: f("m_fWorldTimeScale"),
                player: f("m_fPlayerTimeScale"),
                blend_in: f("m_fTransitionInTime"),
                blend_out: f("m_fTransitionOutTime"),
                adrenaline: class == BEND_CLASSES[1],
            });
        }
        break;
    }
    out
}

/// When the damage zone of a sequence's `DishonoredNotify_AttackZone` opens: the notify's end
/// (`(Time + Duration) / RateScale`) less its `m_fDamageZoneMaxTime` (class default when unset),
/// not before its start.
fn damage_zone(content: &Content, db: &TweakDb, sequence: &str) -> Option<f32> {
    let pkg = content.package(SET.0).ok()?;
    let set = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(SET.1))?;
    let h = pkg.read_object(set).ok()?;
    for s in find(&h.properties, "Sequences")?.array_objects().ok()? {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let Ok(sh) = pkg.read_object(e) else { continue };
        if !matches!(find(&sh.properties, "SequenceName").map(|p| &p.value), Some(Value::Name(n)) if n.eq_ignore_ascii_case(sequence)) {
            continue;
        }
        let rate = find(&sh.properties, "RateScale").and_then(|p| p.as_float()).filter(|r| *r > 0.0).unwrap_or(1.0);
        for ev in find(&sh.properties, "Notifies")?.array_structs(&pkg).ok()? {
            let Some(ObjRef::Export(ni)) = find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
            if pkg.export_class(ni) != "DishonoredNotify_AttackZone" {
                continue;
            }
            let own = pkg.read_object(ni).map(|h| crate::tweaks::props_to_map(&pkg, &h.properties)).unwrap_or_default();
            let d = db.class_defaults("DishonoredNotify_AttackZone").unwrap_or_default();
            let max = get_f32(&own, "m_fDamageZoneMaxTime").or_else(|| get_f32(&d, "m_fDamageZoneMaxTime")).unwrap_or(0.0);
            let t = find(&ev, "Time").and_then(|p| p.as_float()).unwrap_or(0.0);
            let dur = find(&ev, "Duration").and_then(|p| p.as_float()).unwrap_or(0.0);
            return Some(((t + dur - max).max(t)) / rate);
        }
        return None;
    }
    None
}

/// The player's skeletal mesh (its `RotOrigin` turns mesh space into the pawn's frame).
const PLAYER_MESH: (&str, &str) = ("Engine", "Ply_Player.Skm_Player");
/// The paired moves' AnimSets (`m_PawnItemAnimSets` of the sword).
const MASTER_SETS: [(&str, &str); 2] = [("Startup", "Ply_Sword_Assassination_as.Ply_Sword_Assassination_as"), SET];
/// Samples per second of the baked root motion.
pub const ROOT_MOTION_RATE: f32 = 30.0;

/// The root motion of the paired moves' masters (`DisTweaks_PlayerPawn_FullBodyAnimation` states
/// `Assassinate_*`, `ShortFatality_*`, `Fatality_*`: `RMM_Translate`, `RMRM_RotateActor`): the
/// root bone (Edge joint 0, `root0_jnt`) offset from the clip's first frame, turned into the pawn's
/// frame by the mesh's `RotOrigin`, and the yaw of its turn since the first frame in that frame.
/// Upper-body states (the generic finisher swing) move nothing.
pub fn load_root_motion(content: &Content, fatality: &FatalityConfig) -> Result<std::collections::BTreeMap<String, RootMotion>> {
    let pkg = content.package(PLAYER_MESH.0)?;
    let mesh = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SkeletalMesh" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(PLAYER_MESH.1)).with_context(|| format!("{} not in {}", PLAYER_MESH.1, PLAYER_MESH.0))?;
    let raw = crate::skeletal_mesh::read_skeletal_mesh(&pkg, mesh)?;
    let rot = crate::util::rotator_matrix(raw.rot_origin[0], raw.rot_origin[1], raw.rot_origin[2]);
    let mut names: Vec<String> = dishonored_ir::gameplay::ASSASSINATION_MASTERS.iter().map(|s| s.to_string()).collect();
    names.extend(fatality.moves.iter().flatten().filter(|c| !c.upper_body).map(|c| c.master.clone()));
    let wanted: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let mut out = std::collections::BTreeMap::new();
    for (pkg_name, set) in MASTER_SETS {
        let lib = crate::characters::load_anim_set(content, pkg_name, set, &wanted)?;
        for (name, clip) in &lib.clips {
            if out.contains_key(name) {
                continue;
            }
            let at = |t: f32| clip.sample(t.min((clip.duration - 1e-4).max(0.0)), 0);
            let (q0, r0) = at(0.0);
            let (q0, r0) = (q0.unwrap_or_default(), r0.unwrap_or_default());
            let n = (clip.duration.max(0.0) * ROOT_MOTION_RATE).ceil() as usize + 1;
            let (mut keys, mut yaw) = (Vec::with_capacity(n), Vec::with_capacity(n));
            for i in 0..n {
                let (q, r) = at(i as f32 / ROOT_MOTION_RATE);
                let d = rot.transform_vector3(r.unwrap_or(r0) - r0);
                keys.push([d.x, d.y]);
                // The root's turn since the first frame, seen in the pawn's frame: where it takes the
                // pawn's forward axis (UE3 yaw: x forward, y right).
                let turn = q.unwrap_or(q0) * q0.inverse();
                let f = rot.transform_vector3(turn * rot.inverse().transform_vector3(glam::Vec3::X));
                yaw.push(f.y.atan2(f.x).to_degrees());
            }
            out.insert(name.clone(), RootMotion { rate: ROOT_MOTION_RATE, keys, yaw });
        }
    }
    Ok(out)
}

/// The sword's fatality config.
pub fn load_fatality(content: &Content, db: &TweakDb) -> Result<FatalityConfig> {
    let t = db.resolve(FATALITY)?;
    let Some(TVal::Struct(set)) = get(&t, "m_FatalityMoveSets") else { anyhow::bail!("{FATALITY}: no move set 0") };
    let sf = |k: &str| get_f32(set, k).unwrap_or(0.0);
    let tf = |k: &str| get_f32(&t, k).unwrap_or(0.0);
    let prefix = match get(&db.resolve(SWORD)?, "m_ItemPrefix") {
        Some(TVal::Name(n)) => n.clone(),
        _ => String::new(),
    };
    let states = state_table(db)?;
    let lineage = db.raw_lineage(FATALITY);
    let mut moves = Vec::with_capacity(fatality_move::COUNT);
    for field in fatality_move::FIELDS {
        let slave = match get(set, &format!("{field}.m_SlaveAction.m_Animation")) {
            Some(TVal::Name(n)) => n.strip_prefix("DisNPCAnim_").and_then(|n| NPC_ANIM_NAMES.iter().position(|x| x.eq_ignore_ascii_case(n))).unwrap_or(0) as u8,
            _ => 0,
        };
        let mut clips = Vec::new();
        for state in action_states(&lineage, field) {
            let Some((seq, add, upper)) = states.get(&state) else { continue };
            let master = if *add { format!("{prefix}{seq}") } else { seq.clone() };
            let exit = crate::npc_anim::sequence_notify_time(content, SET.0, SET.1, &master, "DishonoredNotify_AnimStateExit").or_else(|| crate::npc_anim::sequence_play_length(content, SET.0, SET.1, &master));
            let Some(exit) = exit else { continue };
            let bends = bend_windows(content, db, &master);
            let hit = (slave == 0).then(|| damage_zone(content, db, &master)).flatten();
            clips.push(FatalityClip { state, master, exit, slave, upper_body: *upper, hit, bends });
        }
        moves.push(clips);
    }
    let fin = |k: &str| get(&t, k).and_then(|v| if let TVal::I(i) = v { Some(*i) } else { None }).unwrap_or(0);
    Ok::<_, anyhow::Error>(FatalityConfig {
        behead_chance: sf("m_fBeheadRandomChance"),
        slomo_chance: sf("m_fSlomoFinisherRandomChance"),
        slomo_min_interval: sf("m_fMinTimeBetweenRandomSlomoFinisher"),
        slomo_realtime_chance: sf("m_fChanceSlomoFinisherWillBeNormalSpeed"),
        generic_chance: sf("m_fGenericFatalityChance"),
        finishers_before_slow: [fin("m_NumFinishersBeforeSlow_Min"), fin("m_NumFinishersBeforeSlow_Max")],
        time_before_slow: [tf("m_fTimeBeforeSlow_Min"), tf("m_fTimeBeforeSlow_Max")],
        adrenaline_mult: tf("m_fAdrenalineMultOnHit"),
        max_height_difference: tf("m_fMaxContextHeightDifference"),
        moves,
    })
    .context("fatality")
}
