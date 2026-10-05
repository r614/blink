//! `obj_dump <package.upk> <class-or-path substring> [max]`: print the tagged properties of
//! matching exports (research; output is derived from retail data, keep it out of the repo).
use anyhow::Result;
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
    let a: Vec<String> = std::env::args().skip(1).collect();
    let pkg = Package::open(&a[0])?;
    let filt = a[1].to_ascii_lowercase();
    let max: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let mut n = 0;
    for i in 0..pkg.exports.len() {
        let class = pkg.export_class(i);
        let path = pkg.obj_path(ObjRef::Export(i));
        if !class.to_ascii_lowercase().contains(&filt) && !path.to_ascii_lowercase().contains(&filt) {
            continue;
        }
        match pkg.read_object(i) {
            Ok(h) => {
                println!("=== [{i}] {path} ({class})");
                print_props(&pkg, &h.properties, 2);
            }
            Err(e) => println!("=== [{i}] {path} ({class}) ERROR {e:#}"),
        }
        n += 1;
        if n >= max {
            break;
        }
    }
    Ok(())
}
