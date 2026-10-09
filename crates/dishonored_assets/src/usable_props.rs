//! Placed doors and usable objects of a map, drawn with their skeletal meshes.
//!
//! `DisDoor` and `DishonoredUsableObject` (with its subclasses `DisWaterSource`, `DisAlarmBell`,
//! `DisAudioLogPlayer`, `DishonoredDynamicUsableObject`) are `DisSkeletalBreakable`s: their
//! visible part is the `SkeletalMeshComponent` (`SkeletalMeshComponent0`), animated through the
//! usable's stages. Their `StaticUsablePart` component carries no mesh on Boyle. The level
//! importer only reads `StaticMeshComponent`s, so the transition door the `street` viewpoint
//! looks at (`DishonoredUsableObject_7`, `double_doors.DBdoor_big_rich_ext_metal_sk`) and the
//! eight `DisDoor`s of `L_Boyle_Ext_Audio` were not drawn at all.
//!
//! A door's stage animations come from its `DisTweaks_Door` (`m_pDoorTweaks`): stage
//! `m_DoorOpenCW` .. `m_DoorCloseCCW`, each with a `m_TransitionAnimName` in the component's
//! `AnimSets` (`Doors.Doors_as`, `double_doors.double_doors_as`).

use crate::content::Content;
use crate::tweaks::{TVal, TweakDb, get};
use dishonored_ir::{DoorAnims, SkeletalProp};
use glam::{Mat4, Vec3};
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

/// Actor classes whose skeletal mesh component is their visible part (`DisSkeletalBreakable`
/// usables).
pub fn is_usable_class(class: &str) -> bool {
    matches!(class, "DisDoor" | "DishonoredUsableObject" | "DishonoredDynamicUsableObject" | "DisWaterSource" | "DisAlarmBell" | "DisAudioLogPlayer")
}

/// Placed creatures drawn as animated skeletal props: `DisRiverKrust`. Its skeletal component
/// is a native member (`m_pSkeletalMeshCpnt`) that only `CollisionComponent` names; its tree
/// (`Npc_RiverKrust_at`: a `FullBodySlot` over an unnamed sequence node) plays the tweak's
/// `m_AnimSequences[ERiverKrustAnimState]`, the ambient loop at rest (`ClamUp_Idle`).
pub fn is_animated_prop_class(class: &str) -> bool {
    class == "DisRiverKrust"
}

/// The loop a placed skeletal component plays on its own: its `Animations` node is an
/// `AnimNodeSequence` with `bPlaying` and `bLooping` (Boyle Int's pendulum clock,
/// `Clock_TickAnim`), at its `Rate` from its `CurrentTime`. A `SkeletalMeshActor` doing so is
/// drawn as an animated prop rather than as scenery in its reference pose.
pub fn playing_loop(pkg: &Package, component: usize) -> Option<dishonored_ir::PropIdle> {
    let c = pkg.read_object(component).ok()?.properties;
    let ObjRef::Export(n) = find(&c, "Animations").and_then(|p| p.as_object()).map(ObjRef::from_index)? else { return None };
    if pkg.export_class(n) != "AnimNodeSequence" {
        return None;
    }
    let node = pkg.read_object(n).ok()?.properties;
    let anim = match find(&node, "AnimSeqName").map(|p| &p.value) {
        Some(Value::Name(s)) if s != "None" => s.clone(),
        _ => return None,
    };
    if !(bool_or(&node, "bPlaying", false) && bool_or(&node, "bLooping", false)) {
        return None;
    }
    let f = |k: &str, d: f32| find(&node, k).and_then(|p| p.as_float()).unwrap_or(d);
    let (rate, start) = (f("Rate", 1.0), f("CurrentTime", 0.0));
    Some(dishonored_ir::PropIdle { anim, rate: [rate, rate], start: [start, start] })
}

/// Exports a package's Kismet names (`SeqVar_Object.ObjValue`): those actors belong to the
/// mission's scripts (matinee props), not to the static scenery.
fn kismet_objects(pkg: &Package) -> std::collections::HashSet<usize> {
    let mut out = std::collections::HashSet::new();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) == "SeqVar_Object" {
            if let Some(ObjRef::Export(o)) = pkg.read_object(i).ok().and_then(|h| find(&h.properties, "ObjValue").and_then(|p| p.as_object())).map(ObjRef::from_index) {
                out.insert(o);
            }
        }
    }
    out
}

