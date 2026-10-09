//! Placed movables (`DishonoredMovable`) against the canonical GOG build (skipped without the
//! install): their `DisTweaks_Movable`, weight class and mesh box.

use dishonored_assets::Content;
use dishonored_assets::world_actors::extract_world_actors;

#[test]
fn boyle_interior_movables() {
    let Ok(c) = Content::locate() else { return };
    let pkgs = vec!["L_Boyle_Int_Script".to_string()];
    if !c.has_package(&pkgs[0]) {
        return;
    }
    let wa = extract_world_actors(&c, &pkgs, &[0.0; 20]).unwrap();
    assert!(!wa.movables.is_empty(), "Boyle Int places movables");
    if std::env::var("DH_PRINT_MOVABLES").is_ok() {
        let db = dishonored_assets::tweaks::TweakDb::load_with(&c, &pkgs).unwrap();
        for t in ["movable_twk.Base.LargeMoveableBase", "movable_twk.Base.MoveableBase", "movable_twk.candle_stick_01_twk"] {
            let mut s = String::new();
            dishonored_assets::tweaks::dump(&db.try_resolve(t).unwrap_or_default(), 2, &mut s);
            println!("--- {t}\n{s}");
        }
        for m in &wa.movables {
            println!("{} at {:?} rot {:?}", m.name, m.position, m.rotation);
            println!("{} {} w={} dmg={} inter={} key={} dist={} speeds={}/{}/{} contact={} mesh={} box={:?}..{:?} brk={:?} tweak={}", m.name, m.class, m.weight, m.damage, m.interactable, m.key_item, m.max_distance, m.min_speed_to_cause_damage, m.min_speed_to_be_damaged, m.min_speed_to_be_damaged_on_drop, m.contact_type, m.mesh, m.bounds_min, m.bounds_max, m.breakable, m.tweak);
        }
    }
    // The bar's bottles are breakable movables with a mesh box.
    let bottle = wa.movables.iter().find(|m| m.name == "DishonoredMovable_1").expect("DishonoredMovable_1");
    assert!(bottle.breakable.is_some());
    assert!(bottle.bounds_max.x > bottle.bounds_min.x, "mesh box read");
}
