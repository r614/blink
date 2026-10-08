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
    /// Orbit (summed over the orbit modules): offset, rotation and rotation rate (radians).
    pub orbit_off: Vec3,
    pub orbit_rot: Vec3,
    pub orbit_rate: Vec3,
    /// Collisions so far, the maximum, and frozen / no-longer-colliding flags.
    pub collisions: u32,
    pub max_collisions: u32,
    pub frozen: bool,
    pub halted: bool,
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
}

impl Default for EmitterSim {
    fn default() -> Self {
        Self { time: 0.0, spawn_acc: 0.0, loops: 0, done: false, particles: Vec::new(), last_burst_t: -1.0, mesh_empty: true }
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
    let mut vel = d3(&em.velocity, et, r[2], [0.0; 3]);
    if let Some((radius, pos_axes, surface, velocity)) = &em.sphere {
        let mut v = Vec3::new(rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0);
        for k in 0..3 {
            if !pos_axes[k] {
                v[k] = -v[k].abs();
            }
        }
        let v = v.normalize_or_zero();
        let rad = radius.eval1(et, r[3]) * if *surface { 1.0 } else { rnd().cbrt() };
        local += v * rad;
        if let Some(vs) = velocity {
            vel += v * rad * vs.eval1(et, r[3]);
        }
    }
    if let Some((radius, height, surface)) = &em.cylinder {
        let a = rnd() * std::f32::consts::TAU;
        let rad = radius.eval1(et, r[3]) * if *surface { 1.0 } else { rnd().sqrt() };
        local += Vec3::new(a.cos() * rad, a.sin() * rad, (rnd() - 0.5) * height.eval1(et, r[3]));
    }
    if let Some(vr) = &em.velocity_radial {
        vel += local.normalize_or_zero() * vr.eval1(et, r[2]);
    }
    let (pos, vel) = if em.local_space { (local, vel) } else { (xf.transform_point3(local), xf.transform_vector3(vel)) };
    let size = d3(&em.size, et, r[0], [1.0; 3]);
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
    });
}

