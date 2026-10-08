//! Tests of `dishonored_runtime::handoff` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::handoff::*;
#[test]
fn return_line_round_trips() {
    let p = Placement { pos: Some(IVec2::new(-1910, 42)), size: (1600, 900) };
    assert_eq!(Placement::parse_return(&format!("2026 INFO x: {}", p.return_line())), Some(p));
    assert_eq!(Placement::parse_return("nothing"), None);
}
