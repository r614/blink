//! The sky dome material `Skies.Skies_PMAT`, rendered from its compiled shaders (research/LOG.md
//! "Sky dome from Skies_PMAT").
//!
//! The cooked material keeps only its parameter and switch expressions; the maths is read from
//! the base pass pixel shader of each Skies_PMAT shader map in `RefShaderCache-PC-D3D-SM3.upk`
//! (`TBasePassPixelShaderFNoLightMapPolicyNoSkyLightFALSEFALSE`, unlit: the output is the
//! emissive colour) and the map's uniform expressions (`tools/package_dump` `shader_maps`):
//!
//! - cloud layers sample `C_Clouds_Texture` / `C2_Clouds_2_Texture` at UV0 turned about (0.5,
//!   0.5) by `Speed x Time x 0.1` radians (`Append(cos, -sin)`, `Append(sin, cos)` expressions);
//! - base colour: with `G_Use_Gradiant_Sky`, `G_Gradiant_Top + (G_Gradiant_Bottom -
//!   G_Gradiant_Top) x |UV0.y + clouds.r x Cloud_Distortion|^G_Gradiant_Power` (0 below 1e-6);
//!   without it `D_Diffuse` at `(U_Offset - UV1.x, UV1.y) + clouds.r x Cloud_Distortion`; times
//!   `D_Diffuse_color.rgb x a`;
//! - each cloud layer: `c + clouds x Visibility x (Color.rgb x a x clouds - c)`;
//! - contrast: `max(|c|, 1e-6)^sky_contrast`;
//! - `Enable_Storm`: `+ Storm(Storm_Offset + UV1.x, UV1.y).rgb x Storm_Color.rgb x
//!   (Storm_Intensity + Storm_Min_Intensity)`.
//!
//! Shader maps used: Boyle Ext `Boyle_2_inst` (storm, clouds, clouds 2, gradient, UV channel 2)
//! and Boyle Int `Boyle_01_inst` (clouds, UV channel 2). Not decoded (U-520): `H_Enable_Horizon`
//! (its noise is an unnamed material texture), `TP_Enable_Top_Texture`, `Enable_Storm_Mask`,
//! `Enable_GRadiant_Top`, `G_Grandiant_UV2`, and the variants without `Use_UV_Channel_2`.

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{MaterialParams, TextureId};

/// The base material this module renders.
pub const SKIES_PMAT: &str = "Skies.Skies_PMAT";

/// Switches whose terms are not decoded (the material renders without them).
pub const UNDECODED: [&str; 5] = ["H_Enable_Horizon", "TP_Enable_Top_Texture", "Enable_Storm_Mask", "Enable_GRadiant_Top", "G_Grandiant_UV2"];

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct SkyUniform {
    pub flags: Vec4,
    pub diffuse_color: Vec4,
    pub grad_top: Vec4,
    pub grad_bottom: Vec4,
    pub clouds_color: Vec4,
    pub clouds2_color: Vec4,
    pub storm_color: Vec4,
    pub speeds: Vec4,
    pub misc: Vec4,
    pub misc2: Vec4,
}

/// Parameter defaults of Skies_PMAT (its shader maps' uniform expressions).
fn scalar_default(name: &str) -> f32 {
    match name {
        "C_Clouds_Speed" => 1.0,
        "C_Clouds_Visibility" => 3.0,
        "Cloud_Distortion" => 0.25,
        "G_Gradiant_Power" => 1.0,
        "C2_Clouds_2_Speed" => 2.0,
        "C2_Clouds_2_Visibility" => 3.0,
        "sky_contrast" => 1.0,
        "Storm_Intensity" => 1.0,
        _ => 0.0,
    }
}

