//! Decoding of the original NPC animation data: Arkane locomotion configs
//! (`ArkComponentLocomotionConfig`) and animation definition classes (`DisAnimDefinitions`
//! subclasses in `DishonoredGameContent`). See research/dishonored/ANIMATION.md.

use crate::content::Content;
use anyhow::{Context, Result};
use dishonored_ir::npc_anim::*;
use std::sync::Arc;
use ue3_package::properties::{Property, Value, find};
use ue3_package::reader::Reader;
use ue3_package::{FName, ObjRef, Package};

fn f32_of(props: &[Property], n: &str) -> f32 {
    find(props, n).and_then(|p| p.as_float()).unwrap_or(0.0)
}

fn bool_of(props: &[Property], n: &str) -> bool {
    matches!(find(props, n).map(|p| &p.value), Some(Value::Bool(true)))
}

fn name_of(props: &[Property], n: &str) -> String {
    match find(props, n).map(|p| &p.value) {
        Some(Value::Name(s)) => s.clone(),
        _ => String::new(),
    }
}

fn enum_index(props: &[Property], n: &str, variants: &[&str]) -> u8 {
    match find(props, n).map(|p| &p.value) {
        Some(Value::Enum(s)) => variants.iter().position(|v| v.eq_ignore_ascii_case(s)).unwrap_or(0) as u8,
        Some(Value::Byte(b)) => *b,
        _ => 0,
    }
}

/// An `array<name>` property.
fn names(pkg: &Package, props: &[Property], n: &str) -> Vec<String> {
    let Some((count, raw)) = find(props, n).and_then(|p| p.array_raw()) else { return Vec::new() };
    let mut r = Reader::new(raw);
    (0..count.min(4096)).filter_map(|_| FName::read(&mut r).ok().map(|f| pkg.name(f))).collect()
}

/// An `array<int>` property.
fn ints(props: &[Property], n: &str) -> Vec<i32> {
    let Some((count, raw)) = find(props, n).and_then(|p| p.array_raw()) else { return Vec::new() };
    raw.chunks_exact(4).take(count).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn structs(pkg: &Package, props: &[Property], n: &str) -> Vec<Vec<Property>> {
    find(props, n).and_then(|p| p.array_structs(pkg).ok()).unwrap_or_default()
}

fn anim_names(pkg: &Package, props: &[Property], n: &str) -> Vec<String> {
    structs(pkg, props, n).iter().map(|s| name_of(s, "m_AnimName")).filter(|s| !s.is_empty() && s != "None").collect()
}

/// Decode an `ArkComponentLocomotionConfig` export.
pub fn read_loco_config(pkg: &Package, export: usize) -> Result<LocoConfig> {
    let h = pkg.read_object(export)?;
    let p = &h.properties;
    let speed = |n: &str, d: f32| find(p, n).and_then(|x| x.as_float()).unwrap_or(d);
    let mut c = LocoConfig { name: pkg.obj_path(ObjRef::Export(export)), speed_names: names(pkg, p, "m_SpeedNames"), table: ints(p, "m_SpeedModes"), ref_mesh: name_of(p, "m_SkeletalRefMeshName"), stop_speed: speed("m_fVelocityToStopMoving", 20.0), start_speed: speed("m_fVelocityToStartToMove", 60.0), ..Default::default() };
    for m in structs(pkg, p, "m_Modifiers") {
        c.modifiers.push(LocoModifier { name: name_of(&m, "m_ModifierName"), acceleration: f32_of(&m, "m_fAcceleration"), deceleration: f32_of(&m, "m_fDeceleration"), speed_multiplier: find(&m, "m_fSpeedMultiplier").and_then(|x| x.as_float()).unwrap_or(1.0) });
    }
    for m in structs(pkg, p, "m_SpeedModePropDictionary") {
        let move_anims = structs(pkg, &m, "m_MoveAnimProps").iter().map(|a| (name_of(a, "m_AnimName"), f32_of(a, "m_fAnimSpeed"))).collect();
        c.modes.push(SpeedMode {
            name: name_of(&m, "m_SpeedModeName"),
            rotation_max_speed: f32_of(&m, "m_fRotationMaxSpeed"),
            move_anims,
            start_anims: anim_names(pkg, &m, "m_StartAnimProps"),
            stop_anims: anim_names(pkg, &m, "m_StopAnimProps"),
            turn_anims: anim_names(pkg, &m, "m_TurnAnimProps"),
            foot_on_ground: f32_of(&m, "m_fFootOnGroundPct"),
        });
    }
    for t in structs(pkg, p, "m_Transitions") {
        let i = |n: &str| match find(&t, n).map(|x| &x.value) {
            Some(Value::Int(v)) => *v,
            _ => 0,
        };
        c.transitions.push((name_of(&t, "m_AnimName"), i("m_StartModifier"), i("m_EndModifier")));
    }
    c.feet = structs(pkg, p, "m_Feet").iter().map(|f| f32_of(f, "m_fOnGroundPct")).collect();
    Ok(c)
}

/// Every `ArkComponentLocomotionConfig` of the named packages (first package wins a name).
pub fn package_loco_configs(content: &Content, packages: &[&str]) -> Vec<LocoConfig> {
    let mut out: Vec<LocoConfig> = Vec::new();
    for p in packages {
        let Ok(pkg) = content.package(p) else { continue };
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "ArkComponentLocomotionConfig" {
                continue;
            }
            let path = pkg.obj_path(ObjRef::Export(i));
            if path.starts_with("Default__") || out.iter().any(|c| c.name.eq_ignore_ascii_case(&path)) {
                continue;
            }
            if let Ok(c) = load_loco_config(content, &(p.to_string(), path)) {
                out.push(c);
            }
        }
    }
    out
}

