//! Melee weapon trails (`dishonored_assets::melee_trail`): while a `DisNotify_Trails` window of
//! the animation playing runs, each update feeds the item's trail component — an AnimTrail —
//! the blade extent's two corner sockets. The tangents follow 0x00C02090: from the last record to
//! this one, each edge's tangent is the edge span's perpendicular in the swing plane
//! (`span x (span x motion)`, normalized) times the distance the edge moved. The knots spawn on
//! the spline between records and die after the template's lifetime; a frame without a window
//! breaks the trail.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

/// Who owns a trail: (pawn, weapon index); the first-person sword uses [`FIRST_PERSON`].
pub type TrailKey = (u32, u32);

/// The first-person rig's sword's weapon index in [`TrailKey`].
pub const FIRST_PERSON: u32 = u32::MAX;

/// The instances' lifetime past their last feed (seconds; the knots live 0.15 s).
const IDLE_LIFE: f32 = 1.0;

#[derive(Resource, Default)]
pub struct WeaponTrails {
    /// This frame's source data: (owner, template, first edge, second edge), Dishonored world.
    pub sources: Vec<(TrailKey, String, Vec3, Vec3)>,
    live: HashMap<TrailKey, Live>,
}

struct Live {
    entity: Entity,
    template: String,
    last: Option<[Vec3; 2]>,
    idle: f32,
}

/// The tangents of an update from the last record (0x00C02090); none for the first.
pub fn tangents(edges: [Vec3; 2], last: Option<[Vec3; 2]>) -> [Vec3; 2] {
    let Some(l) = last else { return [Vec3::ZERO; 2] };
    let span = edges[1] - edges[0];
    let dir = span.cross(span.cross(l[0] - edges[0])).normalize_or_zero();
    [dir * (l[0] - edges[0]).length(), dir * (l[1] - edges[1]).length()]
}

/// Feed this frame's trail sources to their instances (spawned on the first feed), and remove
/// the instances unfed for [`IDLE_LIFE`].
pub fn drive(mut commands: Commands, mut trails: ResMut<WeaponTrails>, mut fx: ResMut<crate::effects::Effects>, mut meshes: ResMut<Assets<Mesh>>, time: Res<Time>) {
    let sources = std::mem::take(&mut trails.sources);
    let mut fed = HashSet::new();
    for (key, template, a, b) in sources {
        if !fed.insert(key) {
            continue;
        }
        if trails.live.get(&key).is_some_and(|l| !l.template.eq_ignore_ascii_case(&template)) {
            if let Some(l) = trails.live.remove(&key) {
                commands.entity(l.entity).despawn();
            }
        }
        if !trails.live.contains_key(&key) {
            let Some(entity) = crate::effects::spawn_system(&mut commands, &mut meshes, &mut fx, &template, Mat4::from_translation(a), false) else {
                warn!("weapon trail: {template} not decoded for this level");
                continue;
            };
            info!("weapon trail: {template} for {key:?}");
            trails.live.insert(key, Live { entity, template: template.clone(), last: None, idle: 0.0 });
        }
        let Some(live) = trails.live.get_mut(&key) else { continue };
        let edges = [a, b];
        crate::effects::feed_trail(&mut commands, live.entity, crate::effects::TrailSource { edges, tangents: tangents(edges, live.last) });
        live.last = Some(edges);
        live.idle = 0.0;
    }
    let dt = time.delta_secs();
    trails.live.retain(|key, l| {
        if fed.contains(key) {
            return true;
        }
        l.last = None;
        l.idle += dt;
        if l.idle > IDLE_LIFE {
            commands.entity(l.entity).despawn();
            return false;
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blade swung sideways: both tangents point along the swing, each as long as its edge
    /// moved (the tip, farther out, moves more).
    #[test]
    fn tangents_follow_the_swing() {
        let last = [Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, 50.0)];
        let edges = [Vec3::new(2.0, 0.0, 10.0), Vec3::new(10.0, 0.0, 50.0)];
        let t = tangents(edges, Some(last));
        assert!(t[0].normalize().dot(Vec3::X) > 0.95 && t[1].normalize().dot(Vec3::X) > 0.95, "{t:?}");
        assert!((t[0].length() - 2.0).abs() < 1e-3 && (t[1].length() - 10.0).abs() < 1e-3, "{t:?}");
        assert_eq!(tangents(edges, None), [Vec3::ZERO; 2]);
    }

    /// Fed updates lay knots at the spawn rate on the swept band between the edges; without data
    /// none spawn and the knots die after their lifetime.
    #[test]
    fn fed_updates_lay_knots_along_the_swing() {
        use crate::present_rules::{EmitterSim, TrailSource, next_rand, step};
        use dishonored_ir::particles::{Dist, ParticleEmitterAsset, TrailParams};
        let em = ParticleEmitterAsset {
            rate: Some(Dist::constant([100.0; 3], 1)),
            lifetime: Some(Dist::constant([0.15; 3], 1)),
            trail: Some(TrailParams { tiling: 1.0, ribbon: true, anim: true, tessellation_step: Some(50.0) }),
            ..Default::default()
        };
        let mut sim = EmitterSim::default();
        let mut seed = 1u64;
        let mut rnd = || next_rand(&mut seed);
        let mut last = None;
        for k in 0..6 {
            let x = k as f32 * 10.0;
            let edges = [Vec3::new(x, 0.0, 0.0), Vec3::new(x, 0.0, 40.0)];
            sim.trail_src = Some(TrailSource { edges, tangents: tangents(edges, last) });
            last = Some(edges);
            step(&em, &mut sim, &Mat4::IDENTITY, 1.0 / 60.0, &mut rnd, None);
        }
        // One knot on the first update, then 100 per second.
        assert!((8..=11).contains(&sim.particles.len()), "{} knots", sim.particles.len());
        for p in &sim.particles {
            assert!(p.pos.z.abs() < 1e-3 && (p.edge2.z - 40.0).abs() < 1e-3 && (-1e-3..=50.001).contains(&p.pos.x), "{} {}", p.pos, p.edge2);
        }
        for _ in 0..12 {
            step(&em, &mut sim, &Mat4::IDENTITY, 1.0 / 60.0, &mut rnd, None);
        }
        assert!(sim.particles.is_empty());
    }
}
