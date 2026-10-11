//! The Kismet rain (`DisSeqAct_SetRainEmitter`, `MatchView::rain`): the player camera keeps the
//! rain `Emitter` in front of the view and drives its particle parameters, and Arkane's rain
//! modules keep drops inside the emitter's box and place splashes where the camera's traces hit.
//! Decoded from Dishonored.exe (research/dishonored/systems/rain.md):
//!
//! * `DishonoredPlayerCamera::SetRainEmitter` 0x00AC0570 (from 0x00BA1770 -> 0x00B9ADF0):
//!   deactivates the previous `m_pRainBoxEmitter` (PSC `SetActive(false)`, 0x008A9880), stores
//!   the box extent (the template's `DisParticleModuleRainDrops.m_Extent`), activates the new
//!   emitter, copies the drop count, impact distances and rain direction, and turns the emitter
//!   to the rotation actor's rotation.
//! * The camera tick (end of 0x00AD80A0, first-person mode): the box placement, the covered
//!   test, `SpawnKillRate` / `MaxParticles` and the impact traces ([`drive`]).
//! * `DisParticleModuleRainDrops`: Spawn 0x00C00700, Update 0x00C00790, GetSpawnAmount
//!   0x00C01370, the drop placement 0x00C00470 ([`step_emitter`]).
//! * `DisParticleModuleRainImpacts`: Spawn 0x00C04450, FinalUpdate 0x00C01670, GetSpawnAmount
//!   0x00C01470.

use crate::effects::{Effects, FxInstance, PlacedEmitter};
use crate::players::GameCamera;
use crate::present_rules::{spawn_particle, EmitterSim, Particle};
use crate::session::SessionView;
use bevy::prelude::*;
use dishonored_ir::particles::{ParticleEmitterAsset, RainDrops};

/// The float the camera uses for an axis the view does not move along (0x7F7FC99E).
pub const BIG: f32 = 3.4e38;
/// Length of the covered and impact traces along the rain direction (uu).
pub const TRACE_LENGTH: f32 = 5000.0;
/// The covered test's random offset about the camera (uu): `appFrand() * 70 - 35` in X and Y.
pub const COVER_JITTER: f32 = 35.0;
/// `m_fRainSpawnKillRate` when the camera becomes (un)covered, and the rate it stops growing at.
pub const KILL_RATE_RESET: f32 = 30.0;
pub const KILL_RATE_CAP: f32 = 10000.0;

/// `appSRand` (the LCG at DAT_0141B1F8): [0, 1).
pub fn srand(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(0x0bb3_8435).wrapping_add(0x3619_636b);
    f32::from_bits((*seed & 0x7f_ffff) | 0x3f80_0000) - 1.0
}

/// `appCeil` as compiled (x87 / SSE: `-(round(-0.5 - (x + x)) >> 1)`, round to nearest even).
pub fn app_ceil(x: f32) -> i32 {
    let v = -0.5 - (x + x);
    -((v.round_ties_even() as i32) >> 1)
}

/// `appRound` as compiled (`round(x * 2 + 0.5) >> 1`).
pub fn app_round(x: f32) -> i32 {
    ((x * 2.0 + 0.5).round_ties_even() as i32) >> 1
}

/// 0x00AD80A0 rain box placement: the distance along the view `v` (already turned by the
/// emitter's matrix, `v_box`) at which the view leaves the box of half size `extent`: the
/// smallest `extent[i] / |v_box[i]|` (an axis the view does not move along: [`BIG`]). The
/// emitter is put at `camera + view * t`, so the camera sits on the box's boundary.
pub fn box_distance(extent: Vec3, v_box: Vec3) -> f32 {
    let axis = |e: f32, v: f32| if v == 0.0 { BIG } else { e / v.abs() };
    let (a, b, c) = (axis(extent.x, v_box.x), axis(extent.y, v_box.y), axis(extent.z, v_box.z));
    let m = if a <= b { a } else { b };
    if c < m { c } else { m }
}

/// The camera's spawn-kill rate rule: unchanged cover, the rate grows (`DeltaSeconds x rate x
/// 100`, while below [`KILL_RATE_CAP`]); a change of cover resets it to [`KILL_RATE_RESET`].
/// Returns (`m_bWasUncovered`, `m_fRainSpawnKillRate`).
pub fn spawn_kill_rate(was_uncovered: bool, uncovered: bool, rate: f32, delta_seconds: f32) -> (bool, f32) {
    if was_uncovered == uncovered {
        if rate < KILL_RATE_CAP {
            (uncovered, delta_seconds * rate * 100.0)
        } else {
            (uncovered, rate)
        }
    } else {
        (uncovered, KILL_RATE_RESET)
    }
}

