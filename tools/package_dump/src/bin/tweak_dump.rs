//! Single pass over every package: dump all `DisTweaks_*` objects (instances and class
//! defaults) with their properties. Arkane's tuning lives in these objects.
//!
//! `tweak_dump <CookedPCConsole dir> [filter-substring] > tweaks.txt`
//! Output is derived from retail data: keep it out of the repository.
use anyhow::Result;
use std::collections::HashSet;
use ue3_package::properties::{Property, Value, format_value};
use ue3_package::{ObjRef, Package};

fn print_props(pkg: &Package, props: &[Property], indent: usize) {
    for p in props {
        let pad = " ".repeat(indent);
        match &p.value {
            Value::Struct(n, inner) => {
                println!("{pad}{}[{}] : {n}", p.name, p.array_index);
                print_props(pkg, inner, indent + 2);
            }
            Value::Array(_) => {
                // Try struct elements first (most tweak arrays), then object refs.
                if let Ok(elems) = p.array_structs(pkg) {
                    if !elems.is_empty() && elems.iter().all(|e| !e.is_empty()) {
                        for (i, e) in elems.iter().enumerate() {
                            println!("{pad}{}({i}) :", p.name);
                            print_props(pkg, e, indent + 2);
                        }
                        continue;
                    }
                }
                println!("{pad}{}[{}] = {}", p.name, p.array_index, format_value(pkg, &p.value));
            }
            v => println!("{pad}{}[{}] = {}", p.name, p.array_index, format_value(pkg, v)),
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: tweak_dump <dir> [filter]");
    let filter = args.get(1).map(|s| s.to_ascii_lowercase());
    let mut seen = HashSet::new();
    let mut paths: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("upk")))
        .collect();
    paths.sort();
    for path in paths {
        let Ok(pkg) = Package::open(&path) else { continue };
        for i in 0..pkg.exports.len() {
            let class = pkg.export_class(i);
            if !class.starts_with("DisTweaks_") {
                continue;
            }
            let opath = pkg.obj_path(ObjRef::Export(i));
            if let Some(f) = &filter {
                if !opath.to_ascii_lowercase().contains(f) && !class.to_ascii_lowercase().contains(f) {
                    continue;
                }
            }
            if !seen.insert(opath.clone()) {
                continue;
            }
            match pkg.read_object(i) {
                Ok(h) => {
                    println!("=== {opath} ({class}) [{}]", pkg.name);
                    print_props(&pkg, &h.properties, 2);
                }
                Err(e) => println!("=== {opath} ({class}) [{}] ERROR {e:#}", pkg.name),
            }
        }
    }
    Ok(())
}
