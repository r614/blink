//! Finds the tweak classes and objects whose properties mention a text (skipped without the
//! install). `DH_TWEAK_TEXT=m_FocusRegions cargo test -p dishonored_assets --test tweak_search_retail -- --ignored --nocapture`

use dishonored_assets::Content;
use dishonored_assets::tweaks::{TweakDb, dump};

#[test]
#[ignore]
fn search() {
    let Ok(c) = Content::locate() else { return };
    let text = std::env::var("DH_TWEAK_TEXT").unwrap_or_else(|_| "m_FocusRegions".into());
    let db = TweakDb::load(&c).unwrap();
    let mut classes = db.subclasses_of("DisTweaksBase");
    classes.push("DisTweaksBase".into());
    for class in &classes {
        let Ok(m) = db.class_defaults(class) else { continue };
        let mut s = String::new();
        dump(&m, 0, &mut s);
        if s.contains(&text) {
            println!("class {class}:");
            for l in s.lines().filter(|l| l.contains(&text)).take(4) {
                println!("    {l}");
            }
            for p in db.paths_with_class(class).iter().take(3) {
                let mut s = String::new();
                dump(&db.try_resolve(p).unwrap_or_default(), 2, &mut s);
                let pos = s.find(&text).unwrap_or(0);
                println!("  object {p}: {}", s[pos.saturating_sub(200)..(pos + 2500).min(s.len())].replace('\n', " | "));
            }
        }
    }
}
