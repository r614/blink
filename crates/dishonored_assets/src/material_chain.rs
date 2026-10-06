//! Cross-package material resolution: walks `MaterialInstanceConstant -> Parent -> ...
//! -> Material` through any package the importer has indexed, collecting texture, scalar
//! and vector parameters (child values win) plus the base material's blend state and the
//! default values of its parameter expressions.

use std::sync::Arc;
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

/// (package slot, export index)
pub type Key = (usize, usize);

#[derive(Debug, Default, Clone)]
pub struct MaterialChain {
    /// Texture parameters set explicitly on an instance in the chain (child wins).
    pub textures: Vec<(String, Key)>,
    /// Default values of the base material's texture expressions. These are often
    /// placeholders (checkers) behind static switches that the instance turns off.
    pub default_textures: Vec<(String, Key)>,
    pub scalars: Vec<(String, f32)>,
    pub vectors: Vec<(String, [f32; 4])>,
    pub blend_mode: Option<String>,
    pub two_sided: Option<bool>,
    pub lighting_model: Option<String>,
    pub opacity_clip: Option<f32>,
    pub base_material: Option<String>,
    /// Texture wired directly into the base material's DiffuseColor / Normal inputs.
    pub diffuse_input: Option<Key>,
    pub normal_input: Option<Key>,
    /// `PhysMaterial` of the first material in the chain that sets one (child wins).
    pub phys_material: Option<Key>,
    /// Chain ended at an import nobody defines.
    pub broken_at: Option<String>,
}

fn param_name(props: &[Property]) -> Option<String> {
    match find(props, "ParameterName").map(|p| &p.value) {
        Some(Value::Name(n)) if n != "None" => Some(n.clone()),
        _ => None,
    }
}

fn push_unique<T>(v: &mut Vec<(String, T)>, name: String, val: T) {
    if !v.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
        v.push((name, val));
    }
}

/// Walk the chain starting at `start`. `resolve(slot, ref)` maps a package-relative
/// reference to a definition anywhere in `pkgs`.
pub fn walk(pkgs: &[Arc<Package>], start: Key, resolve: &mut dyn FnMut(usize, ObjRef) -> Option<Key>) -> MaterialChain {
    let mut out = MaterialChain::default();
    let mut cur = start;
    for _ in 0..24 {
        let pkg = &pkgs[cur.0];
        let class = pkg.export_class(cur.1);
        let Ok(hdr) = pkg.read_object(cur.1) else { break };
        let props = hdr.properties;
        if out.phys_material.is_none() {
            if let Some(pm) = find(&props, "PhysMaterial").and_then(|p| p.as_object()).filter(|o| *o != 0) {
                out.phys_material = resolve(cur.0, ObjRef::from_index(pm));
            }
        }
        if class == "Material" {
            read_base_material(pkgs, cur, &props, resolve, &mut out);
            if cur == start {
                // A plain Material used directly: its defaults are the real values.
                out.textures = out.default_textures.clone();
            }
            break;
        }
        if let Some(p) = find(&props, "TextureParameterValues") {
            for s in p.array_structs(pkg).unwrap_or_default() {
                let (Some(name), Some(t)) = (param_name(&s), find(&s, "ParameterValue").and_then(|p| p.as_object())) else {
                    continue;
                };
                if let Some(k) = resolve(cur.0, ObjRef::from_index(t)) {
                    push_unique(&mut out.textures, name, k);
                }
            }
        }
        if let Some(p) = find(&props, "ScalarParameterValues") {
            for s in p.array_structs(pkg).unwrap_or_default() {
                if let (Some(name), Some(v)) = (param_name(&s), find(&s, "ParameterValue").and_then(|p| p.as_float())) {
                    push_unique(&mut out.scalars, name, v);
                }
            }
        }
        if let Some(p) = find(&props, "VectorParameterValues") {
            for s in p.array_structs(pkg).unwrap_or_default() {
                if let (Some(name), Some(Value::LinearColor(c))) = (param_name(&s), find(&s, "ParameterValue").map(|p| &p.value)) {
                    push_unique(&mut out.vectors, name, *c);
                }
            }
        }
        let Some(parent) = find(&props, "Parent").and_then(|p| p.as_object()) else { break };
        let r = ObjRef::from_index(parent);
        match resolve(cur.0, r) {
            Some(k) => cur = k,
            None => {
                out.broken_at = Some(pkg.obj_path(r));
                break;
            }
        }
    }
    out
}

