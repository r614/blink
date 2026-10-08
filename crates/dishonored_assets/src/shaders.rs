// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
//! The install's compiled shaders (`ue3_shader`): the reference shader cache's material shader
//! maps and bytecode, and Arkane's global post-processing shaders, loaded once per process. The
//! importer links materials to their maps (`level`'s `ue3_material`); the runtime builds and
//! renders the programs from the same library, so the map indices agree.
//!
//! `DISHONORED_LEGACY_MATERIALS=1` turns the original-shader path off (no linking, no library).

use std::sync::{Arc, OnceLock};
use ue3_shader::shadercache::{self, CookedLibrary, ShaderLibrary};

/// The library in the two shapes its users need: the shader cache's index (its maps' static
/// parameter sets, for `ShaderLibrary::find`; the bytecode itself is dropped) and the cooked
/// form the program builder reads (maps, bytecode by GUID, the `FArkPp*` global shaders).
pub struct Shaders {
    pub index: ShaderLibrary,
    pub cooked: Arc<CookedLibrary>,
}

static LIBRARY: OnceLock<Option<Shaders>> = OnceLock::new();

/// Whether the original-shader path is on (`DISHONORED_LEGACY_MATERIALS=1` turns it off).
pub fn enabled() -> bool {
    std::env::var("DISHONORED_LEGACY_MATERIALS").map(|v| v == "0" || v.is_empty()).unwrap_or(true)
}

/// The install's shader library, read on first use (None: the path is off, or the shader
/// cache is missing or unreadable).
pub fn library() -> Option<&'static Shaders> {
    if !enabled() {
        return None;
    }
    LIBRARY.get_or_init(load).as_ref()
}

fn load() -> Option<Shaders> {
    let content = crate::Content::locate().ok()?;
    let pkg = match content.package("RefShaderCache-PC-D3D-SM3") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("shader cache: {e:#}");
            return None;
        }
    };
    let mut index = match ShaderLibrary::load(&pkg) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("shader cache unreadable: {e:#}");
            return None;
        }
    };
    let mut cooked = CookedLibrary::from_library(&index);
    // Arkane's post-processing shaders from the global shader cache (dishonored-bevy's
    // `cook_shader_library`).
    match std::fs::read(content.cooked.join("GlobalShaderCache-PC-D3D-SM3.bin")).map_err(anyhow::Error::from).and_then(|d| shadercache::global_shaders(&d)) {
        Ok(globals) => {
            for g in globals.into_iter().filter(|g| g.name.starts_with("FArkPp") || g.name.starts_with("FKuwa")) {
                cooked.globals.insert(g.name, g.code);
            }
        }
        Err(e) => eprintln!("global shader cache: {e:#}"),
    }
    // The index keeps only what `find` reads.
    index.data = Vec::new();
    index.records.clear();
    Some(Shaders { index, cooked: Arc::new(cooked) })
}
