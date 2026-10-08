//! Tests of `dishonored_runtime::reflect` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use dishonored_ir::{DishonoredMap, MaterialParams, ReflectCapture, reflect_channel};
use dishonored_runtime::map_render::RenderFeatures;
use dishonored_runtime::reflect::{Layers, material_target, screen_target, toggled_enabled};

fn capture(target: &str, channels: u32) -> ReflectCapture {
    ReflectCapture { target: target.to_string(), channels, ..Default::default() }
}

fn map(captures: Vec<ReflectCapture>) -> DishonoredMap {
    DishonoredMap { reflect_captures: captures, ..Default::default() }
}

/// A capture draws the primitives that share a `ReflectionChannels` bit with it, in one render
/// layer per distinct channel set.
#[test]
fn primitives_follow_the_capture_channels() {
    let features = RenderFeatures::from_flags(&|_| false);
    // Every placed actor: Group_1 only. Two actors with the same set share a layer.
    let one = Layers::new(&map(vec![capture("a", reflect_channel::GROUP_1), capture("b", reflect_channel::GROUP_1)]), &features);
    assert_eq!(one.bits(reflect_channel::GROUP_1), 1);
    // A primitive without the channel (the default: none) is only in the main view.
    assert_eq!(one.bits(0), 0);
    // The class default set (BSP, Static, Dynamic, Skeletal, Particles, Decals) is another layer.
    let two = Layers::new(&map(vec![capture("a", reflect_channel::GROUP_1), capture("c", reflect_channel::CAPTURE_DEFAULT)]), &features);
    assert_eq!(two.bits(reflect_channel::GROUP_1), 0b01);
    assert_eq!(two.bits(1 << 1), 0b10);
    assert_eq!(two.bits(reflect_channel::GROUP_1 | (1 << 1)), 0b11);
}

/// `--no-reflections` draws nothing.
#[test]
fn reflections_can_be_turned_off() {
    let features = RenderFeatures::from_flags(&|f| f == "--no-reflections");
    assert!(!features.reflections);
    let layers = Layers::new(&map(vec![capture("a", reflect_channel::GROUP_1)]), &features);
    assert_eq!(layers.bits(reflect_channel::GROUP_1), 0);
}

/// The water's render target is the `C_Realtime_reflection_Texture` parameter.
#[test]
fn the_water_names_its_render_target() {
    let p = MaterialParams {
        render_targets: vec![("Other".into(), "x.y".into()), ("c_realtime_reflection_texture".into(), "boyle_ext_MS.RealTime_refl".into())],
        ..Default::default()
    };
    assert_eq!(material_target(&p), Some("boyle_ext_MS.RealTime_refl"));
    assert_eq!(material_target(&MaterialParams::default()), None);
}

/// A material that samples a capture's target under another name is a screen (the lens TV).
#[test]
fn a_screen_names_its_render_target() {
    let p = MaterialParams { render_targets: vec![("MaterialExpressionTextureSampleParameter2D_0".into(), "saili_FT_LensTV.screen_TextureRender".into())], ..Default::default() };
    assert_eq!(screen_target(&p), Some("saili_FT_LensTV.screen_TextureRender"));
    assert_eq!(material_target(&p), None);
    let water = MaterialParams { render_targets: vec![("C_Realtime_reflection_Texture".into(), "boyle_ext_MS.RealTime_refl".into())], ..Default::default() };
    assert_eq!(screen_target(&water), None);
}

/// `SceneCaptureActor.OnToggle`: On / Off set `bEnabled`, Toggle flips whatever it is (the level's
/// value until Kismet sets it).
#[test]
fn kismet_toggles_set_the_capture_enabled_state() {
    // Starts disabled in the level (PrsnSewer, Streetsewer): On enables it.
    assert!(!toggled_enabled(false, None, false));
    assert!(toggled_enabled(false, Some(true), false));
    // Toggle without a prior On / Off flips the level's value.
    assert!(toggled_enabled(false, None, true));
    assert!(!toggled_enabled(true, None, true));
    // On, then Toggle: off.
    assert!(!toggled_enabled(false, Some(true), true));
    assert!(!toggled_enabled(true, Some(false), false));
}
