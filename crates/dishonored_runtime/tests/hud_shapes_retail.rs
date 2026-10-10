//! The HUD effects movie against the canonical GOG build (skipped without the install).

use dishonored_assets::swf::{Fill, Movie};
use dishonored_runtime::gfx_stage::{Fonts, MovieData, Stage};
use std::sync::Arc;

fn hudfx() -> Option<Movie> {
    let c = dishonored_assets::Content::locate().ok()?;
    let pkg = c.package("DishonoredGame").ok()?;
    Movie::parse(&dishonored_runtime::gfx_stage::movie_raw(&pkg, "UI_HUDFX.HUDFX")?).ok()
}

/// `blackShroud_mc` holds one 512 x 512 bitmap (the vignette `HUDFX_I29`, black with the shroud
/// in its alpha) centred on the stage; the HUD layout (0x00B96250) sizes the clip to the stage's
/// extent, so the shroud covers all of 1280 x 720 (drawn unscaled it was a dark square in the
/// middle of the screen).
#[test]
fn the_stealth_shroud_covers_the_stage() {
    let Some(m) = hudfx() else { return };
    assert!(m.shapes.values().any(|s| s.bounds.xmax == 10240.0 && matches!(s.fills.first(), Some(Fill::Bitmap { .. }))));
    let mut st = Stage::new(Arc::new(MovieData::new(m, Fonts::default())), Vec::new());
    let _w = dishonored_runtime::hud_widgets::HudFxWidgets::new(&mut st);
    let mc = st.child(st.root, "blackShroud_mc").expect("blackShroud_mc");
    let r = st.bounds_in_parent(mc).expect("bounds");
    assert!(r.min.x.abs() < 0.5 && r.min.y.abs() < 0.5 && (r.width() - 1280.0).abs() < 0.5 && (r.height() - 720.0).abs() < 0.5, "{r:?}");
}
