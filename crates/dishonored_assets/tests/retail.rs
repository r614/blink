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
    // 70 instances more; one fewer since a StaticMeshActor whose component lives in its level's
    // collection actor is left to the collection (it was placed twice, MAP_SWEEP.md).
    assert_eq!(map.instances.len(), 3826);
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

/// Every `ArkDynamicPylon` of the shipped `*_Nav` packages: its mover, attachment and mesh, and
/// the cross-pylon edges that reach it from the level's static pylons.
#[test]
#[ignore]
fn dynamic_pylons_retail() {
    use dishonored_assets::navigation::read_package_pylons;
    use dishonored_ir::nav::NavEdgeClass;
    let Some(c) = content() else { return };
    let mut names: Vec<String> = std::fs::read_dir(&c.cooked)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".upk").map(str::to_string))
        .filter(|n| n.to_ascii_lowercase().ends_with("_nav"))
        .collect();
    names.sort();
    let mut total = 0;
    for name in names {
        let p = c.package(&name).unwrap();
        if !(0..p.exports.len()).any(|i| p.export_class(i) == "ArkDynamicPylon") {
            continue;
        }
        let pylons = read_package_pylons(&p).unwrap();
        for (i, py) in pylons.iter().enumerate() {
            let Some(d) = &py.dynamic else { continue };
            total += 1;
            let m = &py.mesh;
            println!(
                "{name}:{} at {:?} bases {:?} rel {:?} hard {} ceiling {} cost check {}: {} verts {} polys {} edges, bounds {:?}..{:?}",
                py.name, py.location, d.bases, d.relative_location, d.hard_attach, d.ceiling_height, d.needs_cost_check, m.verts.len(), m.polys.len(), m.edges.len(), m.min, m.max
            );
            for (j, q) in pylons.iter().enumerate() {
                let n = q.mesh.edges.iter().filter(|e| matches!(e.class, NavEdgeClass::CrossPylon | NavEdgeClass::PathObject) && e.path_object.as_ref().is_some_and(|l| l.poly_refs.iter().any(|r| r.0 == py.object_ref))).count();
                if n > 0 {
                    println!("    {n} cross edges in {} ({}) touch it", q.name, if i == j { "itself" } else { "other" });
                }
            }
        }
    }
    assert!(total > 0);
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
    // The escape's tweak and flee sub-state match the documented resolved values; the surprise
    // (the guard's Panic animation group 0) has a length.
    let e = &ai.escape_explosion;
    assert_eq!((e.clearance, e.discovery_angle_deg, e.end_timer, e.threat_radius, e.threat_timer, e.after_explosion, e.surprise_chance), (600.0, 45.0, 5.0, 300.0, 2.0, 1.0, 1.0));
    assert_eq!(e.flee, d.escape_explosion.flee);
    assert!(e.anim_surprise > 0.0, "{:?}", ai.provenance.get("ai.escape_explosion.anim_surprise"));
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
    // The doors that drive portals (17 of the 18 doors link one; most tweaks occlude 0.8).
    assert!(lvl.portal_doors.len() >= 15, "{}", lvl.portal_doors.len());
    assert!(lvl.portal_doors.iter().all(|d| d.portal < a.portals.len()));
    assert!(lvl.portal_doors.iter().any(|d| (d.ai_occlusion - 0.8).abs() < 1e-6), "{:?}", lvl.portal_doors);
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

/// The AI noise table from the contact system and the pistol tweak is the documented one
/// (stealth.md section 5.2), and the swimming-on-surface bonus is read.
#[test]
fn ai_noise_table_from_the_contact_system() {
    let Some(c) = content() else { return };
    let ai = dishonored_assets::ai_config::load_ai_config(&c, &[]);
    assert_eq!(ai.noise, dishonored_ir::ai::NoiseTable::default(), "{:#?}", ai.provenance.iter().filter(|(k, _)| k.starts_with("ai.noise")).collect::<Vec<_>>());
    let pv = &ai.player_visibility;
    assert_eq!((pv.bonus_autocrouching, pv.bonus_sliding, pv.bonus_swimming, pv.bonus_swimming_surface), (-1.0, 0.0, -1.0, 0.0));
}

/// The prison's stealth volumes: brush hulls, priorities and their vis settings' range.
#[test]
fn prison_stealth_volumes() {
    let Some(c) = content() else { return };
    let lvl = dishonored_assets::ai_level::extract_ai_level(&c, &["L_Prison_Stealth".to_string()], &[], &[]).unwrap();
    assert_eq!(lvl.stealth_volumes.len(), 3, "{:#?}", lvl.stealth_volumes.iter().map(|v| (&v.name, v.priority, v.light_range)).collect::<Vec<_>>());
    for v in &lvl.stealth_volumes {
        assert!(!v.hulls.is_empty() && v.hulls.iter().all(|h| h.len() >= 4), "{}", v.name);
    }
    eprintln!("{:#?}", lvl.stealth_volumes.iter().map(|v| (&v.name, v.priority, v.light_range)).collect::<Vec<_>>());
}

