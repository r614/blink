//! Tests of `dishonored_runtime::sky_disc` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::sky_disc::*;

fn params(base: &str, scalars: &[(&str, f32)], vectors: &[(&str, [f32; 4])]) -> MaterialParams {
    MaterialParams {
        base_material: Some(base.into()),
        scalars: scalars.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        vectors: vectors.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        ..Default::default()
    }
}

/// Boyle Ext's `Skies02.BOYLE.Moon_Boyle_INST`.
#[test]
fn decodes_the_boyle_moon() {
    let p = params("Vfx_Materials.main_materials.Moon_PMAT", &[("Cloud_Speed", -0.015)], &[("Moon_Color", [0.9525315, 0.9456522, 1.0, 2.0])]);
    let u = DiscUniform::decode(&p).unwrap();
    assert!((u.color.truncate() - Vec3::new(1.905063, 1.8913044, 2.0)).length() < 1e-5);
    assert_eq!(u.params, Vec4::new(0.0, -0.015, 0.0, 0.0));
    assert!(!u.is_sun());
}

/// Sun_PMAT ignores the colour's alpha; defaults from its shader map.
#[test]
fn decodes_the_sun_defaults() {
    let u = DiscUniform::decode(&params("Vfx_Materials.Material.Sun_PMAT", &[], &[("Color", [0.5, 0.5, 0.5, 9.0])])).unwrap();
    assert_eq!(u.color.truncate(), Vec3::splat(1.0));
    assert_eq!(u.params.z, 2.0);
    assert!(u.is_sun());
    assert!(DiscUniform::decode(&params("Skies.Skies_PMAT", &[], &[])).is_none());
}
