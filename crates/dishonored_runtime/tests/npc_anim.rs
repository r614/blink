//! Tests of `dishonored_runtime::npc_anim` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use dishonored_runtime::npc_anim::*;

#[test]
fn diagonal_ratio() {
    // Straight ahead and the snapped neighbourhood: no correction.
    assert!((diagonal_factor(0.0) - 1.0).abs() < 1e-5);
    assert!((diagonal_factor(1.0) - 1.0).abs() < 1e-5);
    // Halfway between N and E: the chord is 1/sqrt(2) from the centre.
    assert!((diagonal_factor(45.0) - std::f32::consts::SQRT_2).abs() < 1e-4);
    assert!((diagonal_factor(-135.0) - std::f32::consts::SQRT_2).abs() < 1e-4);
}

#[test]
fn direction_weights_snap() {
    let (w, both) = dir_weights(1.0);
    assert!(!both && w[2] == 1.0);
    let (w, both) = dir_weights(45.0);
    assert!(both && (w[2] - 0.5).abs() < 1e-5 && (w[3] - 0.5).abs() < 1e-5);
}

#[test]
fn velocity_smoothing_rate() {
    let p = LocoNodeParams::default();
    // 300 uu/s apart: full rate (2000 uu/s^2), 1/60 s moves 33.3 uu/s.
    let v = smooth_velocity((0.0, 0.0), (0.0, 300.0), 1.0 / 60.0, &p);
    assert!((v.1 - 2000.0 / 60.0).abs() < 1e-3);
    // 50 apart: half the rate.
    let v = smooth_velocity((0.0, 100.0), (0.0, 150.0), 1.0 / 60.0, &p);
    assert!((v.1 - (100.0 + 1000.0 / 60.0)).abs() < 1e-3);
    // Small gaps close at the minimum ratio, never overshooting.
    let v = smooth_velocity((0.0, 100.0), (0.0, 101.0), 1.0, &p);
    assert!((v.1 - 101.0).abs() < 1e-4);
}

#[test]
fn feet_windows() {
    // Walk (m_fFootOnGroundPct 0.4): left foot planted within 0.2 of 0.44, right of 0.94.
    let feet = [0.44, 0.94];
    assert_eq!(feet_on_ground(&feet, 0.30, 0.2), [true, false]);
    assert_eq!(feet_on_ground(&feet, 0.05, 0.2), [false, true]);
    assert_eq!(feet_on_ground(&feet, 0.69, 0.2), [false, false]);
    assert_eq!(feet_on_ground(&feet, 0.3, 0.0), [false, false]);
}

#[test]
fn hit_directions() {
    assert_eq!(cardinal(10.0), 0);
    assert_eq!(cardinal(-90.0), 1);
    assert_eq!(cardinal(90.0), 2);
    assert_eq!(cardinal(170.0), 3);
    assert_eq!(eight_way(-45.0), 1);
    assert_eq!(eight_way(45.0), 2);
    assert_eq!(eight_way(-135.0), 6);
    assert_eq!(eight_way(135.0), 7);
    assert_eq!(eight_way(180.0), 5);
}

#[test]
fn notify_crossing_window() {
    let n = |t: f32| Notify { t, duration: 0.0, kind: NotifyKind::Ragdoll };
    let notifies = vec![n(0.0), n(0.5), n(1.0)];
    let times = |t0: Option<f32>, t1: f32| -> Vec<f32> { crossing(&notifies, t0, t1).iter().map(|n| n.t).collect() };
    // A clip started this frame fires from its start: the notify at 0 too.
    assert_eq!(times(None, 0.0), vec![0.0]);
    assert_eq!(times(None, 0.6), vec![0.0, 0.5]);
    // Afterwards the window is half-open: (t0, t1].
    assert_eq!(times(Some(0.0), 1.0), vec![0.5, 1.0]);
    assert_eq!(times(Some(0.4), 0.5), vec![0.5]);
    assert_eq!(times(Some(0.5), 0.6), Vec::<f32>::new());
    assert_eq!(times(Some(0.4), 0.9), vec![0.5]);
}