/// Play length (s) of an `AnimSequence` of an `AnimSet`: `SequenceLength / RateScale`.
pub fn sequence_play_length(content: &Content, package: &str, set_path: &str, sequence: &str) -> Option<f32> {
    let pkg = content.package(package).ok()?;
    let set = find_export(&pkg, set_path)?;
    let h = pkg.read_object(set).ok()?;
    for s in find(&h.properties, "Sequences")?.array_objects().ok()? {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let Ok(sh) = pkg.read_object(e) else { continue };
        if name_of(&sh.properties, "SequenceName").eq_ignore_ascii_case(sequence) {
            let len = find(&sh.properties, "SequenceLength").and_then(|p| p.as_float())?;
            let rate = find(&sh.properties, "RateScale").and_then(|p| p.as_float()).unwrap_or(1.0);
            return Some(if rate > 0.0 { len / rate } else { len });
        }
    }
    None
}

/// Play time (s) of the first notify of class `notify_class` in an `AnimSequence` of an
/// `AnimSet`: the notify's `Time / RateScale`.
pub fn sequence_notify_time(content: &Content, package: &str, set_path: &str, sequence: &str, notify_class: &str) -> Option<f32> {
    let pkg = content.package(package).ok()?;
    let set = find_export(&pkg, set_path)?;
    let h = pkg.read_object(set).ok()?;
    for s in find(&h.properties, "Sequences")?.array_objects().ok()? {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let Ok(sh) = pkg.read_object(e) else { continue };
        if !name_of(&sh.properties, "SequenceName").eq_ignore_ascii_case(sequence) {
            continue;
        }
        let rate = find(&sh.properties, "RateScale").and_then(|p| p.as_float()).unwrap_or(1.0);
        for ev in find(&sh.properties, "Notifies")?.array_structs(&pkg).ok()? {
            let Some(ObjRef::Export(ni)) = find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
            if pkg.export_class(ni) == notify_class {
                let t = find(&ev, "Time").and_then(|p| p.as_float())?;
                return Some(if rate > 0.0 { t / rate } else { t });
            }
        }
        return None;
    }
    None
}

/// Load a locomotion config by (package, object path).
pub fn load_loco_config(content: &Content, r: &(String, String)) -> Result<LocoConfig> {
    let pkg = content.package(&r.0)?;
    let i = find_export(&pkg, &r.1).with_context(|| format!("{} not in {}", r.1, r.0))?;
    let mut c = read_loco_config(&pkg, i)?;
    // Unset arrays come from the class default object (Engine): e.g. FemaleLocomotionConfig
    // keeps the default three speed names.
    if c.speed_names.is_empty() || c.feet.is_empty() {
        if let Ok(e) = content.package("Engine") {
            if let Some(d) = find_export(&e, "Default__ArkComponentLocomotionConfig") {
                let h = e.read_object(d)?;
                if c.speed_names.is_empty() {
                    c.speed_names = names(&e, &h.properties, "m_SpeedNames");
                    if c.table.is_empty() {
                        c.table = ints(&h.properties, "m_SpeedModes");
                    }
                }
                if c.feet.is_empty() {
                    c.feet = structs(&e, &h.properties, "m_Feet").iter().map(|f| f32_of(f, "m_fOnGroundPct")).collect();
                }
            }
        }
    }
    Ok(c)
}

