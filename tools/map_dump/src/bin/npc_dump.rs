//! Print original NPC placement data for a map.
use anyhow::Result;
use dishonored_assets::{Content, map_packages};
use dishonored_assets::npcs::extract_npc_data;

fn main() -> Result<()> {
    let map = std::env::args().nth(1).unwrap_or("L_Boyle_Ext_P".into());
    let c = Content::locate()?;
    let mut pkgs = map_packages(&c, &map);
    if map.starts_with("L_Boyle_Ext") { pkgs.push("L_BoyleStreet_Ext_Script".into()); }
    let d = extract_npc_data(&c, &pkgs)?;
    for s in &d.spawns { println!("spawn {:?} {:.0},{:.0},{:.0} yaw {:.0} {}", s.role, s.position.x, s.position.y, s.position.z, s.yaw, s.tweak); }
    let mut kinds = std::collections::BTreeMap::new();
    for p in &d.points { *kinds.entry(format!("{:?}", p.kind)).or_insert(0) += 1; }
    println!("{} spawns, points {:?}", d.spawns.len(), kinds);
    Ok(())
}
