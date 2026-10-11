//! UE3 Cascade particle emitter simulation over the cooked `ParticleSystem` data in
//! [`dishonored_ir::particles`]. Bevy-free, deterministic given the caller's random source and
//! step. Research/dishonored/EFFECTS.md documents the module semantics.

use dishonored_ir::particles::ParticleEmitterAsset;
use glam::{Mat4, Vec3};

pub const MAX_PARTICLES: usize = 400;
/// Longest life of a gameplay one-shot instance (s): looping emitters spawned as one-shots
/// are cut here.
pub const ONE_SHOT_LIMIT: f32 = 12.0;

#[derive(Clone, Copy, Default)]
pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    pub age: f32,
    pub life: f32,
    pub size: Vec3,
    pub rot: f32,
    pub rot_rate: f32,
    pub color: Vec3,
    pub alpha: f32,
    pub r: [f32; 4],
    /// Mesh particles: rotation (radians about X, Y, Z) and its rate.
    pub mrot: Vec3,
    pub mrot_rate: Vec3,
    /// UE3 `BaseVelocity`: accelerations integrate into it; per-frame modules (velocity over
    /// life, attractors) derive `vel` from it.
    pub base_vel: Vec3,
    /// `ParticleModuleAcceleration`'s value, drawn at spawn at the emitter time (0x008B5A20
    /// keeps it in the particle's payload).
    pub accel: Vec3,
    /// Orbit (summed over the orbit modules): offset, rotation and rotation rate (radians).
    pub orbit_off: Vec3,
    pub orbit_rot: Vec3,
    pub orbit_rate: Vec3,
    /// Collisions so far, the maximum, and frozen / no-longer-colliding flags.
    pub collisions: u32,
    pub max_collisions: u32,
    pub frozen: bool,
    pub halted: bool,
    /// A decal particle (`ParticleModuleTypeDataDecal`): the normal (Dishonored world) of the
    /// surface it lies on.
    pub normal: Option<Vec3>,
    /// An AnimTrail knot's second edge (`pos` is the first).
    pub edge2: Vec3,
}

/// AnimTrail source data (`ParticleModuleTypeDataAnimTrail`): the two edges the game sets for an
/// update and their tangents (the motion over the last record, 0x00C02090 -> 0x008AB910),
/// Dishonored world.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrailSource {
    pub edges: [Vec3; 2],
    pub tangents: [Vec3; 2],
}

/// The cubic Hermite point between `p0` (tangent `m0`) and `p1` (tangent `m1`) at `f`.
pub fn hermite(p0: Vec3, m0: Vec3, p1: Vec3, m1: Vec3, f: f32) -> Vec3 {
    let (f2, f3) = (f * f, f * f * f);
    p0 * (2.0 * f3 - 3.0 * f2 + 1.0) + m0 * (f3 - 2.0 * f2 + f) + p1 * (3.0 * f2 - 2.0 * f3) + m1 * (f3 - f2)
}

/// An event an emitter reports (`ParticleModuleEventGenerator`): its kind
/// (`dishonored_ir::particles::EVENT_*`), `CustomName`, the place (Dishonored world), the
/// surface normal of a collision and the velocity (reflected, for a collision).
#[derive(Clone, Debug)]
pub struct PfxEvent {
    pub kind: u8,
    pub name: String,
    pub pos: Vec3,
    pub vel: Vec3,
    pub normal: Option<Vec3>,
}

/// World ray query for particle collision: (from, to) -> (point, normal).
pub type RayFn<'a> = &'a dyn Fn(Vec3, Vec3) -> Option<(Vec3, Vec3)>;

