//! Tests of `dishonored_runtime::effects` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::effects::*;
use dishonored_ir::particles::ParticleEmitterAsset;

#[test]
fn burst_at_time_zero_fires_on_the_first_cycle() {
    // `Ps_PlayerPistol_StdMuzzle_01`'s shockwave: one burst of 1 at time 0, duration 0.5, one loop.
    let em = ParticleEmitterAsset { duration: 0.5, loops: 1, bursts: vec![(1, 1, 0.0)], ..Default::default() };
    let mut sim = EmitterSim::default();
    let mut r = || 0.5;
    step(&em, &mut sim, &Mat4::IDENTITY, 1.0 / 30.0, &mut r, None);
    assert_eq!(sim.particles.len(), 1);
    // Not again within the cycle.
    for _ in 0..10 {
        step(&em, &mut sim, &Mat4::IDENTITY, 1.0 / 30.0, &mut r, None);
    }
    assert_eq!(sim.particles.len(), 1);
}

#[test]
fn particles_without_lifetime_go_with_their_completed_emitter() {
    // The muzzle's sprites have no Lifetime module: they last until the emitter's single loop
    // (0.5 s) has played, then the emitter and its particles are gone.
    let em = ParticleEmitterAsset { duration: 0.5, loops: 1, bursts: vec![(1, 1, 0.0)], ..Default::default() };
    let mut sim = EmitterSim::default();
    let mut r = || 0.5;
    for _ in 0..12 {
        step(&em, &mut sim, &Mat4::IDENTITY, 1.0 / 30.0, &mut r, None);
    }
    assert_eq!(sim.particles.len(), 1);
    for _ in 0..12 {
        step(&em, &mut sim, &Mat4::IDENTITY, 1.0 / 30.0, &mut r, None);
    }
    assert!(sim.done);
    assert!(sim.particles.is_empty());
}

/// The depth-biased alpha scale of the particle bases' compiled passes (36033, 35519, ...):
/// 1 / ((1 - intensity) x 250), 1000 when the product is below 0.001; absent without the switch.
#[test]
fn soft_particle_scale_follows_the_compiled_passes() {
    let mut p = dishonored_ir::MaterialParams::default();
    assert_eq!(soft_scale(&p), 0.0);
    p.static_switches.push(("Db_Enable_Depth_Biased_Alpha".into(), true));
    // The base's intensity default 0.05: 1 / (0.95 x 250).
    assert!((soft_scale(&p) - 1.0 / 237.5).abs() < 1e-7);
    p.scalars.push(("Db_Depth_Biased_Alpha_Intensity".into(), 1.0));
    assert_eq!(soft_scale(&p), 1000.0);
}

#[test]
fn look_up_glow_colour_comes_from_its_lookup() {
    // `smoky_lookup` (diffuse_color2 (1, 0.838, 0.401), diffuse_color (0.406, 0.408, 0.368)) under
    // the pistol smoke's ColorOverLife: (1, 0, 1) early is diffuse_color2, the blue is not read,
    // and (0, 1, 1) late (drawn as cyan before the decode) adds nothing.
    let lerp = [Vec3::new(1.0, 0.838, 0.401), Vec3::new(0.406, 0.408, 0.368)];
    assert!(lookup_colour(Vec3::new(1.0, 0.0, 1.0), &lerp).abs_diff_eq(lerp[0], 1e-6));
    assert!(lookup_colour(Vec3::new(0.5, 1.0, 0.0), &lerp).abs_diff_eq(lerp[1] * 0.5, 1e-6));
    assert_eq!(lookup_colour(Vec3::new(0.0, 1.0, 1.0), &lerp), Vec3::ZERO);
}
