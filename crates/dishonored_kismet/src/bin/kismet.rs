//! Research CLI for the Kismet loader and interpreter (output is derived from retail data:
//! keep it out of the repository).
//!
//! ```text
//! kismet stats [MAP...]            load the global sequence + maps (default Boyle Ext + Int)
//! kismet op PACKAGE EXPORT         one op with its typed properties and links
//! kismet find TEXT [MAP...]        ops whose class, name, comment or properties contain TEXT
//! kismet actors [MAP...]           referenced level actors with their touch shapes
//! kismet run MAP [SECONDS]         run a map's Kismet with a host that supports nothing
//! ```

use anyhow::Result;
use dishonored_assets::Content;
use dishonored_kismet::load;
use dishonored_kismet::*;
use std::sync::Arc;

fn db_for(content: &Content, maps: &[String]) -> Result<KismetDb> {
    let maps: Vec<String> = if maps.is_empty() { vec!["L_Boyle_Ext_P".into(), "L_Boyle_Int_P".into()] } else { maps.to_vec() };
    let groups: Vec<(String, Vec<String>)> = maps.iter().map(|m| (m.clone(), load::map_packages(content, m))).collect();
    load::load(content, &["DishonoredGameFull_P".to_string()], &groups)
}

fn print_op(db: &KismetDb, id: OpId) {
    let o = db.op(id);
    println!("[{}] {} {} ({:?}) {}", o.id, db.describe(id), o.class, o.kind, o.path);
    if !o.comment.is_empty() {
        println!("    // {}", o.comment);
    }
    if let Some(p) = o.parent {
        println!("    parent {}", db.describe(p));
    }
    for (k, v) in &o.props.0 {
        println!("    {k} = {v:?}");
    }
    for (i, l) in o.inputs.iter().enumerate() {
        println!("    in {i} \"{}\"{}{}", l.name, l.linked_op.map(|x| format!(" -> {}", db.describe(x))).unwrap_or_default(), if l.disabled { " (disabled)" } else { "" });
    }
    for (i, l) in o.outputs.iter().enumerate() {
        let t: Vec<String> = l.links.iter().map(|(t, ix)| format!("{} {}#{ix}", db.op(*t).class, db.describe(*t))).collect();
        println!("    out {i} \"{}\"{} -> {}", l.name, if l.disabled { " (disabled)" } else { "" }, t.join(", "));
    }
    for l in &o.vars {
        let t: Vec<String> = l.vars.iter().map(|v| {
            let vo = db.op(*v);
            let named = vo.named.map(|n| format!(" => {}", db.describe(n))).unwrap_or_default();
            format!("{} {}{named} {:?}", vo.class, db.describe(*v), vo.props.0.iter().filter(|(k, _)| !matches!(k.as_str(), "SupportedClasses")).collect::<Vec<_>>())
        }).collect();
        println!("    var \"{}\" prop={} w={} : {}", l.name, l.property, l.writeable, t.join("; "));
    }
    for l in &o.events {
        println!("    events \"{}\": {:?}", l.name, l.events.iter().map(|e| db.describe(*e)).collect::<Vec<_>>());
    }
    if let Some(i) = &o.interp {
        println!("    interp length {} events {:?}", i.length, i.events);
    }
}

fn short(v: &Value) -> String {
    match v {
        Value::Obj(Obj::Path(p)) => p.rsplit(['.', ':']).next().unwrap_or(p).to_string(),
        Value::Obj(o) => format!("{o:?}"),
        Value::Name(s) | Value::Str(s) | Value::Enum(s) => format!("{s:?}"),
        Value::Array(a) => format!("[{}]", a.iter().map(short).collect::<Vec<_>>().join(",")),
        Value::Struct(_, f) => format!("{{{}}}", f.iter().map(|(k, v)| format!("{k}={}", short(v))).collect::<Vec<_>>().join(",")),
        other => format!("{other:?}"),
    }
}

