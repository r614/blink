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
    let mut c = LocoConfig { name: pkg.obj_path(ObjRef::Export(export)), speed_names: names(pkg, p, "m_SpeedNames"), table: ints(p, "m_SpeedModes"), ref_mesh: name_of(p, "m_SkeletalRefMeshName"), ..Default::default() };
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
        });
    }
    for t in structs(pkg, p, "m_Transitions") {
        let i = |n: &str| match find(&t, n).map(|x| &x.value) {
            Some(Value::Int(v)) => *v,
            _ => 0,
        };
        c.transitions.push((name_of(&t, "m_AnimName"), i("m_StartModifier"), i("m_EndModifier")));
    }
    Ok(c)
}

/// Load a locomotion config by (package, object path).
pub fn load_loco_config(content: &Content, r: &(String, String)) -> Result<LocoConfig> {
    let pkg = content.package(&r.0)?;
    let i = find_export(&pkg, &r.1).with_context(|| format!("{} not in {}", r.1, r.0))?;
    let mut c = read_loco_config(&pkg, i)?;
    // Unset arrays come from the class default object (Engine): e.g. FemaleLocomotionConfig
    // keeps the default three speed names.
    if c.speed_names.is_empty() {
        if let Ok(e) = content.package("Engine") {
            if let Some(d) = find_export(&e, "Default__ArkComponentLocomotionConfig") {
                let h = e.read_object(d)?;
                c.speed_names = names(&e, &h.properties, "m_SpeedNames");
                if c.table.is_empty() {
                    c.table = ints(&h.properties, "m_SpeedModes");
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

/// The skeletal mesh an AnimSet was authored on (`PreviewSkelMeshName`), when cooked.
pub fn anim_set_skeleton(pkg: &Package, set: usize) -> Option<String> {
    let h = pkg.read_object(set).ok()?;
    match find(&h.properties, "PreviewSkelMeshName").map(|p| &p.value) {
        Some(Value::Name(n)) if n != "None" => Some(n.clone()),
        _ => None,
    }
}
