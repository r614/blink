//! Tests of `dishonored_runtime::gfx_hud` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::gfx_hud::*;

fn part(ext: bool, index: u8, clip: [f32; 6]) -> CrossPart {
    CrossPart { ext, index, centre: Vec2::new(640.0, 360.0), clip, part: Vec2::ZERO, local: [Vec2::ZERO, Vec2::X, Vec2::new(1.0, 1.0), Vec2::Y] }
}

#[test]
fn gun_parts_go_to_the_corners_and_the_clips_turn() {
    let id = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    // p = 0: no rotation, part 1 at (d, d), full alpha.
    let (c, a) = cross_layout(&part(true, 1, id), false, 15.0, 0.0);
    assert!((c[0] - Vec2::new(655.0, 375.0)).length() < 1e-4 && (a - 1.0).abs() < 1e-6);
    // p = 100: ext clip at -20 degrees (counter-clockwise on screen), alpha 100 - 75.
    let (c, a) = cross_layout(&part(true, 0, id), false, 45.0, 100.0);
    let r = (-20f32).to_radians();
    let v = Vec2::new(45.0, -45.0);
    let want = Vec2::new(640.0, 360.0) + Vec2::new(r.cos() * v.x - r.sin() * v.y, r.sin() * v.x + r.cos() * v.y);
    assert!((c[0] - want).length() < 1e-3, "{:?} {want:?}", c[0]);
    assert!((a - 0.25).abs() < 1e-6);
    // The internal clip keeps its scale (0.5857) when its rotation is replaced.
    let s = 0.414_154;
    let (c, _) = cross_layout(&part(false, 1, [s, -s, s, s, 0.0, 0.0]), false, 10.0, 0.0);
    let k = (2.0f32).sqrt() * s;
    assert!((c[0] - Vec2::new(640.0 + 10.0 * k, 360.0 + 10.0 * k)).length() < 1e-3, "{:?}", c[0]);
}

#[test]
fn crossbow_parts_move_on_their_axes() {
    let id = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut p = part(true, 1, id);
    p.part = Vec2::new(3.0, 7.0);
    // offset = -15 p / 100 = -7.5 at p = 50: part1 x = d + offset, y authored.
    let (c, a) = cross_layout(&p, true, 20.0, 50.0);
    assert!((c[0] - Vec2::new(640.0 + 12.5, 367.0)).length() < 1e-4);
    assert!((a - 0.85).abs() < 1e-6);
    let (c, _) = cross_layout(&part(false, 3, id), true, 20.0, 50.0);
    assert!((c[0] - Vec2::new(620.0, 340.0)).length() < 1e-4);
}
