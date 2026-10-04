//! Minimal material resolution: find the diffuse and normal textures of a
//! `MaterialInstanceConstant` / `Material` so geometry renders recognizably.
//! This is presentation-only; it does not attempt to evaluate material graphs.

use anyhow::Result;
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

#[derive(Debug, Default, Clone)]
pub struct ResolvedMaterial {
    pub diffuse: Option<ObjRef>,
    pub normal: Option<ObjRef>,
    pub masked: bool,
    pub translucent: bool,
    pub texture_params: Vec<(String, ObjRef)>,
}

fn is_normal_name(param: &str, tex: &str) -> bool {
    let p = param.to_ascii_lowercase();
    let t = tex.to_ascii_lowercase();
    p.contains("normal") || p.ends_with("_n") || t.ends_with("_n") || t.contains("_n_") || t.ends_with("_nm")
}

fn is_diffuse_name(param: &str, tex: &str) -> bool {
    let p = param.to_ascii_lowercase();
    let t = tex.to_ascii_lowercase();
    p.contains("diff") || p.contains("albedo") || p.contains("color") || p.contains("base")
        || t.ends_with("_d") || t.contains("_d_") || t.ends_with("_diff")
}

fn obj_props(pkg: &Package, r: ObjRef) -> Option<Vec<Property>> {
    match r {
        ObjRef::Export(i) => pkg.read_object(i).ok().map(|h| h.properties),
        _ => None,
    }
}

pub fn resolve_material(pkg: &Package, mat: ObjRef) -> Result<ResolvedMaterial> {
    let mut out = ResolvedMaterial::default();
    let mut cur = mat;
    // Walk the MIC parent chain; child parameters take precedence.
    for _ in 0..16 {
        let ObjRef::Export(i) = cur else { break };
        let class = pkg.export_class(i);
        let Some(props) = obj_props(pkg, cur) else { break };
        if class == "Material" {
            resolve_base_material(pkg, i, &props, &mut out);
            break;
        }
        if let Some(p) = find(&props, "TextureParameterValues") {
            for s in p.array_structs(pkg).unwrap_or_default() {
                let name = match find(&s, "ParameterName").map(|p| &p.value) {
                    Some(Value::Name(n)) => n.clone(),
                    _ => continue,
                };
                let Some(tex) = find(&s, "ParameterValue").and_then(|p| p.as_object()) else {
                    continue;
                };
                let tex = ObjRef::from_index(tex);
                if tex == ObjRef::Null {
                    continue;
                }
                if !out.texture_params.iter().any(|(n, _)| *n == name) {
                    out.texture_params.push((name, tex));
                }
            }
        }
        match find(&props, "Parent").and_then(|p| p.as_object()) {
            Some(parent) => cur = ObjRef::from_index(parent),
            None => break,
        }
    }
    // Arkane's parameter naming convention (observed across all 420 Boyle Ext
    // materials): `D_` diffuse, `N_` normal, `S_` specular, `O_` opacity, `E_` emissive,
    // `D2_`/`N2_` variation layers. Rank exact conventions above heuristics.
    let rank_diffuse = |n: &str, t: &str| -> u8 {
        let l = n.to_ascii_lowercase();
        if l.starts_with("d_") && !l.contains("mask") {
            3
        } else if l == "diffuse" || l.starts_with("diffuse") {
            2
        } else if is_diffuse_name(n, t) && !l.contains("mask") && !l.starts_with("d2_") {
            1
        } else {
            0
        }
    };
    let rank_normal = |n: &str, t: &str| -> u8 {
        let l = n.to_ascii_lowercase();
        if l.starts_with("n_") {
            2
        } else if is_normal_name(n, t) && !l.starts_with("n2_") && !l.starts_with("dn_") {
            1
        } else {
            0
        }
    };
    let best = |f: &dyn Fn(&str, &str) -> u8| {
        out.texture_params
            .iter()
            .map(|(n, t)| (f(n, &pkg.obj_name(*t)), *t))
            .filter(|(r, _)| *r > 0)
            .max_by_key(|(r, _)| *r)
            .map(|(_, t)| t)
    };
    if out.diffuse.is_none() {
        out.diffuse = best(&rank_diffuse);
    }
    if out.normal.is_none() {
        out.normal = best(&rank_normal);
    }
    if out.diffuse.is_none() {
        out.diffuse = out
            .texture_params
            .iter()
            .map(|(_, t)| *t)
            .find(|t| !is_normal_name("", &pkg.obj_name(*t)));
    }
    Ok(out)
}

fn resolve_base_material(pkg: &Package, mat: usize, props: &[Property], out: &mut ResolvedMaterial) {
    if let Some(Value::Enum(b)) = find(props, "BlendMode").map(|p| &p.value) {
        out.masked = b == "BLEND_Masked";
        out.translucent = b == "BLEND_Translucent" || b == "BLEND_Additive";
    }
    // Expression the DiffuseColor / Normal inputs are wired to.
    let input_expr = |name: &str| -> Option<ObjRef> {
        match find(props, name).map(|p| &p.value) {
            Some(Value::Struct(_, s)) => find(s, "Expression")
                .and_then(|p| p.as_object())
                .map(ObjRef::from_index),
            _ => None,
        }
    };
    let texture_of = |e: ObjRef| -> Option<ObjRef> {
        let p = obj_props(pkg, e)?;
        find(&p, "Texture")
            .and_then(|p| p.as_object())
            .map(ObjRef::from_index)
            .filter(|r| *r != ObjRef::Null)
    };
    let mut samples = Vec::new();
    for i in 0..pkg.exports.len() {
        if pkg.exports[i].outer as usize == mat + 1 && pkg.export_class(i).starts_with("MaterialExpressionTextureSample") {
            if let Some(t) = texture_of(ObjRef::Export(i)) {
                let pname = obj_props(pkg, ObjRef::Export(i))
                    .and_then(|p| match find(&p, "ParameterName").map(|p| p.value.clone()) {
                        Some(Value::Name(n)) => Some(n),
                        _ => None,
                    })
                    .unwrap_or_default();
                samples.push((pname, t));
            }
        }
    }
    if out.diffuse.is_none() {
        out.diffuse = input_expr("DiffuseColor").and_then(texture_of);
    }
    if out.normal.is_none() {
        out.normal = input_expr("Normal").and_then(texture_of);
    }
    for (n, t) in samples {
        if !out.texture_params.iter().any(|(_, x)| *x == t) {
            out.texture_params.push((n, t));
        }
    }
}