fn var_short(db: &KismetDb, v: OpId) -> String {
    let o = db.op(v);
    let t = o.named.unwrap_or(v);
    let to = db.op(t);
    let val = to.props.0.iter().filter(|(k, _)| !matches!(k.as_str(), "bDeletable" | "SupportedClasses")).map(|(k, x)| match x {
        Value::Obj(Obj::Actor(a)) => format!("{k}={}", db.actors[*a as usize].name),
        _ => format!("{k}={}", short(x)),
    }).collect::<Vec<_>>().join(",");
    if o.class == "SeqVar_Named" { format!("Named({})={}({val})", o.props.str("FindVarName"), to.class) } else { format!("{}({val})", o.class) }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let content = Content::locate()?;
    match args.first().map(String::as_str) {
        Some("stats") => {
            let t = std::time::Instant::now();
            let db = db_for(&content, &args[1..])?;
            println!("loaded in {:.1} s: {} packages, {} ops, {} actors, {} objectives", t.elapsed().as_secs_f32(), db.packages.len(), db.ops.len(), db.actors.len(), db.objectives.len());
            for (i, p) in db.packages.iter().enumerate() {
                let n = db.ops.iter().filter(|o| o.package == i as u16).count();
                println!("  {p}: {n} ops, roots {:?}", db.roots[i].iter().map(|r| db.op(*r).path.clone()).collect::<Vec<_>>());
            }
            let counts = db.class_counts(&[]);
            let mut v: Vec<_> = counts.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            println!("{} classes", v.len());
            for (c, n) in v {
                println!("  {n:5} {c}  {:?}", db.classes.get(&c).map(|x| x.iter().skip(1).take(3).cloned().collect::<Vec<_>>()).unwrap_or_default());
            }
            let named = db.ops.iter().filter(|o| o.class == "SeqVar_Named").count();
            let resolved = db.ops.iter().filter(|o| o.class == "SeqVar_Named" && o.named.is_some()).count();
            println!("SeqVar_Named resolved {resolved}/{named}");
            for o in &db.objectives {
                println!("objective {} \"{}\" ({} tasks)", o.path, o.description, o.tasks.len());
            }
        }
        Some("op") => {
            let db = db_for(&content, &args[3..])?;
            let id = db.find_export(&args[1], args[2].parse()?).ok_or_else(|| anyhow::anyhow!("no such op"))?;
            print_op(&db, id);
        }
        Some("find") => {
            let db = db_for(&content, &args[2..])?;
            let q = args[1].to_ascii_lowercase();
            for o in &db.ops {
                let actors: Vec<String> = o.props.0.iter().filter_map(|(_, v)| match v {
                    Value::Obj(Obj::Actor(a)) => Some(db.actors[*a as usize].name.clone()),
                    _ => None,
                }).collect();
                let hay = format!("{} {} {} {:?} {}", o.class, o.name, o.comment, o.props.0, actors.join(" ")).to_ascii_lowercase();
                if hay.contains(&q) {
                    print_op(&db, o.id);
                }
            }
        }
        Some("obj") => {
            // obj PACKAGE PATH: one export's raw properties (exact path).
            let pkg = content.package(&args[1])?;
            for i in 0..pkg.exports.len() {
                let p = pkg.obj_path(ue3_package::ObjRef::Export(i));
                if args[2..].iter().any(|a| p.eq_ignore_ascii_case(a)) {
                    println!("[{i}] {p} ({})", pkg.export_class(i));
                    if let Ok(h) = pkg.read_object(i) {
                        for pr in &h.properties {
                            println!("  {}[{}] = {}", pr.name, pr.array_index, ue3_package::properties::format_value(&pkg, &pr.value));
                        }
                    }
                    for c in 0..pkg.exports.len() {
                        if pkg.exports[c].outer == i as i32 + 1 {
                            println!("  child [{c}] {} ({})", pkg.obj_name(ue3_package::ObjRef::Export(c)), pkg.export_class(c));
                        }
                    }
                }
            }
        }
        Some("fields") => {
            // fields PACKAGE CLASS: a class's own fields in declaration order (UField::Next).
            let pkg = content.package(&args[1])?;
            let Some(ci) = (0..pkg.exports.len()).find(|&i| pkg.exports[i].class == 0 && pkg.obj_name(ue3_package::ObjRef::Export(i)).eq_ignore_ascii_case(&args[2])) else { anyhow::bail!("no class") };
            for i in 0..pkg.exports.len() {
                if pkg.exports[i].outer == ci as i32 + 1 {
                    // UField: (object header) Next:i32 ... ; print raw first dwords after the header.
                    let data = pkg.export_data(i).unwrap_or(&[]);
                    let dw: Vec<i32> = data.chunks_exact(4).take(6).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
                    let next = dw.get(1).map(|n| pkg.obj_name(ue3_package::ObjRef::from_index(*n))).unwrap_or_default();
                    println!("[{i}] {} ({}) next={next} raw={dw:?}", pkg.obj_name(ue3_package::ObjRef::Export(i)), pkg.export_class(i));
                }
            }
        }
        Some("fwd") => {
            // fwd PACKAGE EXPORT [MAX]: ops reached forwards (outputs, sequence entry, remote
            // events), breadth first, with a one-line summary each.
            let db = db_for(&content, &[])?;
            let id = db.find_export(&args[1], args[2].parse()?).ok_or_else(|| anyhow::anyhow!("no such op"))?;
            let max: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
            let mut remote: std::collections::BTreeMap<String, Vec<OpId>> = Default::default();
            for o in &db.ops {
                if o.class == "SeqEvent_RemoteEvent" {
                    remote.entry(o.props.str("EventName").to_ascii_lowercase()).or_default().push(o.id);
                }
            }
            let mut q = std::collections::VecDeque::from([(id, 0usize, String::new())]);
            let mut seen = std::collections::BTreeSet::new();
            while let Some((o, depth, via)) = q.pop_front() {
                if !seen.insert(o) || seen.len() > max {
                    continue;
                }
                let op = db.op(o);
                let summary: Vec<String> = op
                    .props
                    .0
                    .iter()
                    .filter(|(k, _)| !matches!(k.as_str(), "bDeletable" | "bAutoActivateOutputLinks" | "bCallHandler" | "bLatentExecution" | "m_bAlwaysOutOfBendTime" | "bEnabled" | "bPlayerOnly" | "MaxTriggerCount" | "PlayRate" | "BlendOutTime" | "m_bIsSkippable"))
                    .map(|(k, v)| format!("{k}={}", short(v)))
                    .collect();
                let vars: Vec<String> = op.vars.iter().filter(|l| !l.vars.is_empty()).map(|l| format!("{}:{}", l.name, l.vars.iter().map(|v| var_short(&db, *v)).collect::<Vec<_>>().join("|"))).collect();
                println!("{}{via}{} {} {} {} {}", "  ".repeat(depth.min(12)), db.describe(o), op.class, if op.comment.is_empty() { String::new() } else { format!("// {}", op.comment) }, summary.join(" "), vars.join(" "));
                for (i, l) in op.outputs.iter().enumerate() {
                    for (t, ix) in &l.links {
                        let tin = db.op(*t).inputs.get(*ix as usize).map(|x| x.name.clone()).unwrap_or_default();
                        q.push_back((*t, depth + 1, format!("[{} {i} -> {tin}] ", l.name)));
                    }
                }
                if op.kind == Kind::Sequence {
                    for l in &op.inputs {
                        if let Some(e) = l.linked_op {
                            q.push_back((e, depth + 1, format!("[enter {}] ", l.name)));
                        }
                    }
                }
                if op.class == "SeqAct_ActivateRemoteEvent" {
                    for e in remote.get(&op.props.str("EventName").to_ascii_lowercase()).cloned().unwrap_or_default() {
                        q.push_back((e, depth + 1, "[remote] ".into()));
                    }
                }
                if op.class == "SeqAct_FinishSequence" {
                    if let Some(s) = op.parent {
                        for l in &db.op(s).outputs {
                            if l.linked_op == Some(o) {
                                for (t, _) in &l.links {
                                    q.push_back((*t, depth + 1, format!("[finish {}] ", l.name)));
                                }
                            }
                        }
                    }
                }
            }
        }
        Some("bwd") => {
            // bwd PACKAGE EXPORT [DEPTH]: what leads to an op (incoming links, remote event
            // senders, sequence entries, finish ops).
            let db = db_for(&content, &[])?;
            let id = db.find_export(&args[1], args[2].parse()?).ok_or_else(|| anyhow::anyhow!("no such op"))?;
            let depth: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
            let mut incoming: std::collections::BTreeMap<OpId, Vec<(OpId, String)>> = Default::default();
            for o in &db.ops {
                for (i, l) in o.outputs.iter().enumerate() {
                    for (t, ix) in &l.links {
                        incoming.entry(*t).or_default().push((o.id, format!("{} {i} -> in {ix}", l.name)));
                    }
                }
                if o.class == "SeqAct_ActivateRemoteEvent" {
                    let n = o.props.str("EventName").to_ascii_lowercase();
                    for e in db.ops.iter().filter(|e| e.class == "SeqEvent_RemoteEvent" && e.props.str("EventName").to_ascii_lowercase() == n) {
                        incoming.entry(e.id).or_default().push((o.id, "remote".into()));
                    }
                }
                if o.kind == Kind::Sequence {
                    for l in &o.inputs {
                        if let Some(e) = l.linked_op {
                            incoming.entry(e).or_default().push((o.id, format!("sequence input {}", l.name)));
                        }
                    }
                    for l in &o.outputs {
                        if let Some(f) = l.linked_op {
                            for (t, _) in &l.links {
                                incoming.entry(*t).or_default().push((f, format!("finish {}", l.name)));
                            }
                        }
                    }
                }
            }
            fn walk(db: &KismetDb, inc: &std::collections::BTreeMap<OpId, Vec<(OpId, String)>>, o: OpId, d: usize, max: usize, seen: &mut std::collections::BTreeSet<OpId>) {
                let op = db.op(o);
                let extra: Vec<String> = op.props.0.iter().filter(|(k, _)| matches!(k.as_str(), "EventName" | "m_StoryFlagName" | "m_Tag" | "Originator" | "InputLabel" | "ConsoleEventName")).map(|(k, v)| format!("{k}={}", short(v))).collect();
                println!("{}{} {} {} {}", "  ".repeat(d), db.describe(o), op.class, if op.comment.is_empty() { String::new() } else { format!("// {}", op.comment) }, extra.join(" "));
                if d >= max || !seen.insert(o) {
                    return;
                }
                for (src, via) in inc.get(&o).cloned().unwrap_or_default() {
                    print!("{}  <- [{via}] ", "  ".repeat(d));
                    walk(db, inc, src, d + 1, max, seen);
                }
            }
            walk(&db, &incoming, id, 0, depth, &mut Default::default());
        }
        Some("actors") => {
            let db = db_for(&content, &args[1..])?;
            for a in &db.actors {
                println!("{}:{} {} at {:?} rot {:?} scale {:?} tag {} shape {:?}", a.package, a.name, a.class, a.location, a.rotation, a.scale, a.tag, a.shape);
            }
        }
        Some("run") => {
            let map = args.get(1).cloned().unwrap_or_else(|| "L_Boyle_Ext_P".into());
            let secs: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10.0);
            let db = Arc::new(db_for(&content, std::slice::from_ref(&map))?);
            let mut k = Kismet::new(db.clone(), 1);
            let mut host = NullHost { players: vec![1], ..Default::default() };
            k.players = vec![1];
            k.load_map(&mut host, &map);
            let steps = (secs * 60.0) as u32;
            for _ in 0..steps {
                k.tick(&mut host, 1.0 / 60.0);
            }
            for l in &k.log {
                println!("{l}");
            }
            println!("handled {:?}", k.coverage.handled);
            println!("unhandled {:?}", k.coverage.unhandled);
        }
        _ => println!("usage: kismet stats|op|find|actors|run ..."),
    }
    Ok(())
}
