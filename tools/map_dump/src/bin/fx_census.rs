//! `fx_census <out dir> [package dir]`: deterministic census of the install's animation and
//! particle data for the animation / particle audit (research; the output quotes retail names
//! and goes to harness_out/, not to git). Every object is counted once by its path.
//!
//! - `anim_sequences.tsv`: package, AnimSet, sequence, length, rate, tagged properties, and each
//!   notify as `class@time+duration{fields}`;
//! - `anim_notify_classes.tsv`: notify class, uses, sequences, field names seen;
//! - `anim_nodes.tsv`: AnimTree node / skeletal control / morph node classes with their uses and
//!   the fields set on them;
//! - `particle_systems.tsv`: package, system, emitter, LOD 0 modules (class{fields});
//! - `particle_modules.tsv`: module class, uses, systems, field -> uses;
//! - `particle_refs.tsv`: every property (nested structs and object arrays included) of any
//!   object that names a ParticleSystem: holder class, holder, property path, system.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use ue3_package::properties::{Property, Value, find, format_value};
use ue3_package::{ObjRef, Package};

#[derive(Default)]
struct ClassUse {
    uses: usize,
    owners: BTreeSet<String>,
    fields: BTreeMap<String, usize>,
}

fn short(pkg: &Package, v: &Value) -> String {
    let s = format_value(pkg, v);
    if s.len() > 80 { format!("{}..", &s[..s.char_indices().nth(78).map_or(s.len(), |(i, _)| i)]) } else { s }
}

fn is_node_class(c: &str) -> bool {
    c == "AnimTree" || c.contains("AnimNode") || c.contains("SkelControl") || c.starts_with("MorphNode") || c.contains("AnimTree")
}

