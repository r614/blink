//! Import a map through the full pipeline and print what came out.
use anyhow::Result;
use dishonored_assets::{Content, ImportOptions, import_level, map_packages};
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let map = args.first().map(String::as_str).unwrap_or("L_Boyle_Ext_P");
    let content = Content::locate()?;
    if let Some(n) = args.iter().position(|a| a == "--materials") {
        return dump_materials(&content, &args[n + 1]);
    }
    let pkgs = map_packages(&content, map);
    println!("packages: {pkgs:?}");
    if args.iter().any(|a| a == "--survey") {
        // Which actor classes own primitive components, per package (coverage audit).
        use ue3_package::ObjRef;
        for p in &pkgs {
            let pkg = content.package(p)?;
            let mut owners: BTreeMap<String, usize> = BTreeMap::new();
            for i in 0..pkg.exports.len() {
                let c = pkg.export_class(i);
                if !(c.ends_with("MeshComponent") || c == "BrushComponent" || c == "Model") {
                    continue;
                }
                let outer = ObjRef::from_index(pkg.exports[i].outer);
                let oc = if outer == ObjRef::Null { "<none>".into() } else { pkg.class_name(outer) };
                let in_level = pkg.obj_name(pkg.obj_outer(outer)) == "PersistentLevel" || pkg.obj_name(outer) == "PersistentLevel";
                *owners.entry(format!("{c} <- {oc}{}", if in_level { "" } else { " (not placed)" })).or_default() += 1;
            }
            println!("== {p}");
            for (k, n) in owners {
                println!("  {n:5} {k}");
            }
        }
        return Ok(());
    }
    let t = std::time::Instant::now();
    let opts = ImportOptions {
        load_textures: !args.iter().any(|a| a == "--no-textures"),
        ..Default::default()
    };
    let (m, stats) = import_level(&content, map, &pkgs, &opts)?;
    println!("import took {:.2?}", t.elapsed());
    let mut s = stats.clone();
    let errs = std::mem::take(&mut s.errors);
    println!("{s:#?}");
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for e in &errs {
        let k: String = e.split(':').last().unwrap_or("").chars().take(70).collect();
        *kinds.entry(k).or_default() += 1;
    }
    for (k, n) in kinds.iter().take(30) {
        println!("err x{n}: {k}");
    }
    for e in errs.iter().take(5) {
        println!("sample error: {e}");
    }
    let tris: usize = m.meshes.iter().map(|x| x.indices.len() / 3).sum();
    let ctris: usize = m.meshes.iter().map(|x| x.collision_triangles.len()).sum();
    println!("unique mesh triangles {tris}, kDOP collision triangles {ctris}");
    let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
    for i in m.instances.iter().filter(|i| !i.hidden) {
        let p = i.transform.w_axis.truncate();
        lo = lo.min(p);
        hi = hi.max(p);
    }
    println!("instance origin bounds {lo} .. {hi}");
    println!("spawns: {:?}", m.spawn_candidates.iter().take(5).collect::<Vec<_>>());
    if let Some(n) = args.iter().position(|a| a == "--inst") {
        for i in m.instances.iter().take(args[n + 1].parse()?) {
            println!("{} mesh={} hidden={} m={:?}", i.source, m.meshes[i.mesh].source, i.hidden, i.transform.to_cols_array_2d());
        }
    }
    // `--near X Y R`: visible instances whose world bounds come within R of (X, Y), with their
    // materials' blend modes (finding what a capture shows at a place).
    if let Some(n) = args.iter().position(|a| a == "--near") {
        let (x, y, r): (f32, f32, f32) = (args[n + 1].parse()?, args[n + 2].parse()?, args[n + 3].parse()?);
        for i in m.instances.iter().filter(|i| !i.hidden) {
            let mesh = &m.meshes[i.mesh];
            let (a, b) = (i.transform.transform_point3(mesh.bounds_min), i.transform.transform_point3(mesh.bounds_max));
            let (lo, hi) = (a.min(b), a.max(b));
            let d = glam::Vec2::new((x - x.clamp(lo.x, hi.x)).abs(), (y - y.clamp(lo.y, hi.y)).abs()).length();
            if d > r {
                continue;
            }
            let mats: Vec<String> = mesh.sections.iter().enumerate().map(|(si, s)| {
                let mid = i.materials.get(si).copied().flatten().or(s.material);
                mid.map(|k| format!("{} {:?} {:?}", m.materials[k].source.rsplit('.').next().unwrap_or(""), m.materials[k].blend, m.material_params.get(k).map(|p| p.blend))).unwrap_or_default()
            }).collect();
            // Mean flat irradiance luminance at the mesh's UV1 vertices (lit texels only).
            let lm = i
                .lightmap
                .map(|l| {
                    use dishonored_assets::lightmap::{directional_irradiance, unpack_rgb9e5};
                    let a = &m.lightmaps[l.atlas];
                    let (mut sum, mut n, mut zero) = (0.0f32, 0usize, 0usize);
                    for u in &mesh.uv1 {
                        let uv = l.uv_min + *u * (l.uv_max - l.uv_min);
                        let (x, y) = (((uv.x * a.width as f32) as u32).min(a.width - 1), ((uv.y * a.height as f32) as u32).min(a.height - 1));
                        let k = (y * a.width + x) as usize;
                        if a.texels[k] == 0 {
                            zero += 1;
                            continue;
                        }
                        let c = directional_irradiance(unpack_rgb9e5(a.texels[k]), a.directional.get(k).map(|d| unpack_rgb9e5(*d)).unwrap_or([1.0; 3]), [0.0, 0.0, 1.0]);
                        sum += 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                        n += 1;
                    }
                    format!("atlas {} uv {:.3}..{:.3} lum {:.3} ({n} lit, {zero} empty)", l.atlas, l.uv_min, l.uv_max, sum / n.max(1) as f32)
                })
                .unwrap_or_else(|| "none".into());
            println!("{:.0} uu: {} [{}] bounds {:.0}..{:.0} collides {} lightmap {lm} uv1 {} mats {mats:?}", d, mesh.source, i.source, lo, hi, i.collides, mesh.uv1.len() == mesh.positions.len());
        }
        for e in &m.emitters {
            let p = e.transform.w_axis.truncate();
            let d = glam::Vec2::new(p.x - x, p.y - y).length();
            if d <= r {
                let s = &m.particle_systems[e.system];
                println!("{d:.0} uu: emitter {} at {p:.0} xf {:?} emitters {:?}", s.source, e.transform.to_scale_rotation_translation().0, s.emitters.iter().map(|em| (em.mesh.clone(), em.blend, em.material.map(|k| m.materials[k].source.rsplit('.').next().unwrap_or("").to_string()), format!("size {:?} color {:?} alpha {:?}", em.size, em.color, em.alpha))).collect::<Vec<_>>());
            }
        }
    }
    // `--mesh SUBSTR`: vertex / triangle counts, bounds and UV ranges of matching meshes.
    if let Some(n) = args.iter().position(|a| a == "--mesh") {
        for mesh in m.meshes.iter().filter(|x| x.source.contains(&args[n + 1])) {
            let uv = |v: &[glam::Vec2]| v.iter().fold((glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN)), |(a, b), u| (a.min(*u), b.max(*u)));
            let pb = mesh.positions.iter().fold((glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
            let up = mesh.normals.iter().filter(|n| n.z.abs() > 0.7).count();
            println!(
                "mesh {} verts {} tris {} bounds {:.0}..{:.0} positions {:.0}..{:.0} normals |z|>0.7 {up} uv0 {:?} uv1 {:?} sections {:?}",
                mesh.source,
                mesh.positions.len(),
                mesh.indices.len() / 3,
                mesh.bounds_min,
                mesh.bounds_max,
                pb.0,
                pb.1,
                uv(&mesh.uv0),
                uv(&mesh.uv1),
                mesh.sections.iter().map(|s| (s.first_index, s.num_triangles, s.material)).collect::<Vec<_>>()
            );
        }
    }
    let fmts: BTreeMap<String, usize> = m.textures.iter().fold(BTreeMap::new(), |mut acc, t| {
        *acc.entry(format!("{:?} srgb={}", t.format, t.srgb)).or_default() += 1;
        acc
    });
    println!("texture formats {fmts:?}");
    for mesh in m.meshes.iter().filter(|x| x.source.starts_with("Skies") || x.source.contains("vista")) {
        println!("mesh {} bounds {:.0} .. {:.0}", mesh.source, mesh.bounds_min, mesh.bounds_max);
    }
    for l in &m.lights {
        println!(
            "light {:?} dominant={} shadows={} pos {:.0} dir {:.2} color {:.2?} brightness {} radius {} falloff {} <- {}",
            l.kind, l.dominant, l.casts_shadows, l.position, l.direction, l.color, l.brightness, l.radius, l.falloff_exponent, l.source
        );
    }
    println!("environment: cubemap faces {}, post {:?}", m.environment.cubemap_faces.len(), m.environment.post);
    for f in &m.environment.fog {
        println!("fog {f:?}");
    }
    // `--dump-texture SUBSTR OUT.rgba`: raw RGBA8 of the first texture whose source contains
    // SUBSTR (local inspection only; convert with ffmpeg -f rawvideo -pix_fmt rgba -s WxH).
    if let Some(n) = args.iter().position(|a| a == "--dump-texture") {
        if let Some(t) = m.textures.iter().find(|t| t.source.contains(&args[n + 1])) {
            if let Some((w, h, px)) = dishonored_assets::texture::decode_rgba8(t, 0) {
                std::fs::write(&args[n + 2], px.concat())?;
                println!("dumped {} {}x{}", t.source, w, h);
            }
        }
    }
    if args.iter().any(|a| a == "--material-params") {
        for (i, p) in m.material_params.iter().enumerate() {
            println!(
                "MAT {} | blend {:?} known={} two_sided={} unlit={} spec={} emis={:?} opac={} cube={} diffuse={:?} | base {:?}\n    scalars {:?}\n    vectors {:?}",
                m.materials[i].source,
                p.blend,
                p.blend_known,
                p.two_sided,
                p.unlit,
                p.specular.is_some(),
                p.emissive.map(|t| &m.textures[t].source),
                p.opacity.is_some(),
                p.uses_cubemap,
                m.materials[i].diffuse.map(|t| &m.textures[t].source),
                p.base_material,
                p.scalars,
                p.vectors
            );
        }
    }
    // Baked irradiance distribution (luminance of non-empty texels), for calibration.
    let mut lum: Vec<f32> = m
        .lightmaps
        .iter()
        .flat_map(|a| a.texels.iter().zip(a.directional.iter()).filter(|(t, _)| **t != 0))
        .map(|(&t, &d)| {
            use dishonored_assets::lightmap::{directional_irradiance, unpack_rgb9e5};
            let c = directional_irradiance(unpack_rgb9e5(t), unpack_rgb9e5(d), [0.0, 0.0, 1.0]);
            0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
        })
        .collect();
    if !lum.is_empty() {
        lum.sort_by(f32::total_cmp);
        let pct = |p: f32| lum[((lum.len() - 1) as f32 * p) as usize];
        println!(
            "lightmap irradiance luminance: {} texels, p10 {:.3} p50 {:.3} p90 {:.3} p99 {:.3} max {:.3}",
            lum.len(),
            pct(0.1),
            pct(0.5),
            pct(0.9),
            pct(0.99),
            lum[lum.len() - 1]
        );
    }
    report_unmaterialed(&m);
    Ok(())
}

