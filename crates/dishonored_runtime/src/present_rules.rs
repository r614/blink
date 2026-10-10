//! Presentation rules without Bevy: which effects, sounds and animation reactions the
//! simulation's events ask for, and the particle simulation of an effect instance. The runtime
//! (`effects`, `audio`, `characters`) applies them to its Bevy world; `tools/oracle` compiles
//! this same file (`#[path]`) to record them headless, so the oracle checks the game's own
//! decisions. Keep it free of `crate::` paths and of Bevy: only glam and the game crates.

use dishonored_audio::contact::ContactSystem;
use dishonored_audio::cues;
use dishonored_ir::particles::ParticleEmitterAsset;
use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::{PawnKind, PlayerId, PlayerState, ProjectileState};
use dishonored_world::{CollisionWorld, WorldQuery};
use glam::{Mat4, Vec3};
use std::collections::HashMap;

/// Muzzle flash of the player's first-person pistol (`Twk_Inv_PistolCorvo`'s
/// `DisTweaks_FirePistol.m_pMuzzleFlashFX`, at the pistol's `Spitfire` socket).
pub const PLAYER_MUZZLE: &str = "Vfx_Weapon.Effects.pistol.Ps_PlayerPistol_StdMuzzle_01";
/// Muzzle flash of a pistol seen in third person (the pawn-specific `DisTweaks_NPCFireGun`).
pub const PAWN_MUZZLE: &str = "Vfx_Weapon.Effects.Ps_PistolMuzzle";
/// Shadow Kill ash (`Twk_Pawn_DefaultNPC.m_ShadowKillFXs`): one system per listed bone,
/// `m_fShadowKillDelay` 0.1 s after the kill.
pub const SHADOW_KILL_PS: &str = "Vfx_GamePlay.ShadowKill.ps_SK_Leg_Arms";
pub const SHADOW_KILL_BONES: &[&str] = &[
    "upper_leg_L_jnt",
    "lower_leg_L_jnt",
    "upper_leg_R_jnt",
    "lower_leg_R_jnt",
    "upper_arm_L_jnt",
    "lower_arm_L_jnt",
    "upper_arm_R_jnt",
    "lower_arm_R_jnt",
    "spine_1_jnt",
    "head_jnt",
];
pub const SHADOW_KILL_DELAY: f32 = 0.1;

/// ParticleSystems the gameplay effects use (decoded with every level, from DishonoredGame):
/// the tweaks' effects and every contact-system particle (`particle_material::GAMEPLAY_PARTICLES`).
pub const GAMEPLAY_PARTICLES: &[&str] = dishonored_assets::particle_material::GAMEPLAY_PARTICLES;

