//! Particle effects (presentation only, never in the simulation): the level's placed `Emitter`
//! actors and gameplay effects (impacts from the contact system, sword sparks, pistol muzzle
//! blast, Blink and Windblast), simulated on the CPU from the decoded `ParticleSystem`s
//! (`dishonored_assets::particles`) and drawn as camera-facing sprites (SubUV, colour and alpha
//! over life, size over life, rotation) with their material's texture and blend mode.
//! research/dishonored/EFFECTS.md documents the module semantics and what is approximated.

use crate::coords;
use crate::level::LevelOf;
use crate::map_render::to_image;
use crate::session::SessionView;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use dishonored_ir::particles::{ParticleBlend, ParticleEmitterAsset, ParticleSystemAsset};
use dishonored_ir::DishonoredMap;
use dishonored_sim::events::{DamageCause, SimEvent};
use std::collections::HashMap;
use std::sync::Arc;

/// ParticleSystems the gameplay effects use (decoded with every level, from DishonoredGame).
pub const GAMEPLAY_PARTICLES: &[&str] = &[
    "Vfx_PhysMat.pistol.Ps_Pistol_Blast",
    "Vfx_Weapon.Effects.Ps_SwordSparks_Parry",
    "Vfx_PhysMat.Blood.Pfx_PistolBlood_Blast",
    "Vfx_PhysMat.Blood.Pfx_BloodSProjectile_Default",
    "Vfx_GamePlay.Blink.Blink_Ground_01",
    "Vfx_GamePlay.Blink.Blink_Fall_01",
    "Vfx_GamePlay.Windblast.Windblast_lvl1",
    "Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt",
    "Vfx_PhysMat.pistol.Ps_PistolImpact_MetalPs_PistolImpact_Dirt",
    "Vfx_PhysMat.pistol.Ps_PistolImpact_WoodPs_PistolImpact_Dirt",
    "Vfx_PhysMat.Wood.Pfx_wooden_shard",
    "Vfx_PhysMat.Water.Pfx_WaterSplash",
];

const MAX_PARTICLES: usize = 400;
/// Placed emitters are simulated only within this distance of the camera (uu).
const SIM_RANGE: f32 = 9000.0;

/// Per-level decoded systems with their GPU materials.
struct LevelFx {
    systems: Vec<ParticleSystemAsset>,
    by_path: HashMap<String, usize>,
    /// (system, emitter) -> material handle.
    materials: HashMap<(usize, usize), Handle<StandardMaterial>>,
}

#[derive(Resource, Default)]
pub struct Effects {
    levels: HashMap<String, Arc<LevelFx>>,
    active: String,
    rng: u64,
    pub spawned_oneshots: u64,
    pub live_particles: usize,
}

#[derive(Clone, Copy, Default)]
struct Particle {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    life: f32,
    size: Vec3,
    rot: f32,
    rot_rate: f32,
    color: Vec3,
    alpha: f32,
    r: [f32; 4],
}

#[derive(Default)]
struct EmitterSim {
    time: f32,
    spawn_acc: f32,
    loops: i32,
    done: bool,
    particles: Vec<Particle>,
    last_burst_t: f32,
}

/// One particle system instance (placed or gameplay); children carry the sprite meshes.
#[derive(Component)]
pub struct FxInstance {
    level: String,
    system: usize,
    /// Dishonored world transform.
    transform: Mat4,
    sims: Vec<EmitterSim>,
    meshes: Vec<Handle<Mesh>>,
    one_shot: bool,
    warmed: bool,
}

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Effects>().add_systems(Update, (gameplay_effects.before(crate::audio::drive), simulate).chain());
    }
}

impl Effects {
    /// The level shown after travel (gameplay effects use its decoded systems).
    pub fn set_active(&mut self, level: &str) {
        if self.levels.contains_key(level) {
            self.active = level.to_string();
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) | 0;
        ((self.rng >> 40) as f32) / (1u64 << 24) as f32
    }
}

fn material_for(map: &DishonoredMap, em: &ParticleEmitterAsset, images: &mut Assets<Image>, cache: &mut HashMap<usize, Handle<Image>>, mats: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
    let tex = em.material.and_then(|m| {
        let ma = map.materials.get(m)?;
        ma.diffuse.or_else(|| ma.texture_params.first().map(|t| t.1)).or_else(|| map.material_params.get(m).and_then(|p| p.opacity.or(p.emissive)))
    });
    let image = tex.and_then(|t| {
        if let Some(h) = cache.get(&t) {
            return Some(h.clone());
        }
        let h = images.add(to_image(map.textures.get(t)?));
        cache.insert(t, h.clone());
        Some(h)
    });
    let alpha_mode = match em.blend {
        ParticleBlend::Additive => AlphaMode::Add,
        ParticleBlend::Modulate => AlphaMode::Multiply,
        ParticleBlend::Masked => AlphaMode::Mask(0.33),
        ParticleBlend::Opaque => AlphaMode::Opaque,
        ParticleBlend::Translucent => AlphaMode::Blend,
    };
    mats.add(StandardMaterial { base_color_texture: image, unlit: true, alpha_mode, cull_mode: None, double_sided: true, fog_enabled: true, ..default() })
}