pub struct EmitterSim {
    pub time: f32,
    pub spawn_acc: f32,
    pub loops: i32,
    pub done: bool,
    pub particles: Vec<Particle>,
    /// Emitter-time fraction of the last burst check; -1 before the first, so bursts at time
    /// 0 fire on the first cycle too (they only fired from the second loop on, so one-shot
    /// effects lost every time-0 burst, e.g. the muzzle shockwave).
    pub last_burst_t: f32,
    /// The emitter's sprite mesh holds no particles (the placeholder triangle): it is not
    /// rebuilt (and re-uploaded) again until particles exist.
    pub mesh_empty: bool,
    /// Events the emitter generated since the instance last dispatched them
    /// ([`dispatch_events`]).
    pub events: Vec<PfxEvent>,
    /// `SetupEmitterDuration` (0x00882DF0) ran: the loop's delay (`CurrentDelay`) and length
    /// (`EmitterDuration` plus the delay), and the time within the loop (`EmitterTime`).
    pub setup: bool,
    pub cur_delay: f32,
    pub cur_dur: f32,
    pub loop_time: f32,
    /// The instance's velocity (`PartSysVelocity`, from its origin's motion) for
    /// `ParticleModuleVelocityInheritParent`.
    pub parent_vel: Vec3,
    pub last_origin: Option<Vec3>,
    /// `ParticleModuleLocationEmitter`: the source emitter's live particles (world position,
    /// velocity), fed by [`feed_location_sources`] before the step, and the sequential pick.
    pub loc_src: Vec<(Vec3, Vec3)>,
    pub loc_seq: usize,
    /// The instance was deactivated (`bKillOnDeactivate` emitters lose their particles).
    pub deactivated: bool,
    /// AnimTrail: the source data set for this update, and the last update's (the knots of this
    /// update lie on the spline between them; a frame without data breaks the trail).
    pub trail_src: Option<TrailSource>,
    pub trail_last: Option<TrailSource>,
}

impl Default for EmitterSim {
    fn default() -> Self {
        Self {
            time: 0.0,
            spawn_acc: 0.0,
            loops: 0,
            done: false,
            particles: Vec::new(),
            last_burst_t: -1.0,
            mesh_empty: true,
            events: Vec::new(),
            setup: false,
            cur_delay: 0.0,
            cur_dur: 0.0,
            loop_time: 0.0,
            parent_vel: Vec3::ZERO,
            last_origin: None,
            loc_src: Vec::new(),
            loc_seq: 0,
            deactivated: false,
            trail_src: None,
            trail_last: None,
        }
    }
}

/// `DetermineUnitDirection` (0x0089D900): one draw per axis, in [-1, 1] when both signs are
/// allowed, [0, 1] or [-1, 0] for one sign, 0 for none.
pub fn unit_direction(positive: [bool; 3], negative: [bool; 3], rnd: &mut dyn FnMut() -> f32) -> Vec3 {
    let r = [rnd(), rnd(), rnd()];
    let mut v = Vec3::ZERO;
    for k in 0..3 {
        v[k] = match (positive[k], negative[k]) {
            (true, true) => r[k] * 2.0 - 1.0,
            (true, false) => r[k],
            (false, true) => -r[k],
            _ => 0.0,
        };
    }
    v
}

