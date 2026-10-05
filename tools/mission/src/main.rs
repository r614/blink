//! Mission authoring tool.
//!
//! ```text
//! mission markers  <map> [filter]   named original actors usable as Place::Actor
//! mission original <map>            the original objectives/tasks and Kismet summary
//! mission new      <map> [title]    print a starter mission (RON) for the map
//! mission check    <file.ron>       parse + validate (references, actors in the level)
//! mission graph    <file.ron>       rule graph as Graphviz DOT (events -> rules -> actions)
//! ```
//! Output that lists original data (markers, objectives) is derived from the install: use it
//! for authoring, do not commit it.

use anyhow::{Context, Result, bail};
use dishonored_assets::Content;
use dishonored_assets::npcs::{LevelMarker, extract_npc_data};
use dishonored_mission::{Action, MissionDef, validate_actors};

fn level_markers(content: &Content, map: &str) -> Result<Vec<LevelMarker>> {
    let pkgs = dishonored_assets::map_packages(content, map);
    Ok(extract_npc_data(content, &pkgs)?.markers)
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cmd = a.first().map(String::as_str).unwrap_or("help");
    match cmd {
        "markers" => {
            let c = Content::locate()?;
            let map = a.get(1).context("map")?;
            let filt = a.get(2).map(|s| s.to_ascii_lowercase());
            for m in level_markers(&c, map)? {
                if filt.as_ref().is_some_and(|f| !m.class.to_ascii_lowercase().contains(f) && !m.name.to_ascii_lowercase().contains(f)) {
                    continue;
                }
                println!("{:<28} {:<22} {:<28} ({:.0}, {:.0}, {:.0}) yaw {:.0}", m.name, m.class, m.package, m.position.x, m.position.y, m.position.z, m.yaw);
            }
        }
        "original" => {
            let c = Content::locate()?;
            let map = a.get(1).context("map")?;
            for p in dishonored_assets::map_packages(&c, map) {
                let Ok(pkg) = c.package(&p) else { continue };
                let objs = dishonored_assets::kismet::read_objectives(&pkg);
                let g = dishonored_assets::kismet::read_kismet(&pkg);
                if objs.is_empty() && g.ops.is_empty() {
                    continue;
                }
                println!("== {p}: {} Kismet nodes", g.ops.len());
                for o in objs {
                    println!("  OBJECTIVE \"{}\"", o.description);
                    for t in o.tasks {
                        println!("    [{}{}] {}", t.initial_state.trim_start_matches("DTS_"), if t.hidden { ", hidden" } else { "" }, t.description);
                    }
                }
                let mut counts = std::collections::BTreeMap::new();
                for op in &g.ops {
                    if op.kind() != "variable" {
                        *counts.entry(op.class.clone()).or_insert(0usize) += 1;
                    }
                }
                let mut v: Vec<_> = counts.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1));
                let top: Vec<String> = v.iter().take(16).map(|(k, n)| format!("{k} x{n}")).collect();
                println!("  most used nodes: {}", top.join(", "));
            }
        }
        "new" => {
            let map = a.get(1).context("map")?;
            let title = a.get(2).cloned().unwrap_or_else(|| "New Mission".into());
            print!("{}", starter(map, &title));
        }
        "check" => {
            let path = a.get(1).context("file")?;
            let def = MissionDef::load(std::path::Path::new(path))?;
            let mut errs = def.validate();
            match Content::locate() {
                Ok(c) => {
                    let names: Vec<(String, String)> = level_markers(&c, &def.map)?.into_iter().map(|m| (m.package, m.name)).collect();
                    errs.extend(validate_actors(&def, &names));
                }
                Err(_) => println!("(install not found: original actor names not checked)"),
            }
            if errs.is_empty() {
                let tasks: usize = def.objectives.iter().map(|o| o.tasks.len()).sum();
                println!("OK \"{}\" on {}: {} objectives, {} tasks, {} markers, {} npcs, {} rules", def.title, def.map, def.objectives.len(), tasks, def.markers.len(), def.npcs.len(), def.rules.len());
            } else {
                for e in &errs {
                    println!("error: {e}");
                }
                bail!("{} problem(s)", errs.len());
            }
        }
        "graph" => {
            let path = a.get(1).context("file")?;
            let def = MissionDef::load(std::path::Path::new(path))?;
            println!("digraph mission {{\n  rankdir=LR; node [fontsize=10];");
            for (i, r) in def.rules.iter().enumerate() {
                let label = r.id.clone().unwrap_or_else(|| format!("rule {i}"));
                let conds: Vec<String> = r.when.iter().map(|c| format!("{c:?}")).collect();
                println!("  r{i} [shape=box,label=\"{}\\n{}\"];", label, conds.join("\\n").replace('"', "'"));
                println!("  e{i} [shape=ellipse,label=\"{}\"];", format!("{:?}", r.on).replace('"', "'"));
                println!("  e{i} -> r{i};");
                let mut n = 0;
                graph_actions(i, &r.then, &mut n);
            }
            println!("}}");
        }
        _ => println!("usage: mission markers|original|new|check|graph ... (see the tool's source header)"),
    }
    Ok(())
}

fn graph_actions(i: usize, v: &[Action], n: &mut usize) {
    for a in v {
        *n += 1;
        let id = format!("a{i}_{n}");
        match a {
            Action::After(s, inner) => {
                println!("  {id} [shape=note,label=\"after {s}s\"];");
                println!("  r{i} -> {id};");
                graph_actions(i, inner, n);
            }
            other => {
                println!("  {id} [shape=plaintext,label=\"{}\"];", format!("{other:?}").replace('"', "'"));
                println!("  r{i} -> {id};");
            }
        }
    }
}

const STARTER: &str = include_str!("starter.ron");

fn starter(map: &str, title: &str) -> String {
    STARTER.replace("{MAP}", map).replace("{TITLE}", title)
}