/// Advance one emitter by `dt`.
pub fn step(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, dt: f32, rnd: &mut dyn FnMut() -> f32, ray: Option<RayFn>) {
    // Particles.
    let accel = |p: &Particle| d3(&em.acceleration, p.age / p.life, p.r[3], [0.0; 3]);
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
        let a = accel(p);
        let a = if em.local_space { a } else { xf.transform_vector3(a) };
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
        p.rot += p.rot_rate * dt;
        p.mrot += p.mrot_rate * dt;
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
    let before = sim.time;
    sim.time += dt;
    if sim.time < em.delay {
        return;
    }
    let dur = em.duration.max(1e-3);
    let mut t = sim.time - em.delay;
    // Loops only advance for an emitter with a duration (UE3 tests `EmitterDuration > 0`): a
    // zero-duration emitter bursts once, instead of re-firing its time-0 burst every frame
    // (particles without a Lifetime module would pile up forever).
    if em.duration > 1e-3 && t >= dur * (sim.loops + 1) as f32 {
        sim.loops += 1;
        sim.last_burst_t = -1.0;
        if em.loops > 0 && sim.loops >= em.loops {
            sim.done = true;
            return;
        }
    }
    t -= dur * sim.loops as f32;
    let rate = em.rate.as_ref().map(|d| d.eval1(t, 0.5)).unwrap_or(0.0) * em.rate_scale.as_ref().map(|d| d.eval1(t, 0.5)).unwrap_or(1.0);
    sim.spawn_acc += rate.max(0.0) * dt;
    let saved = sim.time;
    sim.time = t;
    while sim.spawn_acc >= 1.0 {
        sim.spawn_acc -= 1.0;
        spawn_particle(em, sim, xf, rnd);
    }
    let frac = t / dur;
    for (count, low, bt) in &em.bursts {
        if *bt > sim.last_burst_t.max(-0.5) && *bt <= frac || (before == 0.0 && *bt <= 0.0 && sim.last_burst_t < 0.0) {
            let n = if *low >= 0 { *low + (rnd() * (*count - *low + 1) as f32) as i32 } else { *count };
            for _ in 0..n.max(0) {
                spawn_particle(em, sim, xf, rnd);
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
pub const SWARM_SUMMON_PS: &str = "Vfx_GamePlay.RatSwarm.RSwarm_Summoning_lvl1";
pub const SWARM_SUMMON2_PS: &str = "Vfx_GamePlay.RatSwarm.RSwarm_Summoning_lvl2";
/// Grenade explosion and Spring Razor burst (`Startup`).
pub const GRENADE_EXPLOSION_PS: &str = "Vfx_Weapon.Effects.Grenade.ps_grenade";
pub const SPRING_RAZOR_PS: &str = "Vfx_Weapon.Effects.SpringRazor.Ps_SpringRazor";

/// Projectile trails (`Twk_Proj_*` trail templates): pistol tracer, bolt trail, grenade trail.
pub const PISTOL_TRACER_PS: &str = "Vfx_Weapon.Effects.Trails.Ps_Pistol_StdTracer_01";
pub const BOLT_TRAIL_PS: &str = "Vfx_Weapon.Effects.Trails.StdBolt_Trail";
pub const GRENADE_TRAIL_PS: &str = "Vfx_Weapon.Effects.Grenade.Ps_Grenade_Trail";
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
    /// A beam one-shot (`PEB2STM_UserSet` target) from `from` to `to`.
    Beam { path: &'static str, from: Vec3, to: Vec3 },
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
                SimEvent::Fired { id, bolt, projectile } => {
                    self.bolts.insert(*projectile, *bolt);
                    let Some(p) = find(*id) else { continue };
                    if *bolt {
                        // The crossbow's muzzle (`Twk_Inv_Crossbow`'s fire tweak), in front of the eye.
                        (CROSSBOW_MUZZLE.into(), p.position + Vec3::Z * 60.0 + look(&p) * 50.0, look(&p))
                    } else {
                        // Grenade throws also report `Fired` (a projectile without a bolt): no muzzle.
                        if matches!(p.left.item, LeftHand::Grenade | LeftHand::StickyGrenade) {
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
                    out_push(&mut out, POSSESSION_CAST_PS, p.position + Vec3::Z * 40.0 + look(&p) * 40.0, look(&p));
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
                        out_push(&mut out, SWARM_CAST_PS, p.position + Vec3::Z * 40.0 + look(&p) * 40.0, look(&p));
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
                        DamageCause::Crossbow | DamageCause::CrossbowHeadshot => "DisContactType_Arrow",
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
                    if victim.is_some() {
                        self.bolts.remove(projectile);
                        continue;
                    }
                    let sf = surface(*point);
                    self.bolts.remove(projectile);
                    let path = s.contact.and_then(|c| c.particle("DisContactType_Bullet", &sf)).unwrap_or_else(|| "Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt".into());
                    (path, *point, Vec3::Z)
                }
                SimEvent::ProjectileImpact { projectile, point, victim: None } => {
                    let striker = if self.bolts.remove(projectile).unwrap_or(false) { "DisContactType_Arrow" } else { "DisContactType_Bullet" };
                    let sf = surface(*point);
                    let path = s.contact.and_then(|c| c.particle(striker, &sf)).unwrap_or_else(|| "Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt".into());
                    (path, *point, Vec3::Z)
                }
                SimEvent::ProjectileImpact { projectile, .. } => {
                    self.bolts.remove(projectile);
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
        use dishonored_sim::state::ProjectileKind as K;
        let path = match p.kind {
            // The pistol tracer is a beam from the muzzle to the hit (a `Beam` request on impact).
            K::Bullet => continue,
            K::Bolt | K::SleepDart | K::IncendiaryBolt => BOLT_TRAIL_PS,
            K::Grenade | K::StickyGrenade => GRENADE_TRAIL_PS,
        };
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
            SimEvent::Fired { id, bolt, projectile } => {
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
                    DamageCause::Crossbow => Some("bolt_body"),
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
            SimEvent::ProjectileImpact { projectile, .. } => {
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
    Melee,
    Projectile,
    Headshot,
    Windblast,
    Swing,
    Parried,
    Assassinated,
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
pub fn collect_hits(events: &[SimEvent], all: &[&PlayerState]) -> HashMap<u32, Vec<(HitKind, Vec3)>> {
    use HitKind as H;
    use dishonored_sim::events::{DamageCause as C, SimEvent as E};
    let pos = |id: Option<u32>, fallback: u32| -> Vec3 {
        let f = all.iter().find(|p| p.id == fallback);
        match id.and_then(|i| all.iter().find(|p| p.id == i)) {
            Some(a) => a.position,
            None => f.map(|v| v.position + dishonored_sim::movement::forward_vector(v.yaw)).unwrap_or_default(),
        }
    };
    let mut out: HashMap<u32, Vec<(H, Vec3)>> = HashMap::new();
    for e in events {
        match *e {
            E::Damage { victim, attacker, cause, .. } => {
                let k = match cause {
                    C::Sword | C::SwordCounter => Some(H::Melee),
                    C::Pistol | C::Crossbow => Some(H::Projectile),
                    C::PistolHeadshot | C::CrossbowHeadshot => Some(H::Headshot),
                    C::Windblast => Some(H::Windblast),
                    C::Assassination | C::DropAssassination => Some(H::Assassinated),
                    _ => None,
                };
                if let Some(k) = k {
                    out.entry(victim).or_default().push((k, pos(attacker, victim)));
                }
            }
            E::Death { victim, killer, .. } => out.entry(victim).or_default().push((H::Death, pos(killer, victim))),
            E::SwordSwing { id } => out.entry(id).or_default().push((H::Swing, Vec3::ZERO)),
            E::SwordParried { attacker, defender } => out.entry(defender).or_default().push((H::Parried, pos(Some(attacker), defender))),
            E::KnockedOut { victim, by, cause } if cause != dishonored_sim::events::KnockoutCause::Choke => out.entry(victim).or_default().push((H::KnockedOut, pos(by, victim))),
            E::ChokeEnd { attacker, victim, outcome } => {
                use dishonored_sim::events::ChokeOutcome as O;
                let k = match outcome {
                    O::Win => H::ChokeWin,
                    O::Lose => H::ChokeLose,
                    _ => H::ChokeCancel,
                };
                out.entry(victim).or_default().push((k, pos(Some(attacker), victim)));
            }
            E::BodyDropped { carrier, body } => out.entry(body).or_default().push((H::Dropped, pos(Some(carrier), body))),
            _ => {}
        }
    }
    out
}
