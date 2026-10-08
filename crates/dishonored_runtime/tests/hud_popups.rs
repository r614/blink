//! Tests of `dishonored_runtime::hud_popups` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use dishonored_runtime::hud_popups::*;

fn game() -> Triggers {
    Triggers { game: Some(("ffa".into(), 1)), score: Some((0, Some(1))), ..Default::default() }
}

#[test]
fn banner_opens_after_the_game_start_delay_and_closes() {
    let mut p = HudPopups::default();
    p.step(10.0, game(), 1, false, true);
    assert!(!p.banner_env(11.9).shown());
    // Open tween: 0.2 s from x + 250.
    let e = p.banner_env(12.1);
    assert!(e.alpha > 0.9 && e.dx > 0.0 && e.dx < 10.0, "{e:?}");
    assert_eq!(p.banner_env(13.0), Envelope { alpha: 1.0, dx: 0.0 });
    // One line: 0.7 + 3.5 s, then the 0.25 s fade and hidden 0.5 s after Close().
    let close = 12.0 + 4.2;
    assert_eq!(p.banner_env(close - 0.01).alpha, 1.0);
    let e = p.banner_env(close + 0.1);
    assert!(e.alpha > 0.0 && e.alpha < 0.2 && e.dx > 80.0, "{e:?}");
    assert!(!p.banner_env(close + 0.3).shown());
    assert!(!p.banner_env(close + 1.0).shown());
}

#[test]
fn score_lead_and_key_reopen_the_banner() {
    let mut p = HudPopups::default();
    p.step(0.0, game(), 1, false, true);
    p.step(20.0, game(), 1, false, true);
    assert!(!p.banner_env(20.0).shown());
    let mut g = game();
    g.score = Some((1, Some(1)));
    p.step(21.0, g.clone(), 1, false, true);
    assert!(p.banner_env(21.5).shown());
    // Unchanged afterwards: closes on time.
    p.step(30.0, g.clone(), 1, false, true);
    assert!(!p.banner_env(30.0).shown());
    // Leader change.
    g.score = Some((1, Some(7)));
    p.step(31.0, g.clone(), 1, false, true);
    assert!(p.banner_env(31.5).shown());
    // Journal key, also with the popups option off.
    p.step(40.0, g.clone(), 1, true, false);
    assert!(p.banner_env(40.5).shown());
    g.score = Some((2, Some(7)));
    p.step(50.0, g, 1, false, false);
    assert!(!p.banner_env(50.5).shown());
}

#[test]
fn target_paper_follows_the_target_notification() {
    let mut p = HudPopups::default();
    let mut g = game();
    g.target = Some(4);
    p.step(0.0, g.clone(), 1, false, true);
    assert!(p.target_env(2.5).shown());
    // 1 s open + 5 s, then 0.25 s fade, hidden 1 s after Close().
    assert_eq!(p.target_env(2.0 + 5.9).alpha, 1.0);
    assert!(!p.target_env(2.0 + 6.0 + 0.3).shown());
    g.target = Some(5);
    p.step(20.0, g, 1, false, true);
    assert!(p.target_env(20.1).shown());
}

#[test]
fn no_match_no_popup() {
    let mut p = HudPopups::default();
    p.step(0.0, Triggers::default(), 0, true, true);
    assert!(!p.banner_env(1.0).shown());
}

#[test]
fn mission_paper_opens_for_a_notification_only() {
    let mut p = HudPopups::default();
    let mut g = Triggers { game: Some(("mission".into(), 1)), objectives: vec![("Try the stations".into(), false, vec![("Blink".into(), 1, false)])], ..Default::default() };
    p.step(0.0, g.clone(), 1, false, true);
    // The banner opens after the start delay; the target paper does not (no notification).
    assert!(p.banner_env(2.5).shown());
    assert!(!p.target_env(2.5).shown());
    // A task change reopens the banner, not the paper.
    g.objectives[0].2[0].1 = 2;
    p.step(20.0, g.clone(), 1, false, true);
    assert!(p.banner_env(20.5).shown());
    assert!(!p.target_env(20.5).shown());
    // A Notify action opens the paper with the target timing (1 s + 5 s, then the fade).
    g.notification = Some(3);
    p.step(30.0, g, 1, false, true);
    assert!(p.target_env(30.5).shown());
    assert_eq!(p.target_env(35.9).alpha, 1.0);
    assert!(!p.target_env(36.3).shown());
}
