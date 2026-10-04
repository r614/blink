//! Dishonored-specific decoding of UE3 objects into `dishonored_ir`.

pub mod characters;
pub mod content;
pub mod level;
pub mod lightmap;
pub mod material;
pub mod skeletal_mesh;
pub mod static_mesh;
pub mod gameplay;
pub mod ini;
pub mod texture;
pub mod tweaks;
pub mod util;

pub use content::Content;
pub use level::{ImportOptions, ImportStats, import_level};

/// Packages that make up a map, in load order: the persistent level then every
/// sublevel sharing its prefix (`L_Boyle_Ext_P` -> `L_Boyle_Ext_*`).
pub fn map_packages(content: &Content, persistent: &str) -> Vec<String> {
    let prefix = persistent.strip_suffix("_P").unwrap_or(persistent).to_ascii_lowercase();
    let mut out = vec![persistent.to_string()];
    let mut rest: Vec<String> = std::fs::read_dir(&content.cooked)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if !p.extension()?.eq_ignore_ascii_case("upk") {
                return None;
            }
            Some(p.file_stem()?.to_string_lossy().into_owned())
        })
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            l.starts_with(&format!("{prefix}_")) && l != persistent.to_ascii_lowercase()
        })
        .collect();
    rest.sort();
    out.extend(rest);
    out
}
