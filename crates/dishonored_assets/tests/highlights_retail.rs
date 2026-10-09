//! The highlights of the retail maps' `DisSeqAct_Highlight` targets (skipped without the
//! install): a usable (skeletal mesh), pickups and a key (static meshes), all with the
//! interactable tweak's `vfx_interactivity.interactivity_INST`.

use dishonored_assets::Content;
use dishonored_assets::level::{ImportOptions, import_level};

#[test]
fn highlight_targets_and_their_material() {
    let Ok(c) = Content::locate() else { return };
    let opts = ImportOptions { load_lightmaps: false, max_texture_size: 64, ..Default::default() };
    for (map, actor, statics) in [("L_Brothel_P", "L_Brothel_Script:DishonoredUsableObject_3", false), ("L_Flooded_FStreets_P", "L_Flooded_FStreets_Script:DishonoredInventoryPickup_10", true), ("L_Flooded_FAssassins_P", "L_Flooded_FAssassins_Script:DisKey_Base_1", true)] {
        let pkgs = dishonored_assets::map_packages(&c, map);
        let (m, _) = import_level(&c, map, &pkgs, &opts).unwrap();
        for h in &m.highlights {
            println!("{map}: {} allowed {} {} -> {:?} instances {:?}", h.actor, h.allowed, h.material_path, h.material, h.instances);
        }
        let name = actor.rsplit(':').next().unwrap().to_ascii_lowercase();
        for i in m.instances.iter().filter(|i| i.source.to_ascii_lowercase().contains(&name)) {
            println!("instance {} level {} mesh {} at {:?}", i.source, i.level, i.mesh, i.transform.w_axis);
        }
        let h = m.highlights.iter().find(|h| h.actor.eq_ignore_ascii_case(actor)).unwrap_or_else(|| panic!("{map}: {actor}"));
        assert!(h.allowed);
        assert!(h.material_path.eq_ignore_ascii_case("vfx_interactivity.interactivity_INST"), "{}", h.material_path);
        let p = &m.material_params[h.material.expect("decoded")];
        println!("{:?} {:?} {:?}", p.base_material, p.scalars, p.vectors);
        assert!(p.base_material.as_deref().is_some_and(|b| b.ends_with("golden_focus_PMAT")));
        assert_eq!(!h.instances.is_empty(), statics, "{actor}: static mesh components");
    }
}