fn vector_default(name: &str) -> [f32; 4] {
    match name {
        "D_Diffuse_color" | "Storm_Color" => [1.0, 1.0, 1.0, 1.0],
        "C_Clouds_Color" => [0.0, 0.0, 0.0, 1.0],
        "C2_Clouds_2_Color" => [1.0, 1.0, 1.0, 0.5],
        "G_Gradiant_Top" => [0.0, 1.0, 0.091_391_56, 1.0],
        "G_Gradiant_Bottom" => [1.0, 0.022_799_492, 0.013_465_107, 1.0],
        _ => [0.0; 4],
    }
}

impl SkyUniform {
    /// The uniform of a Skies_PMAT material, or None for other bases.
    pub fn decode(p: &MaterialParams) -> Option<SkyUniform> {
        if !p.base_material.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(SKIES_PMAT)) {
            return None;
        }
        let s = |n: &str| p.scalars.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or_else(|| scalar_default(n));
        let v = |n: &str| p.vectors.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or_else(|| vector_default(n));
        let premul = |c: [f32; 4]| Vec4::new(c[0] * c[3], c[1] * c[3], c[2] * c[3], 1.0);
        let rgb = |c: [f32; 4]| Vec4::new(c[0], c[1], c[2], 1.0);
        let on = |n: &str| if p.switch(n) == Some(true) { 1.0 } else { 0.0 };
        let storm = v("Storm_Color");
        Some(SkyUniform {
            flags: Vec4::new(on("G_Use_Gradiant_Sky"), on("C_Enable_Clouds"), on("C2_Enable_Clouds_2"), on("Enable_Storm")),
            diffuse_color: premul(v("D_Diffuse_color")),
            grad_top: rgb(v("G_Gradiant_Top")),
            grad_bottom: rgb(v("G_Gradiant_Bottom")),
            clouds_color: premul(v("C_Clouds_Color")),
            clouds2_color: premul(v("C2_Clouds_2_Color")),
            storm_color: Vec4::new(storm[0], storm[1], storm[2], s("Storm_Intensity") + s("Storm_Min_Intensity")),
            speeds: Vec4::new(s("C_Clouds_Speed"), s("C2_Clouds_2_Speed"), s("Cloud_Distortion"), s("G_Gradiant_Power")),
            misc: Vec4::new(s("C_Clouds_Visibility"), s("C2_Clouds_2_Visibility"), s("sky_contrast"), s("U_Offset")),
            // Use_UV_Channel_2 defaults to on in Skies_PMAT.
            misc2: Vec4::new(s("Storm_Offset"), if p.switch("Use_UV_Channel_2") == Some(false) { 0.0 } else { 1.0 }, 0.0, 0.0),
        })
    }
}

/// The sky dome material (see the module docs and `sky.wgsl`).
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct SkyMaterial {
    #[uniform(0)]
    pub params: SkyUniform,
    /// Any of the textures below; carries the (repeating) sampler.
    #[texture(1)]
    #[sampler(2)]
    pub any: Option<Handle<Image>>,
    #[texture(3)]
    pub diffuse: Option<Handle<Image>>,
    #[texture(4)]
    pub clouds: Option<Handle<Image>>,
    #[texture(5)]
    pub clouds2: Option<Handle<Image>>,
    #[texture(6)]
    pub storm: Option<Handle<Image>>,
}

impl SkyMaterial {
    /// Build from a sky material's parameters and its kept texture parameters.
    pub fn new(p: &MaterialParams, textures: &[(String, TextureId)], images: &[Handle<Image>]) -> Option<SkyMaterial> {
        let params = SkyUniform::decode(p)?;
        let missing: Vec<&str> = UNDECODED.iter().copied().filter(|s| p.switch(s) == Some(true)).collect();
        if !missing.is_empty() {
            warn!("sky material: switches not decoded (U-520): {}", missing.join(", "));
        }
        let tex = |n: &str| textures.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).and_then(|t| images.get(t.1)).cloned();
        let (diffuse, clouds, clouds2, storm) = (tex("D_Diffuse"), tex("C_Clouds_Texture"), tex("C2_Clouds_2_Texture"), tex("Storm"));
        let any = [&clouds, &diffuse, &clouds2, &storm].into_iter().flatten().next().cloned();
        Some(SkyMaterial { params, any, diffuse, clouds, clouds2, storm })
    }
}

