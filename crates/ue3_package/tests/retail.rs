//! Regression tests against the canonical GOG build (Dishonored.exe SHA-256 70da9613…c6fb).
//! Skipped automatically when the install is not present.

use std::path::PathBuf;
use ue3_package::{ObjRef, Package};

fn cooked() -> Option<PathBuf> {
    let root = std::env::var_os("DISHONORED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored"));
    let c = root.join("DishonoredGame").join("CookedPCConsole");
    c.is_dir().then_some(c)
}

macro_rules! require_install {
    () => {
        match cooked() {
            Some(c) => c,
            None => {
                eprintln!("Dishonored install not found; skipping");
                return;
            }
        }
    };
}

#[test]
fn boyle_env_tables() {
    let c = require_install!();
    let p = Package::open(c.join("L_Boyle_Ext_Env.upk")).unwrap();
    assert_eq!(p.summary.file_version, 801);
    assert_eq!(p.summary.licensee_version, 30);
    assert_eq!(p.summary.engine_version, 9098);
    assert_eq!(p.summary.cooker_version, 133);
    assert_eq!(p.summary.compression_flags, 2);
    assert_eq!(p.names.len(), 3685);
    assert_eq!(p.imports.len(), 372);
    assert_eq!(p.exports.len(), 9352);
    assert_eq!(p.obj_path(ObjRef::Export(4412)), "air_duct_01.air_duct_09");
    assert_eq!(p.export_class(4412), "StaticMesh");
    assert_eq!(
        p.obj_path(ObjRef::Export(4848)),
        "TheWorld.PersistentLevel.StaticMeshCollectionActor"
    );
}

#[test]
fn streets1_tables() {
    let c = require_install!();
    let p = Package::open(c.join("L_Streets1_P.upk")).unwrap();
    assert_eq!(p.names.len(), 5045);
    assert_eq!(p.imports.len(), 785);
    assert_eq!(p.exports.len(), 4911);
    assert_eq!(p.import_package(0), "AkAudio");
}

#[test]
fn collection_actor_properties() {
    let c = require_install!();
    let p = Package::open(c.join("L_Boyle_Ext_Env.upk")).unwrap();
    let h = p.read_object(4848).unwrap();
    let comps = ue3_package::properties::find(&h.properties, "StaticMeshComponents").unwrap();
    assert_eq!(comps.array_objects().unwrap().len(), 100);
    // 100 FMatrix entries follow the properties.
    assert_eq!(p.exports[4848].serial_size as usize - h.native_offset, 100 * 64);
}

#[test]
fn state_frame_actor() {
    // StaticMeshActor_1737 has RF_HasStack (flags stored high dword first).
    let c = require_install!();
    let p = Package::open(c.join("L_Boyle_Ext_Env.upk")).unwrap();
    assert_ne!(p.exports[4845].object_flags & ue3_package::properties::RF_HAS_STACK, 0);
    let h = p.read_object(4845).unwrap();
    assert!(ue3_package::properties::find(&h.properties, "StaticMeshComponent").is_some());
}

#[test]
fn texture_properties() {
    let c = require_install!();
    let p = Package::open(c.join("L_Boyle_Ext_Env.upk")).unwrap();
    let h = p.read_object(8478).unwrap();
    let get = |n| ue3_package::properties::find(&h.properties, n).unwrap().value.clone();
    assert!(matches!(get("SizeX"), ue3_package::properties::Value::Int(512)));
    assert!(matches!(get("SizeY"), ue3_package::properties::Value::Int(1024)));
    assert!(matches!(get("Format"), ue3_package::properties::Value::Enum(ref s) if s == "PF_DXT1"));
}
