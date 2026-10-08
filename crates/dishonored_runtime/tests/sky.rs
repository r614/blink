//! Tests of `dishonored_runtime::sky` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::sky::*;
use dishonored_ir::MaterialParams;

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

/// L_Out_P's `Skies.Outsider.Outsider_sky_inst` (6059): the horizon terms, Storm_mask R.
#[test]
fn decodes_the_outsider_horizon() {
    let mut p = params(
        &[("Enable_Storm", true), ("H_Enable_Horizon", true), ("C_Enable_Clouds", true), ("G_Use_Gradiant_Sky", true), ("Use_UV_Channel_2", false), ("C2_Enable_Clouds_2", true)],
        &[("H_HorizonIntensity", 10.0), ("H_HorizonExponent", 3.0), ("H_Horizon_UV_Distortion", 4.0), ("H_Horizon_Speed", 1.0), ("H_HorizonOffset", 0.1)],
        &[("Horizon_Color", [0.5326087, 0.83247864, 1.0, 0.15])],
    );
    p.mask_channels = vec![("Storm_mask".into(), 0)];
    let u = SkyUniform::decode(&p).unwrap();
    assert_eq!(u.horizon.w, 1.0);
    assert!((u.horizon.truncate() - Vec3::new(0.5326087, 0.83247864, 1.0) * 0.15).length() < 1e-6);
    assert_eq!(u.horizon2, Vec4::new(1.0, 4.0, 0.1, 3.0));
    assert_eq!(u.misc2.z, 10.0);
    assert_eq!(u.misc2.w, 0.0);
    assert_eq!(u.misc2.y, 0.0);
    // Defaults: speed 2, distortion 0.5, exponent 5, intensity 1; RGB storm.
    let d = SkyUniform::decode(&params(&[], &[], &[])).unwrap();
    assert_eq!(d.horizon2, Vec4::new(2.0, 0.5, 0.0, 5.0));
    assert_eq!(d.misc2.z, 1.0);
    assert_eq!(d.misc2.w, -1.0);
    assert_eq!(d.horizon.w, 0.0);
}