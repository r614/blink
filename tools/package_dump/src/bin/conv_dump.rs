//! Research: Dishonored conversation data (`DisDialogOneShot`, `DisDialogTree`,
//! `DisConversation`, `DisConv_*` nodes, `DisDialogVoiceData*`) in a level package.
//!
//! conv_dump props <upk> <path-suffix> [...]   tagged properties of exports whose path ends with a suffix
//! conv_dump convs <upk>                       every conversation: entry, node graph, blurbs -> VO event + bank
//! conv_dump vo <upk>                          blurb GUID -> Wwise event name / bank table (tab-separated)
//! conv_dump stats <upk>                       counts only
//!
//! Output is derived from retail data (it contains subtitle text): keep it under harness_out/.
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use ue3_package::properties::{Property, Value, find, format_value};
use ue3_package::{ObjRef, Package};

const NOISE: &[&str] = &[
    "bHasImpulse", "QueuedActivations", "bDisabled", "bDisabledPIE", "DrawY", "DrawX", "bHidden", "ActivateDelay",
    "bClampedMax", "bClampedMin", "OverrideDelta", "bWriteable", "bSequenceNeverReadsOnlyWritesToThisVar",
    "bModifiesLinkedObject", "MinVars", "MaxVars", "bAllowAnyType", "ExpectedType", "ObjInstanceVersion",
    "DrawWidth", "DrawHeight", "LinkVar", "bDirtiedWithoutExport", "m_BlurbVersionNum", "m_bSubtitleTypeSet",
];

fn print_props(pkg: &Package, props: &[Property], indent: usize) {
    for p in props {
        if NOISE.contains(&p.name.as_str()) {
            continue;
        }
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
                if let Some((n, body)) = p.array_raw() {
                    // Array of FStrings?
                    let mut o = 0;
                    let mut strs = Vec::new();
                    for _ in 0..n {
                        let Some(l) = body.get(o..o + 4).map(|x| i32::from_le_bytes(x.try_into().unwrap())) else { break };
                        if !(1..512).contains(&l) || o + 4 + l as usize > body.len() {
                            break;
                        }
                        let s = &body[o + 4..o + 4 + l as usize];
                        if s.last() != Some(&0) || !s[..s.len() - 1].iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
                            break;
                        }
                        strs.push(String::from_utf8_lossy(&s[..s.len() - 1]).into_owned());
                        o += 4 + l as usize;
                    }
                    if n > 0 && strs.len() == n && o == body.len() {
                        println!("{pad}{}[{}] = strs{:?}", p.name, p.array_index, strs);
                        continue;
                    }
                }
                if let Some((n, body)) = p.array_raw() {
                    if n > 0 && body.len() == n * 8 {
                        let names: Vec<Option<String>> = body
                            .chunks(8)
                            .map(|c| {
                                let idx = i32::from_le_bytes(c[..4].try_into().unwrap());
                                let num = i32::from_le_bytes(c[4..].try_into().unwrap());
                                (idx >= 0 && (idx as usize) < pkg.names.len() && (0..10000).contains(&num)).then(|| pkg.name(ue3_package::FName { index: idx, number: num }))
                            })
                            .collect();
                        if names.iter().all(Option::is_some) {
                            let v: Vec<String> = names.into_iter().flatten().collect();
                            println!("{pad}{}[{}] = names[{}]", p.name, p.array_index, v.join(", "));
                            continue;
                        }
                    }
                }
                if let Ok(objs) = p.array_objects() {
                    if !objs.is_empty() && objs.iter().all(|o| o.unsigned_abs() as usize <= pkg.exports.len().max(pkg.imports.len())) {
                        let names: Vec<String> = objs.iter().map(|o| pkg.obj_ref_name(*o)).collect();
                        println!("{pad}{}[{}] = objs[{}]", p.name, p.array_index, names.join(", "));
                        continue;
                    }
                }
                println!("{pad}{}[{}] = {}", p.name, p.array_index, format_value(pkg, &p.value));
            }
            v => println!("{pad}{}[{}] = {}", p.name, p.array_index, format_value(pkg, v)),
        }
    }
}

