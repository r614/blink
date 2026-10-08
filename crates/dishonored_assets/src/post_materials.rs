// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
//! The original post-process graph's materials (`AltScreen_Effects.PostProcessChain.Test_PPG`):
//! the vector fields, compositions and screen effects the runtime's `ppgraph` draws with their
//! translated shaders, linked to their shader maps by the level importer's material code, with
//! the curves of the time-varying ones; and the graph nodes that grade the image their own way
//! while they run (`ArkPpNodeMaterial::m_bOverrideUberPp`). From dishonored-bevy's cooker
//! (`POST_MATERIALS`, `mitv_curves`, gamedata's `post_nodes`).

use super::*;
use dishonored_ir::PostCurve;
use std::collections::BTreeMap;

/// The graph's materials by the names `ppgraph` asks for, and their object paths.
pub const POST_MATERIALS: &[(&str, &str)] = &[
    ("bendtime_vectors", "AltScreen_Effects.PostProcessMaterial.PPG_BendTimeVectors_Std_INST"),
    ("bendtime_vectors_water", "AltScreen_Effects.PostProcessMaterial.PPG_BendTimeVectors_UnderWater_INST"),
    ("blink_vectors", "AltScreen_Effects.PostProcessMaterial.PPG_BlinkVectors_Std_INST"),
    ("blink_vectors_previous", "AltScreen_Effects.PostProcessMaterial.PPG_BlinkVectors_WithPrevious_INST"),
    ("underwater_vectors", "AltScreen_Effects.PostProcessMaterial.PPG_UnderWaterVectorField"),
    ("adrenaline_vectors", "AltScreen_Effects.PostProcessMaterial.PPG_AdrenalineVectors_INST"),
    ("possession_vectors", "AltScreen_Effects.PostProcessMaterial.PPG_PossessionVectors"),
    ("ko_vectors", "AltScreen_Effects.PostProcessMaterial.PPG_KOVectors"),
    ("lens_vectors", "AltScreen_Effects.PostProcessMaterial.PPG_LensVectors"),
    ("ko_combine", "AltScreen_Effects.PostProcessMaterial.PPG_KO_Combine"),
    ("ko_backup", "AltScreen_Effects.PostProcessMaterial.TEST_PPG_BackupRed"),
    ("lens_compose", "AltScreen_Effects.PostProcessMaterial.PPG_LensCompose"),
    ("blinded", "AltScreen_Effects.PostProcessMaterial.TEST_PPG_Blinded_INST"),
    ("spyglass", "AltScreen_Effects.PostProcessMaterial.TEST_PPG_Lens"),
    ("outsider", "AltScreen_Effects.PostProcessMaterial.PPG_Outsider"),
    ("possession_in", "Screen_Effects.PostProcessMaterial.POSSESSION_IN_INST"),
    ("possession_out", "Screen_Effects.PostProcessMaterial.POSSESSION_OUT_INST"),
];

/// The graph's materials: `map` holds their [`MaterialParams::ue3`] links and the textures they
/// sample; `materials` lists (name, material in `map`, time-varying curves).
pub struct PostMaterials {
    pub map: DishonoredMap,
    pub materials: Vec<(String, MaterialId, Vec<PostCurve>)>,
    /// The nodes that override the uber post settings while they run, by `EffectName`
    /// ("BendTimeVectors", "BlinkVectors2"...): their `m_UberParameters`, flattened
    /// (`m_CBParameters.m_CrMgYbShadTones.x`, `m_HDRParameters.m_Exposure`...).
    pub nodes: BTreeMap<String, BTreeMap<String, f32>>,
}