/// Register a level's particle systems and spawn its placed emitters (called on level spawn).
pub fn spawn_level_effects(name: &str, map: &DishonoredMap, commands: &mut Commands, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>, fx: &mut Effects) -> usize {
    let mut cache = HashMap::new();
    let mut materials = HashMap::new();
    for (si, s) in map.particle_systems.iter().enumerate() {
        for (ei, e) in s.emitters.iter().enumerate() {
            materials.insert((si, ei), material_for(map, e, images, &mut cache, mats));
        }
    }
    let lf = Arc::new(LevelFx {
        by_path: map.particle_systems.iter().enumerate().map(|(i, s)| (s.source.to_ascii_lowercase(), i)).collect(),
        systems: map.particle_systems.clone(),
        materials,
    });
    fx.levels.insert(name.to_string(), lf.clone());
    fx.active = name.to_string();
    let mut n = 0;
    if std::env::var_os("DISH_FX_LIST").is_some() {
        for e in &map.emitters {
            let s = &map.particle_systems[e.system];
            let unsupported: Vec<&String> = s.emitters.iter().flat_map(|x| x.unsupported.iter()).collect();
            info!("fx emitter {} {} at {:.0} ({} emitters, unsupported {:?})", e.source, s.source, e.position(), s.emitters.len(), unsupported);
        }
    }
    for e in map.emitters.iter().filter(|e| e.auto_activate) {
        spawn_instance(commands, meshes, &lf, name, e.system, e.transform, false);
        n += 1;
    }
    info!("effects: {name}: {} particle systems, {n} placed emitters", map.particle_systems.len());
    n
}

fn spawn_instance(commands: &mut Commands, meshes: &mut Assets<Mesh>, lf: &LevelFx, level: &str, system: usize, transform: Mat4, one_shot: bool) {
    let Some(s) = lf.systems.get(system) else { return };
    let mut handles = Vec::new();
    let root = commands
        .spawn((Transform::IDENTITY, Visibility::Inherited, LevelOf(level.to_string())))
        .id();
    for (ei, _) in s.emitters.iter().enumerate() {
        let mesh = meshes.add(empty_mesh());
        handles.push(mesh.clone());
        let Some(mat) = lf.materials.get(&(system, ei)) else { continue };
        commands.spawn((Mesh3d(mesh), MeshMaterial3d(mat.clone()), Transform::IDENTITY, bevy::light::NotShadowCaster, bevy::camera::visibility::NoFrustumCulling, ChildOf(root)));
    }
    commands.entity(root).insert(FxInstance {
        level: level.to_string(),
        system,
        transform,
        sims: s.emitters.iter().map(|_| EmitterSim::default()).collect(),
        meshes: handles,
        one_shot,
        warmed: one_shot,
    });
}

fn empty_mesh() -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new());
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, Vec::<[f32; 3]>::new());
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, Vec::<[f32; 2]>::new());
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, Vec::<[f32; 4]>::new());
    m.insert_indices(Indices::U32(Vec::new()));
    m
}

/// Spawn a gameplay effect by ParticleSystem path at a Dishonored position and direction.
pub fn spawn_effect(commands: &mut Commands, meshes: &mut Assets<Mesh>, fx: &mut Effects, path: &str, at: Vec3, dir: Vec3) -> bool {
    let Some(lf) = fx.levels.get(&fx.active).cloned() else { return false };
    let Some(&si) = lf.by_path.get(&path.to_ascii_lowercase()) else { return false };
    // Emitter X axis along the direction (UE3 effects are authored facing +X).
    let x = if dir.length_squared() > 1e-6 { dir.normalize() } else { Vec3::X };
    let z = if x.z.abs() > 0.99 { Vec3::X } else { Vec3::Z };
    let y = z.cross(x).normalize();
    let z = x.cross(y);
    let m = Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), at.extend(1.0));
    let level = fx.active.clone();
    spawn_instance(commands, meshes, &lf, &level, si, m, true);
    fx.spawned_oneshots += 1;
    true
}

fn d3(d: &Option<dishonored_ir::particles::Dist>, t: f32, r: f32, def: [f32; 3]) -> Vec3 {
    Vec3::from(d.as_ref().map(|d| d.eval(t, r)).unwrap_or(def))
}

