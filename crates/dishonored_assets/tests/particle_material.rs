//! Particle material recipes (decoded shaders, EFFECTS.md "Particle materials").

use dishonored_assets::particle_material::{Colour, Opacity, recipe};

fn r(base: &str, on: &[&str], masks: &[(&str, usize)]) -> dishonored_assets::particle_material::Recipe {
    let on: Vec<String> = on.iter().map(|s| s.to_string()).collect();
    let masks: Vec<(String, usize)> = masks.iter().map(|(n, c)| (n.to_string(), *c)).collect();
    recipe(base, &|s: &str| on.iter().any(|x| x == s), &|s: &str| masks.iter().find(|m| m.0 == s).map(|m| m.1)).expect(base)
}

#[test]
fn every_particle_base_has_an_opacity_source() {
    for base in [
        "DefaultTranslu_PMAT",
        "DefaultTranslu2sided_PMAT",
        "DefaultAdditive_PMAT",
        "DefaultAdditive_2Sided_PMAT",
        "HeightMapAnim_PMAT",
        "SubUV_HeightMap_PMAT",
        "HeavySmoke_PMAT",
        "Water_Translu__PMAT",
        "Water_Additive__PMAT",
        "WaterSplash_PMAT",
        "Fire_Explosion_PMAT",
        "Look_Up_Fire_PMAT",
        "Volume_Light_translu_PMAT",
    ] {
        let x = r(&format!("Vfx_Materials.main_materials.{base}"), &[], &[]);
        assert!(matches!(x.opacity, Opacity::Texture { .. }), "{base}: {x:?}");
    }
    assert_eq!(r("Glow_PMAT", &[], &[]).opacity, Opacity::Radial);
    assert_eq!(r("Default_Simple_Translucent_PMAT", &[], &[]).opacity, Opacity::VertexOnly);
    assert!(recipe("Some_Wall_PMAT", &|_: &str| false, &|_: &str| None).is_none());
}

#[test]
fn blood_splash_reads_the_height_map_channel() {
    // Blood_Spray2_INST: Use_Blue (shader 5872 kills on H.b).
    let x = r("SubUV_HeightMap_PMAT", &["Use_Blue", "N_Enable_NormalMap"], &[]);
    assert_eq!(x.opacity, Opacity::Texture { param: "H_HightMap_RGB_R_is_default", channel: 2 });
    assert!(x.erode);
    assert_eq!(x.colour, Colour::Texture { param: "D_diffuse_texture", channel: None });
    let y = r("HeightMapAnim_PMAT", &["Use_Green", "B_Enable_Blood"], &[]);
    assert_eq!(y.opacity, Opacity::Texture { param: "O_OpacityRGB_R_is_default", channel: 1 });
}

#[test]
fn arkane_default_switches() {
    let steam = r("DefaultTranslu_PMAT", &["Mo_Enable_Opacity_Motion", "Mo_Enable_Diffuse_Motion"], &[]);
    assert_eq!(steam.opacity, Opacity::Texture { param: "Motion_Mask", channel: 0 });
    assert_eq!(steam.colour, Colour::Texture { param: "Motion_Mask", channel: None });
    let smoke = r("DefaultTranslu_PMAT", &["D_DisableDiffuse"], &[("O_Opacity_Channel", 1)]);
    assert_eq!(smoke.colour, Colour::White);
    assert_eq!(smoke.opacity, Opacity::Texture { param: "O_Opacity", channel: 1 });
    let add = r("DefaultAdditive_PMAT", &["D_Enable_Diffuse_Channel", "O_EnableOpacityAlpha"], &[("D_Diffuse_Channel", 0)]);
    assert_eq!(add.colour, Colour::Texture { param: "D_Diffuse", channel: Some(0) });
    assert_eq!(add.opacity, Opacity::Texture { param: "O_Opacity", channel: 3 });
}

#[test]
fn look_up_glow_reads_the_particle_colour_as_a_lookup() {
    // smoky_lookup (the pistol muzzle's smoke): alpha_channel G, shader 36315.
    let x = r("tguichard_test_test.smoke_look_up.test_look_up", &[], &[("alpha_channel", 1)]);
    assert_eq!(x.opacity, Opacity::Texture { param: "Diffuse", channel: 1 });
    assert_eq!(x.colour, Colour::White);
    assert!(x.erode && x.erode_smooth && x.erode_bare);
    assert_eq!(x.vertex_lerp.map(|(a, b)| (a.0, b.0)), Some(("diffuse_color2", "diffuse_color")));
    assert!(x.tint.is_none());
    // The look-up smoke keeps its vertex alpha factor and colours by its constant.
    let smoke = r("Look_Up_Smoke_translu_PMAT", &[], &[]);
    assert!(smoke.erode_smooth && !smoke.erode_bare && smoke.vertex_lerp.is_none());
}
