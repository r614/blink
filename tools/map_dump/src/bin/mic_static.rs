//! `mic_static <package.upk> [name substring] [--hex]`: decode the static permutation parameters
//! (static switches, component masks) of every `MaterialInstanceConstant` in a package, and
//! report how many instances with `bHasStaticPermutationResource` decode. `--hex` prints the
//! tail of each export's native data (research; output is derived from retail data).
use anyhow::{Context, Result};
use dishonored_assets::material_chain::read_static_parameters;
use ue3_package::Package;
use ue3_package::properties::{Value, find};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let pkg = Package::open(a.first().context("package")?)?;
    let filter = a.get(1).filter(|s| !s.starts_with("--")).cloned().unwrap_or_default();
    let hex = a.iter().any(|s| s == "--hex");
    let (mut with, mut ok) = (0, 0);
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "MaterialInstanceConstant" {
            continue;
        }
        let path = pkg.obj_path(ue3_package::ObjRef::Export(i));
        if !path.contains(&filter) {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        let perm = matches!(find(&h.properties, "bHasStaticPermutationResource").map(|p| &p.value), Some(Value::Bool(true)));
        let data = pkg.export_data(i)?;
        if !perm {
            continue;
        }
        with += 1;
        let sp = read_static_parameters(&pkg, data, h.native_offset, 0);
        println!("{path}: native {} bytes", data.len() - h.native_offset);
        if hex {
            let from = h.native_offset.max(data.len().saturating_sub(512));
            for (k, c) in data[from..].chunks(32).enumerate() {
                println!("  {:6x}: {}", from + k * 32, c.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
            }
        }
        match sp {
            Some(sp) => {
                ok += 1;
                for (n, v, o) in &sp.switches {
                    println!("  switch {n} = {v}{}", if *o { "" } else { " (not overridden)" });
                }
                for (n, m, o) in &sp.masks {
                    println!("  mask {n} = {}{}", ["R", "G", "B", "A"].iter().zip(m).filter(|(_, b)| **b).map(|(c, _)| *c).collect::<String>(), if *o { "" } else { " (not overridden)" });
                }
            }
            None => println!("  static parameters: not found"),
        }
    }
    println!("{ok}/{with} instances with a static permutation decoded");
    Ok(())
}
