//! Placed actors of the map sweep's work list (research/dishonored/MAP_SWEEP.md), against the
//! original level data (skip without the install).

/// `SplineLoftActor` / `DisSplineLoftActor` (cables, rails) carry their generated meshes as
/// cooked `SplineMeshComponent`s (the loft of `DeformMesh` along the spline, `SplineParams`):
/// the importer deforms each component's mesh along its spline, for drawing and collision, like
/// any placed component. Only the actors with an outgoing connection carry a loft (the end
/// points of a cable have no `SplineMeshComps`): every cooked component is imported. None of
/// them collides: their collision flags (archetypes resolved) do not make them collide, and the
/// original's own line checks on Boyle (77 k + 45 k queries, `oracle collision`) disagree with
/// the port nowhere near a cable.
#[test]
fn spline_loft_actors_import_their_spline_meshes() {
    use ue3_package::ObjRef;
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    for (map, class) in [("L_Boyle_Ext_P", "SplineLoftActor"), ("L_Boyle_Int_P", "DisSplineLoftActor")] {
        if !content.has_package(map) {
            continue;
        }
        // Cooked spline mesh components of placed loft actors.
        let mut cooked = 0;
        for p in dishonored_assets::map_packages(&content, map) {
            let Ok(pkg) = content.package(&p) else { continue };
            for i in 0..pkg.exports.len() {
                if pkg.export_class(i) != "SplineMeshComponent" {
                    continue;
                }
                let ObjRef::Export(a) = ObjRef::from_index(pkg.exports[i].outer) else { continue };
                if pkg.export_class(a) == class && pkg.obj_name(ObjRef::from_index(pkg.exports[a].outer)) == "PersistentLevel" {
                    cooked += 1;
                }
            }
        }
        let m = dishonored_content::import_geometry(&content, map).unwrap();
        let tag = format!(".{class}_");
        let lofts: Vec<_> = m.instances.iter().filter(|i| i.source.contains(&tag)).collect();
        let colliding = lofts.iter().filter(|i| i.collides).count();
        eprintln!("{map}: {} of {cooked} cooked spline mesh components of {class} imported, {colliding} collide", lofts.len());
        assert!(cooked > 0 && lofts.len() == cooked, "{map}: {} of {cooked} imported", lofts.len());
        assert_eq!(colliding, 0, "{map}: loft components collide");
    }
}

/// `DisRiverKrust` and `DisWatchTower` name their skeletal mesh component only through
/// `CollisionComponent` (their own pointer is native): their physics asset bodies collide.
#[test]
fn river_krust_and_watch_tower_bodies_collide() {
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    for (map, actor) in [("L_Flooded_FRefinery_P", "L_Flooded_FRefinery_Script:DisRiverKrust_0"), ("L_Streets2_P", "L_Streets2_Script:DisWatchTower_1")] {
        if !content.has_package(map) {
            continue;
        }
        let pk = dishonored_assets::map_packages(&content, map);
        let b = dishonored_assets::prop_collision::read_prop_bodies(&content, &pk, actor, &Default::default()).unwrap();
        eprintln!("{actor}: {} body elements", b.tris.len());
        assert!(!b.tris.is_empty(), "{actor} has no bodies");
    }
}
