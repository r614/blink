//! Tests of `dishonored_runtime::effects` (integration tests, so they link the normal library build
//! instead of recompiling the whole crate in test mode).

use bevy::prelude::*;
use dishonored_runtime::effects::*;

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
