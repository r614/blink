//! Open every package in a directory and report parse failures and class histograms.
//!
//! `package_scan <dir> [--class NAME] [--props] [--prefix L_Boyle]`
//! `--props` additionally parses the UObject header + tagged properties of every
//! non-script export and reports failures by class.
use anyhow::Result;
use std::collections::BTreeMap;
use ue3_package::{ObjRef, Package};

/// Script/metadata classes whose serialization is not "NetIndex + tagged properties".
const SCRIPT_CLASSES: &[&str] = &[
    "Class", "Function", "State", "ScriptStruct", "Struct", "Enum", "Const", "Package",
    "TextBuffer", "MetaData",
];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: package_scan <CookedPCConsole dir> [--class C] [--props] [--prefix P]");
    let opt = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let filter = opt("--class").map(|s| s.to_ascii_lowercase());
    let prefix = opt("--prefix").map(|s| s.to_ascii_lowercase());
    let check_props = args.iter().any(|a| a == "--props");
    let mut ok = 0;
    let mut failed = 0;
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut prop_ok = 0usize;
    let mut prop_fail: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("upk")))
        .filter(|p| {
            prefix.as_ref().is_none_or(|pre| {
                p.file_name().unwrap().to_string_lossy().to_ascii_lowercase().starts_with(pre)
            })
        })
        .collect();
    entries.sort();
    for p in entries {
        match Package::open(&p) {
            Ok(pkg) => {
                ok += 1;
                for i in 0..pkg.exports.len() {
                    let c = pkg.export_class(i);
                    if let Some(f) = &filter {
                        if c.to_ascii_lowercase() == *f {
                            println!("{} {}", pkg.name, pkg.obj_path(ObjRef::Export(i)));
                        }
                    }
                    if check_props && !SCRIPT_CLASSES.contains(&c.as_str()) && !c.ends_with("Property") {
                        match pkg.read_object(i) {
                            Ok(_) => prop_ok += 1,
                            Err(e) => {
                                let ent = prop_fail.entry(c.clone()).or_insert((0, String::new()));
                                if ent.0 == 0 {
                                    ent.1 = format!("{} export {i}: {e:#}", pkg.name);
                                }
                                ent.0 += 1;
                            }
                        }
                    }
                    *classes.entry(c).or_default() += 1;
                }
            }
            Err(e) => {
                failed += 1;
                println!("FAIL {}: {e:#}", p.display());
            }
        }
    }
    if filter.is_none() && !check_props {
        let mut v: Vec<_> = classes.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (c, n) in v.iter().take(80) {
            println!("{n:8} {c}");
        }
    }
    if check_props {
        let nfail: usize = prop_fail.values().map(|v| v.0).sum();
        for (c, (n, sample)) in &prop_fail {
            println!("prop FAIL {c} x{n}: {sample}");
        }
        println!("objects: properties ok={prop_ok} failed={nfail}");
    }
    println!("packages ok={ok} failed={failed}");
    Ok(())
}
