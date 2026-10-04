//! Find which cooked packages export given objects (cross-package import resolution).
//!
//! `find_export <CookedPCConsole dir> <obj.path> [<obj.path> ...] [--suffix]`
//! Paths match case-insensitively against the full export path; with `--suffix` a path
//! matches any export path ending in it (useful for `Package.Group.Name` imports whose
//! outermost package was folded into another file by the cooker).
use anyhow::Result;
use ue3_package::{ObjRef, Package};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: find_export <dir> <path>... [--suffix]");
    let suffix = args.iter().any(|a| a == "--suffix");
    let wanted: Vec<String> = args[1..].iter().filter(|a| !a.starts_with("--")).map(|s| s.to_ascii_lowercase()).collect();
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("upk")))
        .collect();
    entries.sort();
    for path in entries {
        let Ok(pkg) = Package::open(&path) else { continue };
        for i in 0..pkg.exports.len() {
            let p = pkg.obj_path(ObjRef::Export(i)).to_ascii_lowercase();
            if wanted.iter().any(|w| if suffix { p.ends_with(w.as_str()) } else { p == *w }) {
                println!("{}\t{}\t{}", pkg.name, pkg.obj_path(ObjRef::Export(i)), pkg.export_class(i));
            }
        }
    }
    Ok(())
}