/// A location primitive's offset (emitter space, `StartLocation` included) and the velocity it
/// adds. Sphere (0x008B42A0): the unit sample u x radius, each enabled axis clamped to
/// |normalize(u)| x radius (u itself normalized with `SurfaceOnly`), disabled axes 0. Cylinder
/// (0x008B3A40): the same on the two radial axes against the normalized radial direction, the
/// height axis u x height / 2. `Velocity`: the offset x `VelocityScale`, without its height part
/// for a cylinder with `RadialVelocity`.
pub fn primitive_offset(prim: &dishonored_ir::particles::LocationPrimitive, et: f32, rnd: &mut dyn FnMut() -> f32) -> (Vec3, Vec3) {
    let start = prim.start_location.as_ref().map(|d| Vec3::from(d.eval_rand(et, 0, rnd))).unwrap_or(Vec3::ZERO);
    let mut u = unit_direction(prim.positive, prim.negative, rnd);
    let radius = prim.radius.eval1(et, rnd());
    let enabled = |k: usize| prim.positive[k] || prim.negative[k];
    let clamp = |v: f32, m: f32| v.clamp(-m.abs(), m.abs());
    let mut off = Vec3::ZERO;
    let ha = prim.height_axis.min(2) as usize;
    match &prim.height {
        None => {
            let n = u.normalize_or_zero();
            if prim.surface_only {
                u = n;
            }
            for k in 0..3 {
                if enabled(k) {
                    off[k] = clamp(u[k] * radius, n[k] * radius);
                }
            }
        }
        Some(h) => {
            let (a0, a1) = match ha {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            let half = h.eval1(et, rnd()) * 0.5;
            let n2 = glam::Vec2::new(u[a0], u[a1]).normalize_or_zero();
            let (r0, r1) = if prim.surface_only { (n2.x, n2.y) } else { (u[a0], u[a1]) };
            for (k, rv, nv) in [(a0, r0, n2.x), (a1, r1, n2.y)] {
                if enabled(k) {
                    off[k] = clamp(rv * radius, nv * radius);
                }
            }
            if enabled(ha) {
                off[ha] = clamp(u[ha] * half, half);
            }
        }
    }
    let vel = match &prim.velocity {
        Some(s) => {
            let mut v = off;
            if prim.height.is_some() && prim.radial_velocity {
                v[ha] = 0.0;
            }
            v * s.eval1(et, rnd())
        }
        None => Vec3::ZERO,
    };
    (start + off, vel)
}

/// `ParticleModuleLocationEmitter` (0x008ABCE0): the source particle, picked at random among the
/// source emitter's live ones or the next in turn (wrapping to the first).
pub fn pick_source(le: &dishonored_ir::particles::LocationEmitter, sim: &mut EmitterSim, rnd: &mut dyn FnMut() -> f32) -> Option<(Vec3, Vec3)> {
    let n = sim.loc_src.len();
    if n == 0 {
        return None;
    }
    let i = if le.sequential {
        sim.loc_seq += 1;
        if sim.loc_seq >= n {
            sim.loc_seq = 0;
        }
        sim.loc_seq
    } else {
        ((rnd() * n as f32) as usize).min(n - 1)
    };
    sim.loc_src.get(i).copied()
}

/// Give each emitter with a `ParticleModuleLocationEmitter` the live particles (world position,
/// velocity) of the instance's emitter it names.
pub fn feed_location_sources(ems: &[ParticleEmitterAsset], sims: &mut [EmitterSim], xf: &Mat4) {
    for i in 0..ems.len().min(sims.len()) {
        let Some(le) = ems[i].location_emitter() else { continue };
        let src: Vec<(Vec3, Vec3)> = match ems.iter().position(|e| e.name == le.emitter) {
            Some(j) if j < sims.len() => {
                let w = |p: &Particle| if ems[j].local_space { (xf.transform_point3(p.pos), xf.transform_vector3(p.vel)) } else { (p.pos, p.vel) };
                sims[j].particles.iter().map(w).collect()
            }
            _ => Vec::new(),
        };
        sims[i].loc_src = src;
    }
}

/// `SetupEmitterDuration` (0x00882DF0): the loop's delay (`EmitterDelay`, or a draw in
/// [`EmitterDelayLow`, `EmitterDelay`]) and its length (`EmitterDuration` or a draw in
/// [`EmitterDurationLow`, `EmitterDuration`], plus the delay; the delay leaves it after the first
/// loop with `bDelayFirstLoopOnly` unless the emitter plays a single loop).
pub fn setup_duration(em: &ParticleEmitterAsset, sim: &mut EmitterSim, rnd: &mut dyn FnMut() -> f32) {
    sim.setup = true;
    sim.cur_delay = match em.delay_low {
        Some(lo) => (em.delay - lo) * rnd() + lo,
        None => em.delay,
    };
    let d = match em.duration_low {
        Some(lo) => (em.duration - lo) * rnd() + lo,
        None => em.duration,
    };
    sim.cur_dur = d + sim.cur_delay;
    if sim.loops == 1 && em.delay_first_loop_only && (em.loops == 0 || em.loops > 1) {
        sim.cur_dur -= sim.cur_delay;
    }
}

pub fn d3(d: Option<&dishonored_ir::particles::Dist>, t: f32, r: f32, def: [f32; 3]) -> Vec3 {
    Vec3::from(d.map(|d| d.eval(t, r)).unwrap_or(def))
}

pub fn spawn_particle(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, rnd: &mut dyn FnMut() -> f32) {
    if sim.particles.len() >= MAX_PARTICLES {
        return;
    }
    let et = sim.time;
    let r = [rnd(), rnd(), rnd(), rnd()];
    // No lifetime module (or 0): the particle lives until the emitter is destroyed (UE3).
    let life = em.lifetime().map(|d| d.eval1(et, r[0])).filter(|l| *l > 0.0).unwrap_or(f32::INFINITY);
    let mut local = d3(em.location(), et, r[1], [0.0; 3]);
    let velocity = em.velocity();
    let start_v = d3(velocity.map(|v| v.0), et, r[2], [0.0; 3]);
    let velocity_world = velocity.is_some_and(|v| v.1);
    let mut prim_vel = Vec3::ZERO;
    for prim in [em.sphere(), em.cylinder()].into_iter().flatten() {
        let (off, v) = primitive_offset(prim, et, rnd);
        local += off;
        prim_vel += v;
    }
    let radial = em.velocity_radial().map(|vr| local.normalize_or_zero() * vr.eval1(et, r[2])).unwrap_or(Vec3::ZERO);
    // `ParticleModuleLocationEmitter`: at a particle of the source emitter (the other location
    // modules' offsets on top).
    let src = em.location_emitter().and_then(|le| pick_source(le, sim, rnd));
    let inv = xf.inverse();
    // The start velocity turns with the component unless `bInWorldSpace` (a local emitter turns
    // a world one into its space); radial and primitive velocities follow the location (0x008B8C20).
    let (pos, mut vel) = if em.local_space {
        let base = src.map(|s| inv.transform_point3(s.0)).unwrap_or(Vec3::ZERO);
        let sv = if velocity_world { inv.transform_vector3(start_v) } else { start_v };
        (base + local, sv + radial + prim_vel)
    } else {
        let pos = match src {
            Some(s) => s.0 + xf.transform_vector3(local),
            None => xf.transform_point3(local),
        };
        let sv = if velocity_world { start_v } else { xf.transform_vector3(start_v) };
        (pos, sv + xf.transform_vector3(radial + prim_vel))
    };
    if let (Some(k), Some(s)) = (em.location_emitter().and_then(|le| le.inherit_velocity), src) {
        vel += if em.local_space { inv.transform_vector3(s.1) } else { s.1 } * k;
    }
    // `ParticleModuleVelocityInheritParent` (0x008B92A0): Scale x the instance's velocity.
    if let Some(d) = em.velocity_inherit_parent() {
        let pv = if em.local_space { inv.transform_vector3(sim.parent_vel) } else { sim.parent_vel };
        vel += Vec3::from(d.eval(et, r[2])) * pv;
    }
    let accel = d3(em.acceleration().map(|a| a.0), et, r[3], [0.0; 3]);
    // A scaled instance transform scales the particles too (the first-person hand effects ride
    // the rig drawn at the viewmodel scale); gameplay transforms are unscaled.
    let size = d3(em.size(), et, r[0], [1.0; 3]) * xf.x_axis.truncate().length();
    let color = d3(em.color(), et, r[1], [1.0; 3]);
    let alpha = em.alpha().map(|d| d.eval1(et, r[1])).unwrap_or(1.0);
    let (mut orbit_off, mut orbit_rot, mut orbit_rate) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
    for o in em.orbits() {
        orbit_off += d3(o.offset.as_ref(), et, r[1], [0.0; 3]);
        orbit_rot += d3(o.rotation.as_ref(), et, r[2], [0.0; 3]) * std::f32::consts::TAU;
        orbit_rate += d3(o.rotation_rate.as_ref(), et, r[3], [0.0; 3]) * std::f32::consts::TAU;
    }
    let max_collisions = em.collision().map(|c| c.1.eval1(et, r[0]).max(0.0) as u32).unwrap_or(0);
    sim.particles.push(Particle {
        pos,
        vel,
        base_vel: vel,
        accel,
        orbit_off,
        orbit_rot,
        orbit_rate,
        collisions: 0,
        max_collisions,
        frozen: false,
        halted: false,
        age: 0.0,
        life,
        size,
        rot: em.rotation().map(|d| d.eval1(et, r[3])).unwrap_or(0.0) * std::f32::consts::TAU,
        rot_rate: em.rotation_rate().map(|d| d.eval1(et, r[2])).unwrap_or(0.0) * std::f32::consts::TAU,
        color,
        alpha,
        r,
        mrot: d3(em.mesh_rotation(), et, r[2], [0.0; 3]) * std::f32::consts::TAU,
        mrot_rate: d3(em.mesh_rotation_rate(), et, r[3], [0.0; 3]) * std::f32::consts::TAU,
        normal: None,
        edge2: pos,
    });
}

/// What follows a particle the emitter itself spawned: its `EPET_Spawn` events, and, for a decal
/// emitter that no event feeds, the surface it lies on (projected along its velocity, or down,
/// onto the world; no surface within reach, no decal. LIKELY: the engine projects the decal
/// along the particle's motion).
pub fn after_spawn(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, ray: Option<RayFn>) {
    use dishonored_ir::particles::EVENT_SPAWN;
    let Some(p) = sim.particles.last_mut() else { return };
    let world = if em.local_space { xf.transform_point3(p.pos) } else { p.pos };
    if em.decal.is_some() && em.events_in.is_empty() && p.normal.is_none() {
        let dir = if p.vel.length_squared() > 1.0 { p.vel.normalize() } else { -Vec3::Z };
        match ray.and_then(|ray| ray(world - dir * 8.0, world + dir * DECAL_REACH)) {
            Some((pt, n)) => {
                p.pos = if em.local_space { xf.inverse().transform_point3(pt) } else { pt };
                p.normal = Some(n);
            }
            None => p.age = p.life,
        }
    }
    let vel = p.vel;
    for e in em.events_out.iter().filter(|e| e.kind == EVENT_SPAWN) {
        sim.events.push(PfxEvent { kind: e.kind, name: e.name.clone(), pos: world, vel, normal: None });
    }
}

/// How far (uu) a decal particle without an event looks for its surface.
pub const DECAL_REACH: f32 = 120.0;

/// Spawn one particle of `em` where an event happened: the emitter's modules place it relative
/// to the event (the event point replaces the emitter's origin), `normal` is the surface it lies
/// on and `inherit` the velocity it takes (`InheritVelocityScale` x the event's velocity).
pub fn spawn_at(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, rnd: &mut dyn FnMut() -> f32, at: Vec3, normal: Option<Vec3>, inherit: Option<Vec3>) {
    let before = sim.particles.len();
    spawn_particle(em, sim, xf, rnd);
    if sim.particles.len() == before {
        return;
    }
    let shift = at - xf.w_axis.truncate();
    let shift = if em.local_space { xf.inverse().transform_vector3(shift) } else { shift };
    if let Some(p) = sim.particles.last_mut() {
        p.pos += shift;
        p.normal = normal;
        if let Some(v) = inherit {
            p.vel += v;
            p.base_vel += v;
        }
    }
}

/// Deliver the events the emitters of an instance generated this frame to the emitters that
/// receive them (`ParticleModuleEventReceiverSpawn`: `SpawnCount` particles at the event, taking
/// `InheritVelocityScale` of its velocity). Always drains the events.
///
/// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with
/// permission: the generator / receiver flow of its particles.rs `step` (collision events with the
/// surface normal, receivers spawning at the event place, decal particles taking the normal).
pub fn dispatch_events(ems: &[ParticleEmitterAsset], sims: &mut [EmitterSim], xf: &Mat4, rnd: &mut dyn FnMut() -> f32) {
    let events: Vec<PfxEvent> = sims.iter_mut().flat_map(|s| std::mem::take(&mut s.events)).collect();
    if events.is_empty() {
        return;
    }
    for ev in &events {
        for (em, sim) in ems.iter().zip(sims.iter_mut()) {
            for rx in em.events_in.iter().filter(|r| r.name == ev.name && (r.kind == dishonored_ir::particles::EVENT_ANY || r.kind == ev.kind)) {
                let n = rx.count.as_ref().map(|d| d.eval1(sim.time, 0.5).round().max(0.0) as usize).unwrap_or(1);
                let inherit = rx.inherit_velocity.as_ref().map(|d| Vec3::from(d.eval(sim.time, 0.5)) * ev.vel);
                for _ in 0..n {
                    spawn_at(em, sim, xf, rnd, ev.pos, ev.normal, inherit);
                }
            }
        }
    }
}

/// Advance one emitter by `dt`.
pub fn step(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, dt: f32, rnd: &mut dyn FnMut() -> f32, ray: Option<RayFn>) {
    // The instance's velocity (`PartSysVelocity`: its origin's motion over the frame).
    let origin = xf.w_axis.truncate();
    sim.parent_vel = match sim.last_origin {
        Some(o) if dt > 0.0 => (origin - o) / dt,
        _ => Vec3::ZERO,
    };
    sim.last_origin = Some(origin);
    if sim.deactivated && em.kill_on_deactivate {
        sim.particles.clear();
    }
    // Accelerations are in world axes, or in the emitter's for a local emitter; a local emitter
    // turns a `bAlwaysInWorldSpace` one into its space (0x008B5A20 / 0x008B5D50, 0x008B67A0).
    let inv = xf.inverse();
    let space = |a: Vec3, world: bool| if world && em.local_space { inv.transform_vector3(a) } else { a };
    // Emitter-space points to the particle space (world unless local space).
    let to_space = |v: Vec3| if em.local_space { v } else { xf.transform_point3(v) };
    let et = sim.time;
    // The per-frame modules, looked up once for the particles.
    let accel_world = em.acceleration().is_some_and(|a| a.1);
    let (accel_over_life, drag, attractor_point, attractor_line) = (em.accel_over_life(), em.drag(), em.attractor_point(), em.attractor_line());
    let (vel_over_life, collision, rotation_rate_life, mesh_rotation_rate_life) = (em.vel_over_life(), em.collision(), em.rotation_rate_life(), em.mesh_rotation_rate_life());
    for p in sim.particles.iter_mut() {
        p.age += dt;
        let rel = (p.age / p.life).clamp(0.0, 1.0);
        p.orbit_rot += p.orbit_rate * dt;
        if p.frozen {
            continue;
        }
        let mut a = space(p.accel, accel_world);
        if let Some((d, world)) = accel_over_life {
            a += space(Vec3::from(d.eval(rel, p.r[3])), world);
        }
        p.base_vel += a * dt;
        if let Some(d) = drag {
            p.base_vel *= (1.0 - d.eval1(rel, p.r[2]) * dt).max(0.0);
        }
        let mut v = p.base_vel;
        // Attractors pull toward a point / the nearest point of a line within their range
        // (UE3 `ParticleModuleAttractorPoint` / `Line`), strength fading with distance.
        let pull = |target: Vec3, range: f32, strength: f32, by_distance: bool, base: bool, p: &mut Particle, v: &mut Vec3| {
            let d = target - p.pos;
            let dist = d.length();
            if dist < 1e-3 || (range > 0.0 && dist > range) {
                return;
            }
            let s = if by_distance && range > 0.0 { strength * (1.0 - dist / range) } else { strength };
            let dv = d / dist * s * dt;
            if base {
                p.base_vel += dv;
            }
            *v += dv;
        };
        if let Some(ap) = attractor_point {
            let pos = Vec3::from(ap.position.eval(et, p.r[0]));
            let target = if ap.world_space && !em.local_space { pos } else { to_space(pos) };
            pull(target, ap.range.eval1(et, p.r[1]), ap.strength.eval1(et, p.r[2]), ap.strength_by_distance, ap.affect_base_velocity, p, &mut v);
        }
        if let Some((a0, a1, range, strength)) = attractor_line {
            let (a0, a1) = (to_space(Vec3::from(a0)), to_space(Vec3::from(a1)));
            let seg = a1 - a0;
            let t = ((p.pos - a0).dot(seg) / seg.length_squared().max(1e-6)).clamp(0.0, 1.0);
            pull(a0 + seg * t, range.eval1(t, p.r[1]), strength.eval1(t, p.r[2]), true, true, p, &mut v);
        }
        if let Some((d, absolute)) = vel_over_life {
            let s = Vec3::from(d.eval(rel, p.r[1]));
            v = if absolute { if em.local_space { s } else { xf.transform_vector3(s) } } else { v * s };
        }
        p.vel = v;
        let next = p.pos + v * dt;
        // Collision against the level geometry: reflect with the damping factor; past the
        // maximum count, the completion option (kill, freeze, stop colliding...).
        let mut moved = false;
        if let (Some((damp, _, opt)), Some(ray)) = (collision, ray) {
            if !p.halted {
                let (wf, wt) = if em.local_space { (xf.transform_point3(p.pos), xf.transform_point3(next)) } else { (p.pos, next) };
                if let Some((pt, n)) = ray(wf, wt) {
                    let n_s = if em.local_space { xf.inverse().transform_vector3(n).normalize_or_zero() } else { n };
                    let refl = v - 2.0 * v.dot(n_s) * n_s;
                    p.base_vel = refl * Vec3::from(damp.eval(rel, p.r[3]));
                    let pt_s = if em.local_space { xf.inverse().transform_point3(pt) } else { pt };
                    p.pos = pt_s + n_s * 0.5;
                    moved = true;
                    // `EPET_Collision` events (`UseReflectedImpactVector`: the velocity is the
                    // reflected one), in world space.
                    for e in em.events_out.iter().filter(|e| e.kind == dishonored_ir::particles::EVENT_COLLISION && (!e.first_only || p.collisions == 0)) {
                        let w = |v: Vec3| if em.local_space { xf.transform_vector3(v) } else { v };
                        sim.events.push(PfxEvent { kind: e.kind, name: e.name.clone(), pos: pt, vel: w(refl), normal: Some(n) });
                    }
                    p.collisions += 1;
                    if p.collisions >= p.max_collisions {
                        match opt {
                            0 => p.age = p.life,
                            1 | 5 => p.frozen = true,
                            2 => p.halted = true,
                            3 => {
                                p.base_vel = Vec3::ZERO;
                                p.halted = true;
                            }
                            _ => p.rot_rate = 0.0,
                        }
                    }
                }
            }
        }
        if !moved {
            p.pos = next;
        }
        // `ParticleModuleRotationRateMultiplyLife` scales the (reset) rate every update
        // (0x0089E270); `MeshRotationRateOverLife` adds to (or scales) the mesh rate every update
        // (0x008A5000).
        let rr = rotation_rate_life.map_or(1.0, |d| d.eval1(rel, p.r[2]));
        p.rot += p.rot_rate * rr * dt;
        if let Some((d, scale)) = mesh_rotation_rate_life {
            let v = Vec3::from(d.eval(rel, p.r[3])) * std::f32::consts::TAU;
            if scale {
                p.mrot_rate *= v;
            } else {
                p.mrot_rate += v;
            }
        }
        p.mrot += p.mrot_rate * dt;
    }
    // `EPET_Death` events of the particles that end now.
    if em.events_out.iter().any(|e| e.kind == dishonored_ir::particles::EVENT_DEATH) {
        let dead: Vec<(Vec3, Vec3)> = sim.particles.iter().filter(|p| p.age >= p.life).map(|p| (if em.local_space { xf.transform_point3(p.pos) } else { p.pos }, p.vel)).collect();
        for (pos, vel) in dead {
            for e in em.events_out.iter().filter(|e| e.kind == dishonored_ir::particles::EVENT_DEATH) {
                sim.events.push(PfxEvent { kind: e.kind, name: e.name.clone(), pos, vel, normal: None });
            }
        }
    }
    // `ParticleModuleKillHeight` (0x008BA960): particles above the height (below it with
    // `bFloor`) are killed; the height is relative to the component unless `bAbsolute`.
    if let Some((h, absolute, floor)) = em.kill_height() {
        let height = h.eval1(et, 0.5) + if absolute { 0.0 } else { origin.z };
        for p in sim.particles.iter_mut() {
            let z = if em.local_space { xf.transform_point3(p.pos).z } else { p.pos.z };
            if (floor && z < height) || (!floor && z > height) {
                p.age = p.life;
            }
        }
    }
    sim.particles.retain(|p| p.age < p.life);
    if sim.done {
        // A completed emitter (all `EmitterLoops` played) is destroyed with its particles that
        // have no Lifetime module (they live until the emitter goes, e.g. the five sprite
        // emitters of the pistol muzzle `Ps_PlayerPistol_StdMuzzle_01`).
        sim.particles.retain(|p| p.life.is_finite());
        return;
    }
    // Emitter time, loops, spawn rate and bursts (burst times are fractions of the duration).
    // `Tick_EmitterTimeSetup` (0x00887270): the loop runs its delay then its duration; a loop
    // ends at the duration plus the delay (`SetupEmitterDuration` 0x00882DF0); the delay holds
    // every loop, or the first only with `bDelayFirstLoopOnly`. Loops only advance for an emitter
    // with a length (UE3 tests `EmitterDuration > 0`): a zero-duration emitter bursts once,
    // instead of re-firing its time-0 burst every frame (particles without a Lifetime module would
    // pile up forever).
    if !sim.setup {
        setup_duration(em, sim, rnd);
    }
    let before = sim.time;
    sim.time += dt;
    sim.loop_time += dt;
    if sim.cur_dur > 1e-3 && sim.loop_time >= sim.cur_dur {
        sim.loop_time -= sim.cur_dur;
        sim.loops += 1;
        sim.last_burst_t = -1.0;
        if em.loops > 0 && sim.loops >= em.loops {
            sim.done = true;
            return;
        }
        if em.duration_recalc || (em.delay_first_loop_only && sim.loops == 1) {
            setup_duration(em, sim, rnd);
        }
    }
    let delay = if em.delay_first_loop_only && sim.loops > 0 { 0.0 } else { sim.cur_delay };
    let t = sim.loop_time - delay;
    if t < 0.0 {
        return;
    }
    let dur = (sim.cur_dur - delay).max(1e-3);
    let rate = em.rate.as_ref().map(|d| d.eval1(t, 0.5)).unwrap_or(0.0) * em.rate_scale.as_ref().map(|d| d.eval1(t, 0.5)).unwrap_or(1.0);
    sim.spawn_acc += rate.max(0.0) * dt;
    let saved = sim.time;
    sim.time = t;
    if em.trail.as_ref().is_some_and(|t| t.anim) {
        // AnimTrail: the knots spawned this update (the spawn rate) lie on the spline from the last
        // source data to this update's, the last one on it; the first update spawns one there. No
        // data, no knots.
        let n = sim.spawn_acc.floor().max(0.0) as usize;
        sim.spawn_acc -= n as f32;
        match sim.trail_src.take() {
            Some(src) => {
                let last = sim.trail_last;
                let count = if last.is_some() { n } else { 1 };
                for k in 1..=count {
                    let f = k as f32 / count as f32;
                    let edge = |i: usize| match last {
                        Some(l) => hermite(l.edges[i], l.tangents[i], src.edges[i], src.tangents[i], f),
                        None => src.edges[i],
                    };
                    spawn_particle(em, sim, xf, rnd);
                    if let Some(p) = sim.particles.last_mut() {
                        p.pos = edge(0);
                        p.edge2 = edge(1);
                        p.vel = Vec3::ZERO;
                        p.base_vel = Vec3::ZERO;
                        p.accel = Vec3::ZERO;
                    }
                }
                sim.trail_last = Some(src);
            }
            None => sim.trail_last = None,
        }
        sim.time = saved;
        return;
    }
    while sim.spawn_acc >= 1.0 {
        sim.spawn_acc -= 1.0;
        spawn_particle(em, sim, xf, rnd);
        after_spawn(em, sim, xf, ray);
    }
    let frac = t / dur;
    for (count, low, bt) in &em.bursts {
        if *bt > sim.last_burst_t.max(-0.5) && *bt <= frac || (before == 0.0 && *bt <= 0.0 && sim.last_burst_t < 0.0) {
            let n = if *low >= 0 { *low + (rnd() * (*count - *low + 1) as f32) as i32 } else { *count };
            for _ in 0..n.max(0) {
                spawn_particle(em, sim, xf, rnd);
                after_spawn(em, sim, xf, ray);
            }
        }
    }
    sim.last_burst_t = frac;
    sim.time = saved;
}

/// A transform whose X axis points along `dir` (UE3 effects face +X), at `at`.
pub fn facing(at: Vec3, dir: Vec3) -> Mat4 {
    let x = if dir.length_squared() > 1e-6 { dir.normalize() } else { Vec3::X };
    let z = if x.z.abs() > 0.99 { Vec3::X } else { Vec3::Z };
    let y = z.cross(x).normalize();
    let z = x.cross(y);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), at.extend(1.0))
}

