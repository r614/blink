//! The HUD movies against the canonical GOG build (skipped without the install).

use dishonored_assets::swf::{Fill, Movie};
use dishonored_runtime::gfx_stage::{Fonts, MovieData, MovieSpace, Prop, Stage};
use glam::Vec2;
use std::sync::Arc;

fn movie(package: &str, name: &str) -> Option<Movie> {
    let c = dishonored_assets::Content::locate().ok()?;
    let pkg = c.package(package).ok()?;
    Movie::parse(&dishonored_runtime::gfx_stage::movie_raw(&pkg, name)?).ok()
}

fn stage(m: Movie, viewport: Vec2) -> Stage {
    let mut st = Stage::new(Arc::new(MovieData::new(m, Fonts::default())), Vec::new());
    st.space = MovieSpace::new(viewport);
    st
}

/// `blackShroud_mc` holds one 512 x 512 bitmap (the vignette `HUDFX_I29`, black with the shroud
/// in its alpha) centred on the stage; the HUD layout (0x00B96250) sizes the clip to the visible
/// extent, so the shroud covers the whole screen: 1280 x 720 at 16:9, 1706.7 x 720 at 21:9
/// (sized to the stage only, it was a dark-edged box in the middle of an ultrawide screen),
/// 1280 x 1024 at 5:4. Drawn unscaled it was a dark square in the middle of the screen.
#[test]
fn the_stealth_shroud_covers_the_visible_extent() {
    let Some(m) = movie("DishonoredGame", "UI_HUDFX.HUDFX") else { return };
    assert!(m.shapes.values().any(|s| s.bounds.xmax == 10240.0 && matches!(s.fills.first(), Some(Fill::Bitmap { .. }))));
    for vp in [Vec2::new(1920.0, 1080.0), Vec2::new(2560.0, 1080.0), Vec2::new(1280.0, 1024.0)] {
        let mut st = stage(m.clone(), vp);
        let mut w = dishonored_runtime::hud_widgets::HudFxWidgets::new(&mut st);
        w.layout(&mut st);
        let space = st.space;
        let mc = st.child(st.root, "blackShroud_mc").expect("blackShroud_mc");
        let r = st.bounds_in_parent(mc).expect("bounds");
        assert!((r.min - (-space.empty)).abs().max_element() < 0.5 && (r.size() - space.size).abs().max_element() < 0.5, "{vp}: {r:?}");
    }
}

/// The HUD layout (0x00B96250) on the retail movie: at 16:9 the PC safe area (0.9) puts the
/// gauges at (64, 36) and the objectives at (1216, 36) (the movie is authored for 0.85: (95.5,
/// 54) / (1184, 54)); at 21:9 they move out by the empty space; the cinematic stripes span the
/// visible extent at every aspect.
#[test]
fn the_hud_layout_follows_the_visible_extent() {
    let Some(m) = movie("UI_HUD_SF", "HUD") else { return };
    for vp in [Vec2::new(1920.0, 1080.0), Vec2::new(2560.0, 1080.0), Vec2::new(1280.0, 1024.0)] {
        let mut st = stage(m.clone(), vp);
        let mut w = dishonored_runtime::hud_widgets::HudWidgets::new(&mut st);
        let space = st.space;
        w.layout(&mut st);
        let pos = |st: &Stage, n: &str| {
            let c = st.child(st.root, n).unwrap_or_else(|| panic!("{n}"));
            Vec2::new(st.get(c, Prop::X), st.get(c, Prop::Y))
        };
        let (lt, rb) = space.safe_area(0.9);
        assert!((pos(&st, "masterHUD_mc") - lt).abs().max_element() < 0.01, "{vp}");
        assert!((pos(&st, "objectives_mc") - Vec2::new(rb.x, lt.y)).abs().max_element() < 0.01, "{vp}");
        assert!((pos(&st, "playerStatesIcon_mc") - Vec2::new(lt.x, rb.y)).abs().max_element() < 0.01, "{vp}");
        assert!((pos(&st, "subtitles_mc") - Vec2::new(640.0, 650.0)).abs().max_element() < 0.01, "{vp}");
        if vp.x == 1920.0 {
            assert!((lt - Vec2::new(64.0, 36.0)).abs().max_element() < 0.01);
        }
        // The stripes: the visible extent's width, from its left edge; the upper one from its top,
        // the lower one to its bottom.
        let stripes = st.child(st.root, "blackStripes_mc").expect("blackStripes_mc");
        for (n, top, bottom) in [("_stripeUp_mc", -space.empty.y, 70.0), ("_stripeDown_mc", 650.0, 720.0 + space.empty.y)] {
            let c = st.child(stripes, n).expect(n);
            let r = st.bounds_in_parent(c).expect("bounds");
            assert!((r.min.x + space.empty.x).abs() < 0.5 && (r.width() - space.size.x).abs() < 0.5, "{vp} {n}: {r:?}");
            assert!((r.min.y - top).abs() < 0.5 && (r.max.y - bottom).abs() < 0.5, "{vp} {n}: {r:?}");
        }
    }
}

/// The confirmation box (`UI_Global` `gl_msgBox`): `boxShape_mc/boxPart2_mc` lays a solid
/// light shape (1088 x 132 px) on depth 2 as the mask of the edge particles above it (a
/// `clipDepth` layer, never drawn). The menus drew it: as a 132 px light square in the middle
/// of the box while the image quads kept the texture's aspect, as a light band over the box
/// once stretched; the flattener now keeps mask layers out and crops the masked depths.
#[test]
fn the_message_box_shape_is_a_mask() {
    let Some(m) = movie("DishonoredGame", "UI_Global.Global") else { return };
    let child = |parent: u16, name: &str| -> u16 {
        let s = &m.sprites[&parent];
        m.display_list(s, 0).into_iter().find(|p| p.name.as_deref() == Some(name)).and_then(|p| p.character).unwrap_or_else(|| panic!("{name}"))
    };
    let part = child(child(child(m.exports["gl_msgBox"], "_msgBox_mc"), "boxShape_mc"), "boxPart2_mc");
    let places = m.display_list(&m.sprites[&part], 0);
    let mask = places.iter().find(|p| p.clip_depth.is_some()).expect("a mask layer");
    let shape = &m.shapes[&mask.character.expect("mask character")];
    assert!(matches!(shape.fills.first(), Some(Fill::Solid(_))), "{:?}", shape.fills);
    assert!(places.iter().any(|p| p.depth > mask.depth && p.depth <= mask.clip_depth.unwrap()), "the mask clips the particles");
}