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
    /// Static permutation of the child-most instance that has one (the parameter set its
    /// shaders were compiled with), U-027.
    pub static_params: Option<StaticParameters>,
    /// Defaults of the base material's static switch / component mask expressions.
    pub switch_defaults: Vec<(String, bool)>,
    pub mask_defaults: Vec<(String, [bool; 4])>,
    /// The base material's `FMaterial::UniformExpressionTextures` (the shaders' `Tex#i`), in
    /// order; textures sampled without a parameter (e.g. `Moon_PMAT`'s moon disc) survive
    /// cooking only here.
    pub expression_textures: Vec<Key>,
    /// The base material's `FMaterialResource`: its `Id` (keys the shader cache's maps of its
    /// static permutations) and its `UniformExpressionTextures` by position (None: a null or
    /// unresolved reference).
    pub base_resource: Option<([u8; 16], Vec<Option<Key>>)>,
    /// The compiled resource the original renders with: the leaf-most instance with a static
    /// permutation resource (`bHasStaticPermutationResource`), else the base material's.
    pub resource: Option<([u8; 16], Vec<Option<Key>>)>,
}

impl MaterialChain {
    /// Value of a static switch: the compiled permutation's, else the base default.
    pub fn switch(&self, name: &str) -> Option<bool> {
        self.static_params
            .as_ref()
            .and_then(|s| s.switches.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name)).map(|v| v.1))
            .or_else(|| self.switch_defaults.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|v| v.1))
    }

    /// Channels selected by a static component mask parameter.
    pub fn mask(&self, name: &str) -> Option<[bool; 4]> {
        self.static_params
            .as_ref()
            .and_then(|s| s.masks.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name)).map(|v| v.1))
            .or_else(|| self.mask_defaults.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|v| v.1))
    }

    /// The single channel a mask selects (None when it selects several or none).
    pub fn mask_channel(&self, name: &str) -> Option<usize> {
        let m = self.mask(name)?;
        (m.iter().filter(|b| **b).count() == 1).then(|| m.iter().position(|b| *b).unwrap())
    }
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
        let compiled = class == "Material" || matches!(find(&props, "bHasStaticPermutationResource").map(|p| &p.value), Some(Value::Bool(true)));
        if out.resource.is_none() && compiled {
            if let Some((id, refs)) = pkg.export_data(cur.1).ok().and_then(|data| material_resource(data, hdr.native_offset)) {
                let refs = refs.into_iter().map(|r| if r == 0 { None } else { resolve(cur.0, ObjRef::from_index(r)) }).collect();
                out.resource = Some((id, refs));
            }
        }
        if class == "Material" {
            if let Some((id, refs)) = pkg.export_data(cur.1).ok().and_then(|data| material_resource(data, hdr.native_offset)) {
                let refs = refs.into_iter().map(|r| if r == 0 { None } else { resolve(cur.0, ObjRef::from_index(r)) }).collect();
                out.base_resource = Some((id, refs));
            }
        }
        if out.static_params.is_none() && matches!(find(&props, "bHasStaticPermutationResource").map(|p| &p.value), Some(Value::Bool(true))) {
            if let Ok(data) = pkg.export_data(cur.1) {
                out.static_params = read_static_parameters(pkg, data, hdr.native_offset, 0);
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
        } else if class == "MaterialExpressionStaticSwitchParameter" {
            if let Some(name) = param_name(p) {
                let v = matches!(find(p, "DefaultValue").map(|v| &v.value), Some(Value::Bool(true)));
                if !out.switch_defaults.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                    out.switch_defaults.push((name, v));
                }
            }
        } else if class == "MaterialExpressionStaticComponentMaskParameter" {
            if let Some(name) = param_name(p) {
                let c = |k: &str| matches!(find(p, k).map(|v| &v.value), Some(Value::Bool(true)));
                if !out.mask_defaults.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                    out.mask_defaults.push((name, [c("DefaultR"), c("DefaultG"), c("DefaultB"), c("DefaultA")]));
                }
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
    if let (Ok(h), Ok(data)) = (pkg.read_object(mat.1), pkg.export_data(mat.1)) {
        for r in uniform_expression_textures(data, h.native_offset).unwrap_or_default() {
            if let Some(k) = resolve(mat.0, ObjRef::from_index(r)) {
                out.expression_textures.push(k);
            }
        }
    }
}

/// `FMaterial::UniformExpressionTextures` of a base `Material`. Its native data starts with
/// the material resource:
/// ```text
/// TArray<FString> CompileErrors
/// TMap<UMaterialExpression*, INT> TextureDependencyLengthMap   (8 B per pair)
/// INT MaxTextureDependencyLength
/// FGuid Id
/// UINT NumUserTexCoords
/// TArray<UTexture*> UniformExpressionTextures
/// ```
/// (layout read from L_Boyle_Ext_Env `Moon_PMAT`, whose two entries are the moon texture and
/// the `Cloud` parameter's default). Returns the raw object references.
pub fn uniform_expression_textures(data: &[u8], native_offset: usize) -> Option<Vec<i32>> {
    material_resource(data, native_offset).map(|r| r.1)
}

