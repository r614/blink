//! Asset decode regression tests against the canonical GOG build. Skipped without the install.

use dishonored_assets::{Content, ImportOptions, import_level, map_packages};
use dishonored_assets::static_mesh::read_static_mesh;
use dishonored_assets::util::{half_to_f32, packed_normal, rotator_matrix};
use glam::Vec3;

fn content() -> Option<Content> {
    Content::locate().ok()
}

#[test]
fn half_and_packed_normal() {
    assert_eq!(half_to_f32(0x3C00), 1.0);
    assert_eq!(half_to_f32(0xC000), -2.0);
    assert_eq!(half_to_f32(0x3800), 0.5);
    let n = packed_normal(0x00FF_8080);
    assert!((n - Vec3::new(0.00392, 0.00392, 1.0)).length() < 0.01);
}

#[test]
fn rotator_yaw_quarter_turn() {
    // Yaw 16384 = 90 degrees: +X maps to +Y in Unreal's Z-up frame.
    let m = rotator_matrix(0, 16384, 0);
    let v = m.transform_vector3(Vec3::X);
    assert!((v - Vec3::Y).length() < 1e-5, "{v}");
}

#[test]
fn air_duct_static_mesh() {
    let Some(c) = content() else { return };
    let p = c.package("L_Boyle_Ext_Env").unwrap();
    let m = read_static_mesh(&p, 4412).unwrap();
    assert_eq!(m.positions.len(), 62);
    assert_eq!(m.indices.len(), 150);
    assert_eq!(m.kdop_triangles.len(), 50);
    assert_eq!(m.sections.len(), 1);
    assert_eq!(m.sections[0].num_triangles, 50);
    assert_eq!(m.uvs.len(), 2);
    assert!((m.bounds_origin.x - 200.146).abs() < 0.01);
}

#[test]
fn boyle_ext_import() {
    let Some(c) = content() else { return };
    let pkgs = map_packages(&c, "L_Boyle_Ext_P");
    // Persistent level + the 7 WorldInfo.StreamingLevels (incl. L_BoyleStreet_Ext_Script).
    assert_eq!(pkgs.len(), 8);
    let opts = ImportOptions { load_textures: false, ..Default::default() };
    let (map, stats) = import_level(&c, "L_Boyle_Ext_P", &pkgs, &opts).unwrap();
    assert_eq!(stats.mesh_failures, 0);
    // Includes 33 spline-deformed mesh copies and meshes resolved through DishonoredGame.upk.
    assert_eq!(map.meshes.len(), 511);
    assert_eq!(map.instances.len(), 3754);
    assert_eq!(map.spawn_candidates.len(), 5);
}
