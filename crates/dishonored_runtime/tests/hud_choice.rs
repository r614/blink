//! Tests of the conversation choice menu (`dishonored_runtime::hud_widgets::PlayerChoice`, the HUD
//! movie's `PlayerChoice` / `GenericMenu` / `SelectionHandler`) on keyboard and mouse
//! (integration tests, so they link the normal library build instead of recompiling the whole
//! crate in test mode). The stage runs a small synthetic movie shaped like `hud_playerChoice` /
//! `hud_plC_button`, no game data.

use bevy::input::keyboard::KeyCode;
use bevy::math::Vec2;
use dishonored_assets::swf::{ButtonRecord, Fill, FrameOp, Matrix, Movie, Place, Rect, Shape, Sprite};
use dishonored_runtime::gfx_stage::*;
use dishonored_runtime::hud_widgets::*;
use std::sync::Arc;

fn square(w: f32, h: f32) -> Shape {
    let (w, h) = (w * 20.0, h * 20.0);
    Shape { bounds: Rect { xmin: 0.0, xmax: w, ymin: 0.0, ymax: h }, fills: vec![Fill::Solid([255, 255, 255, 255])], edges: [[0.0, 0.0, w, 0.0], [w, 0.0, w, h], [w, h, 0.0, h], [0.0, h, 0.0, 0.0]].map(|e| (0, e)).to_vec() }
}

fn place(depth: u16, character: u16, name: &str, matrix: Matrix) -> FrameOp {
    FrameOp::Place(Place { depth, character: Some(character), name: (!name.is_empty()).then(|| name.to_string()), matrix: Some(matrix), ..Default::default() })
}

/// Root -> `playerChoice_mc` (sprite 10, at 400, 500) -> `_title_mc` (empty sprite 11); the
/// exported `hud_plC_button` (sprite 20): a 300 x 30 background, the `_a_mc` glyph and the
/// `btn` button (character 30) whose hit state is a 62 x 62 square scaled 5 x 0.5 from
/// (-40, -15), as the retail button 91 (5.53 x 0.65 from (-40.9, -17.85)).
fn movie() -> Movie {
    let mut m = Movie { frame_rate: 30.0, ..Default::default() };
    m.shapes.insert(1, square(62.0, 62.0));
    m.shapes.insert(2, square(10.0, 10.0));
    m.shapes.insert(3, square(300.0, 30.0));
    m.buttons.insert(30, vec![ButtonRecord { states: 8, character: 1, depth: 1, matrix: Matrix::default() }]);
    let btn = Matrix { a: 5.0, d: 0.5, tx: -40.0 * 20.0, ty: -15.0 * 20.0, ..Default::default() };
    m.sprites.insert(20, Sprite { frames: vec![vec![place(1, 3, "", Matrix::default()), place(8, 2, "_a_mc", Matrix::default()), place(9, 30, "btn", btn)]] });
    m.sprites.insert(11, Sprite { frames: vec![vec![]] });
    m.sprites.insert(10, Sprite { frames: vec![vec![place(1, 11, "_title_mc", Matrix::default())]] });
    m.exports.insert("hud_plC_button".into(), 20);
    m.root.frames.push(vec![place(1, 10, "playerChoice_mc", Matrix { tx: 400.0 * 20.0, ty: 500.0 * 20.0, ..Default::default() })]);
    m
}

fn menu() -> (Stage, PlayerChoice, SoundQueue) {
    let mut st = Stage::new(Arc::new(MovieData::new(movie(), Fonts::default())), Vec::new());
    let mc = st.child(st.root, "playerChoice_mc").unwrap();
    let mut c = PlayerChoice::new(&mut st, mc);
    let mut snd = SoundQueue::default();
    let opts: Vec<String> = ["Ask about the party", "Ask about the sisters", "Leave"].map(String::from).to_vec();
    c.set_menu(&mut st, &mut snd, Some(&opts), None);
    (st, c, snd)
}

fn button(st: &Stage, c: &PlayerChoice, i: usize) -> Id {
    let cont = st.child(c.mc, "_btnContainer_mc").unwrap();
    st.child(cont, &format!("btn{i}")).unwrap()
}

fn a_icon(st: &Stage, c: &PlayerChoice, i: usize) -> bool {
    let b = button(st, c, i);
    st.visible(st.child(b, "_a_mc").unwrap())
}

/// The centre of button `i`'s hit area, in stage pixels.
fn centre(st: &Stage, c: &PlayerChoice, i: usize) -> Vec2 {
    let hit = st.child(button(st, c, i), "btn").unwrap();
    let (x, y) = st.world_matrix(hit).apply(31.0 * 20.0, 31.0 * 20.0);
    Vec2::new(x, y) / 20.0
}

