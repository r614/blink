//! Count AnimSequence compression formats across a directory of packages.
use std::collections::BTreeMap;
use ue3_package::properties::{Value, find};
use ue3_package::{ObjRef, Package};

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let mut fmts: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, String> = BTreeMap::new();
    let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("upk"))).collect();
    paths.sort();
    for p in paths {
        let Ok(pkg) = Package::open(&p) else { continue };
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "AnimSequence" { continue; }
            let Ok(h) = pkg.read_object(i) else { continue };
            let g = |n: &str| match find(&h.properties, n).map(|p| &p.value) { Some(Value::Enum(s)) => s.clone(), _ => "default".into() };
            let key = format!("rot={} trans={} keys={}", g("RotationCompressionFormat"), g("TranslationCompressionFormat"), g("KeyEncodingFormat"));
            *fmts.entry(key.clone()).or_default() += 1;
            examples.entry(key).or_insert_with(|| format!("{} {}", pkg.name, pkg.obj_path(ObjRef::Export(i))));
        }
    }
    for (k, n) in &fmts { println!("{n:8} {k}   e.g. {}", examples[k]); }
}
