//! Tests of the Scaleform stage (`dishonored_runtime::gfx_stage`) and the HUD's key-tag text
//! (integration tests, so they link the normal library build instead of recompiling the whole
//! crate in test mode). The stage runs a small synthetic movie, no game data.

use dishonored_assets::swf::{Fill, FrameOp, Matrix, Movie, Place, Rect, ScriptOp, Shape, Sprite};
use dishonored_runtime::gfx_stage::*;
use std::collections::HashMap;
use std::sync::Arc;

/// Root -> "clip_mc" (sprite 2, 10 frames: stop at 0, label "open" at 3, the box moving 20 px
/// per frame from frame 4, stop at 9) -> "box" (a 10x10 red square, shape 1).
fn movie() -> Movie {
    let mut m = Movie { frame_rate: 30.0, ..Default::default() };
    m.shapes.insert(1, Shape { bounds: Rect { xmin: 0.0, xmax: 200.0, ymin: 0.0, ymax: 200.0 }, fills: vec![Fill::Solid([255, 0, 0, 255])], edges: [[0.0, 0.0, 200.0, 0.0], [200.0, 0.0, 200.0, 200.0], [200.0, 200.0, 0.0, 200.0], [0.0, 200.0, 0.0, 0.0]].map(|e| (0, e)).to_vec() });
    let mut s = Sprite::default();
    for f in 0..10usize {
        let mut ops = Vec::new();
        if f == 0 {
            ops.push(FrameOp::Place(Place { depth: 1, character: Some(1), matrix: Some(Matrix::default()), name: Some("box".into()), ..Default::default() }));
            ops.push(FrameOp::Script(vec![ScriptOp::Stop]));
        }
        if f == 3 {
            ops.push(FrameOp::Label("open".into()));
        }
        if f > 3 {
            ops.push(FrameOp::Place(Place { depth: 1, move_only: true, matrix: Some(Matrix { tx: f as f32 * 20.0 * 20.0, ..Default::default() }), ..Default::default() }));
        }
        if f == 9 {
            ops.push(FrameOp::Script(vec![ScriptOp::Stop]));
        }
        s.frames.push(ops);
    }
    m.sprites.insert(2, s);
    m.exports.insert("clip".into(), 2);
    m.root.frames.push(vec![FrameOp::Place(Place { depth: 1, character: Some(2), name: Some("clip_mc".into()), matrix: Some(Matrix { tx: 100.0 * 20.0, ty: 50.0 * 20.0, ..Default::default() }), ..Default::default() })]);
    m
}

fn stage() -> Stage {
    Stage::new(Arc::new(MovieData::new(movie(), Fonts::default())), Vec::new())
}

fn run(st: &mut Stage, secs: f64) -> Vec<Event> {
    let until = st.time + secs;
    let mut out = Vec::new();
    while let Some(e) = st.poll(until) {
        out.push(e);
    }
    out
}

#[test]
fn timeline_stops_plays_labels_and_moves() {
    let mut st = stage();
    let clip = st.child(st.root, "clip_mc").unwrap();
    let bx = st.child(clip, "box").unwrap();
    assert_eq!(st.current_frame(clip), 1);
    run(&mut st, 0.5);
    // stop() on frame 0.
    assert_eq!(st.current_frame(clip), 1);
    st.goto_and_play(clip, "open");
    assert_eq!(st.current_frame(clip), 4);
    // One movie frame later: frame index 4, the box at x = 80.
    run(&mut st, 1.0 / 30.0 + 1e-6);
    assert_eq!(st.current_frame(clip), 5);
    assert!((st.get(bx, Prop::X) - 80.0).abs() < 1e-3);
    // Plays to the stop() at frame 9 and stays.
    run(&mut st, 1.0);
    assert_eq!(st.current_frame(clip), 10);
    assert!((st.get(bx, Prop::X) - 180.0).abs() < 1e-3);
}

#[test]
fn scripted_clips_leave_the_timeline_and_draw_where_set() {
    let mut st = stage();
    let clip = st.child(st.root, "clip_mc").unwrap();
    let bx = st.child(clip, "box").unwrap();
    st.set(bx, Prop::X, 7.0);
    st.goto_and_play(clip, "open");
    run(&mut st, 0.2);
    assert!((st.get(bx, Prop::X) - 7.0).abs() < 1e-3);
    let mut draws = Vec::new();
    st.draw(&mut draws);
    // The square's raster (`swf_shape`), one raster pixel (half a stage pixel) of padding
    // around its bounds.
    let Some(Draw::Quad { tex: TexKey::Shape(_, 1), dst, rgba, .. }) = draws.first() else { panic!("{draws:?}") };
    assert!((dst[0].x - 106.5).abs() < 1e-3 && (dst[0].y - 49.5).abs() < 1e-3, "{dst:?}");
    assert!((dst[2].x - 117.5).abs() < 1e-3);
    assert_eq!(*rgba, [1.0; 4]);
    let r = dishonored_assets::swf_shape::rasterize(&movie().shapes[&1]).unwrap();
    let c = ((r.height / 2 * r.width + r.width / 2) * 4) as usize;
    assert_eq!(&r.rgba[c..c + 4], &[255, 0, 0, 255]);
    // Invisible and zero-alpha clips draw nothing.
    st.set(clip, Prop::Alpha, 0.0);
    st.draw(&mut draws);
    assert!(draws.is_empty());
}