/// The eight boxes 0x00C00790 splits the rain box into at the shift `s` of the box against the
/// drops since the last update, and the cumulative volume fractions of the newly uncovered ones.
/// Per axis the cut is at `E - s` (s >= 0) or `-E - s` (s < 0); box `i` takes the upper part on
/// axis k when bit k of `i` is set. The box still covered by the old box (index: bit k set when
/// `s[k] < 0`) gets no weight; the others are weighted by volume.
pub struct Slabs {
    pub boxes: [(Vec3, Vec3); 8],
    pub cdf: [f32; 8],
}

pub fn slabs(e: Vec3, s: Vec3) -> Slabs {
    let cut = |e: f32, s: f32| if s < 0.0 { -e - s } else { e - s };
    let c = Vec3::new(cut(e.x, s.x), cut(e.y, s.y), cut(e.z, s.z));
    let mut boxes = [(Vec3::ZERO, Vec3::ZERO); 8];
    for (i, b) in boxes.iter_mut().enumerate() {
        let part = |k: usize| if (i >> k) & 1 == 0 { (-e[k], c[k]) } else { (c[k], e[k]) };
        let (x, y, z) = (part(0), part(1), part(2));
        *b = (Vec3::new(x.0, y.0, z.0), Vec3::new(x.1, y.1, z.1));
    }
    // Lower / upper lengths per axis (the decompile's products, in its order).
    let (lx, ly, lz) = (c.x - -e.x, c.y - -e.y, c.z - -e.z);
    let (ux, uy, uz) = (e.x - c.x, e.y - c.y, e.z - c.z);
    let mut v = [lx * ly * lz, ux * ly * lz, uy * lx * lz, uy * ux * lz, uz * lx * ly, uz * ux * ly, uz * uy * lx, uz * uy * ux];
    let covered = (s.x < 0.0) as usize + if s.y < 0.0 { 2 } else { 0 } + if s.z < 0.0 { 4 } else { 0 };
    let held = v[covered];
    v[covered] = 0.0;
    let inv = 1.0 / ((e.x - -e.x) * (e.z - -e.z) * (e.y - -e.y) - held);
    let mut cdf = [0.0; 8];
    let mut acc = 0.0;
    for k in 0..8 {
        acc = v[k] * inv + acc;
        cdf[k] = acc;
    }
    Slabs { boxes, cdf }
}

/// The slab a drop respawns in: the first whose cumulative fraction is >= `r` (the last one
/// past the seventh).
pub fn pick_slab(cdf: &[f32; 8], r: f32) -> usize {
    (0..7).find(|&k| r <= cdf[k]).unwrap_or(7)
}

/// `DistributionFloatParticleParameter` lookups of an instance.
pub type Params<'a> = &'a dyn Fn(&str) -> Option<f32>;

/// A drop's fields the port's [`Particle`] does not hold (UE3 `FBaseParticle`): `RelativeTime`
/// (the module's fade: > 0 fading in, < 0 fading out, 0 steady), `BaseColor` and `BaseSize`.
#[derive(Clone, Copy, Default, Debug)]
pub struct Drop {
    pub rt: f32,
    pub base_alpha: f32,
    pub base_color: Vec3,
    pub base_size: Vec3,
}

/// Per emitter of an instance: the emitter instance's `Location` / `OldLocation`, the drops'
/// own fields (index-aligned with the particles) and the impacts module's per-instance float.
#[derive(Clone, Default, Debug)]
pub struct RainEmitterState {
    pub location: Option<Vec3>,
    pub old_location: Vec3,
    pub drops: Vec<Drop>,
    pub acc: f32,
}

pub fn is_rain_emitter(em: &ParticleEmitterAsset) -> bool {
    em.rain_drops().is_some() || em.rain_impacts().is_some()
}

/// 0x00C00470: a drop placed in `bx` (min, max, relative to the emitter location already in
/// `p.pos`), its velocity (`m_StartVelocity` turned by the component's matrix), colour, alpha
/// (Color.A = BaseColor.A) and size, `RelativeTime` = `rt`.
fn place_drop(rd: &RainDrops, p: &mut Particle, d: &mut Drop, xf: &Mat4, bx: (Vec3, Vec3), rt: f32, et: f32, frand: &mut dyn FnMut() -> f32) {
    for k in 0..3 {
        let r = frand();
        p.pos[k] += (bx.1[k] - bx.0[k]) * r + bx.0[k];
    }
    let v = rd.start_velocity.as_ref().map(|d| Vec3::from(d.eval_rand(et, rd.locks[0], frand))).unwrap_or(Vec3::ZERO);
    let v = xf.transform_vector3(v);
    p.vel = v;
    p.base_vel = v;
    let c = rd.start_color.as_ref().map(|d| Vec3::from(d.eval_rand(et, rd.locks[1], frand))).unwrap_or(Vec3::ONE);
    let a = rd.start_alpha.as_ref().map(|d| d.eval1(et, frand())).unwrap_or(1.0);
    p.color = c;
    d.base_color = c;
    p.alpha = a;
    d.base_alpha = a;
    // The instance's scale scales its sprites (as `present_rules::spawn_particle`).
    let s = rd.start_size.as_ref().map(|d| Vec3::from(d.eval_rand(et, rd.locks[2], frand))).unwrap_or(Vec3::ONE) * xf.x_axis.truncate().length();
    p.size = s;
    d.base_size = s;
    d.rt = rt;
}

