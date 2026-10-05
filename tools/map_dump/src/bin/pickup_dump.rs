//! Print the pickups placed in a map: `pickup_dump [map]` (default L_Boyle_Ext_P).
use anyhow::Result;
use dishonored_assets::pickups::extract_pickups;
use dishonored_assets::{Content, map_packages};

fn main() -> Result<()> {
    let map = std::env::args().nth(1).unwrap_or("L_Boyle_Ext_P".into());
    let c = Content::locate()?;
    let mut pkgs = map_packages(&c, &map);
    if map.starts_with("L_Boyle_Ext") {
        pkgs.push("L_BoyleStreet_Ext_Script".into());
    }
    let v = extract_pickups(&c, &pkgs)?;
    let mut counts = std::collections::BTreeMap::new();
    for p in &v {
        let k = format!("{:?}", p.kind);
        let short: String = k.split([' ', '(', '{']).next().unwrap_or("").to_string();
        *counts.entry(short).or_insert(0) += 1;
        println!(
            "{:<34} {:<26} {:>7.0},{:>7.0},{:>7.0} {:?}{}",
            p.name,
            p.package,
            p.position.x,
            p.position.y,
            p.position.z,
            p.kind,
            p.stealable_from.as_ref().map(|s| format!(" carried by {s}")).unwrap_or_default()
        );
    }
    println!("{} pickups: {:?}", v.len(), counts);
    Ok(())
}