fn guid_str(g: &[u32; 4]) -> String {
    format!("{:08X}{:08X}{:08X}{:08X}", g[0], g[1], g[2], g[3])
}

struct Node {
    idx: usize,
    class: String,
    path: String,
    props: Vec<Property>,
}

fn obj(_pkg: &Package, p: &Property) -> Option<usize> {
    match p.value {
        Value::Object(i) if i > 0 => Some(i as usize - 1),
        _ => None,
    }
}

/// `m_OutputLinks(i).m_pLink` targets, in order (None = unconnected output).
fn outputs(pkg: &Package, props: &[Property]) -> Vec<Option<usize>> {
    let Some(p) = find(props, "m_OutputLinks") else { return Vec::new() };
    let Ok(elems) = p.array_structs(pkg) else { return Vec::new() };
    elems.iter().map(|e| find(e, "m_pLink").and_then(|l| obj(pkg, l))).collect()
}

fn short(path: &str, owner: &str) -> String {
    path.strip_prefix(owner).map(|s| s.trim_start_matches('.').to_string()).unwrap_or_else(|| path.to_string())
}

fn summary(pkg: &Package, n: &Node) -> String {
    let mut parts = Vec::new();
    for p in &n.props {
        let s = match (p.name.as_str(), &p.value) {
            ("m_OutputLinks" | "m_pOwner" | "m_iBlurbGUID" | "m_NodeGUID", _) => continue,
            (_, Value::Str(s)) => format!("{}={:?}", p.name, s.trim()),
            (_, Value::Guid(g)) => format!("{}={}", p.name, guid_str(g)),
            (_, Value::Array(_)) => {
                if let Ok(el) = p.array_structs(pkg) {
                    if !el.is_empty() && el.iter().all(|e| !e.is_empty()) {
                        let v: Vec<String> = el
                            .iter()
                            .map(|e| e.iter().map(|q| format!("{}={}", q.name, match &q.value { Value::Str(s) => format!("{:?}", s.trim()), v => format_value(pkg, v) })).collect::<Vec<_>>().join(","))
                            .collect();
                        format!("{}=[{}]", p.name, v.join(" | "))
                    } else {
                        format!("{}={}", p.name, format_value(pkg, &p.value))
                    }
                } else {
                    format!("{}={}", p.name, format_value(pkg, &p.value))
                }
            }
            (_, v) => format!("{}={}", p.name, format_value(pkg, v)),
        };
        parts.push(s);
    }
    parts.join(" ")
}

struct Vo {
    event: String,
    bank: String,
    nonword: bool,
    voice: String,
}

