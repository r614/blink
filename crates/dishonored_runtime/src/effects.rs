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
use dishonored_sim::state::{MoveMode, PlayerState};
use dishonored_world::WorldQuery;
use std::collections::HashMap;
use std::sync::Arc;

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

/// Sockets of the animated pawns the gameplay effects attach to (written by the character
/// animation each frame, Dishonored world space).
#[derive(Resource, Default)]
pub struct EffectSockets {
    /// Pistol `Spitfire` socket: (position, firing direction).
    pub muzzle: HashMap<u32, (Vec3, Vec3)>,
    /// `SHADOW_KILL_BONES` positions.
    pub bones: HashMap<u32, Vec<Vec3>>,
}

/// Procedural glow of untextured particle mesh materials (`volume_glow.wgsl`).
#[derive(Asset, bevy::render::render_resource::AsBindGroup, Reflect, Debug, Clone)]
pub struct GlowMaterial {
    /// Linear colour x intensity (premultiplied by the particle colour and alpha).
    #[uniform(0)]
    pub color: Vec4,
    /// x = Fresnel_exponent, y = StartOffset camera fade (Bevy units), z = SphereMask_Power,
    /// w = Distance (sphere mask radius in mesh radii).
    #[uniform(1)]
    pub params: Vec4,
    /// Sphere mask centre (Bevy world) and radius.
    #[uniform(2)]
    pub sphere: Vec4,
}

impl Material for GlowMaterial {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/volume_glow.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
}

/// ParticleSystems the gameplay effects use (decoded with every level, from DishonoredGame).
pub const GAMEPLAY_PARTICLES: &[&str] = &[
    PLAYER_MUZZLE,
    PAWN_MUZZLE,
    SHADOW_KILL_PS,
    "Vfx_GamePlay.Blink.Blink_Mantle_01",
    "Vfx_GamePlay.Windblast.Windblast_lvl2",
    "Vfx_GamePlay.Windblast.Windblast_UndWater_lvl1",
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
    /// Mesh emitters: (system, emitter) -> (mesh, material).
    mesh_parts: HashMap<(usize, usize), (Handle<Mesh>, Handle<StandardMaterial>)>,
    /// Emitters not drawn (untextured mesh emitters).
    skipped: std::collections::HashSet<(usize, usize)>,
    /// Untextured (procedural) mesh emitters: mesh, colour x intensity, glow parameters.
    glow: HashMap<(usize, usize), (Handle<Mesh>, Vec4, Vec4)>,
}

/// One particle of a mesh emitter.
#[derive(Component)]
struct FxMeshParticle;

/// A static mesh (Dishonored mesh space) as a Bevy mesh in converted space.
fn bevy_mesh(m: &dishonored_ir::StaticMeshAsset) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.iter().map(|p| coords::pos(*p).to_array()).collect::<Vec<_>>());
    let normals: Vec<[f32; 3]> = if m.normals.len() == m.positions.len() { m.normals.iter().map(|n| coords::dir(*n).to_array()).collect() } else { vec![[0.0, 1.0, 0.0]; m.positions.len()] };
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    let uv: Vec<[f32; 2]> = if m.uv0.len() == m.positions.len() { m.uv0.iter().map(|u| u.to_array()).collect() } else { vec![[0.0; 2]; m.positions.len()] };
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    // Handedness flips with the axis swap: reverse the winding.
    let idx: Vec<u32> = m.indices.chunks_exact(3).flat_map(|t| [t[0], t[2], t[1]]).collect();
    mesh.insert_indices(Indices::U32(idx));
    if m.normals.len() != m.positions.len() {
        // No imported normals: smooth ones from the triangles (the glow shading needs them).
        mesh.compute_smooth_normals();
    }
    mesh
}

#[derive(Resource, Default)]
pub struct Effects {
    levels: HashMap<String, Arc<LevelFx>>,
    active: String,
    rng: u64,
    pub spawned_oneshots: u64,
    /// The last gameplay effects asked for: (system path, spawned) (harness `effects` command).
    pub recent: Vec<(String, bool)>,
    pub live_particles: usize,
    /// Effects due later: (time, system, position, direction).
    delayed: Vec<(f32, String, Vec3, Vec3)>,
    /// Projectile id -> fired as a crossbow bolt (impact striker).
    bolts: HashMap<u32, bool>,
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
    /// Mesh particles: rotation (radians about X, Y, Z) and its rate.
    mrot: Vec3,
    mrot_rate: Vec3,
    /// UE3 `BaseVelocity`: accelerations integrate into it; per-frame modules (velocity over
    /// life, attractors) derive `vel` from it.
    base_vel: Vec3,
    /// Orbit (summed over the orbit modules): offset, rotation and rotation rate (radians).
    orbit_off: Vec3,
    orbit_rot: Vec3,
    orbit_rate: Vec3,
    /// Collisions so far, the maximum, and frozen / no-longer-colliding flags.
    collisions: u32,
    max_collisions: u32,
    frozen: bool,
    halted: bool,
}

