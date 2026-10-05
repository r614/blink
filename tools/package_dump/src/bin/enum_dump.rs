//! `enum_dump <package.upk> <enum name substring>`: print script enums (`UEnum`) with their
//! values in order (research).
//!
//! Layout (v801): net index (i32), `None` property terminator (FName), `UField::Next`
//! (object ref), `TArray<FName>` values.
use anyhow::Result;
use ue3_package::reader::Reader;
use ue3_package::{ObjRef, Package};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let pkg = Package::open(&a[0])?;
    let filt = a.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "Enum" {
            continue;
        }
        let path = pkg.obj_path(ObjRef::Export(i));
        if !path.to_ascii_lowercase().contains(&filt) {
            continue;
        }
        let d = pkg.export_data(i)?;
        let mut r = Reader::new(d);
        r.skip(4 + 8 + 4)?;
        let n = r.count(8)?;
        let mut vals = Vec::new();
        for _ in 0..n {
            let idx = r.i32()?;
            let _num = r.i32()?;
            vals.push(pkg.names.get(idx as usize).map(|n| n.name.clone()).unwrap_or_else(|| format!("?{idx}")));
        }
        println!("{path}:");
        for (k, v) in vals.iter().enumerate() {
            println!("  {k} {v}");
        }
    }
    Ok(())
}
