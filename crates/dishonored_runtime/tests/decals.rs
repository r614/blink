//! Tests of `dishonored_runtime::decals` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::decals::*;
use dishonored_ir::MaterialParams;

fn params(switches: &[(&str, bool)], masks: &[(&str, u8)], vectors: &[(&str, [f32; 4])]) -> MaterialParams {
    MaterialParams {
        blend: dishonored_ir::MaterialBlend::Translucent,
        base_material: Some(GENERIC_TRANSLUCENT_PMAT.into()),
        static_switches: switches.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        mask_channels: masks.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        vectors: vectors.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        ..Default::default()
    }
}

/// `towerEmpress_ground.puddles_decals_01_inst` (Boyle's ground puddles): scene reflection
/// masked by the B channel of `_s`, no diffuse colour, no Fresnel.
#[test]
fn decodes_the_boyle_puddles() {
    let p = params(
        &[("N_Enable_NormalMap", true), ("C_Enable_Cubemap", true), ("C_Enable_CubemapMask", true), ("D_Enable_Diffuse_color", false), ("F_Invert_Fresnel_result", true), ("S_Enable_Specular", false)],
        &[("C_Cubemap_Mask_Channel", 2), ("O_Opacity_Mask_Channel", 1)],
        &[("D_Diffuse_color", [1.0, 1.0, 1.0, 1.0])],
    );
    let u = DecalUniform::decode(&p, 1.0).unwrap();
    assert_eq!(u.mask, Vec4::new(0.0, 0.0, 1.0, 0.0));
    assert_eq!(u.ambient.w, 1.0);
    assert_eq!(u.cube, Vec4::new(1.0, 1.0, 1.0, 1.0));
    assert_eq!(u.fresnel, Vec4::ZERO);
    assert_eq!(DecalUniform::diffuse_color(&p), Vec3::ONE);
}

/// Colour switches premultiply rgb by a; the Fresnel term carries its exponent.
#[test]
fn colour_switches_and_fresnel() {
    let p = params(
        &[("D_Enable_Diffuse_color", true), ("C_Enable_Cubemap", true), ("C_Enable_Cubemap_Color", true), ("F_Enable_Fresnel", true), ("F_Invert_Fresnel_result", true)],
        &[],
        &[("D_Diffuse_color", [0.5, 0.25, 1.0, 2.0]), ("C_Cubemap_color", [1.0, 1.0, 1.0, 2.0]), ("F_Fresnel_Color", [1.0, 0.5, 0.0, 0.5])],
    );
    let u = DecalUniform::decode(&p, 1.0).unwrap();
    assert_eq!(DecalUniform::diffuse_color(&p), Vec3::new(1.0, 0.5, 2.0));
    assert_eq!(u.cube.truncate(), Vec3::splat(2.0));
    assert_eq!(u.cube.w, 0.0, "no cubemap mask");
    assert_eq!(u.fresnel, Vec4::new(0.5, 0.25, 0.0, 4.0));
    // Other bases and opaque materials are not decals.
    assert!(DecalUniform::decode(&MaterialParams::default(), 1.0).is_none());
    let mut q = p.clone();
    q.blend = dishonored_ir::MaterialBlend::Masked;
    assert!(DecalUniform::decode(&q, 1.0).is_none());
}
