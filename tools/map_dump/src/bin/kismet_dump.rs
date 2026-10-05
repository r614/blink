//! `kismet_dump <package> [--sequence substring] [--summary] [--dot]`: the original mission
//! logic of a level package: Arkane objectives/tasks and the Kismet graph (events ->
//! conditions -> actions). Research output derived from retail data: keep it out of the repo.
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::kismet::{read_kismet, read_objectives};
use std::collections::BTreeMap;

fn short(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    let pkg = c.package(&a[0])?;
    let opt = |f: &str| a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).cloned();
    let seq_filter = opt("--sequence");
    for o in read_objectives(&pkg) {
        println!("OBJECTIVE {} \"{}\"", o.path, o.description);
        for t in &o.tasks {
            println!("  task {:<24} {:<14} {}\"{}\"", t.name, t.initial_state, if t.hidden { "(hidden) " } else { "" }, t.description);
        }
    }
    let g = read_kismet(&pkg);
    if a.iter().any(|x| x == "--summary") {
        let mut by_class: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for op in &g.ops {
            *by_class.entry((op.kind(), op.class.as_str())).or_default() += 1;
        }
        let mut seqs: BTreeMap<&str, usize> = BTreeMap::new();
        for op in &g.ops {
            *seqs.entry(op.sequence.as_str()).or_default() += 1;
        }
        println!("\n{} nodes in {} sequences", g.ops.len(), seqs.len());
        for ((k, cl), n) in by_class {
            println!("  {k:<9} {cl:<40} {n}");
        }
        for (s, n) in seqs {
            println!("  seq {s} ({n})");
        }
        return Ok(());
    }
    let dot = a.iter().any(|x| x == "--dot");
    if dot {
        println!("digraph kismet {{\n  rankdir=LR; node [shape=box,fontsize=10];");
    }
    for op in g.ops.iter().filter(|o| seq_filter.as_ref().is_none_or(|f| o.sequence.contains(f.as_str()))) {
        if op.kind() == "variable" && !dot {
            continue;
        }
        let props: Vec<String> = op.props.iter().map(|(k, v)| format!("{k}={v}")).collect();
        if dot {
            println!("  n{} [label=\"{}\n{}\"];", op.export, op.class, props.join("\n").replace('"', "'"));
            for (desc, targets) in &op.outputs {
                for (t, _) in targets {
                    println!("  n{} -> n{} [label=\"{}\"];", op.export, t, desc);
                }
            }
            for (desc, vars) in &op.variables {
                for v in vars {
                    println!("  n{v} -> n{} [style=dashed,label=\"{desc}\"];", op.export);
                }
            }
            continue;
        }
        println!("[{}] {} {} {}", op.export, op.kind(), op.class, props.join(" "));
        if let Some(c) = &op.comment {
            println!("      // {c}");
        }
        for (desc, vars) in op.variables.iter().filter(|v| !v.1.is_empty()) {
            let names: Vec<String> = vars.iter().map(|v| g.op(*v).map(|o| format!("{}{}", o.class, if o.props.is_empty() { String::new() } else { format!("({})", o.props.iter().map(|p| p.1.clone()).collect::<Vec<_>>().join(",")) })).unwrap_or_else(|| v.to_string())).collect();
            println!("      var {desc}: {}", names.join(", "));
        }
        for (desc, targets) in op.outputs.iter().filter(|o| !o.1.is_empty()) {
            let names: Vec<String> = targets.iter().map(|(t, i)| g.op(*t).map(|o| format!("{}[{}]#{i}", short(&o.class), o.export)).unwrap_or_else(|| t.to_string())).collect();
            println!("      {desc} -> {}", names.join(", "));
        }
    }
    if dot {
        println!("}}");
    }
    Ok(())
}
