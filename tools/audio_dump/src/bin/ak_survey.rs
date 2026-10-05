//! Research: AkAudio objects referenced by the game packages (AkEvent/AkBank...),
//! and whether `fnv1_32(lowercase name)` matches the event ids in the banks.
//!
//! ak_survey [--props N]   (prints the properties of the first N AkEvent exports)
use anyhow::Result;
use dishonored_audio::bank::Bank;
use dishonored_audio::pck::Pck;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use ue3_package::properties::format_value;
use ue3_package::{ObjRef, Package};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let nprops: usize = args.iter().position(|a| a == "--props").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(5);
    let root = std::env::var_os("DISHONORED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored"));
    let cooked = root.join("DishonoredGame/CookedPCConsole");

    // Event ids per bank.
    let mut event_bank: HashMap<u32, String> = HashMap::new();
    for e in std::fs::read_dir(&cooked)?.flatten() {
        let p = e.path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pck")) {
            continue;
        }
        let pck = Pck::open(&p)?;
        for be in &pck.banks {
            let b = Bank::parse(&pck.read(be)?)?;
            for o in b.hirc.iter().filter(|o| o.kind == 4) {
                event_bank.insert(o.id, p.file_stem().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    println!("events in banks: {}", event_bank.len());

    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut names: HashSet<String> = HashSet::new();
    let mut shown = 0;
    let mut table: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    let mut entries: Vec<_> = std::fs::read_dir(&cooked)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("upk"))).collect();
    entries.sort();
    for path in entries {
        let Ok(pkg) = Package::open(&path) else { continue };
        for i in 0..pkg.exports.len() {
            let c = pkg.export_class(i);
            if !c.starts_with("Ak") {
                continue;
            }
            *classes.entry(c.clone()).or_default() += 1;
            if c == "AkEvent" {
                let n = pkg.obj_name(ObjRef::Export(i));
                if shown < nprops {
                    shown += 1;
                    println!("{} {} :", pkg.name, pkg.obj_path(ObjRef::Export(i)));
                    if let Ok(h) = pkg.read_object(i) {
                        for p in &h.properties {
                            println!("    {} = {}", p.name, format_value(&pkg, &p.value));
                        }
                        let d = pkg.export_data(i)?;
                        println!("    native {:02x?}", &d[h.native_offset..d.len().min(h.native_offset + 32)]);
                    }
                }
                let bank = pkg
                    .read_object(i)
                    .ok()
                    .and_then(|h| h.properties.iter().find(|p| p.name == "RequiredBank").and_then(|p| p.as_object()))
                    .map(|o| pkg.obj_name(ObjRef::from_index(o)))
                    .unwrap_or_default();
                table.entry(n.clone()).or_insert_with(|| (bank, Vec::new())).1.push(pkg.name.clone());
                names.insert(n);
            }
        }
    }
    println!("Ak classes: {classes:?}");
    std::fs::create_dir_all("harness_out/audio")?;
    let mut tsv = String::from("event\tid\tbank\tin_bank\tpackages\n");
    for (n, (bank, pkgs)) in &table {
        let id = dishonored_audio::fnv1_32(n);
        let mut p = pkgs.clone();
        p.sort();
        p.dedup();
        tsv += &format!(
            "{n}\t{id}\t{bank}\t{}\t{}\n",
            event_bank.get(&id).map(String::as_str).unwrap_or("-"),
            p.iter().take(6).cloned().collect::<Vec<_>>().join(",")
        );
    }
    std::fs::write("harness_out/audio/ak_events.tsv", tsv)?;
    let matched = names.iter().filter(|n| event_bank.contains_key(&dishonored_audio::fnv1_32(n))).count();
    println!("distinct AkEvent names {} ; fnv1_32(lowercase) matches a bank event: {matched}", names.len());
    let mut unmatched: Vec<_> = names.iter().filter(|n| !event_bank.contains_key(&dishonored_audio::fnv1_32(n))).collect();
    unmatched.sort();
    println!("unmatched sample: {:?}", &unmatched[..unmatched.len().min(20)]);
    Ok(())
}