/// The resting loop of a river krust tweak: `m_AnimSequences[RiverKrustAnimState_Ambient]`
/// (element 0) with its `m_PlaybackRateRange` and `m_StartTimeSecondsRange`.
fn krust_idle(t: &crate::tweaks::TMap) -> Option<dishonored_ir::PropIdle> {
    let anim = match get(t, "m_AnimSequences.m_SequenceName") {
        Some(TVal::Name(n)) if n != "None" => n.clone(),
        _ => return None,
    };
    let f = |p: &str, d: f32| crate::tweaks::get_f32(t, &format!("m_AnimSequences.{p}")).unwrap_or(d);
    Some(dishonored_ir::PropIdle {
        anim,
        rate: [f("m_PlaybackRateRange.m_fMinValue", 1.0), f("m_PlaybackRateRange.m_fMaxValue", 1.0)],
        start: [f("m_StartTimeSecondsRange.m_fMinValue", 0.0), f("m_StartTimeSecondsRange.m_fMaxValue", 0.0)],
    })
}

/// `EDisDoorState` from its enum name (`DDS_Closed`, `DDS_Opened_CW`, `DDS_Opened_CCW`).
pub fn door_state(name: &str) -> u8 {
    match name.trim() {
        "DDS_Opened_CW" => 1,
        "DDS_Opened_CCW" => 2,
        _ => 0,
    }
}

pub(crate) fn name_value(props: &[Property], name: &str) -> Option<String> {
    match find(props, name).map(|p| &p.value) {
        Some(Value::Enum(s)) | Some(Value::Name(s)) => Some(s.clone()),
        Some(Value::Byte(b)) => Some(["DDS_Closed", "DDS_Opened_CW", "DDS_Opened_CCW"].get(*b as usize).unwrap_or(&"DDS_Closed").to_string()),
        _ => None,
    }
}

fn bool_or(props: &[Property], name: &str, d: bool) -> bool {
    match find(props, name).map(|p| &p.value) {
        Some(Value::Bool(b)) => *b,
        _ => d,
    }
}

/// The package that defines the object `r` of `pkg`: `pkg` for an export, else the first of
/// `packages` exporting an object of the same path (cooked levels carry their own copies).
pub(crate) fn defining_package(content: &Content, pkg: &Package, r: ObjRef, packages: &[String]) -> Option<(String, String)> {
    let path = pkg.obj_path(r);
    match r {
        ObjRef::Export(_) => Some((pkg.name.clone(), path)),
        ObjRef::Import(_) => {
            for p in packages {
                let Ok(q) = content.package(p) else { continue };
                if (0..q.exports.len()).any(|i| q.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&path)) {
                    return Some((q.name.clone(), path));
                }
            }
            let top = path.split('.').next()?.to_string();
            content.has_package(&top).then_some((top, path))
        }
        ObjRef::Null => None,
    }
}

/// Stage transition animation of a door tweak (`m_DoorOpenCW.m_TransitionAnimName`, ...).
fn stage_anim(t: &crate::tweaks::TMap, stage: &str) -> String {
    match get(t, &format!("{stage}.m_TransitionAnimName")) {
        Some(TVal::Name(n)) if n != "None" => n.clone(),
        _ => String::new(),
    }
}

/// Read the doors and usable objects of `packages` (persistent level and sublevels).
pub fn read_props(content: &Content, packages: &[String]) -> Vec<SkeletalProp> {
    read_props_with_channels(content, packages).into_iter().map(|p| p.0).collect()
}

