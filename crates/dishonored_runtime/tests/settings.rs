//! Tests of `dishonored_runtime::settings` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::settings::*;
#[test]
fn every_default_binding_maps_to_an_input() {
    for (a, k) in dishonored_settings::DEFAULT_BINDINGS {
        if *k != "None" {
            assert!(input_for(k).is_some(), "{a} -> {k}");
        }
    }
    assert_eq!(name_for(Input::Key(KeyCode::Space)), Some("SpaceBar"));
}
