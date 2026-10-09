//! Finds the tweak objects whose properties mention a text (skipped without the install).
//! `DH_TWEAK_TEXT=Twk_Inv_AssassinHand DH_TWEAK_PACKAGES=L_Flooded_FIntro_Script cargo test -p dishonored_assets --test tweak_search_retail -- --ignored --nocapture`

use dishonored_assets::Content;
use dishonored_assets::tweaks::{TweakDb, dump};

#[test]
#[ignore]
fn search() {
    let Ok(c) = Content::locate() else { return };
    let text = std::env::var("DH_TWEAK_TEXT").unwrap_or_else(|_| "m_FocusRegions".into());
    let pkgs: Vec<String> = std::env::var("DH_TWEAK_PACKAGES").unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
    let db = TweakDb::load_with(&c, &pkgs).unwrap();
    let mut classes = db.subclasses_of("DisTweaksBase");
    classes.push("DisTweaksBase".into());
    for class in &classes {
        for p in db.paths_with_class(class) {
            let mut s = String::new();
            dump(&db.try_resolve(&p).unwrap_or_default(), 0, &mut s);
            if let Some(pos) = s.find(&text) {
                let line_start = s[..pos].rfind('\n').map_or(0, |i| i + 1);
                let line_end = s[pos..].find('\n').map_or(s.len(), |i| pos + i);
                println!("{class} {p}: {}", s[line_start..line_end].trim());
            }
        }
    }
}
