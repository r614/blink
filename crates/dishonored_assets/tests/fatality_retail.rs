//! The player sword's finisher kills against the canonical GOG build (skipped without the
//! install): `DisTweaks_Fatality` move set 0, its states' masters, exits, slaves and bend-time
//! windows.

use dishonored_assets::Content;
use dishonored_ir::gameplay::{FatalityConfig, fatality_move as fm};

#[test]
fn the_sword_fatality_decodes() {
    let Ok(c) = Content::locate() else { return };
    let db = dishonored_assets::tweaks::TweakDb::load(&c).unwrap();
    let f = dishonored_assets::fatality::load_fatality(&c, &db).unwrap();
    // The cached default (configs decoded before the field) holds the same chances and pacing.
    let d = FatalityConfig::default();
    assert_eq!((f.behead_chance, f.slomo_chance, f.slomo_min_interval, f.slomo_realtime_chance, f.generic_chance), (d.behead_chance, d.slomo_chance, d.slomo_min_interval, d.slomo_realtime_chance, d.generic_chance));
    assert_eq!((f.finishers_before_slow, f.time_before_slow, f.adrenaline_mult, f.max_height_difference), (d.finishers_before_slow, d.time_before_slow, d.adrenaline_mult, d.max_height_difference));
    assert_eq!(f.moves.len(), fm::COUNT);
    let masters = |k: usize| f.moves[k].iter().map(|c| c.master.as_str()).collect::<Vec<_>>();
    assert_eq!(masters(fm::FRONT), ["Sword_Ready_Fatality_Front_A_Master", "Sword_Ready_Fatality_Front_B_Master"]);
    assert_eq!(masters(fm::FRONT_FAST), ["Sword_Ready_Fatality_FastFront_A_Master", "Sword_Ready_Fatality_FastFront_B_Master", "Sword_Ready_Fatality_FastFront_C_Master", "Sword_Ready_Fatality_FastFront_D_Master"]);
    assert_eq!(masters(fm::HEAD_CHOP), ["Sword_Ready_Fatality_Behead_A_Master", "Sword_Ready_Fatality_Behead_B_Master"]);
    assert_eq!(masters(fm::KNOCKDOWN), ["Sword_Ready_Fatality_KnockDown_Master"]);
    assert_eq!(masters(fm::GENERIC), ["Sword_Ready_Fatality_Generic_Left_A", "Sword_Ready_Fatality_Generic_Right_A"]);
    // The sword's move set leaves the sides and the back empty.
    for k in [fm::LEFT, fm::RIGHT, fm::BACK, fm::LEFT_FAST, fm::RIGHT_FAST, fm::BACK_FAST] {
        assert!(f.moves[k].is_empty(), "action {k}");
    }
    // Slaves: `DisNPCAnim_Fatality` 88, `FatalityFast` 89, `FatalityKnockDown` 90, `FatalityHeadChop` 91;
    // the generic swing has none and hits at its damage zone.
    assert!(f.moves[fm::FRONT].iter().all(|c| c.slave == 88 && !c.upper_body && c.hit.is_none()));
    assert!(f.moves[fm::FRONT_FAST].iter().all(|c| c.slave == 89));
    assert!(f.moves[fm::KNOCKDOWN].iter().all(|c| c.slave == 90));
    assert!(f.moves[fm::HEAD_CHOP].iter().all(|c| c.slave == 91));
    assert!(f.moves[fm::GENERIC].iter().all(|c| c.slave == 0 && c.upper_body && c.hit.is_some_and(|h| h > 0.0 && h < c.exit)));
    // `Sword_Ready_Fatality_Front_A_Master`: AnimStateExit 2.1, two adrenaline windows.
    let a = &f.moves[fm::FRONT][0];
    assert!((a.exit - 2.1).abs() < 1e-3, "{}", a.exit);
    assert_eq!(a.bends.len(), 2);
    assert!(a.bends.iter().all(|w| w.adrenaline));
    assert!((a.bends[0].start - 0.1).abs() < 1e-4 && (a.bends[0].world - 0.4).abs() < 1e-6 && (a.bends[1].world - 0.2).abs() < 1e-6 && (a.bends[1].player - 0.4).abs() < 1e-6);
}

/// The paired moves' masters' root motion (`root0_jnt`, `RMM_Translate` / `RMRM_RotateActor`):
/// the front assassination lunges 85 uu and comes back without turning; the fast front finisher D
/// runs 120 uu past its victim, turning 46 degrees to the right and back on the way.
#[test]
fn the_paired_masters_root_motion_decodes() {
    let Ok(c) = Content::locate() else { return };
    let db = dishonored_assets::tweaks::TweakDb::load(&c).unwrap();
    let f = dishonored_assets::fatality::load_fatality(&c, &db).unwrap();
    let rm = dishonored_assets::fatality::load_root_motion(&c, &f).unwrap();
    for m in dishonored_ir::gameplay::ASSASSINATION_MASTERS {
        assert!(rm.contains_key(m), "{m}");
    }
    let front = &rm["Sword_Ready_Assassination_Front_Master"];
    let peak = front.keys.iter().map(|k| k[0]).fold(0.0f32, f32::max);
    assert!((peak - 85.2).abs() < 0.5, "{peak}");
    assert!(front.at(10.0)[0].abs() < 0.5 && front.yaw.iter().all(|y| y.abs() < 0.5));
    let d = &rm["Sword_Ready_Fatality_FastFront_D_Master"];
    assert!((d.at(10.0)[0] - 120.3).abs() < 0.5, "{:?}", d.at(10.0));
    let turn = d.yaw.iter().copied().fold(0.0f32, f32::max);
    assert!((turn - 46.0).abs() < 1.0 && d.yaw_at(10.0).abs() < 0.5, "{turn}");
    // The generic finisher swing is an upper-body state: no root motion.
    assert!(f.moves[fm::GENERIC].iter().all(|c| !rm.contains_key(&c.master)));
}
