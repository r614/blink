//! Resolve and print a tweak object: `tweaks <object path> [filter]` or `tweaks --class C`.
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::tweaks::{TweakDb, dump};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let content = Content::locate()?;
    let db = TweakDb::load(&content)?;
    if args.first().map(String::as_str) == Some("--class") {
        for p in db.paths_with_class(&args[1]) {
            println!("{p}");
        }
        return Ok(());
    }
    let path = args.first().expect("usage: tweaks <path> [filter] | --class C");
    let m = db.resolve(path)?;
    let mut s = String::new();
    dump(&m, 0, &mut s);
    let filter = args.get(1).map(|f| f.to_ascii_lowercase());
    for line in s.lines() {
        if filter.as_ref().is_none_or(|f| line.to_ascii_lowercase().contains(f)) {
            println!("{line}");
        }
    }
    Ok(())
}
