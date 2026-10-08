//! Tests of `dishonored_runtime::bloom` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::bloom::*;
use dishonored_ir::MaterialParams;

fn params(switches: &[(&str, bool)], scalars: &[(&str, f32)], vectors: &[(&str, [f32; 4])]) -> MaterialParams {
    MaterialParams {
        emissive: Some(0),
        static_switches: switches.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        scalars: scalars.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        vectors: vectors.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        ..Default::default()
    }
}

/// generic_PMAT shader maps: bloom = emissive [x colour with E_Enable_Emissive_color] x
/// B_Bloom_Intensity, only with B_Enable_Bloom.
#[test]
fn bloom_colour_follows_the_switches() {
    let colour = [("E_Emissive_Color", [0.98443985, 1.0, 0.90217394, 2.0])];
    let on = params(&[("B_Enable_Bloom", true), ("E_Enable_Emissive", true), ("E_Enable_Emissive_color", true)], &[("B_Bloom_Intensity", 0.5)], &colour);
    let c = bloom_color(&on).unwrap();
    assert!((c - Vec3::new(0.98443985, 1.0, 0.90217394)).length() < 1e-5, "{c}");
    // Without E_Enable_Emissive_color the texture alone (x intensity).
    let plain = params(&[("B_Enable_Bloom", true), ("E_Enable_Emissive", true), ("E_Enable_Emissive_color", false)], &[], &colour);
    assert_eq!(bloom_color(&plain), Some(Vec3::ONE));
    // B_Enable_Bloom off (or absent), or no emissive: no bloom part.
    assert_eq!(bloom_color(&params(&[("B_Enable_Bloom", false), ("E_Enable_Emissive", true)], &[], &colour)), None);
    assert_eq!(bloom_color(&params(&[("E_Enable_Emissive", true)], &[], &colour)), None);
    let mut no_tex = on.clone();
    no_tex.emissive = None;
    assert_eq!(bloom_color(&no_tex), None);
    // Intensity 0 (the sky instances' value): nothing to draw.
    assert_eq!(bloom_color(&params(&[("B_Enable_Bloom", true)], &[("B_Bloom_Intensity", 0.0)], &[])), None);
}