fn voice_table(pkg: &Package) -> HashMap<[u32; 4], Vec<Vo>> {
    let mut out: HashMap<[u32; 4], Vec<Vo>> = HashMap::new();
    for i in 0..pkg.exports.len() {
        let c = pkg.export_class(i);
        if c != "DisDialogVoiceData" && c != "DisDialogVoiceData_OneShot" {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        let bank = find(&h.properties, "m_pAkBank").and_then(|p| p.as_object()).map(|o| pkg.obj_name(ObjRef::from_index(o))).unwrap_or_default();
        let vpath = pkg.obj_path(ObjRef::Export(i));
        let Some(vd) = find(&h.properties, "m_VoiceData") else { continue };
        let Ok(trees) = vd.array_structs(pkg) else { continue };
        for t in trees {
            let Some(bl) = find(&t, "m_Blurbs") else { continue };
            let Ok(blurbs) = bl.array_structs(pkg) else { continue };
            for b in blurbs {
                let Some(Value::Guid(g)) = find(&b, "m_GUID").map(|p| p.value.clone()) else { continue };
                let ev = match find(&b, "m_AkEventName").map(|p| &p.value) {
                    Some(Value::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let nonword = matches!(find(&b, "m_bIsNonWord").map(|p| &p.value), Some(Value::Bool(true)));
                out.entry(g).or_default().push(Vo { event: ev, bank: bank.clone(), nonword, voice: vpath.clone() });
            }
        }
    }
    out
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mode = a[0].as_str();
    let pkg = Package::open(&a[1])?;
    if mode == "props" {
        for suffix in &a[2..] {
            for i in 0..pkg.exports.len() {
                let path = pkg.obj_path(ObjRef::Export(i));
                if path == *suffix || path.ends_with(&format!(".{suffix}")) {
                    println!("=== [{i}] {path} ({})", pkg.export_class(i));
                    if let Ok(h) = pkg.read_object(i) {
                        print_props(&pkg, &h.properties, 2);
                    }
                }
            }
        }
        return Ok(());
    }
    let vo = voice_table(&pkg);
    if mode == "vo" {
        for (g, v) in &vo {
            for x in v {
                println!("{}\t{}\t{}\t{}\t{}", guid_str(g), x.event, x.bank, x.nonword, x.voice);
            }
        }
        return Ok(());
    }
    if mode == "vocheck" {
        // Every voice-table entry: does fnv1(lowercase event) exist as an Event in the bank file <bank>.INT?
        let dir = std::path::Path::new(&a[1]).parent().unwrap().to_path_buf();
        let lang = a.get(2).cloned().unwrap_or_else(|| "INT".into());
        let mut cache: HashMap<String, Option<HashSet<u32>>> = HashMap::new();
        let (mut ok, mut missing_bank, mut missing_event) = (0usize, 0usize, 0usize);
        let mut banks_used: BTreeMap<String, usize> = BTreeMap::new();
        for v in vo.values().flatten() {
            if v.event.is_empty() {
                continue;
            }
            *banks_used.entry(v.bank.clone()).or_default() += 1;
            let ev = cache.entry(v.bank.clone()).or_insert_with(|| bank_events(&dir.join(format!("{}.{lang}", v.bank))));
            match ev {
                None => missing_bank += 1,
                Some(set) if set.contains(&fnv1(&v.event.to_ascii_lowercase())) => ok += 1,
                Some(_) => {
                    missing_event += 1;
                    println!("missing event {} in {}", v.event, v.bank);
                }
            }
        }
        println!("voice entries {} : event found {ok}, bank file missing {missing_bank}, event missing {missing_event}; banks {}", ok + missing_bank + missing_event, banks_used.len());
        let total: u64 = banks_used.keys().filter_map(|b| std::fs::metadata(dir.join(format!("{b}.{lang}"))).ok()).map(|m| m.len()).sum();
        println!("bank bytes {:.1} MB", total as f64 / 1e6);
        return Ok(());
    }
    // Load every DisConv_* / DisConversation node.
    let mut nodes: BTreeMap<usize, Node> = BTreeMap::new();
    for i in 0..pkg.exports.len() {
        let c = pkg.export_class(i);
        if !(c.starts_with("DisConv") || c == "DisDialogTree" || c == "DisDialogTree_OneShot" || c == "DisDialogOneShot") {
            continue;
        }
        let props = pkg.read_object(i).map(|h| h.properties).unwrap_or_default();
        nodes.insert(i, Node { idx: i, class: c, path: pkg.obj_path(ObjRef::Export(i)), props });
    }
    let mut class_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for n in nodes.values() {
        *class_counts.entry(n.class.as_str()).or_default() += 1;
    }
    // Blurb statistics.
    let is_blurb = |c: &str| c == "DisConv_Blurb" || c == "DisConv_NonWord";
    let (mut blurbs, mut voiced, mut nonword, mut missing) = (0, 0, 0, Vec::new());
    let mut banks: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_root: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for n in nodes.values().filter(|n| is_blurb(&n.class)) {
        blurbs += 1;
        let root = n.path.split('.').take(if n.path.starts_with("TheWorld") { 3 } else { 1 }).collect::<Vec<_>>().join(".");
        let e = by_root.entry(root).or_default();
        e.0 += 1;
        if n.class == "DisConv_NonWord" {
            nonword += 1;
        }
        let g = find(&n.props, "m_iBlurbGUID").and_then(|p| if let Value::Guid(g) = p.value { Some(g) } else { None });
        match g.and_then(|g| vo.get(&g)) {
            Some(v) if v.iter().any(|x| !x.event.is_empty()) => {
                voiced += 1;
                e.1 += 1;
                for x in v {
                    *banks.entry(x.bank.clone()).or_default() += 1;
                }
            }
            _ => missing.push(n.path.clone()),
        }
    }
    if mode == "stats" {
        println!("package {}", a[1]);
        for (c, n) in &class_counts {
            println!("  {c:<40} {n}");
        }
        println!("blurb nodes {blurbs} (non-word {nonword}), with a VO event {voiced}, without {}", missing.len());
        println!("voice table entries {}", vo.values().map(Vec::len).sum::<usize>());
        println!("banks:");
        for (b, n) in &banks {
            println!("  {b} {n}");
        }
        println!("blurbs by owner (blurbs, voiced):");
        for (r, (b, v)) in &by_root {
            println!("  {r} {b} {v}");
        }
        for m in missing.iter().take(40) {
            println!("  no VO: {m}");
        }
        return Ok(());
    }
    // convs: per DisDialogOneShot / DisDialogTree, every conversation with its node graph (after table).
    if mode == "table" {
        // One row per conversation: owner, conversation, label, blurbs, voiced, non-word, choices, kismet events, soirees, banks.
        let mut owned: BTreeMap<String, Vec<&Node>> = BTreeMap::new();
        for n in nodes.values() {
            if let Some(o) = find(&n.props, "m_pOwner").and_then(|p| obj(&pkg, p)) {
                owned.entry(pkg.obj_path(ObjRef::Export(o))).or_default().push(n);
            }
        }
        println!("conversation\tlabel\tdescription\tblurbs\tvoiced\tnonword\tchoices\tkismet_events\tsoirees\tstart\tbanks");
        for n in nodes.values().filter(|n| n.class == "DisConversation") {
            let kids = owned.get(&n.path).cloned().unwrap_or_default();
            let label = match find(&n.props, "m_Label").map(|p| &p.value) { Some(Value::Name(s)) => s.clone(), _ => String::new() };
            let desc = match find(&n.props, "m_Description").map(|p| &p.value) { Some(Value::Str(s)) => s.trim().replace(['\n', '\t'], " "), _ => String::new() };
            let (mut b, mut v, mut nw, mut ch, mut so) = (0, 0, 0, 0, 0);
            let mut evs = Vec::new();
            let mut banks: BTreeSet<String> = BTreeSet::new();
            for k in &kids {
                match k.class.as_str() {
                    "DisConv_Blurb" | "DisConv_NonWord" => {
                        b += 1;
                        if k.class == "DisConv_NonWord" { nw += 1; }
                        if let Some(Value::Guid(g)) = find(&k.props, "m_iBlurbGUID").map(|p| p.value.clone()) {
                            if let Some(x) = vo.get(&g) { v += 1; for e in x { banks.insert(e.bank.clone()); } }
                        }
                    }
                    "DisConv_PlayerChoice" => ch += 1,
                    "DisConv_Soiree" => so += 1,
                    "DisConv_KismetActivateRemoteEvent" => if let Some(Value::Name(s)) = find(&k.props, "m_EventName").map(|p| &p.value) { evs.push(s.clone()) },
                    _ => {}
                }
            }
            let start = outputs(&pkg, &n.props).first().copied().flatten().map(|o| short(&pkg.obj_path(ObjRef::Export(o)), &n.path)).unwrap_or_default();
            println!("{}\t{label}\t{desc}\t{b}\t{v}\t{nw}\t{ch}\t{}\t{so}\t{start}\t{}", n.path, evs.join(","), banks.into_iter().collect::<Vec<_>>().join(","));
        }
        return Ok(());
    }
    let mut owners: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for n in nodes.values() {
        if let Some(o) = find(&n.props, "m_pOwner").and_then(|p| obj(&pkg, p)) {
            owners.entry(pkg.obj_path(ObjRef::Export(o))).or_default().push(n.idx);
        }
    }
    for n in nodes.values().filter(|n| n.class == "DisDialogOneShot" || n.class == "DisDialogTree" || n.class == "DisDialogTree_OneShot") {
        println!("##### {} ({})", n.path, n.class);
        print_props(&pkg, &n.props, 4);
    }
    for n in nodes.values().filter(|n| n.class == "DisConversation") {
        println!("\n=== CONVERSATION {} [{}]", n.path, n.idx);
        print_props(&pkg, &n.props, 4);
        let kids = owners.get(&n.path).cloned().unwrap_or_default();
        let mut seen = BTreeSet::new();
        for k in &kids {
            let c = &nodes[k];
            seen.insert(*k);
            let outs: Vec<String> = outputs(&pkg, &c.props).iter().map(|o| o.map(|o| short(&pkg.obj_path(ObjRef::Export(o)), &n.path)).unwrap_or_else(|| "-".into())).collect();
            let mut extra = String::new();
            if is_blurb(&c.class) {
                if let Some(Value::Guid(g)) = find(&c.props, "m_iBlurbGUID").map(|p| p.value.clone()) {
                    match vo.get(&g) {
                        Some(v) => {
                            for x in v {
                                extra.push_str(&format!(" VO={}@{}", x.event, x.bank));
                            }
                        }
                        None => extra.push_str(" VO=none"),
                    }
                    extra.push_str(&format!(" guid={}", guid_str(&g)));
                }
            }
            println!("  [{}] {} {}{} -> [{}]", c.idx, short(&c.path, &n.path), summary(&pkg, c), extra, outs.join(", "));
        }
    }
    // Tree-level nodes (hooks etc. owned by the tree, not a conversation).
    for n in nodes.values().filter(|n| n.class == "DisDialogTree" || n.class == "DisDialogTree_OneShot") {
        let kids = owners.get(&n.path).cloned().unwrap_or_default();
        println!("\n=== TREE-LEVEL NODES {} ({})", n.path, kids.len());
        for k in kids {
            let c = &nodes[&k];
            if c.class == "DisConversation" {
                continue;
            }
            let outs: Vec<String> = outputs(&pkg, &c.props).iter().map(|o| o.map(|o| short(&pkg.obj_path(ObjRef::Export(o)), &n.path)).unwrap_or_else(|| "-".into())).collect();
            println!("  [{}] {} {} -> [{}]", c.idx, short(&c.path, &n.path), summary(&pkg, c), outs.join(", "));
        }
    }
    Ok(())
}

fn fnv1(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h = h.wrapping_mul(16777619) ^ b as u32;
    }
    h
}

/// Event ids in the (single) soundbank of an AKPK file (4-table layout), or None if unreadable.
fn bank_events(path: &std::path::Path) -> Option<HashSet<u32>> {
    let b = std::fs::read(path).ok()?;
    let u = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?)) };
    if b.get(..4)? != b"AKPK" {
        return None;
    }
    let lang = u(12)? as usize;
    let lut = 28 + lang;
    if u(lut)? < 1 {
        return None;
    }
    let (block, size, start) = (u(lut + 8)? as usize, u(lut + 12)? as usize, u(lut + 16)? as usize);
    let off = start * block;
    let bank = b.get(off..off + size)?;
    let mut o = 0;
    let mut out = HashSet::new();
    while o + 8 <= bank.len() {
        let tag = &bank[o..o + 4];
        let len = u32::from_le_bytes(bank[o + 4..o + 8].try_into().ok()?) as usize;
        if tag == b"HIRC" {
            let h = &bank[o + 8..(o + 8 + len).min(bank.len())];
            let n = u32::from_le_bytes(h.get(..4)?.try_into().ok()?) as usize;
            let mut p = 4;
            for _ in 0..n {
                let kind = *h.get(p)?;
                let sz = u32::from_le_bytes(h.get(p + 1..p + 5)?.try_into().ok()?) as usize;
                let id = u32::from_le_bytes(h.get(p + 5..p + 9)?.try_into().ok()?);
                if kind == 4 {
                    out.insert(id);
                }
                p += 5 + sz;
            }
        }
        o += 8 + len;
    }
    Some(out)
}