/// 0x00C00790 (`DisParticleModuleRainDrops::Update`): fading, drops leaving the box start
/// fading out, dead drops (BaseColor.A <= 0) respawn in the newly uncovered slabs, at most
/// `appCeil(SpawnKillRate x DeltaSeconds)` a frame and only while fewer than `MaxParticles` live.
#[allow(clippy::too_many_arguments)]
pub fn update_drops(rd: &RainDrops, particles: &mut [Particle], drops: &mut [Drop], loc: Vec3, old_loc: Vec3, xf: &Mat4, dt: f32, delta_seconds: f32, et: f32, params: Params, frand: &mut dyn FnMut() -> f32, seed: &mut u32) {
    let e = Vec3::from(rd.extent);
    // Average velocity of the live drops.
    let (mut live, mut sum) = (0i32, Vec3::ZERO);
    for (p, d) in particles.iter().zip(drops.iter()) {
        if d.base_alpha > 0.0 {
            live += 1;
            sum += p.vel;
        }
    }
    let avg = if live > 0 { sum * (1.0 / live as f32) } else { sum };
    let shift = (loc - old_loc) - avg * dt;
    let sl = slabs(e, shift);
    let max = rd.max_particles.value(et, frand(), params) as i32;
    let mut budget = if live != max { app_ceil(rd.spawn_kill_rate.value(et, frand(), params) * delta_seconds) } else { 0 };
    let fade = rd.fading_rate.as_ref().map(|d| d.eval1(et, frand())).unwrap_or(0.0) * delta_seconds;
    let inv = xf.inverse();
    for i in (0..particles.len().min(drops.len())).rev() {
        let (p, d) = (&mut particles[i], &mut drops[i]);
        if d.rt >= 0.0 {
            if d.rt > 0.0 {
                d.rt += fade;
                if d.rt >= 1.0 {
                    d.rt = 0.0;
                    p.alpha = d.base_alpha;
                } else {
                    p.alpha = d.base_alpha * d.rt;
                }
            }
        } else {
            d.rt -= fade;
            if d.rt <= -1.0 {
                p.alpha = 0.0;
                d.base_alpha = 0.0;
                d.rt = 0.0;
                p.vel = Vec3::ZERO;
                p.base_vel = Vec3::ZERO;
            } else {
                p.alpha = (d.rt + 1.0) * d.base_alpha;
            }
        }
        let l = inv.transform_point3(p.pos);
        if d.base_alpha > 0.0 {
            let outside = l.x <= -e.x || e.x <= l.x || l.y <= -e.y || e.y <= l.y || l.z <= -e.z || e.z <= l.z;
            if outside {
                if d.rt > 0.0 {
                    d.rt = -d.rt;
                } else if d.rt == 0.0 {
                    d.rt = -fade;
                }
            } else if d.rt < 0.0 {
                d.rt = -d.rt;
            }
        } else if budget > 0 && live < max {
            let k = pick_slab(&sl.cdf, srand(seed));
            p.pos = loc;
            place_drop(rd, p, d, xf, sl.boxes[k], fade, et, frand);
            budget -= 1;
        }
    }
}

/// 0x00C01370 (`DisParticleModuleRainDrops::GetSpawnAmount`): the rate that fills the emitter
/// to `MaxParticles` this frame, capped by the spawn module's `Rate x max(RateScale, 0)`.
pub fn drops_spawn_rate(max_particles: i32, active: i32, leftover: f32, dt: f32, rate: f32, rate_scale: f32) -> f32 {
    let need = ((max_particles - active) as f32 - leftover) / dt;
    let scale = if rate_scale <= 0.0 { 0.0 } else { rate_scale };
    let cap = rate * scale;
    if need <= cap { need } else { cap }
}

/// 0x00C01470 (`DisParticleModuleRainImpacts::GetSpawnAmount`, with the camera): the module's
/// accumulator `acc` gains `rate x dt` (its whole part asked of the camera); the impacts spawned
/// are the camera's available ones in proportion of what this emitter asked last time.
/// Returns (spawn count, new accumulator, impacts requested).
pub fn impacts_spawn_amount(acc: f32, rate: f32, dt: f32, cam: &mut RainCamera) -> (i32, f32, i32) {
    let old_whole = acc as i32;
    let new_acc = (acc - old_whole as f32) + rate * dt;
    let request = new_acc as i32;
    let number = if cam.available > 0 {
        let n = app_round(cam.available as f32 / cam.last_requested as f32 * old_whole as f32);
        cam.available -= n;
        cam.last_requested -= old_whole;
        n
    } else {
        0
    };
    cam.requested += request;
    (number, new_acc, request)
}

