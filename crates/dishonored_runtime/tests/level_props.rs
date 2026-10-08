//! Tests of `dishonored_runtime::level_props` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::level_props::*;
use dishonored_ir::DoorAnims;
use dishonored_mission::runtime::MoverView;

fn door() -> DoorAnims {
    // DBDoors_big_rich_ext_metal_twk: the stages name the opposite direction's animation.
    DoorAnims { initial: 0, open_cw: "Player_Open_Ccw".into(), open_ccw: "Player_Open_Cw".into(), close_cw: "Player_Close_Ccw".into(), close_ccw: "Player_Close_Cw".into() }
}

#[test]
fn doors_play_the_stage_of_their_new_state() {
    let d = door();
    assert_eq!(transition_anim(&d, 0, 1), "Player_Open_Ccw");
    assert_eq!(transition_anim(&d, 0, 2), "Player_Open_Cw");
    assert_eq!(transition_anim(&d, 1, 0), "Player_Close_Ccw");
    assert_eq!(transition_anim(&d, 2, 0), "Player_Close_Cw");
    // Open doors rest on their open stage, closed ones in the reference pose.
    assert_eq!(rest_anim(&d, 1), Some("Player_Open_Ccw"));
    assert_eq!(rest_anim(&d, 0), None);
    assert_eq!(rest_anim(&DoorAnims::default(), 2), None);
}

#[test]
fn movers_match_actor_and_placement() {
    let mv = vec![MoverView { actor: "L_A:InterpActor_1".into(), placed: [10.0, 0.0, 0.0], translation: [0.0, 0.0, 50.0], rotation: [0.0, 0.0, 0.0, 1.0] }];
    let d = mover_delta(&mv, "l_a:interpactor_1", glam::Vec3::new(10.0, 0.0, 0.0)).unwrap();
    assert_eq!(d.w_axis.truncate(), glam::Vec3::new(0.0, 0.0, 50.0));
    // Same name in another sublevel (elsewhere): not moved.
    assert!(mover_delta(&mv, "L_A:InterpActor_1", glam::Vec3::new(5000.0, 0.0, 0.0)).is_none());
}
