//! The original navigation mesh on Boyle Ext (skips without the install): it lines up with
//! the collision floor, covers the player starts and AI points, and paths connect points that
//! share a mesh island.
use dishonored_content::{DEFAULT_MAP, load_match};
use dishonored_world::nav::{NavWorld, PathOpts};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;

/// Undirected connected components over every link (gated or not).
fn components(nav: &NavWorld) -> (Vec<usize>, Vec<usize>) {
    let n = nav.polys.len();
    let mut adj = vec![Vec::new(); n];
    for (u, ls) in nav.links.iter().enumerate() {
        for l in ls {
            adj[u].push(l.to as usize);
            adj[l.to as usize].push(u);
        }
    }
    let mut comp = vec![usize::MAX; n];
    let mut sizes = Vec::new();
    for s in 0..n {
        if comp[s] != usize::MAX {
            continue;
        }
        let id = sizes.len();
        let mut stack = vec![s];
        comp[s] = id;
        let mut size = 0;
        while let Some(u) = stack.pop() {
            size += 1;
            for &v in &adj[u] {
                if comp[v] == usize::MAX {
                    comp[v] = id;
                    stack.push(v);
                }
            }
        }
        sizes.push(size);
    }
    (comp, sizes)
}

#[test]
fn boyle_navmesh_matches_collision_and_connects() {
    if dishonored_assets::Content::locate().is_err() {
        return;
    }
    let mc = load_match(DEFAULT_MAP).unwrap();
    let nav = mc.collision.nav().expect("original navigation loaded");
    println!("nav: {} polys, {} links, {} path objects", nav.polys.len(), nav.link_count(), nav.path_objects.len());
    assert!(nav.polys.len() > 900);
    // Height of the nav polys above the collision floor at their centres.
    let mut offs: Vec<f32> = Vec::new();
    for (i, p) in nav.polys.iter().enumerate() {
        let c = p.center;
        if let Some(h) = mc.collision.raycast(c + Vec3::Z * 60.0, -Vec3::Z, 200.0, channel::PAWN) {
            offs.push(nav.poly_z(i as u32, c.x, c.y) - h.point.z);
        }
    }
    offs.sort_by(|a, b| a.total_cmp(b));
    let med = offs[offs.len() / 2];
    println!("poly centre minus floor: n {} p10 {:.1} median {:.1} p90 {:.1}", offs.len(), offs[offs.len() / 10], med, offs[offs.len() * 9 / 10]);
    assert!(med.abs() < 40.0, "navmesh should sit on the floor (median offset {med})");
    // Player starts and AI points: on the mesh.
    let pts: Vec<Vec3> = mc.spawns.iter().map(|s| s.position).chain(mc.ai_points.iter().map(|p| p.0)).collect();
    let on: Vec<(Vec3, u32)> = pts.iter().filter_map(|p| nav.closest_point(*p, 200.0).map(|(i, _)| (*p, i))).collect();
    println!("points on mesh: {}/{}", on.len(), pts.len());
    assert!(on.len() * 10 >= pts.len() * 8);
    let (comp, sizes) = components(nav);
    let mut per = vec![0; sizes.len()];
    for (_, i) in &on {
        per[comp[*i as usize]] += 1;
    }
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(sizes[i]));
    for &i in order.iter().take(6) {
        let c = nav.polys.iter().enumerate().filter(|(k, _)| comp[*k] == i).map(|(_, p)| p.center).fold(Vec3::ZERO, |a, b| a + b) / sizes[i] as f32;
        println!("island {i}: {} polys, {} points, centroid {:.0?}", sizes[i], per[i], c);
    }
    let spawn_islands: Vec<usize> = mc.spawns.iter().filter_map(|s| nav.closest_point(s.position, 200.0)).map(|(i, _)| comp[i as usize]).collect();
    println!("{} islands; player starts per island {:?}", sizes.len(), spawn_islands);
    // Same-island pairs: routes exist unless a locked door / intact blocker or a one-way edge
    // is in the way.
    let (mut ok, mut tot, mut ok_any) = (0, 0, 0);
    let t = std::time::Instant::now();
    for (k, (a, ia)) in on.iter().enumerate().step_by(2) {
        for (b, ib) in on.iter().skip(k + 1).step_by(3) {
            if comp[*ia as usize] != comp[*ib as usize] {
                continue;
            }
            tot += 1;
            if nav.find_path(*a, *b, &PathOpts::default()).is_some() {
                ok += 1;
            }
            if nav.find_path(*a, *b, &PathOpts { through_blocking: true, ..Default::default() }).is_some() {
                ok_any += 1;
            }
        }
    }
    println!("same-island paths: {ok}/{tot} (through blocking path objects: {ok_any}/{tot}) in {:?}", t.elapsed());
    assert!(ok_any * 50 >= tot * 49, "same-island point pairs should be connected ({ok_any}/{tot})");
    assert!(ok * 20 >= tot * 17, "locked doors and intact blockers cut only a few routes ({ok}/{tot})");
}
