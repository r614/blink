//! Open every package in a directory and report parse failures and class histograms.
use anyhow::Result;
use std::collections::BTreeMap;
use ue3_package::Package;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: package_scan <CookedPCConsole dir> [class-filter]");
    let filter = args.get(1).map(|s| s.to_ascii_lowercase());
    let mut ok = 0;
    let mut failed = 0;
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("upk")))
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
                            println!("{} {}", pkg.name, pkg.obj_path(ue3_package::ObjRef::Export(i)));
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
    if filter.is_none() {
        let mut v: Vec<_> = classes.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (c, n) in v.iter().take(80) {
            println!("{n:8} {c}");
        }
    }
    println!("packages ok={ok} failed={failed}");
    Ok(())
}
