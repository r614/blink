//! Tests of `dishonored_runtime::ragdoll` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::ragdoll::*;

/// The elite guard's ragdoll, dropped from its bind pose, settles on the floor near where it
/// started (skipped without the install).
#[test]
fn elite_guard_settles_on_the_floor() {
    let Ok(c) = dishonored_assets::Content::locate() else { return };
    let hh = 97.0;
    let m = dishonored_assets::characters::load_character(&c, "elite_guard", hh, 16).unwrap();
    let part = &m.parts[0];
    let g0: Vec<Mat4> = dishonored_assets::characters::bind_globals(&part.bones).iter().map(|g| part.mesh_to_pawn * *g).collect();
    let a = dishonored_assets::npc_anim::load_physics_asset(&c, &("L_Boyle_Ext_Script".into(), "Npc_EliteGuard.Physics.Skm_EliteGuard_Physics".into())).unwrap();
    let mut r = Ragdoll::new(&a, &part.bones, &g0, &g0, &|_, _| -hh, &[], -980.0).unwrap();
    r.settle();
    for b in &r.bodies {
        assert!(b.pos.is_finite(), "{}", part.bones[b.bone].name);
        assert!(b.pos.truncate().length() < 250.0, "{} drifted to {}", part.bones[b.bone].name, b.pos);
        assert!(b.pos.z >= -hh - 1.0 && b.pos.z < -hh + 200.0, "{} at {}", part.bones[b.bone].name, b.pos);
    }
    // Joint lengths held.
    for b in &r.bodies {
        if let Some(p) = b.parent {
            let d = (b.pos - r.bodies[p].pos).length();
            assert!((d - b.rest).abs() < 0.1 * b.rest.max(1.0) + 1.0, "{}: {d} vs {}", part.bones[b.bone].name, b.rest);
        }
    }
    let g = r.globals();
    for (k, m) in g.iter().enumerate() {
        assert!(m.is_finite() && m.w_axis.truncate().length() < 300.0, "{} at {}", part.bones[k].name, m.w_axis);
    }
}


#[test]
fn floor_grid_follows_a_step() {
    // A 30 uu step at x = 0: bilinear between samples, clamped beyond the grid.
    let g = FloorGrid::sample(&|x, _| if x < 0.0 { -97.0 } else { -67.0 });
    assert_eq!(g.at(-50.0, 0.0), -97.0);
    assert_eq!(g.at(50.0, 10.0), -67.0);
    assert_eq!(g.at(500.0, -500.0), -67.0);
    let mid = g.at(-6.0, 0.0);
    assert!(mid > -97.0 && mid < -67.0, "{mid}");
}

/// The elite guard dropped over a 30 uu step lies on both levels (skipped without the install).
#[test]
fn elite_guard_settles_on_a_step() {
    let Ok(c) = dishonored_assets::Content::locate() else { return };
    let hh = 97.0;
    let m = dishonored_assets::characters::load_character(&c, "elite_guard", hh, 16).unwrap();
    let part = &m.parts[0];
    let g0: Vec<Mat4> = dishonored_assets::characters::bind_globals(&part.bones).iter().map(|g| part.mesh_to_pawn * *g).collect();
    let a = dishonored_assets::npc_anim::load_physics_asset(&c, &("L_Boyle_Ext_Script".into(), "Npc_EliteGuard.Physics.Skm_EliteGuard_Physics".into())).unwrap();
    let floor = |x: f32, _y: f32| if x < 0.0 { -hh } else { -hh + 30.0 };
    let mut r = Ragdoll::new(&a, &part.bones, &g0, &g0, &floor, &[], -980.0).unwrap();
    r.settle();
    for b in &r.bodies {
        let f = r.floor.at(b.pos.x, b.pos.y);
        assert!(b.pos.z >= f + b.radius - 1.0, "{} at {} under the floor {f}", part.bones[b.bone].name, b.pos);
    }
}
/// A three-body chain (root, spine, head) as a physics asset and its bind globals.
fn chain() -> (PhysicsAssetData, Vec<BoneAsset>, Vec<Mat4>) {
    let bone = |name: &str, parent: i32, t: Vec3| BoneAsset { name: name.into(), parent, rotation: Quat::IDENTITY, translation: t };
    let part = vec![bone("root", -1, Vec3::ZERO), bone("spine", 0, Vec3::new(0.0, 0.0, 30.0)), bone("head", 1, Vec3::new(0.0, 0.0, 30.0))];
    let g0 = vec![Mat4::from_translation(Vec3::ZERO), Mat4::from_translation(Vec3::new(0.0, 0.0, 30.0)), Mat4::from_translation(Vec3::new(0.0, 0.0, 60.0))];
    let body = |b: &str| dishonored_ir::npc_anim::PhysBody { bone: b.into(), radius: 8.0, ..Default::default() };
    let joint = |a: &str, b: &str| dishonored_ir::npc_anim::PhysConstraint { bone1: a.into(), bone2: b.into(), swing_limited: true, swing1: 45.0, swing2: 45.0, ..Default::default() };
    let asset = PhysicsAssetData { name: "chain".into(), bodies: vec![body("root"), body("spine"), body("head")], constraints: vec![joint("spine", "root"), joint("head", "spine")] };
    (asset, part, g0)
}

