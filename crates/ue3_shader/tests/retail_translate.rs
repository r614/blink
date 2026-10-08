//! Every compiled shader of the retail shader caches translates to WGSL that naga accepts.
//! Skips without a Dishonored install.
use std::collections::BTreeMap;
use ue3_shader::{shadercache, sm3};

fn cooked() -> Option<std::path::PathBuf> {
    let dir = std::env::var("DISHONORED_DIR").unwrap_or_else(|_| r"C:\Program Files\GOG Galaxy\Games\Dishonored".into());
    let d = std::path::Path::new(&dir).join("DishonoredGame").join("CookedPCConsole");
    d.is_dir().then_some(d)
}

/// Translate and validate one shader; Err(stage) names the step that failed.
fn check(code: &[u8]) -> Result<(), (&'static str, String)> {
    let sh = sm3::parse(code).map_err(|e| ("parse", format!("{e:#}")))?;
    let (body, iface) = sm3::translate(&sh, "sm3_").map_err(|e| ("translate", format!("{e:#}")))?;
    let src = sm3::standalone_module(&sh, &body, &iface);
    sm3::validate(&src).map_err(|e| ("validate", e))?;
    Ok(())
}

fn tally(name: &str, codes: impl Iterator<Item = Vec<u8>>) {
    let mut total = 0usize;
    let mut fails: BTreeMap<&'static str, (usize, String)> = BTreeMap::new();
    for code in codes {
        total += 1;
        if let Err((stage, msg)) = check(&code) {
            let e = fails.entry(stage).or_insert((0, msg));
            e.0 += 1;
        }
    }
    let bad: usize = fails.values().map(|v| v.0).sum();
    eprintln!("{name}: {} of {total} shaders translate and validate", total - bad);
    for (stage, (n, first)) in &fails {
        eprintln!("  {n} fail at {stage}; first: {}", first.lines().take(6).collect::<Vec<_>>().join(" | "));
    }
    assert!(total > 0, "{name}: no shaders found");
    assert_eq!(bad, 0, "{name}: {bad} of {total} shaders fail");
}

#[test]
fn ref_shader_cache_translates() {
    let Some(dir) = cooked() else {
        eprintln!("no Dishonored install: skipped");
        return;
    };
    let pkg = ue3_package::Package::open(dir.join("RefShaderCache-PC-D3D-SM3.upk")).unwrap();
    let lib = shadercache::ShaderLibrary::load(&pkg).unwrap();
    tally("RefShaderCache", lib.records.keys().filter_map(|g| lib.code(g)).map(|c| c.to_vec()));
}

#[test]
fn global_shader_cache_translates() {
    let Some(dir) = cooked() else {
        eprintln!("no Dishonored install: skipped");
        return;
    };
    let d = std::fs::read(dir.join("GlobalShaderCache-PC-D3D-SM3.bin")).unwrap();
    let globals = shadercache::global_shaders(&d).unwrap();
    for g in &globals {
        if let Err(e) = sm3::parse(&g.code) {
            eprintln!("  global {} ({} bytes): {e:#}", g.name, g.code.len());
        }
    }
    // The two Bink video colour-conversion records are not SM3 bytecode as stored (one is a
    // 1-byte stub, the other spans the next record); videos are decoded elsewhere.
    tally("GlobalShaderCache", globals.into_iter().filter(|g| !g.name.starts_with("FBink")).map(|g| g.code));
}
