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

/// `EDisDoorState` from its enum name (`DDS_Closed`, `DDS_Opened_CW`, `DDS_Opened_CCW`).
pub fn door_state(name: &str) -> u8 {
    match name.trim() {
        "DDS_Opened_CW" => 1,
        "DDS_Opened_CCW" => 2,
        _ => 0,
    }
}

fn name_value(props: &[Property], name: &str) -> Option<String> {
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
fn defining_package(content: &Content, pkg: &Package, r: ObjRef, packages: &[String]) -> Option<(String, String)> {
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
    let tweaks = TweakDb::load_with(content, packages).ok();
    let mut out = Vec::new();
    for name in packages.iter().filter(|p| content.has_package(p)) {
        let Ok(pkg) = content.package(name) else { continue };
        for i in 0..pkg.exports.len() {
            let class = pkg.export_class(i);
            if !is_usable_class(&class) || pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            let Ok(h) = pkg.read_object(i) else { continue };
            let props = h.properties;
            let Some(ObjRef::Export(ci)) = find(&props, "SkeletalMeshComponent").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
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
            out.push(SkeletalProp {
                actor: format!("{}:{}", pkg.name, pkg.obj_name(ObjRef::Export(i))),
                class,
                mesh,
                transform: world * crate::level::component_transform(&c),
                actor_to_world: Mat4::from_translation(loc) * rot,
                hidden: bool_or(&props, "bHidden", false) || bool_or(&c, "HiddenGame", false),
                anim_sets,
                door,
            });
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