/// World ray query for particle collision: (from, to) -> (point, normal).
type RayFn<'a> = &'a dyn Fn(Vec3, Vec3) -> Option<(Vec3, Vec3)>;

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
    /// Mesh emitters: one entity per live particle.
    pools: Vec<Vec<(Entity, Handle<StandardMaterial>, LinearRgba)>>,
    /// Glow mesh emitters: one entity and material per live particle.
    glow_pools: Vec<Vec<(Entity, Handle<GlowMaterial>)>>,
    one_shot: bool,
    warmed: bool,
}

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "volume_glow.wgsl");
        app.add_plugins(MaterialPlugin::<GlowMaterial>::default());
        app.init_resource::<Effects>().init_resource::<EffectSockets>().add_systems(Update, (gameplay_effects.before(crate::audio::drive), simulate).chain());
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
    // The material graphs are not evaluated (APPROXIMATION, EFFECTS.md): the instance's
    // `Color` vector tints, its intensity/opacity scalars scale, and procedural glow materials
    // (sphere masks, no texture) get a radial falloff.
    let params = em.material.and_then(|m| map.material_params.get(m));
    let tint = params.and_then(|p| p.vectors.iter().find(|v| v.0.eq_ignore_ascii_case("Color")).map(|v| v.1)).unwrap_or([1.0; 4]);
    let gain: f32 = params
        .map(|p| p.scalars.iter().filter(|s| ["LightIntensity", "Opacity", "Intensity", "EmissiveIntensity", "Emissive_Intensity"].iter().any(|n| s.0.eq_ignore_ascii_case(n))).map(|s| s.1).product())
        .unwrap_or(1.0);
    let image = image.or_else(|| {
        let power = params.and_then(|p| p.scalars.iter().find(|s| s.0.eq_ignore_ascii_case("SphereMask_Power")).map(|s| s.1)).unwrap_or(2.0);
        let key = usize::MAX - (power * 100.0) as usize;
        if let Some(h) = cache.get(&key) {
            return Some(h.clone());
        }
        let h = images.add(radial_falloff(power));
        cache.insert(key, h.clone());
        Some(h)
    });
    // Emissive bug / glow materials (`E_Emissive_Color`, alpha = intensity: fireflies x25):
    // drawn additively with the emissive colour (unlit, so it reads as the glowing body).
    let emissive = params.and_then(|p| p.vectors.iter().find(|v| v.0.eq_ignore_ascii_case("E_Emissive_Color")).map(|v| v.1)).filter(|e| e[3] > 1.0);
    let (alpha_mode, tint, gain) = match emissive {
        Some(e) => (AlphaMode::Add, [e[0], e[1], e[2], 1.0], e[3].min(4.0)),
        None => (alpha_mode, tint, gain),
    };
    let base_color = if alpha_mode == AlphaMode::Add {
        Color::linear_rgb(tint[0] * gain, tint[1] * gain, tint[2] * gain)
    } else {
        Color::linear_rgba(tint[0], tint[1], tint[2], gain.clamp(0.0, 1.0))
    };
    mats.add(StandardMaterial { base_color, base_color_texture: image, unlit: true, alpha_mode, cull_mode: None, double_sided: true, fog_enabled: alpha_mode != AlphaMode::Add, ..default() })
}

