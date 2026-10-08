//! Tests of `dishonored_runtime::ark_post` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::ark_post::*;

#[test]
fn struct_defaults_and_boyle_values() {
    // No ArkPp fields: the struct defaults (exposure 0 -> x1, gamma 1 / 2.2, radius 300).
    let u = ArkPostSource { display_gamma: 2.2, post_on: true, ..Default::default() }.uniform(0.0);
    assert!((u.exposure_gamma_desat.x - 1.0).abs() < 1e-6);
    assert!((u.exposure_gamma_desat.y - 1.0 / 2.2).abs() < 1e-6);
    assert!((u.dof.y - 1.0 / 300.0).abs() < 1e-9);
    assert!((u.brightness_contrast.y - 1.0).abs() < 1e-5);
    // Boyle Ext: exposure 0.3, gamma 0.95, contrast 0.05.
    let post = PostSettings {
        source: String::new(),
        values: vec![
            ("m_HDRParameters.m_Exposure".into(), 0.3),
            ("m_HDRParameters.m_GammaAdjustment".into(), 0.95),
            ("m_HDRParameters.m_GimpContrast".into(), 0.05),
        ],
        vectors: vec![],
    };
    let u = ArkPostSource { post: Some(post), display_gamma: 2.2, post_on: true, ..Default::default() }.uniform(0.0);
    assert!((u.exposure_gamma_desat.x - 2f32.powf(0.3)).abs() < 1e-6);
    assert!((u.exposure_gamma_desat.y - 0.95 / 2.2).abs() < 1e-6);
    assert!((u.brightness_contrast.y - (1.05 * std::f32::consts::FRAC_PI_4).tan()).abs() < 1e-5);
    // Live constants of the LUT bake (PS 1906) on Boyle Ext with the default options
    // (engine hook, U-582): exposure 1.231144, gamma 0.431818, contrast 1.081794.
    assert!((u.exposure_gamma_desat.x - 1.231144).abs() < 1e-5);
    assert!((u.exposure_gamma_desat.y - 0.431818).abs() < 1e-5);
    assert!((u.brightness_contrast.y - 1.081794).abs() < 1e-5);
    assert_eq!(u.cb_shadows_opacity.w, 1.0);
}

#[test]
fn custom_transition_scales_opacity() {
    let layer = FogSettings {
        opacity: 0.4,
        origin: -2075.0,
        height: 5500.0,
        near: 3000.0,
        far: 10000.0,
        custom_transition: Some(5000.0),
        height_density: 1.0,
        ..Default::default()
    };
    let src = ArkPostSource { fog: vec![layer], fog_on: true, display_gamma: 2.2, ..Default::default() };
    // Layer top 3425; camera at 170 -> 3255 / 5000 of the opacity.
    let u = src.uniform(170.0);
    assert!((u.layer_opacity.x - 0.4 * 3255.0 / 5000.0).abs() < 1e-5);
    assert_eq!(u.misc.y, 1.0);
    // Above the top: clamped to zero (the original goes negative, U-321).
    assert_eq!(src.uniform(4000.0).layer_opacity.x, 0.0);
}
