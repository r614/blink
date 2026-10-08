//! Tests of the decoded translucent materials: `volume_light` (Volume_Light_PMAT), `spillway`
//! (water_spillway_translu_PMAT) and `water_foam` (water_Foam_MAT).

use bevy::prelude::*;
use dishonored_ir::MaterialParams;
use dishonored_runtime::{spillway, volume_light, water_foam};

fn params(base: &str, switches: &[(&str, bool)], scalars: &[(&str, f32)], vectors: &[(&str, [f32; 4])]) -> MaterialParams {
    MaterialParams {
        base_material: Some(base.into()),
        static_switches: switches.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        scalars: scalars.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        vectors: vectors.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        ..Default::default()
    }
}

/// L_Pub_Day_P's `Hub_FX.Light.Day_window_light_inst` (Use_Vertex_Color + Use_rays, shader 6633).
#[test]
fn decodes_the_pub_window_beam() {
    let mut p = params(
        volume_light::VOLUME_LIGHT_PMAT,
        &[("Use_Vertex_Color", true), ("Use_rays", true)],
        &[("LightIntensity", 1.5), ("Fresnel_exponent", 5.0), ("Db_Depth_Biased_Alpha_Intensity", -1.0), ("StartOffset", 512.0), ("Distance", 3.5), ("Ray_Speed", 0.05), ("Alert", 0.0)],
        &[("Color", [1.0, 0.93397576, 0.7825248, 1.0]), ("ColorAlert", [2.0, 0.0, 0.05, 1.0])],
    );
    p.two_sided = true;
    let u = volume_light::VolumeLightUniform::decode(&p, 1.0).unwrap();
    assert!((u.color.truncate() - Vec3::new(1.5, 1.4009637, 1.1737872)).length() < 1e-5);
    assert_eq!(u.mesh_color, Vec4::ONE);
    // k = 1 / ((1 - (-1)) x 50) = 0.01; camera fade 0.0005 / 3.5.
    assert!((u.terms.z - 0.01).abs() < 1e-7);
    assert_eq!(u.terms.w, 512.0);
    assert!((u.flags.x - 0.0005 / 3.5).abs() < 1e-9);
    assert_eq!((u.flags.y, u.flags.z, u.flags.w), (1.0, 1.0, 0.05));
    assert_eq!(u.sides.x, 1.0);
    // Disable_CameraFade, an intensity of 1 or more (k = 1000), Alert blending to ColorAlert.
    let q = params(volume_light::VOLUME_LIGHT_PMAT, &[("Disable_CameraFade", true)], &[("Db_Depth_Biased_Alpha_Intensity", 1.0), ("Alert", 1.0), ("LightIntensity", 1.0)], &[]);
    let v = volume_light::VolumeLightUniform::decode(&q, 1.0).unwrap();
    assert_eq!(v.flags.x, -1.0);
    assert_eq!(v.terms.z, 1000.0);
    assert!((v.color.truncate() - Vec3::new(2.0, 0.0, 0.05)).length() < 1e-6);
    assert!(volume_light::VolumeLightUniform::decode(&MaterialParams::default(), 1.0).is_none());
}

/// L_PrsnSewer_P's `Prison_FX.Effects.Water_sewer_light_INST`-like instance (shader 1585): base
/// defaults and the instance's LocalToWorld columns.
#[test]
fn decodes_a_water_sheet() {
    let p = params(spillway::WATER_SPILLWAY_PMAT, &[("Vc_Use_Vertex_Color", true)], &[("SpillWay_Translucency", 0.4)], &[("C_Cubemap_color", [0.5, 0.5, 0.5, 2.0])]);
    let m = Mat4::from_scale(Vec3::new(2.0, 1.0, 1.0));
    let u = spillway::SpillwayUniform::decode(&p, m, 1.0).unwrap();
    assert!((u.tweak.truncate() - Vec3::new(0.635515, 0.687244, 0.717391)).length() < 1e-6);
    assert_eq!(u.cube, Vec4::new(1.0, 1.0, 1.0, 0.0));
    assert_eq!(u.params, Vec4::new(1.0, 0.2, 1.0, 1.0));
    assert_eq!(u.params2, Vec4::new(1.0, 0.4, 0.0, 0.0));
    assert_eq!(u.l2w_x, Vec4::new(2.0, 0.0, 0.0, 0.0));
    let river = params(spillway::WATER_SPILLWAY_PMAT, &[("River", true)], &[], &[]);
    assert_eq!(spillway::SpillwayUniform::decode(&river, Mat4::IDENTITY, 1.0).unwrap().params2.z, 1.0);
}

/// L_Isl_LowChaos_P's `island_water_near.Water_Island_LC_near_Inst` (Enable_Foam, mask R: 37074).
#[test]
fn decodes_the_island_sea() {
    let mut p = params(
        water_foam::WATER_FOAM_MAT,
        &[("Enable_Foam", true), ("Enable_Rain", false)],
        &[("Translucency", 0.95), ("Opacity_Depth", 384.0), ("Wavy_UV", 0.01), ("W_Waves_1_tilling", 10.0), ("W_Waves_2_tilling", 12.0), ("W_Waves_Speed", 0.5)],
        &[("Water_color", [0.011765, 0.015686, 0.011765, 0.5]), ("Foam_Brightness", [0.535217, 0.666667, 0.589173, 0.35])],
    );
    p.mask_channels = vec![("F_Foam_Mask_Chanel".into(), 0)];
    let u = water_foam::WaterFoamUniform::decode(&p, Vec3::new(1.0, 2.0, 3.0), 1.0).unwrap();
    assert!((u.water.truncate() - Vec3::new(0.0058825, 0.007843, 0.0058825)).length() < 1e-6);
    assert_eq!(u.foam_color.w, 1.0);
    assert_eq!(u.foam2.z, 0.0);
    assert_eq!(u.foam2.y, 0.95);
    assert_eq!(u.waves, Vec4::new(0.5, 10.0, 12.0, 0.01));
    assert_eq!(u.refl.w, 384.0);
    assert_eq!(u.mask, Vec4::new(1.0, 0.0, 0.0, 0.0));
    assert_eq!(u.object, Vec4::new(1.0, 2.0, 3.0, 0.0));
}