/// A white disc fading to transparent at its edge (`(1 - r)^power`), for procedural glows.
fn radial_falloff(power: f32) -> Image {
    let n = 64usize;
    let mut data = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = ((x as f32 + 0.5) / n as f32 * 2.0 - 1.0, (y as f32 + 0.5) / n as f32 * 2.0 - 1.0);
            let a = (1.0 - (u * u + v * v).sqrt()).clamp(0.0, 1.0).powf(power.max(0.1));
            let b = (a * 255.0) as u8;
            data.extend_from_slice(&[255, 255, 255, b]);
        }
    }
    Image::new(
        bevy::render::render_resource::Extent3d { width: n as u32, height: n as u32, depth_or_array_layers: 1 },
        bevy::render::render_resource::TextureDimension::D2,
        data,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
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
    // Mesh emitters: the mesh with its own (first section) material unless overridden.
    let mut mesh_parts = HashMap::new();
    let mut mesh_handles: HashMap<usize, Handle<Mesh>> = HashMap::new();
    let mut skipped = std::collections::HashSet::new();
    let mut glow = HashMap::new();
    for (si, s) in map.particle_systems.iter().enumerate() {
        for (ei, e) in s.emitters.iter().enumerate() {
            // Distortion-only materials (`Dist_Enable_Distortion` with `D_Disable_Diffuse`, e.g.
            // the pistol muzzle's `WindWave_Disto_01_inst`) draw nothing in the colour pass; UE3
            // renders them in its distortion (refraction) pass, which the port does not have.
            let params = e.material.and_then(|m| map.material_params.get(m));
            if params.is_some_and(|p| p.switch("Dist_Enable_Distortion") == Some(true) && p.switch("D_Disable_Diffuse") == Some(true)) {
                skipped.insert((si, ei));
                continue;
            }
            let Some(mid) = e.mesh_id.filter(|m| *m < map.meshes.len()) else {
                if e.mesh.is_some() {
                    skipped.insert((si, ei));
                }
                continue;
            };
            // Volumetric glow meshes with procedural materials (no texture; the cooked graph keeps
            // only its parameters): the glow material rebuilt from those parameters.
            let eff = if e.mesh_override_material && e.material.is_some() { e.material } else { map.meshes[mid].sections.first().and_then(|s| s.material) };
            let textured = eff.and_then(|m| map.materials.get(m)).is_some_and(|m| m.diffuse.is_some() || !m.texture_params.is_empty());
            if !textured {
                let mh = mesh_handles.entry(mid).or_insert_with(|| meshes.add(bevy_mesh(&map.meshes[mid]))).clone();
                let params = eff.and_then(|m| map.material_params.get(m));
                let scalar = |n: &str, d: f32| params.and_then(|p| p.scalars.iter().find(|s| s.0.eq_ignore_ascii_case(n)).map(|s| s.1)).unwrap_or(d);
                // Defaults: the parameter expressions of Volume_Light_PMAT (Color, LightIntensity 2,
                // Fresnel_exponent 5, StartOffset 128).
                let c = params.and_then(|p| p.vectors.iter().find(|v| v.0.eq_ignore_ascii_case("Color")).map(|v| v.1)).unwrap_or([1.0, 0.940601, 0.74453, 1.0]);
                // Calibration (APPROXIMATION, U-305): the stripped graph scales LightIntensity by
                // terms that are not recoverable (rays mask, depth-biased alpha); 0.1 keeps the
                // stacked cones from saturating.
                let k = scalar("LightIntensity", 2.0) * 0.1;
                glow.insert((si, ei), (mh, Vec4::new(c[0] * k, c[1] * k, c[2] * k, 1.0), Vec4::new(scalar("Fresnel_exponent", scalar("G_GlowPower", 5.0)), scalar("StartOffset", 128.0) * coords::UU_TO_BEVY, scalar("SphereMask_Power", 2.0), scalar("Distance", 1.5) * map.meshes[mid].positions.iter().map(|p| p.length()).fold(0.0, f32::max))));
                continue;
            }
            let mh = mesh_handles.entry(mid).or_insert_with(|| meshes.add(bevy_mesh(&map.meshes[mid]))).clone();
            let mat = if e.mesh_override_material && e.material.is_some() {
                materials[&(si, ei)].clone()
            } else {
                let mut e2 = e.clone();
                e2.material = map.meshes[mid].sections.first().and_then(|s| s.material);
                e2.blend = match e2.material.and_then(|m| map.material_params.get(m)).map(|p| p.blend) {
                    Some(dishonored_ir::MaterialBlend::Additive) => ParticleBlend::Additive,
                    Some(dishonored_ir::MaterialBlend::Opaque) => ParticleBlend::Opaque,
                    Some(dishonored_ir::MaterialBlend::Masked) => ParticleBlend::Masked,
                    Some(dishonored_ir::MaterialBlend::Modulate) => ParticleBlend::Modulate,
                    _ => ParticleBlend::Translucent,
                };
                material_for(map, &e2, images, &mut cache, mats)
            };
            mesh_parts.insert((si, ei), (mh, mat));
        }
    }
    let lf = Arc::new(LevelFx {
        by_path: map.particle_systems.iter().enumerate().map(|(i, s)| (s.source.to_ascii_lowercase(), i)).collect(),
        systems: map.particle_systems.clone(),
        materials,
        mesh_parts,
        skipped,
        glow,
    });
    fx.levels.insert(name.to_string(), lf.clone());
    fx.active = name.to_string();
    let mut n = 0;
    if let Ok(want) = std::env::var("DISH_FX_DUMP") {
        for s in map.particle_systems.iter().filter(|s| s.source.to_ascii_lowercase().contains(&want.to_ascii_lowercase())) {
            info!("fx system {s:?}");
            for e in &s.emitters {
                if let Some(m) = e.material {
                    info!("fx material {} {:?} {:?}", e.name, map.materials.get(m).map(|x| (&x.source, x.diffuse, &x.texture_params)), map.material_params.get(m).map(|p| (&p.scalars, &p.vectors, p.blend, &p.base_material)));
                }
            }
        }
    }
    if std::env::var_os("DISH_FX_LIST").is_some() {
        for e in &map.emitters {
            let s = &map.particle_systems[e.system];
            let unsupported: Vec<&String> = s.emitters.iter().flat_map(|x| x.unsupported.iter()).collect();
            info!("fx emitter {} {} at {:.0} ({} emitters, unsupported {:?})", e.source, s.source, e.position(), s.emitters.len(), unsupported);
        }
    }
    // `--no-fx` (render comparisons) also leaves the particle effects out.
    let off = std::env::args().any(|a| a == "--no-fx") || std::env::var_os("DISH_NO_PARTICLES").is_some();
    for e in map.emitters.iter().filter(|e| e.auto_activate && !off) {
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
        if lf.mesh_parts.contains_key(&(system, ei)) || lf.skipped.contains(&(system, ei)) || lf.glow.contains_key(&(system, ei)) {
            continue;
        }
        let Some(mat) = lf.materials.get(&(system, ei)) else { continue };
        commands.spawn((Mesh3d(mesh), MeshMaterial3d(mat.clone()), Transform::IDENTITY, bevy::light::NotShadowCaster, bevy::camera::visibility::NoFrustumCulling, ChildOf(root)));
    }
    commands.entity(root).insert(FxInstance {
        level: level.to_string(),
        system,
        transform,
        sims: s.emitters.iter().map(|_| EmitterSim::default()).collect(),
        meshes: handles,
        pools: s.emitters.iter().map(|_| Vec::new()).collect(),
        glow_pools: s.emitters.iter().map(|_| Vec::new()).collect(),
        one_shot,
        warmed: one_shot,
    });
}

