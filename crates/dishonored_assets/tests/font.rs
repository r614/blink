//! The loading screen's UE3 bitmap fonts decode (needs the retail install; skipped without it).
use dishonored_assets::Content;
use dishonored_assets::font::load_font;

#[test]
fn loading_fonts_decode() {
    let Ok(c) = Content::locate() else {
        eprintln!("install not found: skipped");
        return;
    };
    eprintln!("checking loading fonts");
    for name in ["UI_Loading.Emerge_BF", "UI_Loading.ChaletComprime-CologneEighty"] {
        let f = load_font(&c, "UI_Loading_SF_LOC_INT", name).unwrap();
        assert!(!f.pages.is_empty(), "{name}: no pages");
        let w = f.width("LOADING");
        assert!(w > 50, "{name}: width {w}");
        for ch in "THE BOYLE ESTATE".chars().filter(|c| *c != ' ') {
            let g = f.glyph(ch).unwrap_or_else(|| panic!("{name}: no glyph {ch}"));
            assert!(g.w > 0 && g.h > 0 && (g.page as usize) < f.pages.len());
            assert!(g.u + g.w <= f.pages[g.page as usize].width as i32 && g.v + g.h <= f.pages[g.page as usize].height as i32);
        }
    }
}
