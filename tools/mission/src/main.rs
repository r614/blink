//! Mission authoring tool.
//!
//! ```text
//! mission markers  <map> [filter]   named original actors usable as Place::Actor
//! mission original <map>            the original objectives/tasks and Kismet summary
//! mission new      <map> [title]    print a starter mission (RON) for the map
//! mission check    <file.ron>       parse + validate (references, actors in the level)
//! mission starts                    the global game sequence's level changes (ChangeLvl_* -> map, sublevels)
//! mission cook     [out dir]        one MissionDef per DefaultGame.ini m_MissionsGame entry, started by the
//!                                   global level change into its first map (default: <cache>/missions)
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
        "spawners" => {
            let c = Content::locate()?;
            let map = a.get(1).context("map")?;
            let pkgs = dishonored_assets::map_packages(&c, map);
            for s in extract_npc_data(&c, &pkgs)?.spawns {
                let p = &s.spawner;
                println!("{:<44} {:?} begin_play={} susp={:?} squad={} patrol={} dead={} ko={} faction={} travel={:?}", s.source.rsplit('.').next().unwrap_or(""), s.role, p.on_begin_play, p.at_suspicion, p.squad, p.patrol_on_startup, p.spawn_dead, p.knocked_out, p.faction.rsplit('.').next().unwrap_or(""), p.travel_tags);
            }
        }
        "travel" => {
            let c = Content::locate()?;
            let map = a.get(1).context("map")?;
            let pkgs = dishonored_assets::map_packages(&c, map);
            for l in dishonored_assets::travel::extract_travel(&c, &pkgs)? {
                println!("{:<28} {:<30} ({:.0},{:.0},{:.0}) -> {:<24} via {} -> {:?}", l.door, l.door_tweak, l.door_position.x, l.door_position.y, l.door_position.z, l.destination_tag, l.change_event, l.destination_map);
                if a.iter().any(|x| x == "--chain") {
                    for s in &l.chain {
                        let props: Vec<String> = s.props.iter().filter(|(k, _)| !matches!(k.as_str(), "ObjInstanceVersion" | "ParentSequence" | "ObjPosX" | "ObjPosY" | "DrawWidth" | "DrawHeight" | "OutputLinks" | "InputLinks" | "VariableLinks" | "EventLinks" | "ObjComment")).map(|(k, v)| format!("{k}={v}")).collect();
                        println!("    {:<36} {:<28} --{}--> {}", s.class, s.name, s.via, props.join(" "));
                    }
                }
            }
            for s in dishonored_assets::travel::tagged_starts(&c, &pkgs) {
                println!("start {:<14} tag {:<28} ({:.0},{:.0},{:.0})", s.name, s.tag, s.position.x, s.position.y, s.position.z);
            }
        }
        "starts" => {
            let c = Content::locate()?;
            for s in campaign_starts(&c)? {
                println!("{:<40} -> {:<24} {:?}", s.event, s.map, s.secondary);
            }
        }
        "cook" => {
            let c = Content::locate()?;
            let out = a.get(1).map(std::path::PathBuf::from).unwrap_or_else(|| dishonored_content::cache_dir().join("missions"));
            std::fs::create_dir_all(&out)?;
            for (file, def) in cook_campaign(&c)? {
                let path = out.join(&file);
                std::fs::write(&path, ron::ser::to_string_pretty(&def, ron::ser::PrettyConfig::default())?)?;
                println!("{:<34} {:<26} start {:<36} maps {}", file, def.map, def.original_kismet.as_ref().map(|k| k.start_event.as_str()).unwrap_or("-"), def.maps.join(" "));
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
                    // Original conversation references resolve against the dialog data.
                    if !def.conversation_refs().is_empty() {
                        match dishonored_content::load_mission_conversations(&def) {
                            Ok(db) => {
                                errs.extend(dishonored_mission::validate_conversations(&def, &db));
                                let (lines, vo, len) = db.line_stats();
                                println!("dialog: {} packages, {} conversations, {lines} lines ({vo} with VO, {len} decodable)", db.packages.len(), db.conversations.len());
                            }
                            Err(e) => errs.push(format!("dialog data: {e:#}")),
                        }
                    }
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

/// `DefaultGame.ini` `m_MissionsGame` entries: (mission number, maps in order).
/// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission (dhcook `missions_ini`).
fn ini_missions(content: &Content) -> Result<Vec<(i32, Vec<String>)>> {
    let ini = content.cooked.parent().context("install layout")?.join("Config").join("DefaultGame.ini");
    let text = std::fs::read_to_string(&ini).with_context(|| ini.display().to_string())?;
    let mut out = Vec::new();
    for line in text.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix(".m_MissionsGame=") else { continue };
        let num = rest.split("MissionNumber=").nth(1).and_then(|r| r.split(',').next()).and_then(|v| v.trim().parse::<i32>().ok());
        let maps: Vec<String> = rest
            .split("Maps=(")
            .nth(1)
            .and_then(|r| r.split(')').next())
            .map(|m| m.split(',').map(|x| x.trim().trim_matches('"').trim().to_string()).filter(|x| !x.is_empty()).collect())
            .unwrap_or_default();
        if let Some(n) = num {
            out.push((n, maps));
        }
    }
    Ok(out)
}

/// A global level change: the `ChangeLvl_*` remote event (`SeqEvent_RemoteEvent.EventName`), the
/// map its `SeqAct_PrepareMapChange` loads (`MainLevelName`) and the sublevels loaded with it
/// (`InitiallyLoadedSecondaryLevelNames`: which Pub script a hub visit runs).
struct LevelChange {
    event: String,
    map: String,
    secondary: Vec<String>,
}

fn campaign_starts(content: &Content) -> Result<Vec<LevelChange>> {
    let db = dishonored_kismet::load::load(content, &["DishonoredGameFull_P".to_string()], &[])?;
    let mut out = Vec::new();
    for ev in db.ops.iter().filter(|o| o.class == "SeqEvent_RemoteEvent") {
        let name = ev.props.str("EventName").to_string();
        if !name.starts_with("ChangeLvl_") {
            continue;
        }
        // Breadth-first along the output links to the first PrepareMapChange (gates, story-flag
        // conditions and delays may sit in between).
        let mut seen = std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<dishonored_kismet::OpId> = ev.outputs.iter().flat_map(|o| o.links.iter().map(|l| l.0)).collect();
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id) || seen.len() > 96 {
                continue;
            }
            let op = db.op(id);
            if op.class == "SeqAct_PrepareMapChange" {
                let secondary = op.props.get("InitiallyLoadedSecondaryLevelNames").map(|v| v.as_array().iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                out.push(LevelChange { event: name.clone(), map: op.props.str("MainLevelName").to_string(), secondary });
                break;
            }
            queue.extend(op.outputs.iter().flat_map(|o| o.links.iter().map(|l| l.0)));
            // A sequence's inputs start the SeqEvent_SequenceActivated inside it.
            queue.extend(op.inputs.iter().filter_map(|i| i.linked_op));
        }
    }
    Ok(out)
}
/// One mission definition per `m_MissionsGame` entry whose first map a global level change
/// loads, run by the original Kismet (the campaign flow of the persistent level, as the
/// original's: the map's scripts end the mission with their own `ChangeLvl_*` remote event).
fn cook_campaign(content: &Content) -> Result<Vec<(String, MissionDef)>> {
    let starts = campaign_starts(content)?;
    let mut out = Vec::new();
    for (i, (num, maps)) in ini_missions(content)?.into_iter().enumerate() {
        let persistent: Vec<String> = maps.iter().filter(|m| m.ends_with("_P") && content.has_package(m)).cloned().collect();
        // The mission proper starts where the hub (the Pub) or the prologue hands over to the
        // mission's own first map; a hub-only entry (the Pub visits) starts at the hub with its
        // script sublevel.
        let is_hub = |m: &String| m.to_ascii_lowercase().starts_with("l_pub_");
        let own: Vec<&String> = persistent.iter().filter(|m| !is_hub(m)).collect();
        let scripts: Vec<&String> = maps.iter().filter(|m| !m.ends_with("_P")).collect();
        let found = own.iter().find_map(|first| {
            let into: Vec<&LevelChange> = starts.iter().filter(|s| s.map.eq_ignore_ascii_case(first)).collect();
            into.iter().copied().find(|s| s.event.contains("fromHub") || s.event.contains("fromTower")).or_else(|| into.first().copied()).map(|c| ((*first).clone(), c))
        }).or_else(|| {
            let first = persistent.first()?;
            starts.iter().find(|s| s.map.eq_ignore_ascii_case(first) && scripts.iter().all(|sc| s.secondary.iter().any(|x| x.eq_ignore_ascii_case(sc)))).map(|c| (first.clone(), c))
        });
        let Some((first, change)) = found else {
            eprintln!("mission {num} ({persistent:?}): no global level change loads it; skipped");
            continue;
        };
        let mut dialog: Vec<String> = Vec::new();
        for m in &persistent {
            for p in dishonored_assets::map_packages(content, m) {
                if !dialog.iter().any(|d: &String| d.eq_ignore_ascii_case(&p)) {
                    dialog.push(p);
                }
            }
        }
        // Script-only sublevels named in the mission list (`L_Pub_FromPrison_Script`...) stream
        // with their persistent map; their dialog graphs are loaded too.
        for m in maps.iter().filter(|m| !m.ends_with("_P") && content.has_package(m)) {
            if !dialog.iter().any(|d| d.eq_ignore_ascii_case(m)) {
                dialog.push(m.clone());
            }
        }
        let def = MissionDef {
            title: format!("Mission {num}: {first}"),
            map: first.clone(),
            maps: persistent.iter().filter(|m| !m.eq_ignore_ascii_case(&first)).cloned().collect(),
            briefing: format!("Cooked from DefaultGame.ini m_MissionsGame {i}; started by {}.", change.event),
            flags_set: vec!["SF_Global_RealGameStarted".into()],
            original_npcs: persistent.clone(),
            dialog_packages: dialog,
            original_kismet: Some(dishonored_mission::OriginalKismet { start_event: change.event.clone(), seed: 1 }),
            settings: dishonored_mission::MissionSettings { respawn: true, respawn_delay: 3.0, time_limit: 0.0, ..Default::default() },
            ..Default::default()
        };
        out.push((format!("m{:02}_{}.ron", i, first.trim_start_matches("L_").trim_end_matches("_P").to_ascii_lowercase()), def));
    }
    Ok(out)
}