fn empty_mesh() -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32; 3]; 3]);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32; 2]; 3]);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0f32; 4]; 3]);
    m.insert_indices(Indices::U32(vec![0, 1, 2]));
    m
}

/// Spawn a gameplay effect by ParticleSystem path at a Dishonored position and direction.
pub fn spawn_effect(commands: &mut Commands, meshes: &mut Assets<Mesh>, fx: &mut Effects, path: &str, at: Vec3, dir: Vec3) -> bool {
    if fx.recent.len() >= 64 {
        fx.recent.remove(0);
    }
    fx.recent.push((path.to_string(), false));
    let Some(lf) = fx.levels.get(&fx.active).cloned() else { return false };
    let Some(&si) = lf.by_path.get(&path.to_ascii_lowercase()) else {
        warn!("effects: {path} not decoded for {}", fx.active);
        return false;
    };
    info!("effects: {path} at {at:.0}");
    // Emitter X axis along the direction (UE3 effects are authored facing +X).
    let x = if dir.length_squared() > 1e-6 { dir.normalize() } else { Vec3::X };
    let z = if x.z.abs() > 0.99 { Vec3::X } else { Vec3::Z };
    let y = z.cross(x).normalize();
    let z = x.cross(y);
    let m = Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), at.extend(1.0));
    let level = fx.active.clone();
    spawn_instance(commands, meshes, &lf, &level, si, m, true);
    fx.spawned_oneshots += 1;
    if let Some(r) = fx.recent.last_mut() {
        r.1 = true;
    }
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
fn step(em: &ParticleEmitterAsset, sim: &mut EmitterSim, xf: &Mat4, dt: f32, rnd: &mut dyn FnMut() -> f32, ray: Option<RayFn>) {
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
fn build_mesh(em: &ParticleEmitterAsset, sim: &EmitterSim, xf: &Mat4, cam_right: Vec3, cam_up: Vec3, cam_pos: Vec3, mesh: &mut Mesh) {
    let n = sim.particles.len();
    let mut pos = Vec::with_capacity(n * 4);
    let mut uv = Vec::with_capacity(n * 4);
    let mut col = Vec::with_capacity(n * 4);
    let mut nrm = Vec::with_capacity(n * 4);
    let mut idx = Vec::with_capacity(n * 6);
    let frames = (em.sub_h * em.sub_v).max(1);
    let look = |p: &Particle| -> (Vec3, f32, Vec3) {
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
        (color, alpha, size)
    };
    // Orbit: an emitter-space offset turned by the orbit rotation, added at render time.
    let world_of = |p: &Particle| -> Vec3 {
        let orb = if p.orbit_off != Vec3::ZERO { Quat::from_euler(EulerRot::ZYX, p.orbit_rot.z, p.orbit_rot.y, p.orbit_rot.x) * p.orbit_off } else { Vec3::ZERO };
        if em.local_space { xf.transform_point3(p.pos + orb) } else { p.pos + xf.transform_vector3(orb) }
    };
    if em.trail.is_some() || em.beam.is_some() {
        strip_mesh(em, sim, xf, cam_pos, &look, &world_of, mesh);
        return;
    }
    for p in &sim.particles {
        let rel = (p.age / p.life).clamp(0.0, 1.0);
        let (color, alpha, size) = look(p);
        let world = world_of(p);
        let c = coords::pos(world);
        let (hw, hh) = (size.x * 0.5 * coords::UU_TO_BEVY, if em.screen_align == 1 { size.y } else { size.x } * 0.5 * coords::UU_TO_BEVY);
        let (s, co) = p.rot.sin_cos();
        // Plane of the sprite: camera facing, or locked to an emitter axis (OrientationAxisLock).
        let axis = |v: glam::Vec4| coords::dir(v.truncate()).normalize_or_zero();
        let (br, bu) = match em.axis_lock {
            1 | 4 => (axis(xf.y_axis), axis(xf.z_axis)),
            2 | 5 => (axis(xf.x_axis), axis(xf.z_axis)),
            3 | 6 => (axis(xf.x_axis), axis(xf.y_axis)),
            7..=9 => {
                let up = axis([xf.x_axis, xf.y_axis, xf.z_axis][(em.axis_lock - 7) as usize]);
                (up.cross(cam_pos - c).normalize_or_zero(), up)
            }
            _ => (cam_right, cam_up),
        };
        let (rx, ry) = (br * co + bu * s, bu * co - br * s);
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
    if pos.is_empty() {
        // Never upload an empty vertex buffer: one degenerate, transparent triangle.
        pos = vec![[0.0; 3]; 3];
        nrm = vec![[0.0, 1.0, 0.0]; 3];
        uv = vec![[0.0; 2]; 3];
        col = vec![[0.0; 4]; 3];
        idx = vec![0, 1, 2];
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
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut glow_mats: ResMut<Assets<GlowMaterial>>,
    mut q: Query<(Entity, &mut FxInstance, &InheritedVisibility)>,
    cams: Query<&GlobalTransform, With<Camera3d>>,
    session: Option<Res<crate::session::Session>>,
) {
    let Some(cam) = cams.iter().next() else { return };
    let cam_pos_b = cam.translation();
    let cam_uu = Vec3::new(cam_pos_b.x, cam_pos_b.z, cam_pos_b.y) / coords::UU_TO_BEVY;
    let (right, up) = (cam.right().as_vec3(), cam.up().as_vec3());
    let dt = time.delta_secs().min(0.1);
    let world = session.as_ref().map(|s| &s.content.collision);
    let ray_closure = |a: Vec3, b: Vec3| -> Option<(Vec3, Vec3)> {
        let w = world?;
        let d = b - a;
        let len = d.length();
        if len < 1e-4 {
            return None;
        }
        w.raycast(a, d / len, len, dishonored_world::channel::ALL).map(|h| (h.point, h.normal))
    };
    let ray_fn: RayFn = &ray_closure;
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
                    step(em, sim, &xf, 1.0 / 15.0, &mut rnd, em.collision.as_ref().map(|_| ray_fn));
                }
            }
        }
        for (i, (em, sim)) in sys.emitters.iter().zip(inst.sims.iter_mut()).enumerate() {
            step(em, sim, &xf, dt, &mut rnd, em.collision.as_ref().map(|_| ray_fn));
            live += sim.particles.len();
            if let Some((mh, color, params)) = lf.glow.get(&(inst.system, i)) {
                let pool = &mut inst.glow_pools[i];
                let want = sim.particles.len().min(64);
                while pool.len() < want {
                    let gm = glow_mats.add(GlowMaterial { color: *color, params: *params, sphere: Vec4::ZERO });
                    let id = commands.spawn((Mesh3d(mh.clone()), MeshMaterial3d(gm.clone()), Transform::IDENTITY, bevy::light::NotShadowCaster, FxMeshParticle, ChildOf(e))).id();
                    pool.push((id, gm));
                }
                while pool.len() > want {
                    if let Some((id, gm)) = pool.pop() {
                        glow_mats.remove(&gm);
                        commands.entity(id).despawn();
                    }
                }
                for (k, p) in sim.particles.iter().take(want).enumerate() {
                    let (world, c, a) = mesh_particle(em, p, &xf);
                    commands.entity(pool[k].0).insert(Transform::from_matrix(coords::transform(world)));
                    if let Some(mut m) = glow_mats.get_mut(&pool[k].1) {
                        let (s, _, t) = world.to_scale_rotation_translation();
                        m.sphere = coords::pos(t).extend(params.w * s.max_element() * coords::UU_TO_BEVY);
                        m.color = Vec4::new(color.x * c.x * a, color.y * c.y * a, color.z * c.z * a, 1.0);
                    }
                }
            } else if let Some((mh, mat)) = lf.mesh_parts.get(&(inst.system, i)) {
                // Mesh particles: one entity each, scaled by the particle size and turned by
                // its mesh rotation, its colour and alpha on its own material copy.
                let pool = &mut inst.pools[i];
                let want = sim.particles.len().min(64);
                while pool.len() < want {
                    let base = mats.get(mat).cloned().unwrap_or_default();
                    let tintc = base.base_color.to_linear();
                    let pm = mats.add(base);
                    let id = commands.spawn((Mesh3d(mh.clone()), MeshMaterial3d(pm.clone()), Transform::IDENTITY, bevy::light::NotShadowCaster, FxMeshParticle, ChildOf(e))).id();
                    pool.push((id, pm, tintc));
                }
                while pool.len() > want {
                    if let Some((id, pm, _)) = pool.pop() {
                        mats.remove(&pm);
                        commands.entity(id).despawn();
                    }
                }
                for (k, p) in sim.particles.iter().take(want).enumerate() {
                    let rel = (p.age / p.life).clamp(0.0, 1.0);
                    let mut size = p.size;
                    if let Some(d) = &em.size_over_life {
                        let m = d.eval(rel, p.r[1]);
                        for a in 0..3 {
                            if em.size_over_life_axes[a] {
                                size[a] *= m[a];
                            }
                        }
                    }
                    let rot = Mat4::from_rotation_z(p.mrot.z) * Mat4::from_rotation_y(p.mrot.y) * Mat4::from_rotation_x(p.mrot.x);
                    let local = Mat4::from_translation(p.pos) * rot * Mat4::from_scale(size);
                    let world = if em.local_space {
                        xf * local
                    } else {
                        let (_, r, _) = xf.to_scale_rotation_translation();
                        Mat4::from_translation(p.pos) * Mat4::from_quat(r) * rot * Mat4::from_scale(size)
                    };
                    commands.entity(pool[k].0).insert(Transform::from_matrix(coords::transform(world)));
                    // Particle colour and alpha drive the material (ParticleColor in the material).
                    let color = em.color_over_life.as_ref().map(|d| Vec3::from(d.eval(rel, p.r[0]))).unwrap_or(p.color);
                    let alpha = em.alpha_over_life.as_ref().map(|d| d.eval1(rel, p.r[0])).unwrap_or(p.alpha).clamp(0.0, 1.0);
                    if let Some(mut m) = mats.get_mut(&pool[k].1) {
                        m.base_color = if em.blend == ParticleBlend::Additive || m.alpha_mode == AlphaMode::Add {
                            Color::linear_rgb(color.x * alpha * pool[k].2.red, color.y * alpha * pool[k].2.green, color.z * alpha * pool[k].2.blue)
                        } else {
                            Color::linear_rgba(color.x * pool[k].2.red, color.y * pool[k].2.green, color.z * pool[k].2.blue, alpha * pool[k].2.alpha)
                        };
                    }
                }
            } else if let Some(mut m) = inst.meshes.get(i).and_then(|h| meshes.get_mut(h)) {
                build_mesh(em, sim, &xf, right, up, cam_pos_b, &mut m);
            }
        }
        if inst.one_shot && inst.sims.iter().all(|s| (s.done || s.time > 10.0) && s.particles.is_empty()) {
            commands.entity(e).despawn();
        }
    }
    fx.live_particles = live;
}