/// The `FMaterialResource` at the start of a material's (or a static permutation instance's)
/// native data: its `Id` (the raw GUID bytes the shader cache keys its maps by) and its
/// `UniformExpressionTextures` (raw object references, in order; 0 = none). Layout as in
/// [`uniform_expression_textures`]. Adapted from dishonored-bevy by Eamon
/// (github.com/Eamo5/dishonored-bevy), used with permission (dhcook's `material_resource`).
pub fn material_resource(data: &[u8], native_offset: usize) -> Option<([u8; 16], Vec<i32>)> {
    let mut p = native_offset;
    let n_err = u32_at(data, p)? as usize;
    if n_err > 64 {
        return None;
    }
    p += 4;
    for _ in 0..n_err {
        let len = u32_at(data, p)? as i32;
        p += 4 + if len < 0 { len.unsigned_abs() as usize * 2 } else { len as usize };
    }
    let n_map = u32_at(data, p)? as usize;
    if n_map > 256 {
        return None;
    }
    p += 4 + n_map * 8 + 4;
    let id: [u8; 16] = data.get(p..p + 16)?.try_into().ok()?;
    p += 16 + 4;
    let n = u32_at(data, p)? as usize;
    if n > 64 {
        return None;
    }
    p += 4;
    let textures = (0..n).map(|i| u32_at(data, p + i * 4).map(|v| v as i32)).collect::<Option<Vec<i32>>>()?;
    Some((id, textures))
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

/// Static permutation parameters of a material instance (`FStaticParameterSet`, serialized
/// after the instance's `FMaterialResource` in its native data when
/// `bHasStaticPermutationResource` is set):
/// ```text
/// FGuid BaseMaterialId
/// TArray<{ FName ParameterName; UBOOL Value; UBOOL bOverride; FGuid ExpressionGUID }>       switches (32 B)
/// TArray<{ FName ParameterName; UBOOL R, G, B, A; UBOOL bOverride; FGuid ExpressionGUID }>  component masks (44 B)
/// TArray<{ FName ParameterName; BYTE CompressionSettings; UBOOL bOverride; FGuid }>         normal parameters (29 B)
/// TArray<{ FName ParameterName; INT WeightmapIndex; UBOOL bOverride; FGuid }>               terrain layer weights (32 B)
/// ```
/// (layout read from L_Boyle_Ext_Env `barbed_wire_01__low_INST`; the set ends the export.)
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StaticParameters {
    pub base_material_id: [u32; 4],
    /// (name, value, overridden)
    pub switches: Vec<(String, bool, bool)>,
    /// (name, RGBA mask, overridden)
    pub masks: Vec<(String, [bool; 4], bool)>,
}

fn u32_at(d: &[u8], p: usize) -> Option<u32> {
    d.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Find and decode the static parameter set in a MIC's native data. The `FMaterialResource`
/// before it is not parsed: the set is located as the last offset where a switch array and a
/// component mask array parse completely with valid names and 0/1 booleans, ending within
/// `slack` bytes of the export end.
pub fn read_static_parameters(pkg: &Package, data: &[u8], native_offset: usize, slack: usize) -> Option<StaticParameters> {
    let name_ok = |p: usize| -> Option<String> {
        let (i, n) = (u32_at(data, p)?, u32_at(data, p + 4)?);
        if (i as usize) < pkg.names.len() && n < 64 {
            Some(pkg.name(ue3_package::FName { index: i as i32, number: n as i32 }))
        } else {
            None
        }
    };
    let b = |p: usize| -> Option<bool> {
        match u32_at(data, p)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    };
    let try_at = |start: usize| -> Option<(StaticParameters, usize)> {
        let mut p = start;
        let base = [u32_at(data, p)?, u32_at(data, p + 4)?, u32_at(data, p + 8)?, u32_at(data, p + 12)?];
        p += 16;
        let ns = u32_at(data, p)? as usize;
        if ns > 128 {
            return None;
        }
        p += 4;
        let mut out = StaticParameters { base_material_id: base, ..Default::default() };
        for _ in 0..ns {
            let name = name_ok(p)?;
            out.switches.push((name, b(p + 8)?, b(p + 12)?));
            p += 32;
        }
        let nm = u32_at(data, p)? as usize;
        if nm > 128 {
            return None;
        }
        p += 4;
        for _ in 0..nm {
            let name = name_ok(p)?;
            out.masks.push((name, [b(p + 8)?, b(p + 12)?, b(p + 16)?, b(p + 20)?], b(p + 24)?));
            p += 44;
        }
        // Normal-map parameters: FName, BYTE CompressionSettings, UBOOL bOverride, FGuid (29 B).
        let nn = u32_at(data, p)? as usize;
        if nn > 128 {
            return None;
        }
        p += 4;
        for _ in 0..nn {
            name_ok(p)?;
            b(p + 9)?;
            p += 29;
        }
        // Terrain layer weights: FName, INT WeightmapIndex, UBOOL bOverride, FGuid (32 B).
        let nt = u32_at(data, p)? as usize;
        if nt > 128 {
            return None;
        }
        p += 4;
        for _ in 0..nt {
            name_ok(p)?;
            b(p + 12)?;
            p += 32;
        }
        if ns + nm + nn == 0 || base == [0; 4] {
            return None;
        }
        Some((out, p))
    };
    let hi = data.len().saturating_sub(24);
    (native_offset..hi).rev().find_map(|s| try_at(s).filter(|(_, end)| *end <= data.len() && data.len() - *end <= slack).map(|(o, _)| o))
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