fn spawn_particle(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, rnd: &mut dyn FnMut() -> f32) {
    if sim.particles.len() >= MAX_PARTICLES {
        return;
    }
    let et = sim.time;
    let r = [rnd(), rnd(), rnd(), rnd()];
    let life = em.lifetime.as_ref().map(|d| d.eval1(et, r[0])).unwrap_or(1.0).max(0.01);
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
    sim.particles.push(Particle {
        pos,
        vel,
        age: 0.0,
        life,
        size,
        rot: em.rotation.as_ref().map(|d| d.eval1(et, r[3])).unwrap_or(0.0) * std::f32::consts::TAU,
        rot_rate: em.rotation_rate.as_ref().map(|d| d.eval1(et, r[2])).unwrap_or(0.0) * std::f32::consts::TAU,
        color,
        alpha,
        r,
    });
}

/// Advance one emitter by `dt`.
fn step(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, dt: f32, rnd: &mut dyn FnMut() -> f32) {
    // Particles.
    let accel = |p: &Particle| d3(&em.acceleration, p.age / p.life, p.r[3], [0.0; 3]);
    for p in sim.particles.iter_mut() {
        p.age += dt;
        let a = accel(p);
        let a = if em.local_space { a } else { xf.transform_vector3(a) };
        p.vel += a * dt;
        if let Some(d) = &em.drag {
            p.vel *= (1.0 - d.eval1(p.age / p.life, p.r[2]) * dt).max(0.0);
        }
        p.pos += p.vel * dt;
        p.rot += p.rot_rate * dt;
    }
    sim.particles.retain(|p| p.age < p.life);
    if sim.done {
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
    if t >= dur * (sim.loops + 1) as f32 {
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

/// Write the sprite quads of one emitter into its mesh (Bevy space, facing the camera).
fn build_mesh(em: &ParticleEmitterAsset, sim: &EmitterSim, xf: &Mat4, cam_right: Vec3, cam_up: Vec3, mesh: &mut Mesh) {
    let n = sim.particles.len();
    let mut pos = Vec::with_capacity(n * 4);
    let mut uv = Vec::with_capacity(n * 4);
    let mut col = Vec::with_capacity(n * 4);
    let mut nrm = Vec::with_capacity(n * 4);
    let mut idx = Vec::with_capacity(n * 6);
    let frames = (em.sub_h * em.sub_v).max(1);
    for p in &sim.particles {
        let rel = (p.age / p.life).clamp(0.0, 1.0);
        let mut size = p.size;
        if let Some(d) = &em.size_over_life {
            let m = d.eval(rel, p.r[1]);
            for k in 0..3 {
                if em.size_over_life_axes[k] {
                    size[k] *= m[k];
                }
            }
        }
        let color = em.color_over_life.as_ref().map(|d| Vec3::from(d.eval(rel, p.r[0]))).unwrap_or(p.color);
        let alpha = em.alpha_over_life.as_ref().map(|d| d.eval1(rel, p.r[0])).unwrap_or(p.alpha);
        let world = if em.local_space { xf.transform_point3(p.pos) } else { p.pos };
        let c = coords::pos(world);
        let (hw, hh) = (size.x * 0.5 * coords::UU_TO_BEVY, if em.screen_align == 1 { size.y } else { size.x } * 0.5 * coords::UU_TO_BEVY);
        let (s, co) = p.rot.sin_cos();
        let (rx, ry) = (cam_right * co + cam_up * s, cam_up * co - cam_right * s);
        let (rx, ry) = if em.screen_align == 2 && p.vel.length_squared() > 1e-3 {
            // Velocity aligned: the sprite's up axis follows the velocity.
            let v = coords::dir(p.vel).normalize();
            let side = v.cross(cam_right.cross(cam_up)).normalize_or_zero();
            (side, v)
        } else {
            (rx, ry)
        };
        let base = pos.len() as u32;
        for (ox, oy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            pos.push((c + rx * hw * ox + ry * hh * oy).to_array());
            nrm.push([0.0, 1.0, 0.0]);
            col.push([color.x.max(0.0), color.y.max(0.0), color.z.max(0.0), alpha.clamp(0.0, 1.0)]);
        }
        let f = match &em.sub_image {
            Some(d) => (d.eval1(rel, p.r[2]).floor().max(0.0) as u32).min(frames - 1),
            None => 0,
        };
        let (fx, fy) = ((f % em.sub_h) as f32, (f / em.sub_h) as f32);
        let (w, h) = (1.0 / em.sub_h as f32, 1.0 / em.sub_v as f32);
        for (u, v) in [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)] {
            uv.push([(fx + u) * w, (fy + v) * h]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    mesh.insert_indices(Indices::U32(idx));
}

fn simulate(
    mut commands: Commands,
    mut fx: ResMut<Effects>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut q: Query<(Entity, &mut FxInstance, &InheritedVisibility)>,
    cams: Query<&GlobalTransform, With<Camera3d>>,
) {
    let Some(cam) = cams.iter().next() else { return };
    let cam_pos_b = cam.translation();
    let cam_uu = Vec3::new(cam_pos_b.x, cam_pos_b.z, cam_pos_b.y) / coords::UU_TO_BEVY;
    let (right, up) = (cam.right().as_vec3(), cam.up().as_vec3());
    let dt = time.delta_secs().min(0.1);
    let mut live = 0;
    for (e, mut inst, vis) in &mut q {
        if !vis.get() && !inst.one_shot {
            continue;
        }
        let Some(lf) = fx.levels.get(&inst.level).cloned() else { continue };
        let Some(sys) = lf.systems.get(inst.system) else { continue };
        let xf = inst.transform;
        if !inst.one_shot && xf.w_axis.truncate().distance(cam_uu) > SIM_RANGE {
            continue;
        }
        let inst = &mut *inst;
        let mut rnd = || fx.rand();
        if !inst.warmed {
            inst.warmed = true;
            let steps = (sys.warmup * 15.0).min(300.0) as usize;
            for _ in 0..steps {
                for (em, sim) in sys.emitters.iter().zip(inst.sims.iter_mut()) {
                    step(em, sim, &xf, 1.0 / 15.0, &mut rnd);
                }
            }
        }
        for (i, (em, sim)) in sys.emitters.iter().zip(inst.sims.iter_mut()).enumerate() {
            step(em, sim, &xf, dt, &mut rnd);
            live += sim.particles.len();
            if let Some(mut m) = inst.meshes.get(i).and_then(|h| meshes.get_mut(h)) {
                build_mesh(em, sim, &xf, right, up, &mut m);
            }
        }
        if inst.one_shot && inst.sims.iter().all(|s| (s.done || s.time > 10.0) && s.particles.is_empty()) {
            commands.entity(e).despawn();
        }
    }
    fx.live_particles = live;
}

/// Gameplay effects from simulation events (sparks, blood, muzzle blast, Blink, Windblast).
fn gameplay_effects(mut commands: Commands, mut fx: ResMut<Effects>, mut meshes: ResMut<Assets<Mesh>>, view: Option<ResMut<SessionView>>, audio: Option<Res<crate::audio::GameAudio>>) {
    let Some(mut view) = view else { return };
    let events = view.audio_events.clone();
    if audio.is_none() {
        // Without the audio plugin nobody else drains the presentation events.
        view.audio_events.clear();
    }

    let find = |id: u32| view.me.iter().chain(view.others.iter()).find(|p| p.id == id).copied();
    let fwd = |yaw: f32| Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
    for ev in events.iter() {
        let (path, at, dir): (&str, Vec3, Vec3) = match ev {
            SimEvent::SwordBlocked { defender, .. } | SimEvent::SwordParried { defender, .. } => {
                let Some(p) = find(*defender) else { continue };
                ("Vfx_Weapon.Effects.Ps_SwordSparks_Parry", p.position + Vec3::Z * 40.0 + fwd(p.yaw) * 40.0, fwd(p.yaw))
            }
            SimEvent::Fired { id, bolt: false, .. } => {
                let Some(p) = find(*id) else { continue };
                ("Vfx_PhysMat.pistol.Ps_Pistol_Blast", p.position + Vec3::Z * 60.0 + fwd(p.yaw) * 50.0, fwd(p.yaw))
            }
            SimEvent::Damage { victim, cause, .. } => {
                let Some(p) = find(*victim) else { continue };
                let path = match cause {
                    DamageCause::Pistol | DamageCause::PistolHeadshot => "Vfx_PhysMat.Blood.Pfx_PistolBlood_Blast",
                    DamageCause::Sword | DamageCause::SwordCounter | DamageCause::Assassination | DamageCause::DropAssassination | DamageCause::Crossbow | DamageCause::CrossbowHeadshot => {
                        "Vfx_PhysMat.Blood.Pfx_BloodSProjectile_Default"
                    }
                    _ => continue,
                };
                (path, p.position + Vec3::Z * 50.0, -fwd(p.yaw))
            }
            SimEvent::BlinkEnd { id } => {
                let Some(p) = find(*id) else { continue };
                ("Vfx_GamePlay.Blink.Blink_Ground_01", p.position - Vec3::Z * 80.0, Vec3::Z)
            }
            SimEvent::WindblastCast { id } => {
                let Some(p) = find(*id) else { continue };
                ("Vfx_GamePlay.Windblast.Windblast_lvl1", p.position + Vec3::Z * 60.0, fwd(p.yaw))
            }
            SimEvent::ProjectileImpact { point, victim: None, .. } => ("Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt", *point, Vec3::Z),
            _ => continue,
        };
        spawn_effect(&mut commands, &mut meshes, &mut fx, path, at, dir);
    }
}
