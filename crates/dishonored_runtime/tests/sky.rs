//! Tests of `dishonored_runtime::sky` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::sky::*;

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
