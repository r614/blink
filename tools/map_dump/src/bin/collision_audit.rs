//! `collision_audit [map...] [--list <class>] [--meshes]`: the collision sources of maps with
//! their effective flags (research/dishonored/COLLISION.md). Output is derived from retail
//! data: keep it out of the repository.
use anyhow::Result;
use dishonored_assets::collision_audit::{CollisionSource, collision_sources};
use dishonored_assets::{Content, map_packages};
use std::collections::BTreeMap;
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

fn mask_str(m: u8) -> String {
    let c = ['n', 'p', 'P', 'c', 'j', 'm', 'v'];
    (0..7).map(|b| if m & (1 << b) != 0 { c[b] } else { '-' }).collect()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let list = args.iter().position(|a| a == "--list").and_then(|i| args.get(i + 1)).cloned();
    let meshes = args.iter().any(|a| a == "--meshes");
    let mut maps: Vec<String> = args.iter().filter(|a| a.starts_with("L_")).cloned().collect();
    if maps.is_empty() {
        maps = vec!["L_Boyle_Ext_P".into(), "L_Boyle_Int_P".into()];
    }
    let c = Content::locate()?;
    for map in &maps {
        let pkgs = map_packages(&c, map);
        println!("##### {map}: {}", pkgs.join(", "));
        let s = collision_sources(&c, &pkgs);
        // Group: class / component class / active / block / world / hidden / mask.
        let mut groups: BTreeMap<String, usize> = BTreeMap::new();
        for x in &s {
            let k = format!(
                "{:<34} {:<24} port={} active={} block={} actorblock={} world={} hid={}{} mask={} ctype={} {}",
                port_rule(x),
                x.class,
                x.component_class,
                x.active() as u8,
                x.block as u8,
                x.actor_block as u8,
                x.world_geometry as u8,
                x.actor_hidden as u8,
                x.hidden_game as u8,
                mask_str(x.trace_mask),
                x.collision_type,
                x.extra.join("+")
            );
            *groups.entry(k).or_default() += 1;
        }
        for (k, n) in &groups {
            println!("{n:>6}  {k}");
        }
        if let Some(cl) = &list {
            for x in s.iter().filter(|x| x.class.eq_ignore_ascii_case(cl) || x.component.eq_ignore_ascii_case(cl) || x.asset.to_ascii_lowercase().contains(&cl.to_ascii_lowercase())) {
                print_source(x);
            }
        }
        if meshes {
            mesh_report(&c, &pkgs, &s);
        }
    }
    Ok(())
}

/// What the port (before this audit) did with a source: `S` static triangles on every channel,
/// `M` mover, `B` blocking volume (pawn only), `-` nothing.
fn port_rule(x: &CollisionSource) -> char {
    let comp = x.own_collide.unwrap_or(true) && x.own_block.unwrap_or(true);
    match x.class.as_str() {
        "StaticMeshActor" | "StaticMeshCollectionActor" if comp => 'S',
        "InterpActor" if comp && x.own_actor_collide.unwrap_or(true) && x.own_actor_block.unwrap_or(true) => 'M',
        "BlockingVolume" => 'B',
        _ => '-',
    }
}

fn print_source(x: &CollisionSource) {
    println!(
        "  {}:{} [{}] {}.{} at ({:.0}, {:.0}, {:.0}) active={} block={} mask={} asset={} {}",
        x.package,
        x.actor,
        x.class,
        x.component,
        x.component_class,
        x.location.x,
        x.location.y,
        x.location.z,
        x.active() as u8,
        x.block as u8,
        mask_str(x.trace_mask),
        x.asset,
        x.extra.join("+")
    );
}

/// Per static mesh of the active static mesh components: kDOP triangle count, body setup
/// elements, and the tagged `UseSimple*` overrides when serialized.
fn mesh_report(c: &Content, pkgs: &[String], s: &[CollisionSource]) {
    let mut uses: BTreeMap<String, usize> = BTreeMap::new();
    for x in s.iter().filter(|x| x.active() && x.component_class.contains("StaticMesh")) {
        *uses.entry(x.asset.clone()).or_default() += 1;
    }
    let (mut with_body, mut without, mut simple_flags) = (0, 0, 0);
    for (path, n) in &uses {
        let mut found = None;
        for p in pkgs {
            let Ok(pkg) = c.package(p) else { continue };
            if let Some(i) = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "StaticMesh" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path)) {
                found = Some((pkg, i));
                break;
            }
        }
        let Some((pkg, i)) = found else {
            println!("  mesh {path} x{n}: not found");
            continue;
        };
        let props = pkg.read_object(i).map(|h| h.properties).unwrap_or_default();
        let flags: Vec<String> = ["UseSimpleLineCollision", "UseSimpleBoxCollision", "UseSimpleRigidBodyCollision", "bUsedForInstancing"]
            .iter()
            .filter_map(|k| match find(&props, k).map(|p| &p.value) {
                Some(Value::Bool(b)) => Some(format!("{k}={b}")),
                _ => None,
            })
            .collect();
        if !flags.is_empty() {
            simple_flags += 1;
        }
        let raw = dishonored_assets::static_mesh::read_static_mesh(&pkg, i);
        let (kdop, body) = match &raw {
            Ok(m) => (m.kdop_triangles.len(), m.body_setup),
            Err(_) => (0, 0),
        };
        let mut elems = String::from("-");
        if body != 0 {
            with_body += 1;
            if let ObjRef::Export(b) = ObjRef::from_index(body) {
                if let Ok(h) = pkg.read_object(b) {
                    if let Some(Value::Struct(_, agg)) = find(&h.properties, "AggGeom").map(|p| &p.value) {
                        let cnt = |k: &str| find(agg, k).and_then(|p| p.array_raw()).map(|r| r.0).unwrap_or(0);
                        elems = format!("convex {} box {} sphere {} sphyl {}", cnt("ConvexElems"), cnt("BoxElems"), cnt("SphereElems"), cnt("SphylElems"));
                    } else {
                        elems = "no AggGeom".into();
                    }
                }
            }
        } else {
            without += 1;
        }
        println!("  mesh {path} x{n}: kdop {kdop} body {elems} {}", flags.join(" "));
    }
    println!("  meshes: {} ({} with a body setup, {} without, {} with serialized UseSimple flags)", uses.len(), with_body, without, simple_flags);
}
