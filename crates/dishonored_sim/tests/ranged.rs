//! Ranged weapon and gadget rules ported from the decompiled exe
//! (research/dishonored/systems/ranged.md F1-F6, F10), against the extracted gameplay config
//! (fixtures/config/gameplay_config.json).

use dishonored_ir::items::{GrenadeConfig, HitBody, HitBodyConfig, HitRegion};
use dishonored_sim::SimConfig;
use dishonored_sim::gadgets::{bounce, world_bounce};
use dishonored_sim::hit_regions::hit_region;
use dishonored_sim::left_hand::{arrow_damage, bullet_damage};
use dishonored_sim::state::PlayerState;
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn bullet_damage_is_ranged_damage_plus_tweak_damage() {
    // F1: (int)Attribute_RangedDamage 10 + m_fDamage 20 (0x00C29820).
    assert_eq!(cfg().tuning.bullet_damage, 30.0);
}

#[test]
fn bullet_range_bands_from_the_spawn_point() {
    // F2: x1.5 strictly below m_fCloseRange 400, x m_fDamageMultiplier_LongRange (1.0) strictly
    // above m_fLongRange 1500; x3 on the head; x0.5 after a pierce.
    let t = cfg().tuning;
    let b = t.bullet_damage;
    assert_eq!(bullet_damage(&t, b, false, false, 399.9, false), 45.0);
    assert_eq!(bullet_damage(&t, b, false, false, 400.0, false), 30.0);
    assert_eq!(bullet_damage(&t, b, false, false, 2000.0, false), 30.0);
    assert_eq!(bullet_damage(&t, b, true, false, 1000.0, false), 90.0);
    assert_eq!(bullet_damage(&t, b, true, false, 100.0, false), 135.0);
    assert_eq!(bullet_damage(&t, b, false, false, 1000.0, true), 15.0);
}

#[test]
fn arrow_damage_regions_and_stealth() {
    // F4 (0x00C503D0): bolt kills on the head; x2 on head / torso of an unaware pawn; limbs
    // capped at 15; the dart and the flare have no stealth factor and do not kill on the head.
    let t = cfg().tuning;
    let bolt = &t.arrows[0];
    assert_eq!(arrow_damage(&t, bolt, 20.0, HitRegion::Head, false), None);
    assert_eq!(arrow_damage(&t, bolt, 20.0, HitRegion::Torso, true), Some(40.0));
    assert_eq!(arrow_damage(&t, bolt, 20.0, HitRegion::Torso, false), Some(20.0));
    assert_eq!(arrow_damage(&t, bolt, 20.0, HitRegion::LeftLeg, true), Some(15.0));
    assert_eq!(arrow_damage(&t, bolt, 20.0, HitRegion::None, true), Some(15.0));
    let flare = &t.arrows[2];
    assert_eq!(arrow_damage(&t, flare, 50.0, HitRegion::Head, true), Some(50.0));
    assert_eq!(arrow_damage(&t, flare, 50.0, HitRegion::RightArm, true), Some(50.0));
}

#[test]
fn grenade_bounce_blends_floor_and_wall() {
    // F6 (0x00C5D940): the floor example of ranged.md: v = (2266, 0, -1302) -> (906, 0, 338).
    let g = GrenadeConfig::default();
    let v = world_bounce(Vec3::new(2266.0, 0.0, -1302.0), Vec3::Z, &g).unwrap();
    assert!((v - Vec3::new(906.4, 0.0, 338.52)).length() < 0.1, "{v}");
    // At rest when the fall is slower than m_fRigidBodyZMagThreshold x n.z.
    assert!(world_bounce(Vec3::new(500.0, 0.0, -149.0), Vec3::Z, &g).is_none());
    // A wall: m_fBounciness 0.3 / m_fBounceDamping 0.1.
    let w = world_bounce(Vec3::new(1000.0, 0.0, 0.0), -Vec3::X, &g).unwrap();
    // (1000 - 1.3 x 1000) x 0.1.
    assert!((w - Vec3::new(-30.0, 0.0, 0.0)).length() < 1e-3, "{w}");
    // A pawn: m_fBodyBounciness 0.1 / m_fBodyBounceDamping 0.05: (1000 - 1.1 x 1000) x 0.05.
    let p = bounce(Vec3::new(1000.0, 0.0, 0.0), -Vec3::X, g.body_bounciness, g.body_bounce_damping);
    assert!((p - Vec3::new(-5.0, 0.0, 0.0)).length() < 1e-3, "{p}");
}

#[test]
fn hit_region_follows_the_rig_bodies() {
    // A two-body rig: a head capsule on top, a leg capsule below, the origin 100 above the feet.
    let cap = |bone: &str, region, z: f32| HitBody { bone: bone.into(), region, center: [0.0, 0.0, z], axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], half: [10.0, 10.0, 10.0], capsule: true };
    let hb = HitBodyConfig { source: "test".into(), half_height: 100.0, bodies: vec![cap("neck_jnt", HitRegion::Head, 80.0), cap("upper_leg_L_jnt", HitRegion::LeftLeg, -50.0)] };
    let c = cfg();
    let mv = &c.gameplay.movement;
    let v = PlayerState::new(1, Vec3::new(0.0, 0.0, 100.0), 0.0);
    let hh = mv.collision_half_height;
    let s = hh / 100.0;
    // Shots along -x at the height of each body (scaled to the pawn's height).
    let at = |z: f32| hit_region(&v, mv, &hb, Vec3::new(mv.collision_radius, 0.0, 100.0 + z * s), -Vec3::X);
    assert_eq!(at(80.0), HitRegion::Head);
    assert_eq!(at(-50.0), HitRegion::LeftLeg);
    // A line between the bodies takes the nearest one.
    assert_eq!(at(-25.0), HitRegion::LeftLeg);
    // Without bodies: the capsule-height rule.
    let none = HitBodyConfig::default();
    assert_eq!(hit_region(&v, mv, &none, Vec3::new(0.0, 0.0, 100.0 + hh * 0.7), -Vec3::X), HitRegion::Head);
    assert_eq!(hit_region(&v, mv, &none, Vec3::new(0.0, 0.0, 100.0), -Vec3::X), HitRegion::Torso);
}

#[test]
fn bone_regions() {
    assert_eq!(HitRegion::of_bone("neck_jnt"), HitRegion::Head);
    assert_eq!(HitRegion::of_bone("spine_3_jnt"), HitRegion::Torso);
    assert_eq!(HitRegion::of_bone("Root_jnt"), HitRegion::Torso);
    assert_eq!(HitRegion::of_bone("upper_arm_L_jnt"), HitRegion::LeftArm);
    assert_eq!(HitRegion::of_bone("lower_arm_R_jnt"), HitRegion::RightArm);
    assert_eq!(HitRegion::of_bone("foot_R_jnt"), HitRegion::RightLeg);
    assert_eq!(HitRegion::of_bone("Back_pan_L_02_jnt"), HitRegion::None);
}

#[test]
fn spawn_distances_and_noise_from_the_tweaks() {
    let t = cfg().tuning;
    assert_eq!(t.bullet_spawn_distance, 150.0);
    assert_eq!(t.bolt_spawn_distance, 150.0);
    assert_eq!(t.bullet_pierce_chance, 1.0);
    assert_eq!(t.bullet_post_pierce_ratio, 0.5);
    // EAINoiseLoudness4 (m_LoudnessDistanceArray[15]).
    assert_eq!(t.pistol_noise_radius, 6000.0);
}