/// Advance a rain emitter (drops or impacts) by `dt`, as the UE3 sprite emitter tick runs its
/// modules: reset (Velocity = BaseVelocity, Size = BaseSize, Color = BaseColor), the update
/// modules, the spawn, then the move (Location += Velocity x dt).
#[allow(clippy::too_many_arguments)]
pub fn step_emitter(em: &ParticleEmitterAsset, sim: &mut EmitterSim, st: &mut RainEmitterState, xf: &Mat4, dt: f32, frand: &mut dyn FnMut() -> f32, params: Params, cam: &mut RainCamera) {
    let loc = xf.w_axis.truncate();
    st.old_location = st.location.unwrap_or(loc);
    st.location = Some(loc);
    sim.time += dt;
    let et = sim.time;
    if dt <= 0.0 {
        return;
    }
    // `WorldInfo.DeltaSeconds` (0x00781240): the frame's time, the same as the emitter's here.
    let delta_seconds = dt;
    let rate_of = |frand: &mut dyn FnMut() -> f32| {
        let scale = em.rate_scale.as_ref().map(|d| d.eval1(et, frand())).unwrap_or(1.0);
        let rate = em.rate.as_ref().map(|d| d.eval1(et, frand())).unwrap_or(0.0);
        (rate, scale)
    };
    if let Some(rd) = em.rain_drops() {
        st.drops.resize(sim.particles.len(), Drop::default());
        for (p, d) in sim.particles.iter_mut().zip(st.drops.iter()) {
            p.vel = p.base_vel;
            p.size = d.base_size;
            p.color = d.base_color;
            p.alpha = d.base_alpha;
        }
        // (`ParticleModuleSizeMultiplyVelocity` is applied where the sprites are built,
        // `effects::particle_look`, from the size kept here.)
        update_drops(rd, &mut sim.particles, &mut st.drops, loc, st.old_location, xf, dt, delta_seconds, et, params, frand, &mut cam.seed);
        if !sim.done {
            let max = rd.max_particles.value(et, frand(), params) as i32;
            let (rate, scale) = rate_of(frand);
            let rate = drops_spawn_rate(max, sim.particles.len() as i32, sim.spawn_acc, dt, rate, scale).max(0.0);
            if rate > 0.0 {
                // UE3 `FParticleEmitterInstance::Spawn`: the leftover carries the fraction, each
                // particle is moved by its time into the frame.
                let old = sim.spawn_acc;
                let nl = old + dt * rate;
                let n = nl.floor();
                sim.spawn_acc = nl - n;
                let inc = 1.0 / rate;
                let start = dt + old * inc - inc;
                for i in 0..n as usize {
                    let spawn_time = start - i as f32 * inc;
                    let mut p = Particle { pos: loc, life: f32::INFINITY, r: [frand(), frand(), frand(), frand()], ..Default::default() };
                    let mut d = Drop::default();
                    let e = Vec3::from(rd.extent);
                    place_drop(rd, &mut p, &mut d, xf, (-e, e), 0.0, et, frand);
                    p.pos += p.vel * spawn_time;
                    sim.particles.push(p);
                    st.drops.push(d);
                }
            }
        }
    } else if let Some(ri) = em.rain_impacts() {
        for p in sim.particles.iter_mut() {
            p.age += dt;
        }
        sim.particles.retain(|p| p.age < p.life);
        for p in sim.particles.iter_mut() {
            p.vel = p.base_vel;
        }
        if !sim.done {
            let (rate, scale) = rate_of(frand);
            let (number, acc, _) = impacts_spawn_amount(st.acc, rate * if scale <= 0.0 { 0.0 } else { scale }, dt, cam);
            st.acc = acc;
            for _ in 0..number.max(0) {
                let Some(&(at, dir)) = cam.impacts.get(cam.current_index) else { break };
                cam.current_index += 1;
                let before = sim.particles.len();
                spawn_particle(em, sim, xf, frand);
                if sim.particles.len() == before {
                    continue;
                }
                if let Some(p) = sim.particles.last_mut() {
                    // 0x00C04450: at the impact; `m_bFixedLocation` adds its direction to the
                    // velocity (the velocity-aligned sprite stands along the reflected rain;
                    // FinalUpdate 0x00C01670 keeps OldLocation = Location - Velocity for it).
                    p.pos = at;
                    if ri.fixed_location {
                        p.base_vel += dir;
                        p.vel += dir;
                    }
                }
            }
        }
    }
    // The move (UE3's bounding-box update). LIKELY: the impacts module's spawn flag (Flags |= 4
    // with `m_bFixedLocation`) keeps its particles where they landed; the velocity only aligns
    // the sprite (FinalUpdate's OldLocation = Location - Velocity).
    if em.rain_impacts().is_some_and(|r| r.fixed_location) {
        return;
    }
    for p in sim.particles.iter_mut() {
        p.pos += p.vel * dt;
    }
}

