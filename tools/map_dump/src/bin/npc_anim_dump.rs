//! `npc_anim_dump defs <Class> [pkg,...]` | `npc_anim_dump loco <package> <config path>` |
//! `npc_anim_dump tree <package> <object path>` |
//! `npc_anim_dump sets <package> <AnimSet path>...`: print decoded NPC animation data
//! (animation definition chains, locomotion speed modes, AnimTree node graphs, AnimSet sequences).
//! Research output is derived from retail data: keep it out of the repository.
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::npc_anim::*;
use dishonored_assets::anim_tree::load_anim_tree;
use dishonored_ir::npc_anim::NPC_ANIM_NAMES;
use dishonored_ir::anim_tree::NodeProps;
use ue3_package::ObjRef;
use std::collections::HashSet;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    match a.first().map(String::as_str) {
        Some("defs") => {
            let pkgs: Vec<String> = a.get(2).map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_else(|| vec!["L_Boyle_Ext_Script".into(), "L_Boyle_Int_Script".into()]);
            let d = load_anim_defs(&c, &pkgs, &a[1])?;
            for (i, ch) in d.chains.iter().enumerate() {
                if ch.groups.is_empty() {
                    continue;
                }
                println!("{i:3} {}{}", NPC_ANIM_NAMES.get(i).unwrap_or(&"?"), if ch.ragdoll_if_pushed { " (ragdoll if pushed)" } else { "" });
                for (g, grp) in ch.groups.iter().enumerate() {
                    let steps: Vec<String> = grp
                        .steps
                        .iter()
                        .map(|s| format!("{}{}{}{}", if s.item_prefix != 0 { format!("<p{}>", s.item_prefix) } else { String::new() }, s.names.join("|"), if s.looped { " (loop)" } else { "" }, if s.optional { " (opt)" } else { "" }))
                        .collect();
                    println!("      g{g} [{}]{}", steps.join(" -> "), if grp.end_after_one { " end-after-one" } else { "" });
                }
            }
        }
        Some("loco") => {
            let l = load_loco_config(&c, &(a[1].clone(), a[2].clone()))?;
            println!("speeds {:?}", l.speed_names);
            for (mi, m) in l.modifiers.iter().enumerate() {
                let modes: Vec<String> = (0..l.speed_names.len()).map(|s| l.mode(mi, s).map(|m| m.name.clone()).unwrap_or("-".into())).collect();
                println!("{mi:2} {} x{} acc {} dec {} -> {}", m.name, m.speed_multiplier, m.acceleration, m.deceleration, modes.join(", "));
            }
            for m in &l.modes {
                println!("  {} rot {} {:?} start {:?} stop {:?}", m.name, m.rotation_max_speed, m.move_anims, m.start_anims, m.stop_anims);
            }
            println!("transitions {:?}", l.transitions);
        }
        Some("tree") => {
            let tree = load_anim_tree(&c, &a[1], &a[2])?;
            println!("AnimTree: {}", tree.name);
            if let Some(root_idx) = tree.root {
                print_tree_node(&tree.nodes, root_idx, "", &mut HashSet::new());
            }
            if !tree.skel_controls.is_empty() {
                println!("\nSkeletal controls:");
                for entry in &tree.skel_controls {
                    print!("  {}: ", entry.bone);
                    for (i, (class, name)) in entry.controls.iter().enumerate() {
                        if i > 0 {
                            print!(", ");
                        }
                        print!("{} \"{}\"", class, name);
                    }
                    println!();
                }
            }
        }
        Some("sets") => {
            let pkg = c.package(&a[1])?;
            for path in &a[2..] {
                let Some(i) = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path)) else { continue };
                println!("{path} skeleton {:?}", anim_set_skeleton(&pkg, i));
                println!("  {}", anim_set_sequences(&pkg, i).join(" "));
            }
        }
        Some("names") => {
            // `names <package> <export path> <property>`: an array<name> property.
            let pkg = c.package(&a[1])?;
            let i = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&a[2])).ok_or_else(|| anyhow::anyhow!("no export"))?;
            let h = pkg.read_object(i)?;
            if let Some((n, raw)) = ue3_package::properties::find(&h.properties, &a[3]).and_then(|p| p.array_raw()) {
                let mut r = ue3_package::reader::Reader::new(raw);
                let v: Vec<String> = (0..n).filter_map(|_| ue3_package::FName::read(&mut r).ok().map(|f| pkg.name(f))).collect();
                println!("{v:?}");
            }
        }
        Some("props") => {
            // `props <package> <export path>`: print all property names.
            let pkg = c.package(&a[1])?;
            let i = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&a[2])).ok_or_else(|| anyhow::anyhow!("no export"))?;
            let h = pkg.read_object(i)?;
            for p in &h.properties {
                println!("{}: {:?}", p.name, p.type_name);
            }
        }
        _ => eprintln!("usage: npc_anim_dump defs <Class> [pkgs] | loco <pkg> <path> | tree <pkg> <path> | sets <pkg> <set>..."),
    }
    Ok(())
}

/// Print an AnimTree node and its children, recursively.
fn print_tree_node(
    nodes: &[dishonored_ir::anim_tree::AnimNode],
    idx: usize,
    indent: &str,
    visited: &mut HashSet<usize>,
) {
    if visited.contains(&idx) {
        println!("{}(shared #{} \"{}\")", indent, idx, nodes[idx].name);
        return;
    }
    visited.insert(idx);

    let node = &nodes[idx];
    let mut props_str = String::new();

    match &node.props {
        NodeProps::Sequence { seq, rate, looping, .. } => {
            if !seq.is_empty() {
                props_str = format!(" seq=\"{}\" rate={}", seq, rate);
                if *looping {
                    props_str.push_str(" (loop)");
                }
            }
        }
        NodeProps::BlendPerBone { branches, exclude_branch, .. } => {
            if !branches.is_empty() {
                props_str = format!(" branches={:?}", branches);
                if *exclude_branch {
                    props_str.push_str(" (exclude)");
                }
            }
        }
        NodeProps::Locomotion { rate, range, min_ratio, .. } => {
            props_str = format!(" rate={} range={} min_ratio={}", rate, range, min_ratio);
        }
        NodeProps::AimOffset { grids } => {
            if !grids.is_empty() {
                props_str = format!(" grids={:?}", grids);
            }
        }
        NodeProps::TreeRef { templates } => {
            if !templates.is_empty() {
                props_str = format!(" templates={:?}", templates.iter().map(|(n, p)| format!("{n}={p}")).collect::<Vec<_>>());
            }
        }
        _ => {}
    }

    println!(
        "{}{} \"{}\"{}",
        indent,
        node.class,
        node.name,
        props_str
    );

    for child in &node.children {
        if let Some(child_idx) = child.node {
            let child_name = if child.name.is_empty() { format!("#{}", child_idx) } else { child.name.clone() };
            let mut flags = Vec::new();
            if child.mirror {
                flags.push("mirror");
            }
            if child.additive {
                flags.push("additive");
            }
            let flags_str = if flags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", flags.join(", "))
            };

            println!("{}{}{}{}", indent, "  ", child_name, flags_str);
            print_tree_node(nodes, child_idx, &format!("{}  ", indent), visited);
        }
    }
}
