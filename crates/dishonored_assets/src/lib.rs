//! Dishonored-specific decoding of UE3 objects into `dishonored_ir`.

pub mod characters;
pub mod ai_config;
pub mod content;
pub mod level;
pub mod lightmap;
pub mod loc;
pub mod material;
pub mod navigation;
pub mod npcs;
pub mod material_chain;
pub mod skeletal_mesh;
pub mod spline_mesh;
pub mod static_mesh;
pub mod swf;
pub mod swf_font;
pub mod edge_anim;
pub mod gameplay;
pub mod ini;
pub mod kismet;
pub mod localization;
pub mod travel;
pub mod texture;
pub mod tweaks;
pub mod util;

pub use content::Content;
pub use level::{ImportOptions, ImportStats, import_level};

/// Packages that make up a map, in load order: the persistent level then the sublevels
/// listed in its `WorldInfo.StreamingLevels` (`LevelStreaming*.PackageName`). Boyle Ext
/// streams `L_Boyle_Ext_{Audio,Env,Fx,Light,Nav,Script}` and `L_BoyleStreet_Ext_Script`,
/// which a prefix match would miss. Falls back to every package sharing the prefix.
pub fn map_packages(content: &Content, persistent: &str) -> Vec<String> {
    match streaming_levels(content, persistent) {
        Some(levels) if !levels.is_empty() => {
            let mut out = vec![persistent.to_string()];
            for l in levels {
                if content.has_package(&l) && !out.iter().any(|o| o.eq_ignore_ascii_case(&l)) {
                    out.push(l);
                }
            }
            out
        }
        _ => map_packages_by_prefix(content, persistent),
    }
}

/// `PackageName`s of the persistent level's `WorldInfo.StreamingLevels`.
pub fn streaming_levels(content: &Content, persistent: &str) -> Option<Vec<String>> {
    use ue3_package::ObjRef;
    use ue3_package::properties::{Value, find};
    let pkg = content.package(persistent).ok()?;
    let wi = (0..pkg.exports.len()).find(|&i| {
        pkg.export_class(i) == "WorldInfo" && pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) == "PersistentLevel"
    })?;
    let props = pkg.read_object(wi).ok()?.properties;
    let refs = find(&props, "StreamingLevels")?.array_objects().ok()?;
    let mut out = Vec::new();
    for r in refs {
        let ObjRef::Export(e) = ObjRef::from_index(r) else { continue };
        let Ok(h) = pkg.read_object(e) else { continue };
        if let Some(Value::Name(n)) = find(&h.properties, "PackageName").map(|p| &p.value) {
            out.push(n.clone());
        }
    }
    Some(out)
}

/// Every package sharing the persistent level's prefix (`L_Boyle_Ext_P` -> `L_Boyle_Ext_*`).
pub fn map_packages_by_prefix(content: &Content, persistent: &str) -> Vec<String> {
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
