//! Prints resolved tweak objects (skipped without the install).
//! `DH_TWEAKS=Streets_Twk.Projectile.Twk_Proj_ScriptedBolt cargo test -p dishonored_assets --test tweak_dump_retail -- --ignored --nocapture`

use dishonored_assets::Content;
use dishonored_assets::tweaks::{TweakDb, dump};

#[test]
#[ignore]
fn print() {
    let Ok(c) = Content::locate() else { return };
    let paths = std::env::var("DH_TWEAKS").unwrap_or_default();
    let pkgs: Vec<String> = std::env::var("DH_TWEAK_PACKAGES").unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
    let db = TweakDb::load_with(&c, &pkgs).unwrap();
    let mut paths: Vec<String> = paths.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
    // `DH_TWEAK_CLASS`: every object of a tweak class.
    if let Ok(class) = std::env::var("DH_TWEAK_CLASS") {
        paths.extend(db.paths_with_class(&class));
    }
    for p in &paths {
        let p = p.as_str();
        let mut s = String::new();
        dump(&db.try_resolve(p).unwrap_or_default(), 2, &mut s);
        println!("--- {p} ({:?})\n{s}", db.class_of(p));
    }
}