/// Import the graph's materials from the always-loaded packages (`LIBRARY_PACKAGES`).
pub fn import_post_materials(content: &Content, max_texture_size: u32) -> Result<PostMaterials> {
    let mut pkgs = Vec::new();
    for p in LIBRARY_PACKAGES {
        if content.has_package(p) {
            pkgs.push(content.package(p).with_context(|| format!("loading {p}"))?);
        }
    }
    let mut index = HashMap::new();
    for (slot, pkg) in pkgs.iter().enumerate() {
        for i in 0..pkg.exports.len() {
            let class = pkg.export_class(i);
            if RESOURCE_CLASSES.contains(&class.as_str()) || class == "MaterialInstanceTimeVarying" {
                index.entry(pkg.obj_path(ObjRef::Export(i)).to_ascii_lowercase()).or_insert((slot, i));
            }
        }
    }
    let opts = ImportOptions { max_texture_size, ..Default::default() };
    let mut imp = Importer {
        content,
        opts: &opts,
        pkgs,
        index,
        map: DishonoredMap { name: "post materials".into(), ..Default::default() },
        mesh_ids: HashMap::new(),
        mat_ids: HashMap::new(),
        tex_ids: HashMap::new(),
        pending_lightmaps: Vec::new(),
        merged_ids: HashMap::new(),
        stats: ImportStats::default(),
        ps_ids: HashMap::new(),
        class_defaults: HashMap::new(),
        skel_meshes: HashMap::new(),
        resolver: crate::collision_audit::PropResolver::new(content),
        tweaks: None,
    };
    let mut materials = Vec::new();
    for (name, path) in POST_MATERIALS {
        let Some(k) = imp.index.get(&path.to_ascii_lowercase()).copied() else { continue };
        let Some(id) = imp.material(k) else { continue };
        let curves = mitv_curves(&imp.pkgs[k.0], k.1);
        materials.push((name.to_string(), id, curves));
    }
    let mut nodes = BTreeMap::new();
    if let Some(startup) = imp.pkgs.iter().find(|p| p.name.eq_ignore_ascii_case("Startup")) {
        for i in 0..startup.exports.len() {
            if startup.export_class(i) != "ArkPpNodeMaterial" || !startup.obj_path(ObjRef::Export(i)).starts_with("AltScreen_Effects.PostProcessChain.Test_PPG.") {
                continue;
            }
            let Ok(h) = startup.read_object(i) else { continue };
            if !matches!(find(&h.properties, "m_bOverrideUberPp").map(|p| &p.value), Some(Value::Bool(true))) {
                continue;
            }
            let Some(Value::Name(effect)) = find(&h.properties, "EffectName").map(|p| &p.value) else { continue };
            let mut fields = BTreeMap::new();
            if let Some(Value::Struct(_, sub)) = find(&h.properties, "m_UberParameters").map(|p| &p.value) {
                flatten(sub, "", &mut fields);
            }
            nodes.insert(effect.clone(), fields);
        }
    }
    Ok(PostMaterials { map: imp.map, materials, nodes })
}

/// A struct's numeric fields by dotted path (vectors as `.x` `.y` `.z`).
fn flatten(props: &[Property], prefix: &str, out: &mut BTreeMap<String, f32>) {
    for p in props {
        let key = if p.array_index > 0 { format!("{prefix}{}[{}]", p.name, p.array_index) } else { format!("{prefix}{}", p.name) };
        match &p.value {
            Value::Float(f) => {
                out.insert(key, *f);
            }
            Value::Int(i) => {
                out.insert(key, *i as f32);
            }
            Value::Bool(b) => {
                out.insert(key, if *b { 1.0 } else { 0.0 });
            }
            Value::Byte(b) => {
                out.insert(key, *b as f32);
            }
            Value::Vector(v) => {
                for (c, x) in ["x", "y", "z"].iter().zip(v) {
                    out.insert(format!("{key}.{c}"), *x);
                }
            }
            Value::Struct(_, sub) => flatten(sub, &format!("{key}."), out),
            _ => {}
        }
    }
}

/// A `MaterialInstanceTimeVarying`'s scalar curves (`ScalarParameterValues`: the parameter, its
/// value before it plays, the curve's keys).
fn mitv_curves(pkg: &Package, export: usize) -> Vec<PostCurve> {
    if pkg.export_class(export) != "MaterialInstanceTimeVarying" {
        return Vec::new();
    }
    let Ok(h) = pkg.read_object(export) else { return Vec::new() };
    let Some(p) = find(&h.properties, "ScalarParameterValues") else { return Vec::new() };
    p.array_structs(pkg)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| {
            let param = match find(&v, "ParameterName").map(|p| &p.value) {
                Some(Value::Name(n)) => n.clone(),
                _ => return None,
            };
            let mut keys = Vec::new();
            if let Some(Value::Struct(_, curve)) = find(&v, "ParameterValueCurve").map(|p| &p.value) {
                if let Some(points) = find(curve, "Points") {
                    for k in points.array_structs(pkg).unwrap_or_default() {
                        keys.push([find(&k, "InVal").and_then(|x| x.as_float()).unwrap_or(0.0), find(&k, "OutVal").and_then(|x| x.as_float()).unwrap_or(0.0)]);
                    }
                }
            }
            Some(PostCurve { param, default: find(&v, "ParameterValue").and_then(|x| x.as_float()).unwrap_or(0.0), keys })
        })
        .collect()
}