/// Gameplay effects from simulation events, with the systems and sockets the original's tweaks
/// name (U-165): the pistol muzzle flash at the `Spitfire` socket (`m_MuzzleSocketName`;
/// first-person `Ps_PlayerPistol_StdMuzzle_01`, third-person `Ps_PistolMuzzle`), Blink's
/// `m_pGroundPS` / `m_pMantlePS` / `m_pLowFallPS` at the arrival, Windblast's per-level
/// `m_EffectTemplate` along the view, Shadow Kill ash on the `m_ShadowKillFXs` bones, impacts and
/// blood from the contact system's particle sub-intersections, sword sparks on blocks and parries.
#[allow(clippy::too_many_arguments)]
fn gameplay_effects(
    mut commands: Commands,
    mut fx: ResMut<Effects>,
    mut meshes: ResMut<Assets<Mesh>>,
    view: Option<ResMut<SessionView>>,
    audio: Option<Res<crate::audio::GameAudio>>,
    sockets: Res<EffectSockets>,
    session: Option<Res<crate::session::Session>>,
    time: Res<Time>,
) {
    let Some(mut view) = view else { return };
    let events = view.audio_events.clone();
    if audio.is_none() {
        // Without the audio plugin nobody else drains the presentation events.
        view.audio_events.clear();
    }
    let now = time.elapsed_secs();
    // Delayed effects (Shadow Kill ash) that are due.
    let due: Vec<(f32, String, Vec3, Vec3)> = fx.delayed.iter().filter(|d| d.0 <= now).cloned().collect();
    fx.delayed.retain(|d| d.0 > now);
    for (_, path, at, dir) in due {
        spawn_effect(&mut commands, &mut meshes, &mut fx, &path, at, dir);
    }

    let me = session.as_ref().and_then(|s| s.local_id());
    let find = |id: u32| view.me.iter().chain(view.others.iter()).find(|p| p.id == id).copied();
    let fwd = |yaw: f32| Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
    let look = |p: &PlayerState| {
        let (y, pi) = (p.yaw.to_radians(), p.pitch.to_radians());
        Vec3::new(y.cos() * pi.cos(), y.sin() * pi.cos(), pi.sin())
    };
    let contact = audio.as_ref().and_then(|a| a.contact());
    let surface = |p: Vec3| -> String {
        let Some(s) = session.as_ref() else { return "DisContactType_Environment".into() };
        let world = &s.content.collision;
        let mut best: Option<(f32, u32)> = None;
        for d in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
            if let Some(hit) = world.raycast(p - d * 12.0, d, 36.0, dishonored_world::channel::ALL) {
                if best.is_none_or(|b| hit.distance < b.0) {
                    best = Some((hit.distance, hit.tri));
                }
            }
        }
        best.and_then(|b| world.surface_of(b.1)).unwrap_or("DisContactType_Environment").to_string()
    };
    let mantled: Vec<u32> = events.iter().filter_map(|e| if let SimEvent::Mantle { id, .. } = e { Some(*id) } else { None }).collect();
    for ev in events.iter() {
        let (path, at, dir): (String, Vec3, Vec3) = match ev {
            SimEvent::SwordBlocked { defender, .. } | SimEvent::SwordParried { defender, .. } => {
                let Some(p) = find(*defender) else { continue };
                ("Vfx_Weapon.Effects.Ps_SwordSparks_Parry".into(), p.position + Vec3::Z * 40.0 + fwd(p.yaw) * 40.0, fwd(p.yaw))
            }
            SimEvent::Fired { id, bolt, projectile } => {
                fx.bolts.insert(*projectile, *bolt);
                if *bolt {
                    continue;
                }
                let Some(p) = find(*id) else { continue };
                let path = if Some(*id) == me { PLAYER_MUZZLE } else { PAWN_MUZZLE };
                // The animated pistol's socket; without one (first person), in front of the eye.
                let (at, dir) = sockets.muzzle.get(id).copied().filter(|m| m.0.distance(p.position) < 200.0).unwrap_or((p.position + Vec3::Z * 60.0 + look(&p) * 50.0, look(&p)));
                (path.into(), at, dir)
            }
            SimEvent::Damage { victim, cause, .. } => {
                let Some(p) = find(*victim) else { continue };
                let striker = match cause {
                    DamageCause::Pistol | DamageCause::PistolHeadshot => "DisContactType_Bullet",
                    DamageCause::Crossbow | DamageCause::CrossbowHeadshot => "DisContactType_Arrow",
                    DamageCause::Sword | DamageCause::SwordCounter | DamageCause::Assassination | DamageCause::DropAssassination => "DisContactType_Sword",
                    _ => continue,
                };
                let path = contact.and_then(|c| c.particle(striker, "DisContactType_Body")).unwrap_or_else(|| match cause {
                    DamageCause::Pistol | DamageCause::PistolHeadshot => "Vfx_PhysMat.Blood.Pfx_PistolBlood_Blast".into(),
                    _ => "Vfx_PhysMat.Blood.Pfx_BloodSProjectile_Default".into(),
                });
                (path, p.position + Vec3::Z * 50.0, -fwd(p.yaw))
            }
            SimEvent::BlinkEnd { id } => {
                let Some(p) = find(*id) else { continue };
                let path = if mantled.contains(id) {
                    "Vfx_GamePlay.Blink.Blink_Mantle_01"
                } else if p.mode == MoveMode::Falling {
                    "Vfx_GamePlay.Blink.Blink_Fall_01"
                } else {
                    "Vfx_GamePlay.Blink.Blink_Ground_01"
                };
                (path.into(), p.position - Vec3::Z * 80.0, Vec3::Z)
            }
            SimEvent::WindblastCast { id } => {
                let Some(p) = find(*id) else { continue };
                let lvl = p.inv.power_levels[dishonored_sim::state::power::WINDBLAST];
                let path = if lvl >= 2 { "Vfx_GamePlay.Windblast.Windblast_lvl2" } else { "Vfx_GamePlay.Windblast.Windblast_lvl1" };
                (path.into(), p.position + Vec3::Z * 60.0, look(&p))
            }
            SimEvent::BodyRemoved { body, ash: true } => {
                let pos = find(*body).map(|p| p.position).unwrap_or(Vec3::ZERO);
                let pts = sockets.bones.get(body).cloned().filter(|v| !v.is_empty()).unwrap_or_else(|| vec![pos]);
                for b in pts {
                    fx.delayed.push((now + SHADOW_KILL_DELAY, SHADOW_KILL_PS.into(), b, Vec3::Z));
                }
                continue;
            }
            SimEvent::ProjectileImpact { projectile, point, victim: None } => {
                let striker = if fx.bolts.remove(projectile).unwrap_or(false) { "DisContactType_Arrow" } else { "DisContactType_Bullet" };
                let s = surface(*point);
                let path = contact.and_then(|c| c.particle(striker, &s)).unwrap_or_else(|| "Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt".into());
                (path, *point, Vec3::Z)
            }
            SimEvent::ProjectileImpact { projectile, .. } => {
                fx.bolts.remove(projectile);
                continue;
            }
            _ => continue,
        };
        spawn_effect(&mut commands, &mut meshes, &mut fx, &path, at, dir);
    }
}