/// `map_dump --materials <package>`: list every material's texture parameters.
#[allow(dead_code)]
pub fn dump_materials(content: &Content, pkg_name: &str) -> Result<()> {
    use dishonored_assets::material::resolve_material;
    let pkg = content.package(pkg_name)?;
    for i in 0..pkg.exports.len() {
        let c = pkg.export_class(i);
        if c != "MaterialInstanceConstant" && c != "Material" {
            continue;
        }
        let r = resolve_material(&pkg, ue3_package::ObjRef::Export(i))?;
        let params: Vec<String> = r
            .texture_params
            .iter()
            .map(|(n, t)| format!("{n}={}", pkg.obj_name(*t)))
            .collect();
        println!(
            "{c} {} | diffuse={:?} | {}",
            pkg.obj_path(ue3_package::ObjRef::Export(i)),
            r.diffuse.map(|t| pkg.obj_name(t)),
            params.join(" ")
        );
    }
    Ok(())
}

/// Meshes whose sections end up with no material at all (render as fallback grey).
pub fn report_unmaterialed(m: &dishonored_ir::DishonoredMap) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for inst in m.instances.iter().filter(|i| !i.hidden) {
        let mesh = &m.meshes[inst.mesh];
        for (si, s) in mesh.sections.iter().enumerate() {
            let mat = inst.materials.get(si).copied().flatten().or(s.material);
            let untextured = mat.is_none_or(|id| m.materials[id].diffuse.is_none());
            if untextured {
                *counts.entry(format!("{} [sec {si}] mat={:?}", mesh.source, mat.map(|id| &m.materials[id].source))).or_default() += 1;
            }
        }
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    println!("untextured section kinds: {}", v.len());
    for (k, n) in v.iter().take(25) {
        println!("  x{n} {k}");
    }
}