fn find_export(pkg: &Package, path: &str) -> Option<usize> {
    (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path))
}

const ITEM_PREFIX: &[&str] = &["eDisItemPrefix_None", "eDisItemPrefix_Both", "eDisItemPrefix_Primary", "eDisItemPrefix_Secondary", "eDisItemPrefix_Intended_Primary", "eDisItemPrefix_Intended_Secondary"];

fn read_chains(pkg: &Package, props: &[Property]) -> Vec<AnimChain> {
    structs(pkg, props, "m_Animations")
        .iter()
        .map(|c| AnimChain {
            ragdoll_if_pushed: bool_of(c, "m_bGoToRagdollIfPushed"),
            groups: structs(pkg, c, "m_Groups")
                .iter()
                .map(|g| AnimGroup {
                    blend_out: find(g, "m_fBlendOutTime").and_then(|x| x.as_float()).unwrap_or(0.1),
                    end_after_one: bool_of(g, "m_bEndChainAfterOneAnimation"),
                    steps: structs(pkg, g, "m_Chain")
                        .iter()
                        .map(|s| AnimStep {
                            names: names(pkg, s, "m_Name"),
                            looped: bool_of(s, "m_bLoop"),
                            optional: bool_of(s, "m_bOptional"),
                            blend_in: find(s, "m_fBlendInTime").and_then(|x| x.as_float()).unwrap_or(0.2),
                            rate_range: f32_of(s, "m_fPlayRateModifierRange"),
                            item_prefix: enum_index(s, "m_FirstItemPrefix", ITEM_PREFIX),
                            face: Some(name_of(s, "m_FaceAnimName")).filter(|n| !n.is_empty() && n != "None").unwrap_or_default(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

/// The animation definitions of a `DisAnimDefinitions` subclass. The class and its default
/// object are cooked into the level script packages (`DishonoredGameContent.<class>`) and the
/// base class into `Startup`; a default object without `m_Animations` inherits its super
/// class's array (cooked defaults are deltas, arrays are written whole).
pub fn load_anim_defs(content: &Content, packages: &[String], class: &str) -> Result<AnimDefs> {
    let mut pkgs: Vec<Arc<Package>> = packages.iter().filter_map(|p| content.package(p).ok()).collect();
    if let Ok(s) = content.package("Startup") {
        pkgs.push(s);
    }
    let mut cur = class.to_string();
    for _ in 0..8 {
        let cdo = format!("DishonoredGameContent.Default__{cur}");
        let mut super_name = None;
        for pkg in &pkgs {
            let Some(i) = find_export(pkg, &cdo) else { continue };
            let h = pkg.read_object(i)?;
            if find(&h.properties, "m_Animations").is_some() {
                return Ok(AnimDefs { class: class.to_string(), chains: read_chains(pkg, &h.properties) });
            }
            if let Some(ci) = find_export(pkg, &format!("DishonoredGameContent.{cur}")) {
                let s = pkg.exports[ci].super_;
                if s != 0 {
                    super_name = Some(pkg.obj_name(ObjRef::from_index(s)));
                }
            }
            break;
        }
        match super_name {
            Some(s) if s != "DisAnimDefinitions" && s != cur => cur = s,
            _ => break,
        }
    }
    anyhow::bail!("no m_Animations for {class}")
}

/// Sequences an AnimSet holds (names only).
pub fn anim_set_sequences(pkg: &Package, set: usize) -> Vec<String> {
    let Ok(h) = pkg.read_object(set) else { return Vec::new() };
    let Some(seqs) = find(&h.properties, "Sequences").and_then(|p| p.array_objects().ok()) else { return Vec::new() };
    seqs.iter()
        .filter_map(|s| match ObjRef::from_index(*s) {
            ObjRef::Export(e) => pkg.read_object(e).ok().and_then(|h| match find(&h.properties, "SequenceName").map(|p| &p.value) {
                Some(Value::Name(n)) => Some(n.clone()),
                _ => None,
            }),
            _ => None,
        })
        .collect()
}

/// Decode a `PhysicsAsset` (`BodySetup`: `RB_BodySetup` per bone with its `AggGeom` spheres,
/// boxes and capsules; `ConstraintSetup`: `RB_ConstraintSetup` joints with swing / twist limits).
pub fn read_physics_asset(pkg: &Package, export: usize) -> Result<PhysicsAssetData> {
    let h = pkg.read_object(export)?;
    let mut out = PhysicsAssetData { name: pkg.obj_path(ObjRef::Export(export)), ..Default::default() };
    for b in find(&h.properties, "BodySetup").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
        let ObjRef::Export(e) = ObjRef::from_index(b) else { continue };
        let Ok(bh) = pkg.read_object(e) else { continue };
        let p = &bh.properties;
        let mut body = PhysBody { bone: name_of(p, "BoneName"), fixed: bool_of(p, "bFixed"), ..Default::default() };
        if let Some(Value::Struct(_, agg)) = find(p, "AggGeom").map(|x| &x.value) {
            for s in structs(pkg, agg, "SphereElems") {
                body.radius = body.radius.max(f32_of(&s, "Radius"));
            }
            for s in structs(pkg, agg, "SphylElems") {
                let r = f32_of(&s, "Radius");
                if r >= body.radius {
                    body.radius = r;
                    body.length = f32_of(&s, "Length");
                    if let Some(c) = matrix_origin(find(&s, "TM").map(|x| &x.value)) {
                        body.center = c;
                    }
                }
            }
            for s in structs(pkg, agg, "BoxElems") {
                let half = [f32_of(&s, "X"), f32_of(&s, "Y"), f32_of(&s, "Z")].iter().fold(f32::MAX, |a, b| a.min(*b)) * 0.5;
                if half.is_finite() && half > body.radius {
                    body.radius = half;
                }
            }
        }
        if !body.bone.is_empty() && body.bone != "None" {
            out.bodies.push(body);
        }
    }
    for c in find(&h.properties, "ConstraintSetup").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
        let ObjRef::Export(e) = ObjRef::from_index(c) else { continue };
        let Ok(ch) = pkg.read_object(e) else { continue };
        let p = &ch.properties;
        out.constraints.push(PhysConstraint {
            bone1: name_of(p, "ConstraintBone1"),
            bone2: name_of(p, "ConstraintBone2"),
            swing_limited: bool_of(p, "bSwingLimited"),
            twist_limited: bool_of(p, "bTwistLimited"),
            swing1: f32_of(p, "Swing1LimitAngle"),
            swing2: f32_of(p, "Swing2LimitAngle"),
            twist: f32_of(p, "TwistLimitAngle"),
        });
    }
    Ok(out)
}

/// Translation row of a cooked `Matrix` struct (`WPlane` row, or 64 raw bytes). The raw bytes
/// hold each plane as (W, X, Y, Z) (`matrix_rows`), so the W plane's x, y, z are floats 13..15
/// (Skm_EliteGuard_Physics `spine_3_jnt`: the capsule centre (7.25, 0, 0)).
fn matrix_origin(v: Option<&Value>) -> Option<[f32; 3]> {
    matrix_rows(v).map(|r| r[3])
}

/// Float properties of the NPC locomotion node: the class defaults (`Engine`,
/// `Default__ArkAnimNodeLocomotion`) overridden by `NPC_Human_at`'s `LocoBlender` (Startup).
pub fn loco_node_floats(content: &Content, names: &[&str]) -> Vec<Option<f32>> {
    let mut out = vec![None; names.len()];
    for (pkg, path) in [("Engine", "Default__ArkAnimNodeLocomotion"), ("Startup", "Npc_Common_at.NPC_Human_at.ArkAnimNodeLocomotion_0")] {
        let Ok(p) = content.package(pkg) else { continue };
        let Some(i) = find_export(&p, path) else { continue };
        let Ok(h) = p.read_object(i) else { continue };
        for (k, n) in names.iter().enumerate() {
            if let Some(v) = find(&h.properties, n).and_then(|x| x.as_float()) {
                out[k] = Some(v);
            }
        }
    }
    out
}

/// Decode an `ArkComponentLookatConfig` (look-at modes; blinking falls back to the class
/// defaults of `Engine.Default__ArkComponentLookatConfig`).
pub fn load_lookat_config(content: &Content, r: &(String, String)) -> Result<LookatConfig> {
    let pkg = content.package(&r.0)?;
    let i = find_export(&pkg, &r.1).with_context(|| format!("{} not in {}", r.1, r.0))?;
    let h = pkg.read_object(i)?;
    let defaults = content.package("Engine").ok().and_then(|e| find_export(&e, "Default__ArkComponentLookatConfig").and_then(|d| e.read_object(d).ok()).map(|h| h.properties)).unwrap_or_default();
    let pick = |n: &str, d: f32| find(&h.properties, n).or_else(|| find(&defaults, n)).and_then(|x| x.as_float()).unwrap_or(d);
    let blink = match find(&h.properties, "BlinkEnabled").or_else(|| find(&defaults, "BlinkEnabled")).map(|x| &x.value) {
        Some(Value::Bool(b)) => *b,
        _ => false,
    };
    const GRID: [&str; 9] = ["LU", "LC", "LD", "CU", "CC", "CD", "RU", "RC", "RD"];
    let modes = structs(&pkg, &h.properties, "m_LookAtModes")
        .iter()
        .map(|m| LookatMode {
            name: name_of(m, "ModeName"),
            head_range: [f32_of(m, "m_HeadRightRange"), f32_of(m, "m_HeadLeftRange"), f32_of(m, "m_HeadUpRange"), f32_of(m, "m_HeadDownRange")],
            torso_range: [f32_of(m, "m_TorsoRightRange"), f32_of(m, "m_TorsoLeftRange"), f32_of(m, "m_TorsoUpRange"), f32_of(m, "m_TorsoDownRange")],
            head_horizontal_pct: f32_of(m, "m_HeadAndTorso_HorizontalPercentage"),
            head_vertical_pct: f32_of(m, "m_HeadAndTorso_VerticalPercentage"),
            start_torso: f32_of(m, "Start_Torso"),
            end_head: f32_of(m, "End_Head"),
            clamp: matches!(find(m, "m_bDoClampRange").map(|x| &x.value), Some(Value::Bool(true))).then(|| [f32_of(m, "m_ClampRightRange"), f32_of(m, "m_ClampLeftRange"), f32_of(m, "m_ClampUpRange"), f32_of(m, "m_ClampDownRange")]),
            head: GRID.iter().map(|g| name_of(m, &format!("Head_AnimName_{g}"))).map(|n| if n == "None" { String::new() } else { n }).collect(),
            torso: GRID.iter().map(|g| name_of(m, &format!("Torso_AnimName_{g}"))).map(|n| if n == "None" { String::new() } else { n }).collect(),
        })
        .collect();
    Ok(LookatConfig { modes, blink, blink_interval: [pick("MinDurationBetweenBlink", 0.5), pick("MaxDurationBetweenBlink", 6.0)], blink_rate: [pick("MinBlinkSpeed", 1.0), pick("MaxBlinkSpeed", 2.0)], torso_influence_max_move_speed: pick("m_fTorsoInfluenceMaxMoveSpeed", 200.0) })
}

/// Rows (X, Y, Z, W planes) of a cooked `Matrix` struct (`WPlane` struct rows, or 64 raw bytes).
fn matrix_rows(v: Option<&Value>) -> Option<[[f32; 3]; 4]> {
    match v? {
        Value::Struct(_, rows) => {
            let row = |n: &str| match find(rows, n).map(|x| &x.value) {
                Some(Value::Struct(_, c)) => Some([f32_of(c, "X"), f32_of(c, "Y"), f32_of(c, "Z")]),
                _ => None,
            };
            Some([row("XPlane")?, row("YPlane")?, row("ZPlane")?, row("WPlane")?])
        }
        // The cooked bytes hold each plane as (W, X, Y, Z): the W plane of an element reads
        // (1, x, y, z) and its rotation rows (0, ...) (checked on Skm_EliteGuard_Physics: the
        // leg capsules come out along the leg, the pelvis box at the pelvis).
        Value::Raw(_, b) if b.len() >= 64 => {
            let f = |i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap_or([0; 4]));
            Some([[f(1), f(2), f(3)], [f(5), f(6), f(7)], [f(9), f(10), f(11)], [f(13), f(14), f(15)]])
        }
        _ => None,
    }
}

/// The physics bodies of a rig in its bind pose, in the pawn's frame: every `KSphylElem`,
/// `KSphereElem` and `KBoxElem` of the physics asset's `RB_BodySetup`s, placed by its `TM`
/// in the bone's space and by the skeletal mesh's bind pose and mesh transform
/// (`Origin` / `RotOrigin`). A capsule lies along its element's Z axis (`KSphylElem`).
pub fn load_hit_bodies(content: &Content, package: &str, physics: &str, mesh: &str) -> Result<dishonored_ir::items::HitBodyConfig> {
    use dishonored_ir::items::{HitBody, HitBodyConfig, HitRegion};
    use glam::{Mat4, Vec3};
    let pkg = content.package(package)?;
    let mi = find_export(&pkg, mesh).with_context(|| format!("{mesh} not in {package}"))?;
    let m = crate::skeletal_mesh::read_skeletal_mesh(&pkg, mi)?;
    let bones: Vec<dishonored_ir::BoneAsset> = m.bones.iter().map(|b| dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
    let g = crate::characters::bind_globals(&bones);
    let m2p = crate::util::rotator_matrix(m.rot_origin[0], m.rot_origin[1], m.rot_origin[2]) * Mat4::from_translation(m.mesh_origin);
    let pi = find_export(&pkg, physics).with_context(|| format!("{physics} not in {package}"))?;
    let h = pkg.read_object(pi)?;
    let mut out = HitBodyConfig { source: format!("{package}:{physics}"), half_height: 0.0, bodies: Vec::new() };
    // The rig's origin sits `half_height` above its feet: the root bone's height below it.
    if let Some(root) = g.first() {
        out.half_height = -(m2p * *root).w_axis.z;
    }
    for b in find(&h.properties, "BodySetup").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
        let ObjRef::Export(e) = ObjRef::from_index(b) else { continue };
        let Ok(bh) = pkg.read_object(e) else { continue };
        let p = &bh.properties;
        let bone = name_of(p, "BoneName");
        let Some(bi) = bones.iter().position(|x| x.name.eq_ignore_ascii_case(&bone)) else { continue };
        let to_pawn = m2p * g[bi];
        let Some(Value::Struct(_, agg)) = find(p, "AggGeom").map(|x| &x.value) else { continue };
        let mut push = |s: &[Property], half: [f32; 3], capsule: bool| {
            let rows = matrix_rows(find(s, "TM").map(|x| &x.value)).unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]]);
            let axis = |r: [f32; 3]| to_pawn.transform_vector3(Vec3::from(r)).normalize_or_zero().to_array();
            out.bodies.push(HitBody {
                bone: bone.clone(),
                region: HitRegion::of_bone(&bone),
                center: to_pawn.transform_point3(Vec3::from(rows[3])).to_array(),
                axes: [axis(rows[0]), axis(rows[1]), axis(rows[2])],
                half,
                capsule,
            });
        };
        for s in structs(&pkg, agg, "SphylElems") {
            let r = f32_of(&s, "Radius");
            push(&s, [r, r, f32_of(&s, "Length") * 0.5], true);
        }
        for s in structs(&pkg, agg, "SphereElems") {
            let r = f32_of(&s, "Radius");
            push(&s, [r, r, 0.0], true);
        }
        for s in structs(&pkg, agg, "BoxElems") {
            push(&s, [f32_of(&s, "X") * 0.5, f32_of(&s, "Y") * 0.5, f32_of(&s, "Z") * 0.5], false);
        }
    }
    anyhow::ensure!(!out.bodies.is_empty(), "no bodies in {physics}");
    Ok(out)
}

/// Load a physics asset by (package, object path).
pub fn load_physics_asset(content: &Content, r: &(String, String)) -> Result<PhysicsAssetData> {
    let pkg = content.package(&r.0)?;
    let i = find_export(&pkg, &r.1).with_context(|| format!("{} not in {}", r.1, r.0))?;
    read_physics_asset(&pkg, i)
}

/// The skeletal mesh an AnimSet was authored on (`PreviewSkelMeshName`), when cooked.
pub fn anim_set_skeleton(pkg: &Package, set: usize) -> Option<String> {
    let h = pkg.read_object(set).ok()?;
    match find(&h.properties, "PreviewSkelMeshName").map(|p| &p.value) {
        Some(Value::Name(n)) if n != "None" => Some(n.clone()),
        _ => None,
    }
}