/// World transform (Dishonored space), colour and alpha of a mesh-emitter particle: the mesh
/// scaled by the particle size (and size over life), turned by its mesh rotation.
fn mesh_particle(em: &ParticleEmitterAsset, p: &Particle, xf: &Mat4) -> (Mat4, Vec3, f32) {
    let rel = (p.age / p.life).clamp(0.0, 1.0);
    let mut size = p.size;
    if let Some(d) = &em.size_over_life {
        let m = d.eval(rel, p.r[1]);
        for a in 0..3 {
            if em.size_over_life_axes[a] {
                size[a] *= m[a];
            }
        }
    }
    let rot = Mat4::from_rotation_z(p.mrot.z) * Mat4::from_rotation_y(p.mrot.y) * Mat4::from_rotation_x(p.mrot.x);
    let world = if em.local_space {
        *xf * Mat4::from_translation(p.pos) * rot * Mat4::from_scale(size)
    } else {
        let (_, r, _) = xf.to_scale_rotation_translation();
        Mat4::from_translation(p.pos) * Mat4::from_quat(r) * rot * Mat4::from_scale(size)
    };
    let color = em.color_over_life.as_ref().map(|d| Vec3::from(d.eval(rel, p.r[0]))).unwrap_or(p.color);
    let alpha = em.alpha_over_life.as_ref().map(|d| d.eval1(rel, p.r[0])).unwrap_or(p.alpha).clamp(0.0, 1.0);
    (world, color, alpha)
}