#[test]
fn the_a_glyph_shows_only_on_a_gamepad_on_pc() {
    assert!(!choice_a_icon_visible(true, false));
    assert!(choice_a_icon_visible(true, true));
    assert!(choice_a_icon_visible(false, false));
    let (mut st, mut c, mut snd) = menu();
    // Keyboard and mouse: the selected button's `_a_mc` is hidden.
    assert_eq!(c.selection, 0);
    assert!(!a_icon(&st, &c, 0));
    c.down_pressed(&mut st, &mut snd);
    assert!(!a_icon(&st, &c, 1));
    // `_global.bUsingGamepad`: the next selection shows it.
    c.using_gamepad = true;
    c.down_pressed(&mut st, &mut snd);
    assert!(a_icon(&st, &c, 2));
}

#[test]
fn arrows_move_the_selection_and_stop_at_the_ends() {
    let (mut st, mut c, mut snd) = menu();
    c.up_pressed(&mut st, &mut snd);
    assert_eq!((c.selection, snd.0.last().copied()), (0, Some("Stop")));
    c.down_pressed(&mut st, &mut snd);
    assert_eq!((c.selection, snd.0.last().copied()), (1, Some("Move")));
    c.down_pressed(&mut st, &mut snd);
    c.down_pressed(&mut st, &mut snd);
    assert_eq!((c.selection, snd.0.last().copied()), (2, Some("Stop")));
    c.up_pressed(&mut st, &mut snd);
    assert_eq!(c.selection, 1);
    // Enter confirms: `Validation`, and the inputs are gone (`RemoveInputs`).
    c.a_pressed(&mut st, &mut snd);
    assert_eq!(snd.0.last().copied(), Some("Validation"));
    c.up_pressed(&mut st, &mut snd);
    assert_eq!(c.selection, 1);
    assert_eq!(snd.0.last().copied(), Some("Validation"));
}

#[test]
fn keys_map_like_the_inputs_manager() {
    assert_eq!(choice_key(KeyCode::ArrowUp, false), Some(ChoiceKey::Up));
    assert_eq!(choice_key(KeyCode::ArrowDown, true), Some(ChoiceKey::Down));
    assert_eq!(choice_key(KeyCode::Enter, false), Some(ChoiceKey::A));
    assert_eq!(choice_key(KeyCode::NumpadEnter, false), Some(ChoiceKey::A));
    // "A" is not a held input: a key repeat does nothing.
    assert_eq!(choice_key(KeyCode::Enter, true), None);
    // The number keys are the "DockBar" input, not the menu's.
    assert_eq!(choice_key(KeyCode::Digit1, false), None);
    assert_eq!(choice_key(KeyCode::KeyF, false), None);
}

#[test]
fn the_mouse_selects_on_entering_and_confirms_on_a_click() {
    let (mut st, mut c, mut snd) = menu();
    let p2 = centre(&st, &c, 2);
    assert_eq!(c.button_at(&st, p2), Some(2));
    assert_eq!(c.button_at(&st, p2 + Vec2::new(-400.0, 0.0)), None);
    // Entering button 2 selects it with `Move`.
    c.mouse(&mut st, &mut snd, Some(p2), true, false, false);
    assert_eq!((c.selection, snd.0.last().copied()), (2, Some("Move")));
    // Staying over it does not run `RollOver` again; a still mouse does nothing.
    let n = snd.0.len();
    c.mouse(&mut st, &mut snd, Some(p2 + Vec2::new(5.0, 0.0)), true, false, false);
    c.mouse(&mut st, &mut snd, Some(p2), false, false, false);
    assert_eq!(snd.0.len(), n);
    // A press on button 2 released over button 0 is no click; the move selects 0.
    let p0 = centre(&st, &c, 0);
    c.mouse(&mut st, &mut snd, Some(p2), false, true, false);
    c.mouse(&mut st, &mut snd, Some(p0), true, false, true);
    assert_eq!(c.selection, 0);
    assert!(!snd.0.contains(&"Validation"));
    // Press and release over button 1: `Click` selects it (no `Move`) and confirms.
    let p1 = centre(&st, &c, 1);
    c.mouse(&mut st, &mut snd, Some(p1), false, true, false);
    let n = snd.0.len();
    c.mouse(&mut st, &mut snd, Some(p1), false, false, true);
    assert_eq!(c.selection, 1);
    assert_eq!(&snd.0[n..], &["Validation"]);
    // The inputs are gone: rolling over another button does nothing.
    c.mouse(&mut st, &mut snd, Some(p2), true, false, false);
    assert_eq!(c.selection, 1);
}