/// The effects' random stream (an LCG; particles' start values and lifetimes).
pub fn next_rand(state: &mut u64) -> f32 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*state >> 40) as f32) / (1u64 << 24) as f32
}

/// A one-shot instance is finished when every emitter is done (or past 10 s) and empty; an
/// emitter that loops forever (`EmitterLoops` 0, e.g. the Blink indicators) is deactivated after
/// [`ONE_SHOT_LIMIT`], its particles killed (`bKillOnDeactivate`), so no spawned effect outlives
/// its use.
pub fn one_shot_finished(sims: &[EmitterSim]) -> bool {
    sims.iter().all(|s| ((s.done || s.time > 10.0) && s.particles.is_empty()) || s.time > ONE_SHOT_LIMIT)
}

#[cfg(test)]
mod decal_tests {
    use super::*;
    use dishonored_ir::particles::{DecalEmitter, Dist, EVENT_COLLISION, EventIn};

    /// A generator's collision event reaches the receiver of its name only: `SpawnCount`
    /// particles at the event, each lying on the struck surface.
    #[test]
    fn a_collision_event_spawns_the_receivers_decals_on_the_surface() {
        let generator = ParticleEmitterAsset::default();
        let receiver = ParticleEmitterAsset {
            decal: Some(DecalEmitter::default()),
            events_in: vec![EventIn { kind: EVENT_COLLISION, name: "Blood_Decal".into(), count: Some(Dist::constant([2.0; 3], 1)), inherit_velocity: None }],
            ..Default::default()
        };
        let mut sims = vec![EmitterSim::default(), EmitterSim::default()];
        let at = Vec3::new(10.0, 20.0, 30.0);
        sims[0].events.push(PfxEvent { kind: EVENT_COLLISION, name: "Blood_Decal".into(), pos: at, vel: Vec3::ZERO, normal: Some(Vec3::Z) });
        sims[0].events.push(PfxEvent { kind: EVENT_COLLISION, name: "Other".into(), pos: Vec3::ZERO, vel: Vec3::ZERO, normal: Some(Vec3::X) });
        let mut r = || 0.5;
        dispatch_events(&[generator, receiver], &mut sims, &Mat4::IDENTITY, &mut r);
        assert!(sims[0].events.is_empty(), "the events are drained");
        assert_eq!(sims[1].particles.len(), 2);
        assert_eq!(sims[1].particles[0].pos, at);
        assert_eq!(sims[1].particles[0].normal, Some(Vec3::Z));
    }
}