fn read_base_material(
    pkgs: &[Arc<Package>],
    mat: Key,
    props: &[Property],
    resolve: &mut dyn FnMut(usize, ObjRef) -> Option<Key>,
    out: &mut MaterialChain,
) {
    let pkg = &pkgs[mat.0];
    out.base_material = Some(pkg.obj_path(ObjRef::Export(mat.1)));
    let en = |n: &str| match find(props, n).map(|p| &p.value) {
        Some(Value::Enum(s)) => Some(s.clone()),
        _ => None,
    };
    out.blend_mode = en("BlendMode");
    out.lighting_model = en("LightingModel");
    out.two_sided = match find(props, "TwoSided").map(|p| &p.value) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    };
    out.opacity_clip = find(props, "OpacityMaskClipValue").and_then(|p| p.as_float());
    // Expressions owned by the material: parameter defaults.
    let expr_texture = |props: &[Property], resolve: &mut dyn FnMut(usize, ObjRef) -> Option<Key>| {
        find(props, "Texture").and_then(|p| p.as_object()).and_then(|t| resolve(mat.0, ObjRef::from_index(t)))
    };
    for i in 0..pkg.exports.len() {
        if pkg.exports[i].outer as usize != mat.1 + 1 {
            continue;
        }
        let class = pkg.export_class(i);
        let Ok(h) = pkg.read_object(i) else { continue };
        let p = &h.properties;
        if class.starts_with("MaterialExpressionTextureSample") {
            if let Some(k) = expr_texture(p, resolve) {
                let name = param_name(p).unwrap_or_else(|| format!("#{}", pkg.obj_name(ObjRef::Export(i))));
                push_unique(&mut out.default_textures, name, k);
            }
        } else if class == "MaterialExpressionScalarParameter" {
            if let Some(name) = param_name(p) {
                push_unique(&mut out.scalars, name, find(p, "DefaultValue").and_then(|v| v.as_float()).unwrap_or(0.0));
            }
        } else if class == "MaterialExpressionVectorParameter" {
            if let Some(name) = param_name(p) {
                let v = match find(p, "DefaultValue").map(|v| &v.value) {
                    Some(Value::LinearColor(c)) => *c,
                    _ => [0.0; 4],
                };
                push_unique(&mut out.vectors, name, v);
            }
        }
    }
    let input = |name: &str| -> Option<ObjRef> {
        match find(props, name).map(|p| &p.value) {
            Some(Value::Struct(_, s)) => find(s, "Expression").and_then(|p| p.as_object()).map(ObjRef::from_index),
            _ => None,
        }
    };
    let wired = |name: &str, resolve: &mut dyn FnMut(usize, ObjRef) -> Option<Key>| -> Option<Key> {
        let ObjRef::Export(e) = input(name)? else { return None };
        let h = pkg.read_object(e).ok()?;
        expr_texture(&h.properties, resolve)
    };
    out.diffuse_input = wired("DiffuseColor", resolve);
    out.normal_input = wired("Normal", resolve);
}

/// Like [`slot`] over an arbitrary parameter list (e.g. the base material defaults).
pub fn slot_in(list: &[(String, Key)], prefix: &str, exclude: &[&str]) -> Option<Key> {
    list.iter()
        .find(|(n, _)| {
            let l = n.to_ascii_lowercase();
            l.starts_with(prefix) && !exclude.iter().any(|x| l.contains(x))
        })
        .map(|(_, k)| *k)
}

/// Pick the texture for an Arkane slot prefix (`d_`, `n_`, `s_`, `e_`, `o_`, `dn_`).
pub fn slot(chain: &MaterialChain, prefix: &str, exclude: &[&str]) -> Option<Key> {
    chain
        .textures
        .iter()
        .find(|(n, _)| {
            let l = n.to_ascii_lowercase();
            l.starts_with(prefix) && !exclude.iter().any(|x| l.contains(x))
        })
        .map(|(_, k)| *k)
}