/// A wall 20 uu in front of the pawn (+X), facing it, as two triangles.
fn wall_at_x(x: f32) -> Vec<[Vec3; 3]> {
    let (lo, hi) = (Vec3::new(x, -200.0, -100.0), Vec3::new(x, 200.0, 300.0));
    vec![
        [lo, Vec3::new(x, lo.y, hi.z), hi],
        [lo, hi, Vec3::new(x, hi.y, lo.z)],
    ]
}

/// The chain, toppling towards a wall, stops at it (every body at least its radius in front
/// of the plane); without the wall it falls through that plane. Two runs settle bit-identically.
#[test]
fn chain_falls_against_a_wall() {
    let (asset, part, g0) = chain();
    // Moving +X at 600 uu/s at the hand-over (previous step 10 uu behind).
    let g_prev: Vec<Mat4> = g0.iter().map(|g| Mat4::from_translation(Vec3::new(-10.0, 0.0, 0.0)) * *g).collect();
    let floor = |_: f32, _: f32| -8.0;
    let walls = wall_at_x(20.0);
    let run = |walls: &[[Vec3; 3]]| {
        let mut r = Ragdoll::new(&asset, &part, &g0, &g_prev, &floor, walls, -980.0).unwrap();
        r.settle();
        r.bodies.iter().map(|b| b.pos).collect::<Vec<Vec3>>()
    };
    let free = run(&[]);
    assert!(free.iter().any(|p| p.x > 20.0), "without the wall the chain should cross x = 20: {free:?}");
    let a = run(&walls);
    for p in &a {
        assert!(p.is_finite());
        assert!(p.x <= 20.0 - 8.0 + 0.5, "body through the wall: {p}");
        assert!(p.z >= -8.0 + 8.0 - 0.5, "body under the floor: {p}");
    }
    let b = run(&walls);
    assert_eq!(a.iter().map(|p| p.to_array().map(f32::to_bits)).collect::<Vec<_>>(), b.iter().map(|p| p.to_array().map(f32::to_bits)).collect::<Vec<_>>());
}

#[test]
fn walls_keep_only_steep_triangles_and_push_out() {
    // A floor triangle is not a wall.
    assert!(WallTri::new([Vec3::ZERO, Vec3::X, Vec3::Y]).is_none());
    let w = WallTri::new(wall_at_x(20.0)[0]).unwrap();
    // Facing the pawn (-X).
    assert!((w.n.x + 1.0).abs() < 1e-6);
    // A sphere straddling the face is pushed to touch it from the front.
    let p = Vec3::new(20.0 + w.n.x * 3.0, 0.0, 200.0);
    let (dir, depth) = w.push(p, 8.0).unwrap();
    assert!((dir - w.n).length() < 1e-5 && (depth - 5.0).abs() < 1e-4, "{dir} {depth}");
    // From behind: pushed back out behind (two-sided).
    let (dir, depth) = w.push(Vec3::new(20.0 - w.n.x * 3.0, 0.0, 200.0), 8.0).unwrap();
    assert!((dir + w.n).length() < 1e-5 && (depth - 5.0).abs() < 1e-4, "{dir} {depth}");
    // Beyond the radius on either side: nothing.
    assert!(w.push(Vec3::new(20.0 + w.n.x * 9.0, 0.0, 200.0), 8.0).is_none());
    assert!(w.push(Vec3::new(20.0 - w.n.x * 9.0, 0.0, 200.0), 8.0).is_none());
    // Past the triangle's edge: no push.
    assert!(w.push(Vec3::new(20.0 + w.n.x * 3.0, 500.0, 200.0), 8.0).is_none());
}

#[test]
fn frame_matches_two_axes() {
    let q = frame(Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X);
    assert!((q * Vec3::X - Vec3::Z).length() < 1e-4);
    assert!((q * Vec3::Y + Vec3::X).length() < 1e-4);
}
