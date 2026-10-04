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
    let fmts: BTreeMap<String, usize> = m.textures.iter().fold(BTreeMap::new(), |mut acc, t| {
        *acc.entry(format!("{:?} srgb={}", t.format, t.srgb)).or_default() += 1;
        acc
    });
    println!("texture formats {fmts:?}");
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
        .flat_map(|a| a.texels.iter().filter(|&&t| t != 0))
        .map(|&t| {
            let c = dishonored_assets::lightmap::unpack_rgb9e5(t);
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