#[test]
fn tweens_follow_their_curve_on_movie_frames_and_report_completion() {
    let mut st = stage();
    let clip = st.child(st.root, "clip_mc").unwrap();
    st.tween_to(clip, 0.5, &[(Prop::Alpha, 0.0)], Ease::Linear, Some(42));
    let t0 = st.time;
    // Values change on movie frames: after 0.25 s the last frame was at or before it.
    run(&mut st, 0.25);
    let a = st.get(clip, Prop::Alpha);
    let frames_t = ((st.time - t0) * 30.0).floor() / 30.0;
    let want = 100.0 * (1.0 - frames_t / 0.5);
    assert!((a as f64 - want).abs() < 7.0, "{a} vs {want}");
    let ev = run(&mut st, 0.3);
    assert!(ev.contains(&Event::TweenDone(42)));
    assert_eq!(st.get(clip, Prop::Alpha), 0.0);
    assert!(!st.tweening(clip));
    // A new tweenTo replaces the clip's running tween.
    st.tween_to(clip, 1.0, &[(Prop::X, 200.0)], Ease::StrongOut, Some(1));
    st.tween_to(clip, 1.0, &[(Prop::Y, 0.0)], Ease::StrongOut, Some(2));
    let ev = run(&mut st, 1.1);
    assert!(ev.contains(&Event::TweenDone(2)) && !ev.contains(&Event::TweenDone(1)));
    assert!((st.get(clip, Prop::X) - 100.0).abs() < 0.5);
}

#[test]
fn intervals_fire_at_their_time_in_order() {
    let mut st = stage();
    let a = st.set_interval(500.0, 1);
    st.set_interval(250.0, 2);
    let c = st.set_interval(100.0, 3);
    st.clear_interval(c);
    let ev = run(&mut st, 0.3);
    assert_eq!(ev, vec![Event::Timer(2)]);
    let ev = run(&mut st, 0.3);
    assert_eq!(ev, vec![Event::Timer(1)]);
    st.clear_interval(a);
    assert!(run(&mut st, 1.0).is_empty());
}

#[test]
fn attach_and_remove_dynamic_clips() {
    let mut st = stage();
    let root = st.root;
    let d = st.next_depth(root);
    let c = st.attach(root, "clip", "dyn", d).unwrap();
    assert_eq!(st.child(root, "dyn"), Some(c));
    assert!(st.child(c, "box").is_some());
    assert_eq!(st.width(c), 10.0);
    st.set_width(c, 30.0);
    assert!((st.get(c, Prop::XScale) - 300.0).abs() < 1e-3);
    st.remove(c);
    assert!(st.child(root, "dyn").is_none());
    assert!(st.attach(root, "missing", "x", 50).is_none());
}

#[test]
fn ease_curves_match_the_movies_classes() {
    // Strong.easeOut(t) = (t - 1)^5 + 1; Back.easeOut overshoots (s = 1.70158).
    assert!((Ease::StrongOut.at(0.2) - (1.0 - 0.8f32.powi(5))).abs() < 1e-6);
    assert!((Ease::StrongIn.at(0.5) - 0.5f32.powi(5)).abs() < 1e-6);
    assert!(Ease::BackOut.at(0.7) > 1.0);
    assert!(Ease::BackIn.at(0.2) < 0.0);
    assert!((Ease::StrongInOut.at(0.5) - 0.5).abs() < 1e-6);
}

#[test]
fn key_tags_become_bracketed_key_names() {
    let mut keys = HashMap::new();
    keys.insert("SpaceBar".to_string(), "Space Bar".to_string());
    let s = dishonored_runtime::gfx_hud::convert_key_tags("`GBA_Use` Talk", None, &keys);
    assert_eq!(s, "[F] Talk");
    let s = dishonored_runtime::gfx_hud::convert_key_tags("`GBA_Jump` Jump off", None, &keys);
    assert_eq!(s, "[Space Bar] Jump off");
    let s = dishonored_runtime::gfx_hud::convert_key_tags("Press `k to go", None, &keys);
    assert_eq!(s, "Press [F] to go");
}