/// Object references in a property value: direct, inside structs, and object arrays.
fn refs(pkg: &Package, p: &Property, path: &str, out: &mut Vec<(String, i32)>) {
    let here = if p.array_index > 0 { format!("{path}{}[{}]", p.name, p.array_index) } else { format!("{path}{}", p.name) };
    match &p.value {
        Value::Object(o) if *o != 0 => out.push((here, *o)),
        Value::Struct(_, inner) => {
            for q in inner {
                refs(pkg, q, &format!("{here}."), out);
            }
        }
        Value::Array(b) => {
            // Object arrays: 4-byte count then that many valid indices filling the payload.
            let n = b.get(..4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])).unwrap_or(-1);
            let valid = |i: i32| (i > 0 && (i as usize) <= pkg.exports.len()) || (i < 0 && ((-i) as usize) <= pkg.imports.len());
            if n >= 0 && b.len() == 4 + 4 * n as usize {
                if let Ok(objs) = p.array_objects() {
                    if objs.iter().all(|i| *i == 0 || valid(*i)) {
                        for (k, o) in objs.iter().enumerate() {
                            if *o != 0 {
                                out.push((format!("{here}[{k}]"), *o));
                            }
                        }
                        return;
                    }
                }
            }
            if let Ok(elems) = p.array_structs(pkg) {
                for (k, e) in elems.iter().enumerate() {
                    for q in e {
                        refs(pkg, q, &format!("{here}[{k}]."), out);
                    }
                }
            }
        }
        _ => {}
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().map(String::as_str).unwrap_or("harness_out/audit/fx"));
    let dir = args.get(1).cloned().unwrap_or(r"C:\Program Files\GOG Galaxy\Games\Dishonored\DishonoredGame\CookedPCConsole".into());
    std::fs::create_dir_all(&out)?;
    let mut paths: Vec<_> = std::fs::read_dir(&dir)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("upk"))).collect();
    paths.sort();

    let mut seen: HashSet<String> = HashSet::new();
    let (mut seqs, mut systems, mut prefs) = (String::new(), String::new(), String::new());
    let mut notify_classes: BTreeMap<String, ClassUse> = BTreeMap::new();
    let mut node_classes: BTreeMap<String, ClassUse> = BTreeMap::new();
    let mut module_classes: BTreeMap<String, ClassUse> = BTreeMap::new();
    let mut seen_ref: HashSet<(String, String, String)> = HashSet::new();

    for p in &paths {
        let Ok(pkg) = Package::open(p) else { continue };
        let pname = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        for i in 0..pkg.exports.len() {
            let class = pkg.export_class(i);
            let path = pkg.obj_path(ObjRef::Export(i));
            if path.starts_with("Default__") || path.contains(".Default__") {
                continue;
            }
            // Particle references from any object.
            let Ok(h) = pkg.read_object(i) else { continue };
            let mut rs = Vec::new();
            for q in &h.properties {
                refs(&pkg, q, "", &mut rs);
            }
            for (ppath, o) in rs {
                let r = ObjRef::from_index(o);
                if pkg.class_name(r) != "ParticleSystem" {
                    continue;
                }
                let target = pkg.obj_path(r);
                if seen_ref.insert((path.clone(), ppath.clone(), target.clone())) {
                    let _ = writeln!(prefs, "{pname}\t{class}\t{path}\t{ppath}\t{target}");
                }
            }
            if !seen.insert(format!("{class}:{path}")) {
                continue;
            }
            match class.as_str() {
                "AnimSequence" => {
                    let Some(name) = find(&h.properties, "SequenceName").map(|q| format_value(&pkg, &q.value)) else { continue };
                    let set = pkg.obj_path(pkg.obj_outer(ObjRef::Export(i)));
                    let len = find(&h.properties, "SequenceLength").and_then(|q| q.as_float()).unwrap_or(0.0);
                    let rate = find(&h.properties, "RateScale").and_then(|q| q.as_float()).unwrap_or(1.0);
                    let tagged: Vec<String> = h
                        .properties
                        .iter()
                        .filter(|q| !matches!(q.name.as_str(), "SequenceName" | "SequenceLength" | "RateScale" | "Notifies" | "NumFrames" | "CompressedTrackOffsets" | "TrackToSkeletonMapTable" | "KeyEncodingFormat" | "RotationCompressionFormat" | "TranslationCompressionFormat" | "CompressionScheme" | "CompressedByteStream"))
                        .map(|q| format!("{}={}", q.name, short(&pkg, &q.value)))
                        .collect();
                    let mut ns = Vec::new();
                    if let Some(n) = find(&h.properties, "Notifies") {
                        for ev in n.array_structs(&pkg).unwrap_or_default() {
                            let t = find(&ev, "Time").and_then(|q| q.as_float()).unwrap_or(0.0);
                            let d = find(&ev, "Duration").and_then(|q| q.as_float()).unwrap_or(0.0);
                            let obj = find(&ev, "Notify").and_then(|q| q.as_object()).map(ObjRef::from_index);
                            let (nc, fields) = match obj {
                                Some(r @ ObjRef::Export(ni)) => {
                                    let props = pkg.read_object(ni).map(|h| h.properties).unwrap_or_default();
                                    let f: Vec<String> = props.iter().map(|q| format!("{}={}", q.name, short(&pkg, &q.value))).collect();
                                    let e = notify_classes.entry(pkg.class_name(r)).or_default();
                                    e.uses += 1;
                                    e.owners.insert(format!("{set}.{name}"));
                                    for q in &props {
                                        *e.fields.entry(q.name.clone()).or_default() += 1;
                                    }
                                    (pkg.class_name(r), f.join(","))
                                }
                                _ => ("None".into(), String::new()),
                            };
                            ns.push(format!("{nc}@{t:.3}+{d:.3}{{{fields}}}"));
                        }
                    }
                    let _ = writeln!(seqs, "{pname}\t{set}\t{name}\t{len:.3}\t{rate}\t{}\t{}", tagged.join(","), ns.join(" "));
                }
                "ParticleSystem" => {
                    let Some(em) = find(&h.properties, "Emitters").and_then(|q| q.array_objects().ok()) else { continue };
                    for (k, e) in em.iter().enumerate() {
                        let ObjRef::Export(ei) = ObjRef::from_index(*e) else { continue };
                        let Ok(eh) = pkg.read_object(ei) else { continue };
                        let ename = find(&eh.properties, "EmitterName").map(|q| format_value(&pkg, &q.value)).unwrap_or_default();
                        let Some(lods) = find(&eh.properties, "LODLevels").and_then(|q| q.array_objects().ok()) else { continue };
                        let Some(ObjRef::Export(li)) = lods.first().map(|l| ObjRef::from_index(*l)) else { continue };
                        let Ok(lh) = pkg.read_object(li) else { continue };
                        let mut mods: Vec<i32> = Vec::new();
                        for key in ["RequiredModule", "SpawnModule", "TypeDataModule"] {
                            if let Some(o) = find(&lh.properties, key).and_then(|q| q.as_object()) {
                                mods.push(o);
                            }
                        }
                        if let Some(ms) = find(&lh.properties, "Modules").and_then(|q| q.array_objects().ok()) {
                            mods.extend(ms);
                        }
                        let mut line = Vec::new();
                        for m in mods {
                            let r = ObjRef::from_index(m);
                            let ObjRef::Export(mi) = r else { continue };
                            let mc = pkg.class_name(r);
                            let props = pkg.read_object(mi).map(|h| h.properties).unwrap_or_default();
                            let u = module_classes.entry(mc.clone()).or_default();
                            u.uses += 1;
                            u.owners.insert(path.clone());
                            for q in &props {
                                *u.fields.entry(q.name.clone()).or_default() += 1;
                            }
                            line.push(format!("{mc}{{{}}}", props.iter().map(|q| q.name.as_str()).collect::<Vec<_>>().join(",")));
                        }
                        let ec = pkg.class_name(ObjRef::from_index(*e));
                        let _ = writeln!(systems, "{pname}\t{path}\t{k}:{ec}:{ename}\t{}", line.join(" "));
                    }
                }
                c if is_node_class(c) => {
                    let u = node_classes.entry(class.clone()).or_default();
                    u.uses += 1;
                    u.owners.insert(pkg.obj_path(pkg.obj_outer(ObjRef::Export(i))));
                    for q in &h.properties {
                        *u.fields.entry(q.name.clone()).or_default() += 1;
                    }
                }
                _ => {}
            }
        }
    }

    let table = |m: &BTreeMap<String, ClassUse>| {
        let mut rows: Vec<_> = m.iter().collect();
        rows.sort_by(|a, b| b.1.uses.cmp(&a.1.uses).then(a.0.cmp(b.0)));
        let mut s = String::new();
        for (c, u) in rows {
            let ex: Vec<&String> = u.owners.iter().take(3).collect();
            let fields: Vec<String> = u.fields.iter().map(|(f, n)| format!("{f}:{n}")).collect();
            let _ = writeln!(s, "{c}\t{}\t{}\t{}\t{}", u.uses, u.owners.len(), fields.join(","), ex.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(" | "));
        }
        s
    };
    std::fs::write(out.join("anim_sequences.tsv"), seqs)?;
    std::fs::write(out.join("anim_notify_classes.tsv"), table(&notify_classes))?;
    std::fs::write(out.join("anim_nodes.tsv"), table(&node_classes))?;
    std::fs::write(out.join("particle_systems.tsv"), systems)?;
    std::fs::write(out.join("particle_modules.tsv"), table(&module_classes))?;
    std::fs::write(out.join("particle_refs.tsv"), prefs)?;
    println!("{} packages; {} notify classes, {} node classes, {} module classes", paths.len(), notify_classes.len(), node_classes.len(), module_classes.len());
    Ok(())
}
