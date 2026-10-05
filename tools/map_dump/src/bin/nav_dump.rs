//! Original navigation meshes (research; output is derived from retail data, keep it in
//! `harness_out/`).
//!
//! `nav_dump survey`            every `L_*` package with a pylon: versions, counts, edge classes/types
//! `nav_dump map <L_..._P>`     the map's pylons: counts, edge types, one-way and path-object edges
//! `nav_dump obj <L_..._P> <out.obj>`  the walkable polys as a Wavefront OBJ (for viewing)
use anyhow::Result;
use dishonored_assets::navigation::{load_nav_data, read_package_pylons};
use dishonored_assets::{Content, map_packages};
use dishonored_ir::nav::{NavEdgeClass, NavMesh};
use std::collections::BTreeMap;
use std::io::Write;

fn summary(m: &NavMesh) -> String {
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for e in &m.edges {
        *kinds.entry(format!("{:?}/{:?}", e.class, e.kind)).or_default() += 1;
    }
    let area: f32 = m.polys.iter().map(|p| poly_area(m, p)).sum();
    format!("v{:?} verts {} polys {} edges {} area {:.0} (100uu)^2 bounds {:.0?}..{:.0?} {:?}", m.version, m.verts.len(), m.polys.len(), m.edges.len(), area / 10000.0, m.min, m.max, kinds)
}

fn poly_area(m: &NavMesh, p: &dishonored_ir::nav::NavPoly) -> f32 {
    let v: Vec<glam::Vec3> = p.verts.iter().map(|&i| m.verts[i as usize]).collect();
    let mut a = 0.0;
    for i in 1..v.len().saturating_sub(1) {
        a += (v[i] - v[0]).cross(v[i + 1] - v[0]).length() * 0.5;
    }
    a
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    match a.first().map(|s| s.as_str()) {
        Some("survey") => {
            let mut names: Vec<String> = std::fs::read_dir(&c.cooked)?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("L_") && n.ends_with(".upk"))
                .map(|n| n.trim_end_matches(".upk").to_string())
                .collect();
            names.sort();
            let mut totals: BTreeMap<String, usize> = BTreeMap::new();
            let (mut ok, mut fail) = (0, 0);
            for n in names {
                let Ok(pkg) = c.package(&n) else { continue };
                if !(0..pkg.exports.len()).any(|i| pkg.export_class(i).ends_with("Pylon")) {
                    continue;
                }
                match read_package_pylons(&pkg) {
                    Ok(ps) => {
                        ok += 1;
                        for p in &ps {
                            for e in &p.mesh.edges {
                                *totals.entry(format!("{:?}/{:?}", e.class, e.kind)).or_default() += 1;
                            }
                            println!("{n}: {} tallboy={} {}", p.name, p.tall_boy, summary(&p.mesh));
                            if let Some(o) = &p.obstacle {
                                println!("    obstacle: {}", summary(o));
                            }
                        }
                    }
                    Err(e) => {
                        fail += 1;
                        println!("{n}: ERROR {e:#}");
                    }
                }
            }
            println!("packages ok {ok} failed {fail}; edge totals {totals:?}");
        }
        Some("map") => {
            let map = a.get(1).cloned().unwrap_or("L_Boyle_Ext_P".into());
            let nav = load_nav_data(&c, &map_packages(&c, &map))?;
            for p in &nav.pylons {
                println!("{}:{} at {:?} guid {:08x?} tallboy {}", p.package, p.name, p.location, p.nav_guid, p.tall_boy);
                println!("  mesh {}", summary(&p.mesh));
                let m = &p.mesh;
                let hs: Vec<f32> = m.polys.iter().map(|p| p.height).collect();
                let mut hh: BTreeMap<i32, usize> = BTreeMap::new();
                for h in hs {
                    *hh.entry((h / 25.0).round() as i32 * 25).or_default() += 1;
                }
                println!("  poly heights {hh:?}");
                for (i, e) in m.edges.iter().enumerate() {
                    if e.class == NavEdgeClass::Base {
                        continue;
                    }
                    let (a, b) = (m.polys[e.poly0 as usize % m.polys.len()].center, m.polys[e.poly1 as usize % m.polys.len()].center);
                    println!("  edge {i} {:?}/{:?} v{}-{} p{}->{} len {:.0}/{:.0} center {:.0?} dz(p1->p0) {:.0} po {:?}", e.class, e.kind, e.v0, e.v1, e.poly1, e.poly0, e.length, e.effective_length, e.center, a.z - b.z, e.path_object.as_ref().map(|l| (l.poly_refs[0].2, l.poly_refs[1].2, format!("{:08x?}", l.path_object.1), l.internal_id)));
                }
                if let Some(o) = &p.obstacle {
                    println!("  obstacle {}", summary(o));
                }
            }
        }
        Some("obj") => {
            let map = a.get(1).cloned().unwrap_or("L_Boyle_Ext_P".into());
            let nav = load_nav_data(&c, &map_packages(&c, &map))?;
            let mut f = std::io::BufWriter::new(std::fs::File::create(a.get(2).cloned().unwrap_or("harness_out/nav.obj".into()))?);
            let mut base = 1;
            for p in &nav.pylons {
                for v in &p.mesh.verts {
                    writeln!(f, "v {} {} {}", v.x, v.z, v.y)?;
                }
                for poly in &p.mesh.polys {
                    let idx: Vec<String> = poly.verts.iter().map(|&i| (i as usize + base).to_string()).collect();
                    writeln!(f, "f {}", idx.join(" "))?;
                }
                base += p.mesh.verts.len();
            }
        }
        _ => eprintln!("usage: nav_dump survey | map <map> | obj <map> <out.obj>"),
    }
    Ok(())
}