/// The camera's rain (`DishonoredPlayerCamera` +0x4C0..+0x524) and the impacts it traced.
#[derive(Resource)]
pub struct RainCamera {
    /// The last `RainView::id` applied.
    seen: u32,
    /// `m_pRainBoxEmitter`: the placed emitter's key and instance.
    emitter: Option<(String, Entity)>,
    /// `m_RainBoxExtent`.
    pub extent: Vec3,
    /// The emitter's rotation (and the placed actor's scale).
    rotation: Mat4,
    pub drops: i32,
    pub impacts_min: f32,
    pub impacts_max: f32,
    pub direction: Vec3,
    /// `m_bWasUncovered`, `m_fRainSpawnKillRate`.
    pub was_uncovered: bool,
    pub spawn_kill_rate: f32,
    /// `m_RainImpacts` (position, reflected direction), `m_CurrentRainImpactIndex`,
    /// `m_LastNumRequestedRainImpacts`, `m_NumRequestedRainImpacts`, `m_NumAvailableRainImpacts`.
    pub impacts: Vec<(Vec3, Vec3)>,
    pub current_index: usize,
    pub last_requested: i32,
    pub requested: i32,
    pub available: i32,
    /// `appSRand`'s seed.
    pub seed: u32,
}

impl Default for RainCamera {
    fn default() -> Self {
        Self {
            seen: 0,
            emitter: None,
            extent: Vec3::ZERO,
            rotation: Mat4::IDENTITY,
            drops: 0,
            impacts_min: 0.0,
            impacts_max: 0.0,
            direction: Vec3::NEG_Z,
            was_uncovered: false,
            spawn_kill_rate: 0.0,
            impacts: Vec::new(),
            current_index: 0,
            last_requested: 0,
            requested: 0,
            available: 0,
            seed: 0x1234_5678,
        }
    }
}

/// UE3 `FRotationMatrix` of a rotator (pitch, yaw, roll in 65536ths of a turn), as a Dishonored
/// world matrix (columns X, Y, Z).
pub fn rotator_matrix(r: [i32; 3]) -> Mat4 {
    let a = |u: i32| u as f32 * std::f32::consts::TAU / 65536.0;
    let (sp, cp) = a(r[0]).sin_cos();
    let (sy, cy) = a(r[1]).sin_cos();
    let (sr, cr) = a(r[2]).sin_cos();
    let x = Vec3::new(cp * cy, cp * sy, sp);
    let y = Vec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp);
    let z = Vec3::new(-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), Vec4::W)
}

/// The rain box extent of a template: its first emitter's first module, if that is a
/// `DisParticleModuleRainDrops` (what 0x00B9ADF0 requires of a target).
pub fn template_extent(sys: &dishonored_ir::particles::ParticleSystemAsset) -> Option<Vec3> {
    sys.emitters.first()?.rain_drops().filter(|r| r.first_module).map(|r| Vec3::from(r.extent))
}

