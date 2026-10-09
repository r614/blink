//! Camera lens effects of the retail maps' `DisSeqAct_SpawnCameraLensEffect` actions (skipped
//! without the install).

use dishonored_assets::Content;
use dishonored_assets::level::{ImportOptions, import_level};

#[test]
fn outsider_dream_and_lighthouse_lens_effects() {
    let Ok(c) = Content::locate() else { return };
    let opts = ImportOptions { load_lightmaps: false, max_texture_size: 64, ..Default::default() };
    for map in ["L_OutsiderDream_P", "L_Isl_HighChaos_P"] {
        let pkgs = dishonored_assets::map_packages(&c, map);
        let (m, _) = import_level(&c, map, &pkgs, &opts).unwrap();
        for l in &m.lens_effects {
            println!("{map}: {} -> {} distance {} base fov {} life {}", l.action, l.system, l.distance, l.base_fov, l.life_span);
            assert!(m.particle_systems.iter().any(|p| p.source.eq_ignore_ascii_case(&l.system) || p.source.to_ascii_lowercase().ends_with(&l.system.to_ascii_lowercase())), "{} decoded", l.system);
        }
        // The dream's action names no tweak, and the effect class's default system is unset:
        // nothing to draw.
        if map == "L_Isl_HighChaos_P" {
            assert!(m.lens_effects.len() >= 2, "{map}: the rain and the lightning");
        }
    }
}
