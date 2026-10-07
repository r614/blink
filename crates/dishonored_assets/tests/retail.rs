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
    // ... plus 20 skeletal meshes drawn in their reference pose (42 placed doors, usable
    // objects, sinks, alarm bells, the climbable chain, the sewer gate and the bridge).
    assert_eq!(map.meshes.len(), 519);
    // Components whose static mesh comes from their archetype (collision audit): 5 meshes and
    // 70 instances more.
    assert_eq!(map.instances.len(), 3827);
    assert_eq!(stats.skeletal_props, 3);
    assert_eq!(map.spawn_candidates.len(), 5);
    // U-580: the sky dome (no LightMapCoordinateIndex, so its lightmap set is 0) keeps its
    // texture coordinate 1, where `Boyle_2_inst` samples the `Storm` texture (the glow and the
    // lit clouds around the moon).
    let dome = map.meshes.iter().find(|m| m.source.ends_with("SkyDome_Multi_UV")).expect("sky dome");
    assert_eq!(dome.uv_set1.len(), dome.positions.len());
    assert!(dome.uv_set1.iter().zip(&dome.uv0).any(|(a, b)| a.distance(*b) > 0.01));
}

#[test]
fn boyle_navmesh_decodes_exactly() {
    use dishonored_assets::navigation::read_package_pylons;
    use dishonored_ir::nav::{NavEdgeClass, NavEdgeType};
    let Some(c) = content() else { return };
    let p = c.package("L_Boyle_Ext_Nav").unwrap();
    let pylons = read_package_pylons(&p).unwrap();
    assert_eq!(pylons.len(), 1);
    let m = &pylons[0].mesh;
    assert_eq!(m.version, (28, 28));
    assert_eq!((m.verts.len(), m.polys.len(), m.edges.len()), (2120, 984, 1345));
    let count = |cl: NavEdgeClass, k: NavEdgeType| m.edges.iter().filter(|e| e.class == cl && e.kind == k).count();
    assert_eq!(count(NavEdgeClass::Base, NavEdgeType::Normal), 1198);
    assert_eq!(count(NavEdgeClass::Base, NavEdgeType::Removed), 53);
    assert_eq!(count(NavEdgeClass::OneWay, NavEdgeType::Normal), 23);
    assert_eq!(count(NavEdgeClass::PathObject, NavEdgeType::PathObject), 71);
    assert_eq!(pylons[0].obstacle.as_ref().map(|o| o.polys.len()), Some(3612));
}

#[test]
fn ai_config_from_tweaks_and_ini() {
    let Some(c) = content() else { return };
    let ai = dishonored_assets::ai_config::load_ai_config(&c, &[]);
    let d = dishonored_ir::ai::AiConfig::default();
    // The loader reads the same values the fallback documents.
    assert_eq!(ai.vision, d.vision);
    assert_eq!(ai.guard_unsuspecting, d.guard_unsuspecting);
    assert_eq!(ai.guard_suspecting, d.guard_suspecting);
    assert_eq!(ai.neutral, d.neutral);
    assert_eq!(ai.loudness_distances, d.loudness_distances);
    assert_eq!(ai.reaction_delay, d.reaction_delay);
    assert_eq!(ai.search, d.search);
    assert_eq!(ai.panic, d.panic);
    assert_eq!(ai.combat, d.combat);
    assert!(ai.provenance.get("ai.vision").is_some_and(|s| s.contains("Twk_Vision_Default")));
    // Rat swarms: the random swarm and Devouring Swarm level 1 tweaks.
    assert_eq!((ai.rats.min_rat_count_per_attack_group, ai.rats.pawn_detection_radius, ai.rats.damage_per_bite), (12, 500.0, 0.4));
    assert_eq!((ai.devouring_swarm.swarm_type, ai.devouring_swarm.min_rat_count_per_attack_group, ai.devouring_swarm.pawn_detection_radius), (3, 10, 700.0));
}

/// Boyle Int's audio cells: portals join the package's cells, the cells link back to their
/// portals, the path table is sorted, and its size matches the propagation info.
#[test]
fn boyle_int_audio_cells() {
    let Some(c) = content() else { return };
    let pkgs = map_packages(&c, "L_Boyle_Int_P");
    let lvl = dishonored_assets::ai_level::extract_ai_level(&c, &pkgs, &[], &[]).unwrap();
    let a = &lvl.audio;
    let prop = a.propagation.as_ref().expect("propagation info");
    assert_eq!(prop.keys.len(), 2704);
    assert!(prop.keys.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(prop.portals.len(), 55);
    // Every path record lies inside the data and names known portals.
    for &v in &prop.values {
        let (off, n) = ((v >> 8) as usize, (v & 0xff) as usize);
        assert!(off + 2 + n <= prop.data.len());
        assert!(prop.data[off + 2..off + 2 + n].iter().all(|&k| (k as usize) < prop.portals.len() && prop.portals[k as usize] < a.portals.len()));
    }
    assert!(a.cells.len() >= 41 && a.portals.len() >= 55, "{} cells {} portals", a.cells.len(), a.portals.len());
    let mut joined = 0;
    for (i, p) in a.portals.iter().enumerate() {
        let (Some(ca), Some(cb)) = (p.cell_a, p.cell_b) else { continue };
        joined += 1;
        assert!(a.cells[ca].portals.contains(&i) || a.cells[cb].portals.contains(&i), "{}", p.name);
        assert!((p.normal.length() - 1.0).abs() < 1e-3 && p.half[0] > 0.0);
        // Points a little to either side of a portal lie in its two cells.
        let side = |q: Vec3| a.cells.iter().position(|c| c.contains(q));
        let (f, b) = (side(p.origin + p.normal * 20.0), side(p.origin - p.normal * 20.0));
        assert!(f.is_some() || b.is_some(), "{}", p.name);
    }
    assert!(joined >= 50, "{joined}");
    assert!(lvl.sky_light > 0.0);
}

/// Boyle Ext names its own player light range (`DishonoredMapInfo.m_pMapPlayerVisSettings` =
/// `Boyle_Twk.Pvs_BoyleExterior`), measured on the original as the normalization of the
/// visibility component's light (0.2 / 0.5).
#[test]
fn boyle_ext_player_light_range() {
    let Some(c) = content() else { return };
    let pkgs = map_packages(&c, "L_Boyle_Ext_P");
    let lvl = dishonored_assets::ai_level::extract_ai_level(&c, &pkgs, &[], &[]).unwrap();
    assert_eq!(lvl.player_light_range, Some([0.2, 0.5]));
}
