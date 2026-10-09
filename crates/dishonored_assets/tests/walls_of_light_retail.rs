//! Placed walls of light (`DisWallOfLight`) against the canonical GOG build (skipped without the
//! install): their tweak, receptacle and collision triangles.

use dishonored_assets::Content;
use dishonored_assets::world_actors::extract_world_actors;

#[test]
fn streets2_walls_of_light() {
    let Ok(c) = Content::locate() else { return };
    let pkgs = dishonored_assets::map_packages(&c, "L_Streets2_P");
    let wa = extract_world_actors(&c, &pkgs, &[0.0; 20]).unwrap();
    assert!(!wa.walls_of_light.is_empty(), "Streets2 places walls of light");
    for w in &wa.walls_of_light {
        assert!(!w.triangles.is_empty(), "{} has collision triangles", w.name);
        assert!(w.damage > 0, "{} kills", w.name);
        assert!(!w.friendly_factions.is_empty(), "{} spares its faction", w.name);
        assert!(w.mesh_hidden, "{}'s mesh is the editor's stand-in (HiddenGame)", w.name);
        assert!((w.windblast_min_force - 0.3).abs() < 1e-6, "{} m_fWindblastMinimumForce", w.name);
        assert!(!w.spark_effects.is_empty() && !w.beams.is_empty(), "{} has its spark and beam emitters", w.name);
        assert!(w.cross_spark.eq_ignore_ascii_case("Vfx_GamePlay.WallLight.Lightnings3_random"), "{}", w.cross_spark);
        assert!(w.shock_particles.eq_ignore_ascii_case("Vfx_GamePlay.WallLight.ps_shock_electrified"), "{}", w.shock_particles);
        assert!(w.sparks_only_while_arcing && w.spark_interval == [2.0, 3.5] && (w.spark_duration - 0.17).abs() < 1e-6);
    }
    assert!(wa.walls_of_light.iter().any(|w| w.receptacle.is_some()), "a wall is powered by a receptacle");
}

/// Prints the placed properties of a wall whose mesh was not found.
#[test]
#[ignore]
fn props() {
    let Ok(c) = Content::locate() else { return };
    let pkg = c.package("L_Flooded_FGate_Script").unwrap();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "DisWallOfLight" || pkg.obj_name(ue3_package::ObjRef::Export(i)) != "DisWallOfLight_0" {
            continue;
        }
        let h = pkg.read_object(i).unwrap();
        for p in &h.properties {
            println!("{} = {:?}", p.name, p.value);
        }
        for (j, e) in pkg.exports.iter().enumerate() {
            if e.outer == (i as i32 + 1) {
                println!("child {} {} {:?}", pkg.obj_name(ue3_package::ObjRef::Export(j)), pkg.export_class(j), pkg.read_object(j).map(|h| h.properties.iter().map(|p| format!("{}={:?}", p.name, p.value)).collect::<Vec<_>>()).unwrap_or_default());
            }
        }
    }
}

/// Prints a Streets2 wall's effective properties, its component's, and its beam / spark emitters'.
#[test]
#[ignore]
fn probe() {
    let Ok(c) = Content::locate() else { return };
    let pkg = c.package(&std::env::var("DH_WALL_PKG").unwrap_or("L_Streets2_Script".into())).unwrap();
    let mut res = dishonored_assets::collision_audit::PropResolver::new(&c);
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "DisWallOfLight" {
            continue;
        }
        println!("== {}", pkg.obj_name(ue3_package::ObjRef::Export(i)));
        for p in res.effective(&pkg, i).iter() {
            println!("  {} = {:?}", p.name, p.value);
        }
        let own = pkg.read_object(i).unwrap().properties;
        for key in ["StaticMeshComponent", "m_Beams", "m_SparkEffects", "m_CrossSparkEffect"] {
            let Some(p) = own.iter().find(|p| p.name == key) else { continue };
            let objs = p.array_objects().ok().or_else(|| p.as_object().map(|o| vec![o])).unwrap_or_default();
            for o in objs.into_iter().filter(|o| *o != 0) {
                if let ue3_package::ObjRef::Export(e) = ue3_package::ObjRef::from_index(o) {
                    println!("  {key} -> {} [{}]", pkg.obj_name(ue3_package::ObjRef::Export(e)), pkg.export_class(e));
                    for q in res.effective(&pkg, e).iter() {
                        println!("      {} = {:?}", q.name, q.value);
                    }
                }
            }
        }
        break;
    }
}

/// Prints every wall of light of every map.
/// `cargo test -p dishonored_assets --test walls_of_light_retail dump -- --ignored --nocapture`
#[test]
#[ignore]
fn dump() {
    let Ok(c) = Content::locate() else { return };
    let mut maps: Vec<String> = std::fs::read_dir(&c.cooked).unwrap().filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".upk").map(str::to_string)).filter(|n| n.starts_with("L_") && n.ends_with("_P")).collect();
    maps.sort();
    for map in maps {
        let pkgs = dishonored_assets::map_packages(&c, &map);
        let Ok(wa) = extract_world_actors(&c, &pkgs, &[0.0; 20]) else { continue };
        for w in &wa.walls_of_light {
            let r = w.receptacle.map(|r| format!("{}:{} battery {:?}", wa.receptacles[r].package, wa.receptacles[r].name, wa.receptacles[r].battery));
            println!("{map} {}:{} at {:?} rev {} en {} dmg {} jib {} act {}/{} friendly {:?} tris {} box {:?}..{:?} receptacle {:?} hidden {} wb {} sparks {:?} beams {:?} cross {} shock {}", w.package, w.name, w.position, w.reversed, w.enabled, w.damage, w.jib, w.activation_time, w.activation_time_pawn, w.friendly_factions, w.triangles.len(), w.bounds_min, w.bounds_max, r, w.mesh_hidden, w.windblast_min_force, w.spark_effects, w.beams, w.cross_spark, w.shock_particles);
        }
    }
}