/// Every gameplay particle system decodes, and no translucent / masked / additive sprite
/// material falls back to an opaque diffuse alpha (the square blood and smoke of U-770): the
/// decoded recipes resolve their opacity texture and the sprite texture's alpha is not opaque
/// everywhere (EFFECTS.md "Particle materials").
#[test]
fn gameplay_particle_sprites_have_an_opacity() {
    use dishonored_assets::particle_material::{GAMEPLAY_PARTICLES, MAP_LOCAL_PARTICLES};
    use dishonored_assets::texture::decode_rgba8;
    use dishonored_ir::MaterialBlend;
    let Some(c) = content() else { return };
    let pkgs = map_packages(&c, "L_Boyle_Ext_P");
    let opts = ImportOptions { load_lightmaps: false, max_texture_size: 256, extra_particles: GAMEPLAY_PARTICLES.iter().map(|s| s.to_string()).collect(), ..Default::default() };
    let (map, _) = import_level(&c, "L_Boyle_Ext_P", &pkgs, &opts).unwrap();
    // Boyle places no wall of light: its map-local systems are not in its packages.
    let missing: Vec<&&str> = GAMEPLAY_PARTICLES.iter().filter(|p| !MAP_LOCAL_PARTICLES.contains(p)).filter(|p| !map.particle_systems.iter().any(|s| s.source.eq_ignore_ascii_case(p))).collect();
    assert!(missing.is_empty(), "gameplay systems not decoded: {missing:?}");
    let mut bad = Vec::new();
    for s in &map.particle_systems {
        for e in s.emitters.iter().filter(|e| e.mesh.is_none()) {
            let Some(m) = e.material else { continue };
            let p = &map.material_params[m];
            if !matches!(p.blend, MaterialBlend::Translucent | MaterialBlend::Masked | MaterialBlend::Additive) {
                continue;
            }
            let name = &map.materials[m].source;
            if let Some(sh) = p.particle {
                if sh.opacity_missing {
                    bad.push(format!("{} / {name}: opacity texture unresolved ({:?})", s.source, p.base_material));
                    continue;
                }
                if sh.vertex_alpha_only || sh.radial_power.is_some() {
                    continue;
                }
            }
            // Additive sprites add their colour: an opaque alpha draws no square (black adds
            // nothing), and some decoded sources are opaque by design, e.g. `SleepBolt_Liquid_inst`
            // (`O_EnableOpacityAlpha` on the DXT1 `Splash03_d`) or `smoky_lookup` on the additive
            // test base `test_look_up`.
            if p.blend == MaterialBlend::Additive {
                continue;
            }
            let Some(t) = map.materials[m].diffuse else { continue };
            let tex = &map.textures[t];
            let opaque = decode_rgba8(tex, 0).is_some_and(|(_, _, px)| px.iter().all(|p| p[3] >= 250));
            if opaque {
                bad.push(format!("{} / {name}: sprite alpha opaque ({}, base {:?})", s.source, tex.source, p.base_material));
            }
        }
    }
    assert!(bad.is_empty(), "particle sprites without an opacity:\n{}", bad.join("\n"));
}

/// The Pub's window beam mesh keeps its vertex colours (Volume_Light_PMAT's `Use_Vertex_Color`
/// reads the red channel): 34 colours, white at the window, black at the beam's far end.
#[test]
fn window_beam_vertex_colours() {
    let Some(c) = content() else { return };
    let p = c.package("L_Pub_Day_FX").unwrap();
    let Some(i) = (0..p.exports.len()).find(|&i| p.export_class(i) == "StaticMesh" && p.obj_path(ue3_package::ObjRef::Export(i)) == "Hub_FX.Light.Hub_Medium_Window") else { return };
    let m = read_static_mesh(&p, i).unwrap();
    assert_eq!(m.colors.len(), m.positions.len());
    assert_eq!(m.colors.len(), 34);
    assert!(m.colors.iter().any(|c| c[0] == 255) && m.colors.iter().any(|c| c[0] == 0));
}

/// Boyle street's tether volumes and the spawner tethered to `DisTetherVolume_3`: its spawn lies
/// inside that volume's hull.
#[test]
fn boyle_street_ext_tether() {
    let Some(c) = content() else { return };
    let pkgs = vec!["L_BoyleStreet_Ext_Script".to_string()];
    let lvl = dishonored_assets::ai_level::extract_ai_level(&c, &pkgs, &[], &[]).unwrap();
    assert!(lvl.tether_volumes.len() >= 4, "{:?}", lvl.tether_volumes.iter().map(|v| &v.name).collect::<Vec<_>>());
    let npcs = dishonored_assets::npcs::extract_npc_data(&c, &pkgs).unwrap();
    let s = npcs.spawns.iter().find(|s| s.spawner.tether_volumes.iter().any(|v| v == "DisTetherVolume_3")).expect("a spawner tethered to DisTetherVolume_3");
    let v = lvl.tether_volumes.iter().find(|v| v.name == "DisTetherVolume_3").unwrap();
    assert!(v.contains(s.position) || v.contains(s.position - glam::Vec3::Z * 90.0), "{:?}", s.position);
}