impl Material for SkyMaterial {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/sky.wgsl".into()
    }
}

/// A sky dome entity waiting for its `SkyMaterial` (spawned with a StandardMaterial by
/// `map_render::spawn_map`, swapped on the next update).
#[derive(Component, Clone)]
pub struct PendingSky(pub SkyMaterial);

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "sky.wgsl");
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default()).add_systems(Update, apply_pending_sky);
    }
}

fn apply_pending_sky(mut commands: Commands, q: Query<(Entity, &PendingSky)>, mut mats: ResMut<Assets<SkyMaterial>>) {
    for (e, p) in &q {
        commands.entity(e).remove::<(PendingSky, MeshMaterial3d<StandardMaterial>)>().insert(MeshMaterial3d(mats.add(p.0.clone())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(switches: &[(&str, bool)], scalars: &[(&str, f32)], vectors: &[(&str, [f32; 4])]) -> MaterialParams {
        MaterialParams {
            base_material: Some(SKIES_PMAT.into()),
            static_switches: switches.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            scalars: scalars.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            vectors: vectors.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            ..Default::default()
        }
    }

    /// Boyle Ext's `Skies02.BOYLE.Boyle_2_inst` (switches from its static permutation).
    #[test]
    fn decodes_the_boyle_street_sky() {
        let p = params(
            &[("Enable_Storm", true), ("H_Enable_Horizon", false), ("C_Enable_Clouds", true), ("G_Use_Gradiant_Sky", true), ("Use_UV_Channel_2", true), ("C2_Enable_Clouds_2", true)],
            &[("sky_contrast", 0.8), ("Cloud_Distortion", 0.1), ("C_Clouds_Speed", 0.2), ("C_Clouds_Visibility", 0.2), ("C2_Clouds_2_Visibility", 0.2), ("C2_Clouds_2_Speed", 0.15), ("Storm_Intensity", 1.5)],
            &[("D_Diffuse_color", [0.07065217, 0.06748211, 0.058685396, 1.0]), ("C2_Clouds_2_Color", [0.09661836, 0.15458934, 0.17391305, 1.0]), ("Storm_Color", [0.3586475, 0.36413044, 0.29491073, 1.0])],
        );
        let u = SkyUniform::decode(&p).unwrap();
        assert_eq!(u.flags, Vec4::new(1.0, 1.0, 1.0, 1.0));
        assert_eq!(u.speeds, Vec4::new(0.2, 0.15, 0.1, 1.0));
        assert_eq!(u.misc, Vec4::new(0.2, 0.2, 0.8, 0.0));
        // Storm_Intensity + Storm_Min_Intensity (default 0).
        assert!((u.storm_color.w - 1.5).abs() < 1e-6);
        assert_eq!(u.misc2.y, 1.0);
        // Not a sky: no uniform.
        assert!(SkyUniform::decode(&MaterialParams::default()).is_none());
    }

    /// Colours with an intensity in alpha are premultiplied (`c.rgb x c.a`), gradients are not.
    #[test]
    fn premultiplies_the_colour_parameters() {
        let p = params(&[], &[], &[("C2_Clouds_2_Color", [0.4, 0.5, 0.6, 0.3]), ("G_Gradiant_Top", [0.1, 0.2, 0.3, 0.5])]);
        let u = SkyUniform::decode(&p).unwrap();
        assert!((u.clouds2_color.truncate() - Vec3::new(0.12, 0.15, 0.18)).length() < 1e-6);
        assert_eq!(u.grad_top.truncate(), Vec3::new(0.1, 0.2, 0.3));
        // Base defaults: C2 colour (1, 1, 1, 0.5), visibility 3.
        let d = SkyUniform::decode(&params(&[], &[], &[])).unwrap();
        assert_eq!(d.clouds2_color.truncate(), Vec3::splat(0.5));
        assert_eq!(d.misc.x, 3.0);
    }
}