/// [`read_props`] with each skeletal mesh component's `LightingChannels` bits (the owner channels
/// of its light environment; `PrimitiveComponent` default bInitialized + Dynamic).
pub fn read_props_with_channels(content: &Content, packages: &[String]) -> Vec<(SkeletalProp, u32)> {
    let tweaks = TweakDb::load_with(content, packages).ok();
    let mut out = Vec::new();
    for name in packages.iter().filter(|p| content.has_package(p)) {
        let Ok(pkg) = content.package(name) else { continue };
        let mut scripted: Option<std::collections::HashSet<usize>> = None;
        for i in 0..pkg.exports.len() {
            let class = pkg.export_class(i);
            let animated = is_animated_prop_class(&class);
            let scenery = class == "SkeletalMeshActor";
            if !(is_usable_class(&class) || animated || scenery) || pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            let Ok(h) = pkg.read_object(i) else { continue };
            let props = h.properties;
            let comp = if animated { "CollisionComponent" } else { "SkeletalMeshComponent" };
            let Some(ObjRef::Export(ci)) = find(&props, comp).and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
            if animated && !pkg.export_class(ci).contains("SkeletalMeshComponent") {
                continue;
            }
            // Scenery skeletal meshes are props only when they loop an animation on their own
            // and no Kismet names them.
            let looping = if scenery { playing_loop(&pkg, ci) } else { None };
            if scenery && (looping.is_none() || scripted.get_or_insert_with(|| kismet_objects(&pkg)).contains(&i)) {
                continue;
            }
            let Ok(ch) = pkg.read_object(ci) else { continue };
            let c = ch.properties;
            let Some(mref) = find(&c, "SkeletalMesh").and_then(|p| p.as_object()).filter(|o| *o != 0) else { continue };
            let Some(mesh) = defining_package(content, &pkg, ObjRef::from_index(mref), packages) else { continue };
            let anim_sets = find(&c, "AnimSets")
                .and_then(|p| p.array_objects().ok())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|o| defining_package(content, &pkg, ObjRef::from_index(o), packages))
                .collect();
            let world = crate::level::actor_transform(&props);
            let loc = find(&props, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
            let rot = match find(&props, "Rotation").and_then(|p| p.as_rotator()) {
                Some([p, y, r]) => crate::util::rotator_matrix(p, y, r),
                None => Mat4::IDENTITY,
            };
            let door = (class == "DisDoor").then(|| {
                let t = tweaks.as_ref().and_then(|db| {
                    let path = find(&props, "m_pDoorTweaks").and_then(|p| p.as_object()).filter(|o| *o != 0).map(|o| pkg.obj_path(ObjRef::from_index(o)))?;
                    db.try_resolve(&path)
                });
                let t = t.unwrap_or_default();
                DoorAnims {
                    initial: name_value(&props, "m_InitialDoorState").map(|n| door_state(&n)).unwrap_or(0),
                    open_cw: stage_anim(&t, "m_DoorOpenCW"),
                    open_ccw: stage_anim(&t, "m_DoorOpenCCW"),
                    close_cw: stage_anim(&t, "m_DoorCloseCW"),
                    close_ccw: stage_anim(&t, "m_DoorCloseCCW"),
                }
            });
            let idle = if animated {
                tweaks.as_ref().and_then(|db| {
                    let path = find(&props, "m_pRiverKrustTweaks").and_then(|p| p.as_object()).filter(|o| *o != 0).map(|o| pkg.obj_path(ObjRef::from_index(o)))?;
                    krust_idle(&db.try_resolve(&path)?)
                })
            } else {
                looping
            };
            let channels = crate::light_env::channel_bits(&c, 1 | 1 << 3);
            out.push((SkeletalProp {
                actor: format!("{}:{}", pkg.name, pkg.obj_name(ObjRef::Export(i))),
                class,
                mesh,
                transform: world * crate::level::component_transform(&c),
                actor_to_world: Mat4::from_translation(loc) * rot,
                hidden: bool_or(&props, "bHidden", false) || bool_or(&c, "HiddenGame", false),
                anim_sets,
                door,
                light_env: None,
                idle,
            }, channels));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn door_states_follow_the_enum() {
        assert_eq!(door_state("DDS_Closed"), 0);
        assert_eq!(door_state("DDS_Opened_CW"), 1);
        assert_eq!(door_state("DDS_Opened_CCW"), 2);
        assert_eq!(door_state("garbage"), 0);
    }

    #[test]
    fn usable_classes_are_the_skeletal_breakables() {
        for c in ["DisDoor", "DishonoredUsableObject", "DisWaterSource", "DisAlarmBell"] {
            assert!(is_usable_class(c), "{c}");
        }
        // Pawns, matinee props and static movers are drawn elsewhere.
        for c in ["SkeletalMeshActor", "InterpActor", "KActor", "DisFish", "DishonoredMovable"] {
            assert!(!is_usable_class(c), "{c}");
        }
    }
}
