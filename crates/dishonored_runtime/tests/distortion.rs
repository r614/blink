//! Tests of `dishonored_runtime::distortion` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::distortion::*;
use dishonored_ir::MaterialParams;

fn params(switches: &[(&str, bool)], scalars: &[(&str, f32)]) -> MaterialParams {
    MaterialParams {
        static_switches: switches.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        scalars: scalars.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        ..Default::default()
    }
}

#[test]
fn decodes_the_pistol_wind_wave() {
    // `Vfx_GamePlay.Windblast.WindWave_Disto_01_inst` (parent DefaultAdditive_PMAT): the
    // instance's scalars first, then the base expression defaults.
    let p = params(
        &[("Dist_Enable_Distortion", true), ("D_Disable_Diffuse", true), ("D_Disable_Opacity_Texture", false), ("Enable_UV_Distortion", false), ("Enable_UV_Scrolling", true), ("Enable_Vertex_Color", true)],
        &[("Dist_Distortion_Intensity", -100.0), ("Dist_Distortion_Speed", 0.0), ("UV_Distortion_Intensity", 0.1), ("U_Scrolling_Speed", 0.1), ("V_Scrolling_Speed", -0.5), ("Dist_Distortion_Intensity", 1.0), ("Dist_Distortion_Speed", 1.0)],
    );
    let tex = vec![("D_Diffuse".to_string(), 1), ("Dist_Distortion".to_string(), 2), ("O_Opacity".to_string(), 3), ("UV_Distortion".to_string(), 4)];
    let m = DistortionMaterial::decode(&p, &tex).unwrap();
    assert_eq!(m.texture, 2);
    assert_eq!(m.intensity, -100.0);
    assert_eq!(m.opacity, Some(3));
    assert_eq!(m.uv_distortion, None);
    assert_eq!((m.scroll, m.pan_speed, m.tweak_alpha), ([0.1, -0.5], 0.0, 1.0));
    assert!(m.vertex_alpha && m.distortion_only);
    // Switch off, or no distortion texture: no distortion.
    let off = params(&[("Dist_Enable_Distortion", false)], &[]);
    assert!(DistortionMaterial::decode(&off, &tex).is_none());
    let on = params(&[("Dist_Enable_Distortion", true)], &[]);
    assert!(DistortionMaterial::decode(&on, &tex[..1]).is_none());
    // Base defaults: intensity 1, opacity on, vertex colour on, colour pass kept.
    let m = DistortionMaterial::decode(&on, &tex).unwrap();
    assert_eq!((m.intensity, m.pan_speed, m.opacity, m.vertex_alpha, m.distortion_only, m.scroll), (1.0, 1.0, Some(3), true, false, [0.0, 0.0]));
    let uvd = params(&[("Dist_Enable_Distortion", true), ("Enable_UV_Distortion", true), ("D_Disable_Opacity_Texture", true)], &[("UV_Distortion_Intensity", 0.1)]);
    let m = DistortionMaterial::decode(&uvd, &tex).unwrap();
    assert_eq!((m.uv_distortion, m.opacity), (Some((4, 0.1, 1.0)), None));
}

#[test]
fn push_lays_out_pans_and_alpha() {
    let m = DistortionMaterial { texture: 2, intensity: -100.0, pan_speed: 0.25, opacity: Some(3), uv_distortion: None, scroll: [0.1, -0.5], tweak_alpha: 1.0, vertex_alpha: false, distortion_only: true };
    let h = Handle::<Image>::default();
    let mut f = DistortionFrame::default();
    // Time 3 s: pan frac(0.75), scroll frac(0.3, -1.5) = (0.3, 0.5); a mesh emitter's alpha applies.
    f.push(&m, (&h, Some(&h), None), [(Vec3::ONE, Vec2::new(0.5, 0.5), 0.5)].into_iter(), 3.0, true);
    assert_eq!(f.batches.len(), 1);
    let v = &f.verts;
    assert_eq!(v.len(), VERTEX_FLOATS);
    assert!((v[5] - 0.75).abs() < 1e-6 && (v[6] - 0.75).abs() < 1e-6);
    assert!((v[9] - 0.3).abs() < 1e-5 && (v[10] - 0.5).abs() < 1e-5);
    assert_eq!((v[11], v[12]), (-50.0, 1.0));
    // A sprite without `Enable_Vertex_Color`: no alpha.
    f.clear();
    f.push(&m, (&h, None, None), [(Vec3::ONE, Vec2::ZERO, 0.5)].into_iter(), 0.0, false);
    assert_eq!(f.verts[11], -100.0);
}

#[test]
fn accumulate_and_apply_round_trip() {
    // |D|^2 < 0.1 is discarded (shader: dp2add D, D, -0.1; texkill).
    assert!(DistortionMaterial::accumulate(Vec2::splat(0.2), false).is_none());
    // D = (10, -20): RG = (40/255, 0), BA = (0, 80/255); applied (RG - BA) * (0.25, -0.25).
    let a = DistortionMaterial::accumulate(Vec2::new(10.0, -20.0), false).unwrap();
    assert!((a - Vec4::new(40.0 / 255.0, 0.0, 0.0, 80.0 / 255.0)).abs().max_element() < 1e-6);
    let uv = DistortionMaterial::apply(Vec2::splat(0.5), a);
    assert!((uv - Vec2::new(0.5 + 10.0 / 255.0, 0.5 + 20.0 / 255.0)).abs().max_element() < 1e-6);
    // Clamped at 4 D / 255 = 1, i.e. a quarter of the screen.
    let a = DistortionMaterial::accumulate(Vec2::new(-100.0, 0.0), false).unwrap();
    assert_eq!(a, Vec4::new(0.0, 0.0, 1.0, 0.0));
    // Occluded: written, but zero.
    assert_eq!(DistortionMaterial::accumulate(Vec2::new(-100.0, 0.0), true), Some(Vec4::ZERO));
    // Leaving the scene rectangle keeps the undistorted pixel.
    assert_eq!(DistortionMaterial::apply(Vec2::new(0.1, 0.5), Vec4::new(0.0, 0.0, 1.0, 0.0)), Vec2::new(0.1, 0.5));
}