/// Trail / ribbon and beam emitters as camera-facing strips (UE3 `TypeDataTrail2` /
/// `TypeDataRibbon`: the emitter's particles in spawn order joined into one strip, width the
/// particle size; `TypeDataBeam2`: each particle a beam from the emitter origin along its X
/// axis by `Distance` or to the `BeamTarget` point, displaced by `BeamNoise`). The texture runs
/// along the strip (U) once per segment set by the tiling.
#[allow(clippy::type_complexity)]
fn strip_mesh(em: &ParticleEmitterAsset, sim: &EmitterSim, xf: &Mat4, cam_pos: Vec3, look: &dyn Fn(&Particle) -> (Vec3, f32, Vec3), world_of: &dyn Fn(&Particle) -> Vec3, mesh: &mut Mesh) {
    let mut pos: Vec<[f32; 3]> = Vec::new();
    let mut uv: Vec<[f32; 2]> = Vec::new();
    let mut col: Vec<[f32; 4]> = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    // One strip through `pts` (Bevy space) with per-point half widths and colours.
    let mut strip = |pts: &[(Vec3, f32, [f32; 4])], tiling: f32| {
        if pts.len() < 2 {
            return;
        }
        let base = pos.len() as u32;
        let total = pts.len() as f32 - 1.0;
        for (k, (c, hw, colr)) in pts.iter().enumerate() {
            let next = pts[(k + 1).min(pts.len() - 1)].0;
            let prev = pts[k.saturating_sub(1)].0;
            let along = (next - prev).normalize_or_zero();
            let side = along.cross(cam_pos - *c).normalize_or_zero();
            let u = k as f32 / total * tiling;
            pos.push((*c - side * *hw).to_array());
            pos.push((*c + side * *hw).to_array());
            uv.push([u, 1.0]);
            uv.push([u, 0.0]);
            col.push(*colr);
            col.push(*colr);
        }
        for k in 0..pts.len() as u32 - 1 {
            let a = base + k * 2;
            idx.extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
        }
    };
    let point = |p: &Particle| {
        let (color, alpha, size) = look(p);
        (coords::pos(world_of(p)), size.x * 0.5 * coords::UU_TO_BEVY, [color.x.max(0.0), color.y.max(0.0), color.z.max(0.0), alpha.clamp(0.0, 1.0)])
    };
    if let Some(t) = &em.trail {
        let pts: Vec<(Vec3, f32, [f32; 4])> = sim.particles.iter().map(point).collect();
        strip(&pts, t.tiling.max(1e-3));
    } else if let Some(b) = &em.beam {
        let origin = xf.w_axis.truncate();
        for (i, p) in sim.particles.iter().enumerate() {
            let (_, hw, colr) = point(p);
            let end = match (&b.target, &b.distance) {
                (Some(t), _) => xf.transform_point3(Vec3::from(t.eval(sim.time, p.r[0]))),
                (None, Some(d)) => origin + xf.x_axis.truncate().normalize_or_zero() * d.eval1(sim.time, p.r[0]),
                _ => origin + xf.x_axis.truncate().normalize_or_zero() * 100.0,
            };
            let n = b.noise.as_ref().map(|x| x.1.max(1)).unwrap_or(1) as usize;
            let mut pts = Vec::with_capacity(n + 1);
            for k in 0..=n {
                let t = k as f32 / n as f32;
                let mut w = origin.lerp(end, t);
                if let (Some((range, _)), true) = (&b.noise, k > 0 && k < n) {
                    // Deterministic per point and particle, re-rolled four times a second.
                    let r = Vec3::from(range.eval(0.0, 0.5));
                    let h = |s: u32| (((i as u32).wrapping_mul(73856093) ^ (k as u32).wrapping_mul(19349663) ^ s.wrapping_mul(83492791) ^ ((p.age * 4.0) as u32).wrapping_mul(2654435761)) % 1000) as f32 / 500.0 - 1.0;
                    w += Vec3::new(r.x * h(1), r.y * h(2), r.z * h(3));
                }
                pts.push((coords::pos(w), hw, colr));
            }
            strip(&pts, b.tiling.max(1e-3));
        }
    }
    if pos.is_empty() {
        pos = vec![[0.0; 3]; 3];
        uv = vec![[0.0; 2]; 3];
        col = vec![[0.0; 4]; 3];
        idx = vec![0, 1, 2];
    }
    let nrm = vec![[0.0f32, 1.0, 0.0]; pos.len()];
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    mesh.insert_indices(Indices::U32(idx));
}
