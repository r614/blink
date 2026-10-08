//! Tests of `dishonored_runtime::prop_light` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::prop_light::*;

#[test]
fn uniform_from_the_environment() {
    let env = LightEnvironment { direction: glam::Vec3::Z, color: [1.0, 0.5, 0.25], upper_sky: [0.1; 3], lower_sky: [0.05; 3] };
    let l = prop_light(&env, None, &[], 2.0);
    // UE3 +Z is Bevy +Y.
    assert!((l.params.direction.truncate() - Vec3::Y).length() < 1e-6);
    assert_eq!(l.params.color, Vec4::new(2.0, 1.0, 0.5, 1.0));
    assert_eq!(l.params.lower_sky.x, 0.1);
    assert_eq!(l.params.spec.y, 0.0);
    assert_eq!(l.params.spec_mask, Vec4::new(1.0, 1.0, 1.0, 0.0));
}