/// The camera's rain tick (0x00AD80A0) on the client's view, after `SetRainEmitter` changes.
#[allow(clippy::too_many_arguments)]
pub fn drive(
    view: Option<Res<SessionView>>,
    session: Option<Res<crate::session::Session>>,
    fx: Option<ResMut<Effects>>,
    mut cam_st: ResMut<RainCamera>,
    cams: Query<(&GlobalTransform, &Projection), With<GameCamera>>,
    mut q: Query<(Entity, &PlacedEmitter, &mut FxInstance)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
    time: Res<Time>,
    mut frand_state: Local<u64>,
) {
    let (Some(view), Some(mut fx)) = (view, fx) else { return };
    let st = &mut *cam_st;
    let rain = view.matchv.rain.clone();
    // SetRainEmitter (0x00AC0570) on a new action, or SetRainEmitter(None) when it ends.
    let wanted: Option<String> = rain.as_ref().map(|r| r.emitter.to_ascii_lowercase());
    let changed = match &rain {
        Some(r) => r.id != st.seen,
        None => st.emitter.is_some(),
    };
    if changed {
        let mut apply = true;
        let mut extent = Vec3::ZERO;
        let mut placed = None;
        // The emitter's level not registered yet (the snapshot can arrive before the level
        // spawn): try again next frame.
        let mut ready = true;
        if let Some(key) = &wanted {
            match crate::effects::placed_emitter(&fx, key) {
                Some((level, system, xf, _)) => match crate::effects::level_system(&fx, &level, system).and_then(template_extent) {
                    Some(e) => {
                        extent = e;
                        placed = Some(xf);
                    }
                    // Not a rain template: the action does not reach the camera (0x00B9ADF0).
                    None => apply = false,
                },
                None => {
                    apply = false;
                    ready = false;
                }
            }
        }
        if let (Some(r), true) = (&rain, ready) {
            st.seen = r.id;
        }
        if apply {
            // The previous emitter is deactivated when another (or none) replaces it.
            if let Some((old, e)) = st.emitter.take() {
                if wanted.as_deref() != Some(old.as_str()) {
                    if let Ok((_, _, mut inst)) = q.get_mut(e) {
                        inst.stop();
                    }
                } else {
                    st.emitter = Some((old, e));
                }
            }
            if let (Some(r), Some(key), Some(xf)) = (&rain, &wanted, placed) {
                st.extent = extent;
                st.drops = r.drops;
                st.impacts_min = r.impacts_min;
                st.impacts_max = r.impacts_max;
                st.direction = Vec3::from(r.direction);
                // The rotation actor's rotation, else the default rotator.
                let scale = xf.x_axis.truncate().length();
                st.rotation = rotator_matrix(r.rotation.unwrap_or([0, 0, 0])) * Mat4::from_scale(Vec3::splat(scale));
                // Activate: the level's live instance, else a new one at the placed spot.
                let live = st.emitter.as_ref().map(|e| e.1).filter(|e| q.get(*e).is_ok_and(|(_, _, i)| !i.stopped()));
                let live = live.or_else(|| q.iter().find(|(_, k, i)| k.0 == *key && !i.stopped()).map(|(e, _, _)| e));
                let e = live.or_else(|| crate::effects::start_placed_emitter(&mut commands, &mut meshes, &mut fx, key, xf, false));
                st.emitter = e.map(|e| (key.clone(), e));
            }
        }
    }
    let Some((cam, proj)) = cams.iter().next() else { return };
    // The camera's point of view (Dishonored world).
    let swap = |v: Vec3| Vec3::new(v.x, v.z, v.y);
    let c = swap(cam.translation()) / crate::coords::UU_TO_BEVY;
    let fwd = swap(*cam.forward()).normalize_or_zero();
    let up = swap(*cam.up()).normalize_or_zero();
    let right = up.cross(fwd).normalize_or_zero();
    let (hfov, aspect) = match proj {
        Projection::Perspective(p) => ((2.0 * ((p.fov * 0.5).tan() * p.aspect_ratio).atan()).to_degrees(), p.aspect_ratio),
        _ => (90.0, 16.0 / 9.0),
    };
    let world = session.as_ref().map(|s| &s.content.collision);
    let dir = st.direction;
    let mut frand = || crate::present_rules::next_rand(&mut frand_state);
    if let Some((_, e)) = st.emitter.clone() {
        if let Ok((_, _, mut inst)) = q.get_mut(e) {
            // (a) The box before the camera.
            let m = st.rotation;
            let t = box_distance(st.extent, m.transform_vector3(fwd));
            let at = c + fwd * t;
            let mut xf = m;
            xf.w_axis = at.extend(1.0);
            inst.set_transform(xf);
            // (b) Covered: in water, or something above within the trace.
            let me = view.me.as_ref();
            let in_water = me.is_some_and(|p| p.mode == dishonored_sim::state::MoveMode::Swimming || p.water.under);
            let uncovered = !in_water && {
                let p = c + Vec3::new(frand() * 70.0 - COVER_JITTER, frand() * 70.0 - COVER_JITTER, 0.0);
                world.is_none_or(|w| dishonored_world::WorldQuery::raycast(w, p, -dir, TRACE_LENGTH, dishonored_world::channel::PAWN).is_none())
            };
            // (c) The parameters.
            let (was, rate) = spawn_kill_rate(st.was_uncovered, uncovered, st.spawn_kill_rate, time.delta_secs());
            st.was_uncovered = was;
            st.spawn_kill_rate = rate;
            inst.set_float_param("SpawnKillRate", rate);
            inst.set_float_param("MaxParticles", if uncovered { st.drops as f32 } else { 0.0 });
        }
    }
    // (d) The impacts the modules asked for last frame.
    st.impacts.clear();
    let count = st.requested;
    if count > 0 && st.impacts_min < st.impacts_max {
        let tan = (hfov * 0.008_726_646).tan();
        for _ in 0..count {
            let d = (st.impacts_max - st.impacts_min) * srand(&mut st.seed) + st.impacts_min;
            let half = d * tan;
            let x = (half - -half) * srand(&mut st.seed) + -half;
            let hv = half / aspect;
            let y = (hv - -hv) * srand(&mut st.seed) + -hv;
            let w = c + fwd * d + right * x + up * y;
            let Some(world) = world else { continue };
            if let Some(h) = dishonored_world::WorldQuery::raycast(world, w - dir * TRACE_LENGTH, dir, 2.0 * TRACE_LENGTH, dishonored_world::channel::PAWN) {
                let n = h.normal;
                if dir.dot(n) < 0.0 {
                    st.impacts.push((h.point, dir - n * (2.0 * dir.dot(n))));
                }
            }
        }
    }
    st.last_requested = count;
    st.requested = 0;
    st.available = st.impacts.len() as i32;
    st.current_index = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::particles::{Dist, FloatDist, ParticleModule, ParticleParam};

    #[test]
    fn rain_box_puts_the_camera_on_its_boundary() {
        let e = Vec3::splat(500.0);
        assert_eq!(box_distance(e, Vec3::X), 500.0);
        // Diagonal view: the nearest face.
        let v = Vec3::new(1.0, 2.0, 0.0);
        assert!((box_distance(e, v) - 250.0).abs() < 1e-4);
        // An axis the view does not move along never limits it.
        assert_eq!(box_distance(e, Vec3::ZERO), BIG);
        let t = box_distance(Vec3::new(500.0, 300.0, 200.0), Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(t, 200.0);
    }

    #[test]
    fn rain_spawn_kill_rate_resets_on_a_change_and_grows_while_steady() {
        assert_eq!(spawn_kill_rate(false, true, 0.0, 0.016), (true, KILL_RATE_RESET));
        let (w, r) = spawn_kill_rate(true, true, 30.0, 0.016);
        assert!(w && (r - 0.016 * 30.0 * 100.0).abs() < 1e-3);
        assert_eq!(spawn_kill_rate(true, true, 20000.0, 0.016), (true, 20000.0), "capped");
        assert_eq!(spawn_kill_rate(true, false, 500.0, 0.016), (false, KILL_RATE_RESET));
    }

    #[test]
    fn rain_slabs_weight_the_newly_uncovered_volume() {
        let e = Vec3::splat(100.0);
        // Moved +10 in X only: the new volume is the 10-wide slab at +X.
        let s = slabs(e, Vec3::new(10.0, 0.0, 0.0));
        // Covered box: lower X part (s.x >= 0), lower Y / Z parts contain everything (c = E).
        assert_eq!(s.boxes[0], (Vec3::splat(-100.0), Vec3::new(90.0, 100.0, 100.0)));
        assert_eq!(s.boxes[1], (Vec3::new(90.0, -100.0, -100.0), Vec3::splat(100.0)));
        assert!(s.cdf[0].abs() < 1e-6, "the covered box has no weight");
        assert!((s.cdf[1] - 1.0).abs() < 1e-5, "all of it in the +X slab");
        assert_eq!(pick_slab(&s.cdf, 0.5), 1);
        // Moved -X and -Z: the covered box is index 1 + 4.
        let s = slabs(e, Vec3::new(-20.0, 0.0, -10.0));
        assert!((s.cdf[7] - 1.0).abs() < 1e-5);
        let w: Vec<f32> = (0..8).map(|k| s.cdf[k] - if k == 0 { 0.0 } else { s.cdf[k - 1] }).collect();
        assert!(w[5].abs() < 1e-6);
        // Cuts at x = -80, z = -90: the corner (20 x 200 x 10, box 0), the -Z slab (180 x 200 x 10,
        // box 1) and the -X slab (20 x 200 x 190, box 4).
        let total = 200.0f32 * 200.0 * 200.0 - 180.0 * 200.0 * 190.0;
        assert!((w[0] - 20.0 * 200.0 * 10.0 / total).abs() < 1e-5);
        assert!((w[1] - 180.0 * 200.0 * 10.0 / total).abs() < 1e-5);
        assert!((w[4] - 20.0 * 200.0 * 190.0 / total).abs() < 1e-5);
    }

    #[test]
    fn rain_parameter_mapping() {
        let mut p = ParticleParam { name: "MaxParticles".into(), constant: 25.0, min_input: 0.0, max_input: 1.0, min_output: 0.0, max_output: 1.0, mode: 2 };
        assert_eq!(p.value(None), 25.0, "no instance parameter: Constant");
        assert_eq!(p.value(Some(800.0)), 800.0, "DPM_Direct");
        p.mode = 0;
        p.min_input = 0.0;
        p.max_input = 10.0;
        p.min_output = 100.0;
        p.max_output = 200.0;
        assert_eq!(p.value(Some(5.0)), 150.0);
        assert_eq!(p.value(Some(20.0)), 200.0, "clamped above");
        assert_eq!(p.value(Some(-3.0)), 100.0, "clamped below");
        p.mode = 1;
        assert_eq!(p.value(Some(-5.0)), 150.0, "DPM_Abs");
        p.max_input = 0.0;
        assert_eq!(p.value(Some(5.0)), 100.0, "no input range: MinOutput");
        let f = FloatDist { table: None, param: Some(ParticleParam { name: "SpawnKillRate".into(), constant: 1.0, mode: 2, ..Default::default() }) };
        let params = |n: &str| (n.eq_ignore_ascii_case("spawnkillrate")).then_some(30.0);
        assert_eq!(f.value(0.0, 0.5, &params), 30.0);
        let f = FloatDist { table: Some(Dist::constant([8.0; 3], 1)), param: None };
        assert_eq!(f.value(0.0, 0.5, &params), 8.0);
    }

    #[test]
    fn rain_rounding_is_the_engines() {
        assert_eq!(app_ceil(1.2), 2);
        assert_eq!(app_ceil(2.0), 2);
        assert_eq!(app_ceil(0.0), 0);
        assert_eq!(app_ceil(0.01), 1);
        assert_eq!(app_round(2.5), 3);
        assert_eq!(app_round(1.2), 1);
        let mut s = 1u32;
        for _ in 0..100 {
            let r = srand(&mut s);
            assert!((0.0..1.0).contains(&r));
        }
    }

    #[test]
    fn rain_spawn_amounts() {
        // Fill to MaxParticles, capped by Rate x RateScale.
        assert_eq!(drops_spawn_rate(500, 0, 0.0, 0.1, 500.0, 1.0), 500.0);
        assert!((drops_spawn_rate(500, 490, 0.0, 0.1, 500.0, 1.0) - 100.0).abs() < 1e-3);
        assert!(drops_spawn_rate(0, 10, 0.0, 0.1, 500.0, 1.0) < 0.0, "covered: nothing");
        assert_eq!(drops_spawn_rate(500, 0, 0.0, 0.1, 500.0, -1.0), 0.0);
        // Impacts: the available ones in proportion of this emitter's last request.
        let mut cam = RainCamera { available: 4, last_requested: 8, ..Default::default() };
        let (n, acc, req) = impacts_spawn_amount(8.4, 30.0, 0.1, &mut cam);
        assert_eq!(n, 4);
        assert!((acc - 3.4).abs() < 1e-4);
        assert_eq!(req, 3);
        assert_eq!((cam.available, cam.last_requested, cam.requested), (0, 0, 3));
    }

    fn drops_module() -> RainDrops {
        RainDrops {
            extent: [100.0; 3],
            max_particles: FloatDist { table: None, param: Some(ParticleParam { name: "MaxParticles".into(), constant: 25.0, mode: 2, ..Default::default() }) },
            spawn_kill_rate: FloatDist { table: None, param: Some(ParticleParam { name: "SpawnKillRate".into(), constant: 25.0, mode: 2, ..Default::default() }) },
            start_velocity: Some(Dist::constant([0.0, 0.0, -1000.0], 3)),
            start_alpha: Some(Dist::constant([0.5; 3], 1)),
            fading_rate: Some(Dist::constant([8.0; 3], 1)),
            first_module: true,
            ..Default::default()
        }
    }

    #[test]
    fn rain_drops_fill_the_box_then_respawn_where_it_moved() {
        let em = ParticleEmitterAsset { modules: vec![ParticleModule::RainDrops(drops_module())], rate: Some(Dist::constant([500.0; 3], 1)), ..Default::default() };
        let mut sim = EmitterSim::default();
        let mut st = RainEmitterState::default();
        let mut cam = RainCamera::default();
        let mut seed = 7u64;
        let mut frand = || crate::present_rules::next_rand(&mut seed);
        let params = |n: &str| n.eq_ignore_ascii_case("maxparticles").then_some(40.0);
        let mut xf = Mat4::IDENTITY;
        for _ in 0..30 {
            step_emitter(&em, &mut sim, &mut st, &xf, 1.0 / 30.0, &mut frand, &params, &mut cam);
        }
        assert_eq!(sim.particles.len(), 40, "filled to MaxParticles");
        assert_eq!(st.drops.len(), 40);
        // Falling at 1000 uu/s through a 200 uu box: drops leave, fade out, die and come back.
        let mut respawned_above = false;
        for _ in 0..60 {
            xf.w_axis.x += 10.0;
            step_emitter(&em, &mut sim, &mut st, &xf, 1.0 / 30.0, &mut frand, &params, &mut cam);
            let c = xf.w_axis.truncate();
            respawned_above |= sim.particles.iter().zip(&st.drops).any(|(p, d)| d.rt > 0.0 && p.pos.z > c.z + 50.0);
        }
        assert_eq!(sim.particles.len(), 40, "dead drops stay in the pool");
        assert!(respawned_above, "dead drops respawn in the slab the box uncovered (above)");
        // Covered: MaxParticles 0 stops the respawns, the drops fade away.
        let covered = |n: &str| n.eq_ignore_ascii_case("maxparticles").then_some(0.0);
        for _ in 0..60 {
            step_emitter(&em, &mut sim, &mut st, &xf, 1.0 / 30.0, &mut frand, &covered, &mut cam);
        }
        assert!(st.drops.iter().all(|d| d.base_alpha <= 0.0));
        assert!(sim.particles.iter().all(|p| p.alpha <= 0.0));
    }
}