pub const MAX_PARTICLES: usize = 400;
/// Longest life of a gameplay one-shot instance (s): looping emitters spawned as one-shots
/// are cut here.
pub const ONE_SHOT_LIMIT: f32 = 12.0;
/// Blink's targeting indicators (`Twk_Blink`: `m_pGroundPS` on a floor, `m_pLowFallPS` /
/// `m_pHighFallPS` in the air, `m_pMantlePS` on a ledge). Their emitters loop forever with
/// `bKillOnDeactivate`: the power keeps one at the target while aiming and kills it on release.
pub const BLINK_GROUND_PS: &str = "Vfx_GamePlay.Blink.Blink_Ground_01";
pub const BLINK_FALL_PS: &str = "Vfx_GamePlay.Blink.Blink_Fall_01";
pub const BLINK_MANTLE_PS: &str = "Vfx_GamePlay.Blink.Blink_Mantle_01";
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
fn unit_direction(positive: [bool; 3], negative: [bool; 3], rnd: &mut dyn FnMut() -> f32) -> Vec3 {
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
fn primitive_offset(prim: &dishonored_ir::particles::LocationPrimitive, et: f32, rnd: &mut dyn FnMut() -> f32) -> (Vec3, Vec3) {
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
fn pick_source(le: &dishonored_ir::particles::LocationEmitter, sim: &mut EmitterSim, rnd: &mut dyn FnMut() -> f32) -> Option<(Vec3, Vec3)> {
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
        let Some(le) = &ems[i].location_emitter else { continue };
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
fn setup_duration(em: &ParticleEmitterAsset, sim: &mut EmitterSim, rnd: &mut dyn FnMut() -> f32) {
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

pub fn d3(d: &Option<dishonored_ir::particles::Dist>, t: f32, r: f32, def: [f32; 3]) -> Vec3 {
    Vec3::from(d.as_ref().map(|d| d.eval(t, r)).unwrap_or(def))
}

pub fn spawn_particle(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, rnd: &mut dyn FnMut() -> f32) {
    if sim.particles.len() >= MAX_PARTICLES {
        return;
    }
    let et = sim.time;
    let r = [rnd(), rnd(), rnd(), rnd()];
    // No lifetime module (or 0): the particle lives until the emitter is destroyed (UE3).
    let life = em.lifetime.as_ref().map(|d| d.eval1(et, r[0])).filter(|l| *l > 0.0).unwrap_or(f32::INFINITY);
    let mut local = d3(&em.location, et, r[1], [0.0; 3]);
    let start_v = d3(&em.velocity, et, r[2], [0.0; 3]);
    let mut prim_vel = Vec3::ZERO;
    for prim in [em.sphere.as_ref(), em.cylinder.as_ref()].into_iter().flatten() {
        let (off, v) = primitive_offset(prim, et, rnd);
        local += off;
        prim_vel += v;
    }
    let radial = em.velocity_radial.as_ref().map(|vr| local.normalize_or_zero() * vr.eval1(et, r[2])).unwrap_or(Vec3::ZERO);
    // `ParticleModuleLocationEmitter`: at a particle of the source emitter (the other location
    // modules' offsets on top).
    let src = em.location_emitter.as_ref().and_then(|le| pick_source(le, sim, rnd));
    let inv = xf.inverse();
    // The start velocity turns with the component unless `bInWorldSpace` (a local emitter turns
    // a world one into its space); radial and primitive velocities follow the location (0x008B8C20).
    let (pos, mut vel) = if em.local_space {
        let base = src.map(|s| inv.transform_point3(s.0)).unwrap_or(Vec3::ZERO);
        let sv = if em.velocity_world { inv.transform_vector3(start_v) } else { start_v };
        (base + local, sv + radial + prim_vel)
    } else {
        let pos = match src {
            Some(s) => s.0 + xf.transform_vector3(local),
            None => xf.transform_point3(local),
        };
        let sv = if em.velocity_world { start_v } else { xf.transform_vector3(start_v) };
        (pos, sv + xf.transform_vector3(radial + prim_vel))
    };
    if let (Some(k), Some(s)) = (em.location_emitter.as_ref().and_then(|le| le.inherit_velocity), src) {
        vel += if em.local_space { inv.transform_vector3(s.1) } else { s.1 } * k;
    }
    // `ParticleModuleVelocityInheritParent` (0x008B92A0): Scale x the instance's velocity.
    if let Some(d) = &em.velocity_inherit_parent {
        let pv = if em.local_space { inv.transform_vector3(sim.parent_vel) } else { sim.parent_vel };
        vel += Vec3::from(d.eval(et, r[2])) * pv;
    }
    let accel = d3(&em.acceleration, et, r[3], [0.0; 3]);
    // A scaled instance transform scales the particles too (the first-person hand effects ride
    // the rig drawn at the viewmodel scale); gameplay transforms are unscaled.
    let size = d3(&em.size, et, r[0], [1.0; 3]) * xf.x_axis.truncate().length();
    let color = d3(&em.color, et, r[1], [1.0; 3]);
    let alpha = em.alpha.as_ref().map(|d| d.eval1(et, r[1])).unwrap_or(1.0);
    let (mut orbit_off, mut orbit_rot, mut orbit_rate) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
    for o in &em.orbit {
        orbit_off += d3(&o.offset, et, r[1], [0.0; 3]);
        orbit_rot += d3(&o.rotation, et, r[2], [0.0; 3]) * std::f32::consts::TAU;
        orbit_rate += d3(&o.rotation_rate, et, r[3], [0.0; 3]) * std::f32::consts::TAU;
    }
    let max_collisions = em.collision.as_ref().map(|c| c.1.eval1(et, r[0]).max(0.0) as u32).unwrap_or(0);
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
        rot: em.rotation.as_ref().map(|d| d.eval1(et, r[3])).unwrap_or(0.0) * std::f32::consts::TAU,
        rot_rate: em.rotation_rate.as_ref().map(|d| d.eval1(et, r[2])).unwrap_or(0.0) * std::f32::consts::TAU,
        color,
        alpha,
        r,
        mrot: d3(&em.mesh_rotation, et, r[2], [0.0; 3]) * std::f32::consts::TAU,
        mrot_rate: d3(&em.mesh_rotation_rate, et, r[3], [0.0; 3]) * std::f32::consts::TAU,
        normal: None,
        edge2: pos,
    });
}

/// What follows a particle the emitter itself spawned: its `EPET_Spawn` events, and, for a decal
/// emitter that no event feeds, the surface it lies on (projected along its velocity, or down,
/// onto the world; no surface within reach, no decal. LIKELY: the engine projects the decal
/// along the particle's motion).
fn after_spawn(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, ray: Option<RayFn>) {
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
    for p in sim.particles.iter_mut() {
        p.age += dt;
        let rel = (p.age / p.life).clamp(0.0, 1.0);
        p.orbit_rot += p.orbit_rate * dt;
        if p.frozen {
            continue;
        }
        let mut a = space(p.accel, em.accel_world);
        if let Some((d, world)) = &em.accel_over_life {
            a += space(Vec3::from(d.eval(rel, p.r[3])), *world);
        }
        p.base_vel += a * dt;
        if let Some(d) = &em.drag {
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
        if let Some(ap) = &em.attractor_point {
            let pos = Vec3::from(ap.position.eval(et, p.r[0]));
            let target = if ap.world_space && !em.local_space { pos } else { to_space(pos) };
            pull(target, ap.range.eval1(et, p.r[1]), ap.strength.eval1(et, p.r[2]), ap.strength_by_distance, ap.affect_base_velocity, p, &mut v);
        }
        if let Some((a0, a1, range, strength)) = &em.attractor_line {
            let (a0, a1) = (to_space(Vec3::from(*a0)), to_space(Vec3::from(*a1)));
            let seg = a1 - a0;
            let t = ((p.pos - a0).dot(seg) / seg.length_squared().max(1e-6)).clamp(0.0, 1.0);
            pull(a0 + seg * t, range.eval1(t, p.r[1]), strength.eval1(t, p.r[2]), true, true, p, &mut v);
        }
        if let Some((d, absolute)) = &em.vel_over_life {
            let s = Vec3::from(d.eval(rel, p.r[1]));
            v = if *absolute { if em.local_space { s } else { xf.transform_vector3(s) } } else { v * s };
        }
        p.vel = v;
        let next = p.pos + v * dt;
        // Collision against the level geometry: reflect with the damping factor; past the
        // maximum count, the completion option (kill, freeze, stop colliding...).
        let mut moved = false;
        if let (Some((damp, _, opt)), Some(ray)) = (&em.collision, ray) {
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
        let rr = em.rotation_rate_life.as_ref().map_or(1.0, |d| d.eval1(rel, p.r[2]));
        p.rot += p.rot_rate * rr * dt;
        if let Some((d, scale)) = &em.mesh_rotation_rate_life {
            let v = Vec3::from(d.eval(rel, p.r[3])) * std::f32::consts::TAU;
            if *scale {
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
    if let Some((h, absolute, floor)) = &em.kill_height {
        let height = h.eval1(et, 0.5) + if *absolute { 0.0 } else { origin.z };
        for p in sim.particles.iter_mut() {
            let z = if em.local_space { xf.transform_point3(p.pos).z } else { p.pos.z };
            if (*floor && z < height) || (!*floor && z > height) {
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

/// Crossbow muzzle (`Startup`).
pub const CROSSBOW_MUZZLE: &str = "Vfx_Weapon.Effects.Ps_CrossBowMuzzle";
/// Possession: the hand cast, and the burst on a human / rat host.
pub const POSSESSION_CAST_PS: &str = "Vfx_GamePlay.Possession.Possession_HandCast_01";
pub const POSSESSION_HUMAN_PS: &str = "Vfx_GamePlay.Possession.PossesTarget_Human_01";
pub const POSSESSION_RAT_PS: &str = "Vfx_GamePlay.Possession.PossesTarget_Rats_01";
/// Devouring Swarm: the hand cast and the summoning (levels 1 / 2).
pub const SWARM_CAST_PS: &str = "Vfx_GamePlay.RatSwarm.Ps_RSwarm_HandCast_01";
/// `Twk_Powers_Possess.m_HandCastParticleAttachSocket` / `Twk_DevouringSwarm.m_HandFXSocketName`.
pub const HAND_CAST_SOCKET: &str = "LeftHand_PowerCastA";
pub const SWARM_SUMMON_PS: &str = "Vfx_GamePlay.RatSwarm.RSwarm_Summoning_lvl1";
pub const SWARM_SUMMON2_PS: &str = "Vfx_GamePlay.RatSwarm.RSwarm_Summoning_lvl2";
/// Grenade explosion and Spring Razor burst (`Startup`).
pub const GRENADE_EXPLOSION_PS: &str = "Vfx_Weapon.Effects.Grenade.ps_grenade";
pub const SPRING_RAZOR_PS: &str = "Vfx_Weapon.Effects.SpringRazor.Ps_SpringRazor";

/// Projectile trails (`Twk_Proj_*` trail templates): pistol tracer, bolt trail, grenade trail.
pub const PISTOL_TRACER_PS: &str = "Vfx_Weapon.Effects.Trails.Ps_Pistol_StdTracer_01";
pub const BOLT_TRAIL_PS: &str = "Vfx_Weapon.Effects.Trails.StdBolt_Trail";
pub const GRENADE_TRAIL_PS: &str = "Vfx_Weapon.Effects.Grenade.Ps_Grenade_Trail";
/// `Twk_Proj_Arrow_Flare.m_pTrailEffect`.
pub const FLARE_TRAIL_PS: &str = "Vfx_Weapon.Effects.FlareBolts.Flare_Bolt_Trail";
/// Dark Vision's NPC vision cones (`Twk_DarkVision` / pawn `m_VisionConePatrolTemplate`).
pub const DV_CONE_PS: &str = "Vfx_GamePlay.DarkVision.Ps_DVision_VCone_Blue_01";

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

/// The contact type of the nearest surface around a point (the collision triangle's physical
/// material; impact particles and sounds).
pub fn surface_near(world: &CollisionWorld, p: Vec3) -> String {
    let mut best: Option<(f32, u32)> = None;
    for d in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        if let Some(hit) = world.raycast(p - d * 12.0, d, 36.0, dishonored_world::channel::ALL) {
            if best.is_none_or(|b| hit.distance < b.0) {
                best = Some((hit.distance, hit.tri));
            }
        }
    }
    best.and_then(|b| world.surface_of(b.1)).unwrap_or("DisContactType_Environment").to_string()
}

/// The outward normal of the surface at a point where something struck along `dir` (the ray
/// along it, its normal turned against it); without a direction, of the nearest surface around
/// the point.
pub fn normal_at(world: &CollisionWorld, p: Vec3, dir: Vec3) -> Option<Vec3> {
    let dir = dir.normalize_or_zero();
    if dir != Vec3::ZERO {
        if let Some(h) = world.raycast(p - dir * 8.0, dir, 16.0, dishonored_world::channel::ALL) {
            return Some(if h.normal.dot(dir) > 0.0 { -h.normal } else { h.normal });
        }
    }
    let mut best: Option<(f32, Vec3)> = None;
    for d in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        if let Some(h) = world.raycast(p - d * 12.0, d, 36.0, dishonored_world::channel::ALL) {
            if best.is_none_or(|b| h.distance < b.0) {
                best = Some((h.distance, if h.normal.dot(d) > 0.0 { -h.normal } else { h.normal }));
            }
        }
    }
    best.map(|b| b.1)
}

/// The decal a contact (`striker` x the surface `struck`) leaves, if its `DisContactSub_Decal`
/// fires ([`dishonored_audio::contact::ContactDecal::fires`]): one of `m_Materials` (the original
/// picks `rand() % Num`; here by the point, so the same hit looks the same), `m_MaxWidth` x
/// `m_MaxHeight` uu, living `m_fLifetime`. `speed`: the contact's speed, if it has one.
pub fn contact_decal(s: &FxScene, striker: &str, struck: &str, at: Vec3, dir: Vec3, speed: Option<f32>) -> Option<FxRequest> {
    let cd = s.contact?.decal(striker, struck).filter(|d| d.fires(speed))?;
    let normal = normal_at(s.world?, at, dir)?;
    let pick = ((at.x * 7.31 + at.y * 3.17 + at.z * 1.91).abs() as usize) % cd.materials.len();
    Some(FxRequest::Decal { material: cd.materials[pick].clone(), at, normal, dir, width: cd.max_width, height: cd.max_height, lifetime: cd.lifetime })
}

/// Contact type of the surface below a point, and its distance.
pub fn surface_below(world: &CollisionWorld, pos: Vec3, depth: f32) -> Option<(String, f32)> {
    let hit = world.raycast(pos, -Vec3::Z, depth, dishonored_world::channel::ALL)?;
    Some((world.surface_of(hit.tri).unwrap_or("DisContactType_Environment").to_string(), hit.distance))
}

/// An effect the events ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum FxRequest {
    /// A one-shot system at a point, its X axis along `dir`.
    Effect { path: String, at: Vec3, dir: Vec3 },
    /// A cast effect on the local player's left hand (`LeftHand_PowerCastA`); `at` / `dir` are
    /// the fallback placement when the rig has no such socket.
    HandCast { path: String, at: Vec3, dir: Vec3 },
    /// A beam one-shot (`PEB2STM_UserSet` target) from `from` to `to`.
    Beam { path: &'static str, from: Vec3, to: Vec3 },
    /// A gameplay decal (`DisContactSub_Decal`, [`contact_decal`]): the material's object path,
    /// the centre and surface normal (Dishonored world), the direction it was struck along (the
    /// decal's texture axis follows it), its size (uu) and lifetime (s).
    Decal { material: String, at: Vec3, normal: Vec3, dir: Vec3, width: f32, height: f32, lifetime: f32 },
    /// The trail of a projectile that ended within its launch tick (never in a snapshot, so
    /// `attached_want` cannot start it): started at the launch and deactivated at once, as the
    /// projectile's trail component is when the projectile explodes (`DeactivateSystem`); it ends
    /// once its particles (if any) die.
    EndedTrail { path: &'static str, xf: Mat4 },
}

/// The trail a projectile kind carries (`Twk_Proj_*` `m_pTrailEffect`): `StdBolt_Trail` for
/// bolts and sleep darts, `Flare_Bolt_Trail` for the incendiary bolt, `Ps_Grenade_Trail` for
/// grenades; bullets have the tracer beam instead (None).
pub fn trail_of(kind: dishonored_sim::state::ProjectileKind) -> Option<&'static str> {
    use dishonored_sim::state::ProjectileKind as K;
    match kind {
        K::Bullet => None,
        K::Bolt | K::SleepDart => Some(BOLT_TRAIL_PS),
        K::IncendiaryBolt => Some(FLARE_TRAIL_PS),
        K::Grenade | K::StickyGrenade => Some(GRENADE_TRAIL_PS),
        // `TallboyExplosiveArrow_twk.m_pTrailEffect` (`Vfx_Weapon.Effects.WhaleOil.ps_TB_Trail`) is not
        // in the presented effect set yet.
        K::TallboyArrow => None,
        // The trap's explosive arrow carries the same `ps_TB_Trail` (`TrapExplosiveArrow_twk`), also
        // not presented yet; the Overseers' Back launcher's plain arrow (`Twk_Proj_Arrow`) the
        // standard bolt trail.
        K::TrapArrow => None,
        K::TrapBolt => Some(BOLT_TRAIL_PS),
    }
}

/// What the gameplay effects remember between frames.
#[derive(Debug, Clone, Default)]
pub struct FxEventState {
    /// Effects due later: (time, system, position, direction).
    pub delayed: Vec<(f32, String, Vec3, Vec3)>,
    /// Projectile id -> fired as a crossbow bolt (impact striker).
    pub bolts: HashMap<u32, bool>,
    /// Spring Razor traps placed: trap id -> position (their trigger effect).
    pub traps: HashMap<u32, Vec3>,
    /// Pistol shots in flight: projectile id -> muzzle point (the tracer's start).
    pub shots: HashMap<u32, Vec3>,
    /// Projectiles in flight: id -> launch direction (the decal of its impact follows it).
    pub dirs: HashMap<u32, Vec3>,
}

/// What the gameplay effects see of the frame.
pub struct FxScene<'a> {
    /// Every pawn (the local one included).
    pub players: &'a [PlayerState],
    pub me: Option<PlayerId>,
    /// Pistol `Spitfire` sockets of the animated pawns: (position, firing direction).
    pub muzzle: &'a HashMap<u32, (Vec3, Vec3)>,
    /// `SHADOW_KILL_BONES` positions of the animated pawns.
    pub bones: &'a HashMap<u32, Vec<Vec3>>,
    pub contact: Option<&'a ContactSystem>,
    pub world: Option<&'a CollisionWorld>,
    pub now: f32,
    /// The projectiles of the snapshot (a fired one missing from it ended in its launch tick).
    pub projectiles: &'a [ProjectileState],
}

impl FxEventState {
    /// Gameplay effects from simulation events, with the systems and sockets the original's
    /// tweaks name (U-165): the pistol muzzle flash at the `Spitfire` socket
    /// (`m_MuzzleSocketName`; first-person `Ps_PlayerPistol_StdMuzzle_01`, third-person
    /// `Ps_PistolMuzzle`), the crossbow muzzle, Possession, Devouring Swarm, grenade and Spring
    /// Razor systems, Windblast's per-level `m_EffectTemplate` along the view, Shadow Kill ash
    /// on the `m_ShadowKillFXs` bones, impacts and blood from the contact system's particle
    /// sub-intersections, sword sparks on blocks and parries, the pistol tracer beam.
    pub fn requests(&mut self, events: &[SimEvent], s: &FxScene) -> Vec<FxRequest> {
        let mut out = Vec::new();
        let now = s.now;
        // Delayed effects (Shadow Kill ash) that are due.
        let due: Vec<(f32, String, Vec3, Vec3)> = self.delayed.iter().filter(|d| d.0 <= now).cloned().collect();
        self.delayed.retain(|d| d.0 > now);
        for (_, path, at, dir) in due {
            out.push(FxRequest::Effect { path, at, dir });
        }
        let find = |id: u32| s.players.iter().find(|p| p.id == id).copied();
        let fwd = |yaw: f32| Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
        let look = |p: &PlayerState| {
            let (y, pi) = (p.yaw.to_radians(), p.pitch.to_radians());
            Vec3::new(y.cos() * pi.cos(), y.sin() * pi.cos(), pi.sin())
        };
        let surface = |p: Vec3| -> String { s.world.map(|w| surface_near(w, p)).unwrap_or_else(|| "DisContactType_Environment".into()) };
        for ev in events {
            let (path, at, dir): (String, Vec3, Vec3) = match ev {
                SimEvent::SwordBlocked { defender, .. } | SimEvent::SwordParried { defender, .. } => {
                    let Some(p) = find(*defender) else { continue };
                    ("Vfx_Weapon.Effects.Ps_SwordSparks_Parry".into(), p.position + Vec3::Z * 40.0 + fwd(p.yaw) * 40.0, fwd(p.yaw))
                }
                SimEvent::Fired { id, bolt, projectile, kind, origin, velocity } => {
                    self.bolts.insert(*projectile, *bolt);
                    self.dirs.insert(*projectile, velocity.normalize_or_zero());
                    // A projectile already gone (it hit within its launch tick): its trail starts
                    // and stops here; live ones get theirs from `attached_want`.
                    if let Some(path) = trail_of(*kind).filter(|_| !s.projectiles.iter().any(|q| q.id == *projectile)) {
                        out.push(FxRequest::EndedTrail { path, xf: facing(*origin, *velocity) });
                    }
                    // A launcher's shot leaves its tripod, not a pawn's hand: no muzzle effect (its
                    // `id` is the sequence's instigator, if any).
                    if kind.is_launcher_shot() {
                        continue;
                    }
                    let Some(p) = find(*id) else { continue };
                    if *bolt {
                        // The crossbow's muzzle (`Twk_Inv_Crossbow`'s fire tweak), in front of the eye.
                        (CROSSBOW_MUZZLE.into(), p.position + Vec3::Z * 60.0 + look(&p) * 50.0, look(&p))
                    } else {
                        // Grenade throws also report `Fired` (a projectile without a bolt): no muzzle;
                        // nor the Tallboy's bow (its own effects are not presented yet).
                        if matches!(p.left.item, LeftHand::Grenade | LeftHand::StickyGrenade) || *kind == dishonored_sim::state::ProjectileKind::TallboyArrow {
                            continue;
                        }
                        let path = if Some(*id) == s.me { PLAYER_MUZZLE } else { PAWN_MUZZLE };
                        // The animated pistol's socket; without one (first person), in front of the eye.
                        let (at, dir) = s.muzzle.get(id).copied().filter(|m| m.0.distance(p.position) < 200.0).unwrap_or((p.position + Vec3::Z * 60.0 + look(&p) * 50.0, look(&p)));
                        self.shots.insert(*projectile, at);
                        (path.into(), at, dir)
                    }
                }
                // Possession: the cast at the caster's hand and the possession burst on the host
                // (`PossesTarget_Rats_01` for a rat).
                SimEvent::PossessStart { id, host } => {
                    let (Some(p), Some(h)) = (find(*id), find(*host)) else { continue };
                    let (at, dir) = (p.position + Vec3::Z * 40.0 + look(&p) * 40.0, look(&p));
                    if Some(*id) == s.me {
                        out.push(FxRequest::HandCast { path: POSSESSION_CAST_PS.into(), at, dir });
                    } else {
                        out_push(&mut out, POSSESSION_CAST_PS, at, dir);
                    }
                    let target = if h.kind == PawnKind::Rat { POSSESSION_RAT_PS } else { POSSESSION_HUMAN_PS };
                    (target.into(), h.position, Vec3::Z)
                }
                SimEvent::PossessEnd { host, .. } => {
                    let Some(h) = find(*host) else { continue };
                    let target = if h.kind == PawnKind::Rat { POSSESSION_RAT_PS } else { POSSESSION_HUMAN_PS };
                    (target.into(), h.position, Vec3::Z)
                }
                // Devouring Swarm: the hand cast and the summoning at the swarm's spot (level 2
                // has its own system).
                SimEvent::SwarmSummoned { id, position, .. } => {
                    if let Some(p) = find(*id) {
                        let (at, dir) = (p.position + Vec3::Z * 40.0 + look(&p) * 40.0, look(&p));
                        if Some(*id) == s.me {
                            out.push(FxRequest::HandCast { path: SWARM_CAST_PS.into(), at, dir });
                        } else {
                            out_push(&mut out, SWARM_CAST_PS, at, dir);
                        }
                    }
                    let lvl = find(*id).map(|p| p.inv.power_levels[dishonored_sim::state::power::DEVOURING_SWARM]).unwrap_or(1);
                    ((if lvl >= 2 { SWARM_SUMMON2_PS } else { SWARM_SUMMON_PS }).into(), *position, Vec3::Z)
                }
                // Grenade / sticky grenade explosion (`DisProjectile_Grenade` explosion template).
                SimEvent::Explosion { position, .. } => (GRENADE_EXPLOSION_PS.into(), *position, Vec3::Z),
                // Spring Razor: the trap's burst when it springs, at the trap.
                SimEvent::TrapPlaced { trap, position, .. } => {
                    self.traps.insert(*trap, *position);
                    continue;
                }
                SimEvent::TrapTriggered { trap } => {
                    let Some(at) = self.traps.remove(trap) else { continue };
                    (SPRING_RAZOR_PS.into(), at, Vec3::Z)
                }
                SimEvent::Damage { victim, cause, .. } => {
                    let Some(p) = find(*victim) else { continue };
                    let striker = match cause {
                        DamageCause::Pistol | DamageCause::PistolHeadshot => "DisContactType_Bullet",
                        DamageCause::Crossbow | DamageCause::CrossbowHeadshot | DamageCause::TrapShot => "DisContactType_Arrow",
                        DamageCause::Sword | DamageCause::SwordCounter | DamageCause::Assassination | DamageCause::DropAssassination => "DisContactType_Sword",
                        _ => continue,
                    };
                    let path = s.contact.and_then(|c| c.particle(striker, "DisContactType_Body")).unwrap_or_else(|| match cause {
                        DamageCause::Pistol | DamageCause::PistolHeadshot => "Vfx_PhysMat.Blood.Pfx_PistolBlood_Blast".into(),
                        _ => "Vfx_PhysMat.Blood.Pfx_BloodSProjectile_Default".into(),
                    });
                    (path, p.position + Vec3::Z * 50.0, -fwd(p.yaw))
                }
                SimEvent::WindblastCast { id } => {
                    let Some(p) = find(*id) else { continue };
                    let lvl = p.inv.power_levels[dishonored_sim::state::power::WINDBLAST];
                    let path = if lvl >= 2 { "Vfx_GamePlay.Windblast.Windblast_lvl2" } else { "Vfx_GamePlay.Windblast.Windblast_lvl1" };
                    (path.into(), p.position + Vec3::Z * 60.0, look(&p))
                }
                SimEvent::BodyRemoved { body, ash: true } => {
                    let pos = find(*body).map(|p| p.position).unwrap_or(Vec3::ZERO);
                    let pts = s.bones.get(body).cloned().filter(|v| !v.is_empty()).unwrap_or_else(|| vec![pos]);
                    for b in pts {
                        self.delayed.push((now + SHADOW_KILL_DELAY, SHADOW_KILL_PS.into(), b, Vec3::Z));
                    }
                    continue;
                }
                SimEvent::ProjectileImpact { projectile, point, victim } if self.shots.contains_key(projectile) => {
                    // `Twk_Proj_Bullet.m_pBulletTracerFX`: a beam (`TypeDataBeam2`, target
                    // `PEB2STM_UserSet`, one 0.1 s loop) from the muzzle to the hit, set by the shot.
                    let from = self.shots.remove(projectile).unwrap_or(*point);
                    out_beam(&mut out, PISTOL_TRACER_PS, from, *point);
                    self.dirs.remove(projectile);
                    if victim.is_some() {
                        self.bolts.remove(projectile);
                        continue;
                    }
                    let sf = surface(*point);
                    self.bolts.remove(projectile);
                    out.extend(contact_decal(s, "DisContactType_Bullet", &sf, *point, *point - from, None));
                    let path = s.contact.and_then(|c| c.particle("DisContactType_Bullet", &sf)).unwrap_or_else(|| "Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt".into());
                    (path, *point, Vec3::Z)
                }
                SimEvent::ProjectileImpact { projectile, point, victim: None } => {
                    let striker = if self.bolts.remove(projectile).unwrap_or(false) { "DisContactType_Arrow" } else { "DisContactType_Bullet" };
                    let sf = surface(*point);
                    let dir = self.dirs.remove(projectile).unwrap_or(Vec3::ZERO);
                    out.extend(contact_decal(s, striker, &sf, *point, dir, None));
                    let path = s.contact.and_then(|c| c.particle(striker, &sf)).unwrap_or_else(|| "Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt".into());
                    (path, *point, Vec3::Z)
                }
                SimEvent::ProjectileImpact { projectile, .. } => {
                    self.bolts.remove(projectile);
                    self.dirs.remove(projectile);
                    continue;
                }
                // A wall of light destroyed the projectile (no impact effects, 0x00C4F3A0): the
                // bullet's tracer still ends where it struck.
                SimEvent::WallOfLightHit { projectile, point, .. } => {
                    if let Some(from) = self.shots.remove(projectile) {
                        out_beam(&mut out, PISTOL_TRACER_PS, from, *point);
                    }
                    self.bolts.remove(projectile);
                    self.dirs.remove(projectile);
                    continue;
                }
                _ => continue,
            };
            out_push(&mut out, &path, at, dir);
        }
        out
    }
}

fn out_push(out: &mut Vec<FxRequest>, path: &str, at: Vec3, dir: Vec3) {
    out.push(FxRequest::Effect { path: path.to_string(), at, dir });
}

fn out_beam(out: &mut Vec<FxRequest>, path: &'static str, from: Vec3, to: Vec3) {
    out.push(FxRequest::Beam { path, from, to });
}

/// Blink's targeting indicator (`Twk_Blink`, EFFECTS.md "Gameplay effects"): while the local
/// player aims Blink (`left.blink_target`), one instance of `m_pGroundPS` stands on the floor
/// under the target, `m_pLowFallPS` floats at a target in the air, `m_pMantlePS` marks a ledge
/// the arrival would mantle; it follows the target every frame and is killed when aiming ends
/// (release, cancel, travel), as its emitters (`bKillOnDeactivate`, endless loops) are
/// deactivated by the power. The (system, point) to show, if any.
pub fn blink_indicator_want(me: Option<&PlayerState>, world: &CollisionWorld, mv: &dishonored_ir::gameplay::MovementConfig) -> Option<(&'static str, Vec3)> {
    let me = me?;
    let t = me.left.blink_target?;
    let hh = mv.collision_half_height;
    // Mantle: the Blink edge finder (`MantleEdgeFinder_Blink`, the search the arrival runs,
    // `movement::try_blink_mantle`) finds a ledge from the target, facing the aim.
    let mut probe = *me;
    probe.position = t;
    probe.velocity = Vec3::ZERO;
    probe.mode = dishonored_sim::state::MoveMode::Falling;
    let ctx = dishonored_sim::movement::MoveCtx { world, cfg: mv, dt: dishonored_sim::movement::DT };
    if dishonored_sim::movement::try_blink_mantle(&mut probe, &ctx, &mut Vec::new()) {
        return Some((BLINK_MANTLE_PS, t - Vec3::Z * hh));
    }
    // The target is raised to CollisionHeight above the floor below it: a floor within that
    // (plus a margin) means the pawn lands standing (Ground); otherwise it falls (Fall).
    let floor = world.raycast(t, -Vec3::Z, hh + 24.0, dishonored_world::channel::PAWN).map(|h| t - Vec3::Z * h.distance);
    Some(match floor {
        Some(f) => (BLINK_GROUND_PS, f),
        None => (BLINK_FALL_PS, t),
    })
}

/// Effects that follow a live object, kept alive while it exists: projectile trails along the
/// flight, and Dark Vision's vision cones on the revealed NPCs while the power is on. Each:
/// ((kind 0 trail / 1 cone, key), system, transform). A trail whose projectile is gone stops
/// spawning and fades out; a cone goes with the power.
pub fn attached_want(me: Option<&PlayerState>, others: &[PlayerState], projectiles: &[ProjectileState], reveals: &[PlayerId]) -> Vec<((u8, u32), &'static str, Mat4)> {
    let mut want: Vec<((u8, u32), &'static str, Mat4)> = Vec::new();
    for p in projectiles {
        // The pistol tracer is a beam from the muzzle to the hit (a `Beam` request on impact).
        let Some(path) = trail_of(p.kind) else { continue };
        want.push(((0, p.id), path, facing(p.position, p.velocity)));
    }
    if me.is_some_and(|me| me.vitals.dark_vision_remaining > 0.0) {
        for o in others.iter().filter(|o| o.alive && reveals.contains(&o.id) && matches!(o.kind, PawnKind::Guard | PawnKind::Civilian)) {
            let dir = Vec3::new(o.yaw.to_radians().cos(), o.yaw.to_radians().sin(), 0.0);
            want.push(((1, o.id), DV_CONE_PS, facing(o.position + Vec3::Z * 60.0, dir)));
        }
    }
    want
}

/// The same sound for the same pawn within this many seconds is one sound: the first- and
/// third-person rigs of a pawn both report a clip's AkEvent notifies (not in step: their clips
/// start on different frames), and a shot is also posted from `SimEvent::Fired`. The pistol
/// played its shot twice when two of them were more than 0.05 s apart (the old window). Shorter
/// than the fastest refire (`Pistol_Fire` exit 0.30 s).
pub const SAME_SOUND_WINDOW: f32 = 0.2;

/// Where a sound is posted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SoundAt {
    /// On a pawn's game object.
    Pawn(PlayerId),
    /// On a temporary object at an impact point.
    Impact(Vec3),
}

/// What the event sounds remember and read.
#[derive(Debug, Clone, Default)]
pub struct SoundEventState {
    /// Projectile id -> fired as a crossbow bolt.
    pub projectiles: HashMap<u32, bool>,
    /// Last post of an event per pawn (`SAME_SOUND_WINDOW`), shared with the animation notifies.
    pub recent: HashMap<(PlayerId, String), f32>,
    /// Pawns whose animation reported sound notifies recently (their swing sounds come from it).
    pub notify_driven: HashMap<PlayerId, f32>,
}

impl SoundEventState {
    /// The AkEvents one simulation event posts (cue names from `dishonored_audio::cues`, contact
    /// sounds from the contact system).
    pub fn posts(&mut self, ev: &SimEvent, players: &[PlayerState], me: Option<PlayerId>, contact: Option<&ContactSystem>, world: &CollisionWorld, now: f32) -> Vec<(String, SoundAt)> {
        let player = |id: PlayerId| players.iter().find(|p| p.id == id);
        let cue = |k: &str| cues::cue_event(k).unwrap_or("").to_string();
        let on = SoundAt::Pawn;
        let mut posts: Vec<(String, SoundAt)> = Vec::new();
        match *ev {
            SimEvent::SwordSwing { id } if !self.notify_driven.contains_key(&id) => posts.push((cue("sword_swing"), on(id))),
            SimEvent::SwordBlocked { defender, .. } => posts.push((cue("sword_clash"), on(defender))),
            SimEvent::SwordParried { defender, .. } => {
                posts.push((cue("sword_clash"), on(defender)));
                posts.push((cue("sword_parry_whoosh"), on(defender)));
            }
            // The Tallboy's bow (`Snd_AI_TBoy_Arrow_Fire*`) is not in the cue set: no pistol shot.
            SimEvent::Fired { kind: dishonored_sim::state::ProjectileKind::TallboyArrow, .. } => {}
            // A launcher's shot (`m_pSoundEvent_InAir`, the launch's own sounds) is not in the cue set.
            SimEvent::Fired { kind, .. } if kind.is_launcher_shot() => {}
            SimEvent::Fired { id, bolt, projectile, .. } => {
                self.projectiles.insert(projectile, bolt);
                // One shot, one sound: skipped when a fire animation's AkEvent notify already
                // posted it for this pawn, and recorded so a later notify of the same shot is
                // skipped.
                let fire = cue(if bolt { "crossbow_fire" } else { "pistol_fire" });
                let key = (id, fire.clone());
                if !self.recent.get(&key).is_some_and(|t| now - *t < SAME_SOUND_WINDOW) {
                    self.recent.insert(key, now);
                    posts.push((fire, on(id)));
                }
            }
            SimEvent::Reloaded { id } => posts.push((cue("pistol_reload"), on(id))),
            SimEvent::BlinkStart { id, .. } => posts.push((cue("blink_start"), on(id))),
            SimEvent::BlinkEnd { id } => posts.push((cue("blink_end"), on(id))),
            SimEvent::BlinkFizzle { id } => posts.push((cue("power_empty"), on(id))),
            SimEvent::WindblastCast { id } => posts.push((cue("windblast"), on(id))),
            SimEvent::DarkVision { id, active } if Some(id) == me => posts.push((cue(if active { "dark_vision_on" } else { "dark_vision_off" }), on(id))),
            SimEvent::Jumped { id } => posts.push((cue("jump"), on(id))),
            // `DisTweaks_Climbable.m_pClimbingSound` of both Boyle chains.
            SimEvent::ClimbSound { id, .. } => posts.push(("Snd_P_Chain_Climbing".to_string(), on(id))),
            SimEvent::Mantle { id, edge_height } => {
                let k = if edge_height < 80.0 { "mantle_low" } else if edge_height < 140.0 { "mantle_medium" } else { "mantle_high" };
                posts.push((cue(k), on(id)));
            }
            SimEvent::Landed { id, speed, .. } if speed > 250.0 => {
                if let Some(p) = player(id) {
                    // JumpLand contact: big above the hard landing speed, stealth when crouched.
                    let striker = if speed > 900.0 { "DisContactType_JumpLand_Big" } else if p.crouched { "DisContactType_JumpLand_Stealth" } else { "DisContactType_JumpLand" };
                    let surface = surface_below(world, p.position, 200.0).map(|s| s.0).unwrap_or_else(|| "DisContactType_Environment".into());
                    match contact.and_then(|c| c.sound(striker, &surface)) {
                        Some(s) => posts.push((s.event, on(id))),
                        None => posts.push((cues::landing_event("Stone", speed > 900.0), on(id))),
                    }
                }
            }
            SimEvent::Damage { victim, cause, amount, .. } => {
                let k = match cause {
                    DamageCause::Sword | DamageCause::SwordCounter => Some("sword_hit_body"),
                    DamageCause::Assassination => Some("assassinate"),
                    DamageCause::DropAssassination => Some("assassinate_drop"),
                    DamageCause::Pistol => Some("bullet_body"),
                    DamageCause::PistolHeadshot => Some("bullet_head"),
                    DamageCause::Crossbow | DamageCause::TrapShot => Some("bolt_body"),
                    DamageCause::CrossbowHeadshot => Some("bolt_head"),
                    _ => None,
                };
                if let Some(k) = k {
                    posts.push((cue(k), on(victim)));
                }
                let human = player(victim).is_none_or(|p| p.kind == PawnKind::Human);
                let vo = if !human { "npc_pain" } else if amount >= 40.0 { "pain_big" } else { "pain_low" };
                posts.push((cue(vo), on(victim)));
            }
            // Shadow Kill: `Twk_Pawn_DefaultNPC.m_pShadowKillSound` on the body.
            SimEvent::BodyRemoved { body, ash: true } => posts.push((cue("shadow_kill"), on(body))),
            SimEvent::Death { victim, .. } => {
                posts.push((cue("death_vo"), on(victim)));
                posts.push((cue("body_fall"), on(victim)));
            }
            SimEvent::ProjectileImpact { projectile, point, victim: None } => {
                let bolt = self.projectiles.remove(&projectile).unwrap_or(false);
                let surface = surface_near(world, point);
                let striker = if bolt { "DisContactType_Arrow" } else { "DisContactType_Bullet" };
                match contact.and_then(|c| c.sound(striker, &surface)) {
                    Some(s) => posts.push((s.event, SoundAt::Impact(point))),
                    None => posts.push((cues::impact_event(bolt, "Stone"), SoundAt::Impact(point))),
                }
            }
            SimEvent::ProjectileImpact { projectile, .. } | SimEvent::WallOfLightHit { projectile, .. } => {
                self.projectiles.remove(&projectile);
            }
            _ => {}
        }
        posts
    }
}

/// An animation reaction a simulation event asks of a pawn (`npc_anim`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HitKind {
    Death,
    DeathFast,
    /// Killed by a weeper's plague cloud (`DisNPCAnim_DeathPlague`).
    DeathPlague,
    /// Killed by rats (`DisNPCAnim_DeathSwarmAttack`).
    DeathSwarm,
    Melee,
    Projectile,
    Headshot,
    Windblast,
    Swing,
    Parried,
    Assassinated,
    /// A drop assassination's victim (`DisNPCAnim_AssassinationDrop`).
    AssassinatedDrop,
    /// A synced short fatality's victim: backhand or forehand, the group (A / B).
    ShortFatality { backhand: bool, group: u8 },
    /// A finisher's victim: the state's slave chain and its group (`SimEvent::Fatality`).
    Fatality { anim: dishonored_ir::npc_anim::NpcAnim, group: u8 },
    /// Knocked out by something other than a choke (sleep dart): falls like a death.
    KnockedOut,
    /// Choke victim: the choke ended (won: knocked out, lost: broke free, cancelled).
    ChokeWin,
    ChokeLose,
    ChokeCancel,
    /// A carried body was let go.
    Dropped,
}

/// Per pawn, the hits, deaths, swings and parries of this frame's simulation events, with the
/// point they come from.
pub fn collect_hits(events: &[(u64, SimEvent)], all: &[&PlayerState]) -> HashMap<u32, Vec<(HitKind, Vec3, u64)>> {
    use HitKind as H;
    use dishonored_sim::events::{DamageCause as C, SimEvent as E};
    let pos = |id: Option<u32>, fallback: u32| -> Vec3 {
        let f = all.iter().find(|p| p.id == fallback);
        match id.and_then(|i| all.iter().find(|p| p.id == i)) {
            Some(a) => a.position,
            None => f.map(|v| v.position + dishonored_sim::movement::forward_vector(v.yaw)).unwrap_or_default(),
        }
    };
    let mut out: HashMap<u32, Vec<(H, Vec3, u64)>> = HashMap::new();
    for (tick, e) in events {
        match *e {
            E::Damage { victim, attacker, cause, .. } => {
                // A sword kill the attacker plays as a paired move (`SwordState::assassination_move`
                // 8..11): the victim plays its slave.
                let synced = attacker.and_then(|a| all.iter().find(|p| p.id == a)).filter(|a| a.sword.phase == dishonored_sim::state::SwordPhase::Assassinating && a.sword.assassination_target == Some(victim) && (8..12).contains(&a.sword.assassination_move)).map(|a| a.sword.assassination_move - 8);
                let k = match cause {
                    C::Sword | C::SwordCounter if synced.is_some() => synced.map(|m| H::ShortFatality { backhand: m >= 2, group: m % 2 }),
                    C::Sword | C::SwordCounter => Some(H::Melee),
                    C::Pistol | C::Crossbow => Some(H::Projectile),
                    C::PistolHeadshot | C::CrossbowHeadshot => Some(H::Headshot),
                    C::Windblast => Some(H::Windblast),
                    _ => None,
                };
                if let Some(k) = k {
                    out.entry(victim).or_default().push((k, pos(attacker, victim), *tick));
                }
            }
            // The death chains named after their damage (`DisNPCAnim_DeathPlague`,
            // `DisNPCAnim_DeathSwarmAttack`; LIKELY: the code choosing them was not traced, U-890).
            E::Death { victim, killer, cause } => {
                // An assassination kills outright (`vitals::kill`, no damage event): the victim
                // plays the move's slave, which its death then keeps.
                let slave = match cause {
                    C::Assassination => Some(H::Assassinated),
                    C::DropAssassination => Some(H::AssassinatedDrop),
                    _ => None,
                };
                if let Some(s) = slave {
                    out.entry(victim).or_default().push((s, pos(killer, victim), *tick));
                }
                let k = match cause {
                    C::Plague => H::DeathPlague,
                    C::Rats | C::RatBite => H::DeathSwarm,
                    _ => H::Death,
                };
                out.entry(victim).or_default().push((k, pos(killer, victim), *tick))
            }
            // A synced finisher: the victim plays the state's slave (the generic swing has none).
            E::Fatality { id, victim, variant, slave, .. } => {
                if let Some(anim) = dishonored_ir::npc_anim::NpcAnim::fatality_slave(slave) {
                    out.entry(victim).or_default().push((H::Fatality { anim, group: variant }, pos(Some(id), victim), *tick));
                }
            }
            E::SwordSwing { id } => out.entry(id).or_default().push((H::Swing, Vec3::ZERO, *tick)),
            E::SwordParried { attacker, defender } => out.entry(defender).or_default().push((H::Parried, pos(Some(attacker), defender), *tick)),
            E::KnockedOut { victim, by, cause } if cause != dishonored_sim::events::KnockoutCause::Choke => out.entry(victim).or_default().push((H::KnockedOut, pos(by, victim), *tick)),
            E::ChokeEnd { attacker, victim, outcome } => {
                use dishonored_sim::events::ChokeOutcome as O;
                let k = match outcome {
                    O::Win => H::ChokeWin,
                    O::Lose => H::ChokeLose,
                    _ => H::ChokeCancel,
                };
                out.entry(victim).or_default().push((k, pos(Some(attacker), victim), *tick));
            }
            E::BodyDropped { carrier, body } => out.entry(body).or_default().push((H::Dropped, pos(Some(carrier), body), *tick)),
            _ => {}
        }
    }
    out
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
