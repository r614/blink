//! Particle effects (presentation only, never in the simulation): the level's placed `Emitter`
//! actors and gameplay effects (impacts from the contact system, sword sparks, pistol muzzle
//! blast, Blink and Windblast), simulated on the CPU from the decoded `ParticleSystem`s
//! (`dishonored_assets::particles`) and drawn as camera-facing sprites (SubUV, colour and alpha
//! over life, size over life, rotation) with their material's texture and blend mode.
//! research/dishonored/EFFECTS.md documents the module semantics and what is approximated.

use crate::coords;
use crate::level::LevelOf;
use crate::distortion::{DistortionFrame, DistortionMaterial};
use crate::map_render::to_image;
use crate::session::SessionView;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use dishonored_ir::particles::{ParticleBlend, ParticleEmitterAsset, ParticleSystemAsset};
use dishonored_ir::DishonoredMap;
// The effect paths, the particle simulation and the event -> effect decisions live in the
// Bevy-free present_rules (also compiled by tools/oracle).
pub use crate::present_rules::*;
use dishonored_sim::state::PlayerState;
use dishonored_world::WorldQuery;
use std::collections::HashMap;
use std::sync::Arc;

/// Sockets of the animated pawns the gameplay effects attach to (written by the character
/// animation each frame, Dishonored world space).
#[derive(Resource, Default)]
pub struct EffectSockets {
    /// Pistol `Spitfire` socket: (position, firing direction).
    pub muzzle: HashMap<u32, (Vec3, Vec3)>,
    /// `SHADOW_KILL_BONES` positions.
    pub bones: HashMap<u32, Vec<Vec3>>,
    /// `head_jnt` world position (Dishonored units) of every drawn pawn: where the awareness
    /// markers hang (`DisAwarenessMarker` constructor 0x00BCE9A0 binds to that bone).
    pub heads: HashMap<u32, Vec3>,
}

/// Particle notifies of the third-person animations (`AnimNotify_PlayParticleEffect` of the NPC
/// sets: the assassination blood on the neck sockets, sever-limb stumps, puke...): written by the
/// character animation, played by [`pawn_effects`].
#[derive(Resource, Default)]
pub struct PawnEffects {
    /// (system path, pawn, socket or bone, world transform then, attached) to start.
    pub pending: Vec<(String, u32, String, Mat4, bool)>,
    /// (pawn, socket or bone) -> world transform this frame.
    pub points: HashMap<(u32, String), Mat4>,
    /// Live attached instances and the point they follow.
    live: Vec<(Entity, u32, String)>,
}

impl PawnEffects {
    /// The points of a pawn its attached effects follow.
    pub fn wanted(&self, pawn: u32) -> Vec<String> {
        let mut v: Vec<String> = self.live.iter().filter(|l| l.1 == pawn).map(|l| l.2.clone()).collect();
        v.dedup();
        v
    }
}

fn pawn_effects(mut commands: Commands, mut fx: ResMut<Effects>, mut meshes: ResMut<Assets<Mesh>>, mut pawns: ResMut<PawnEffects>, mut q: Query<&mut FxInstance>) {
    let h = &mut *pawns;
    // Attached instances follow their point while they live; a pawn no longer animated
    // deactivates them (one-shots despawn themselves).
    h.live.retain(|(e, pawn, point)| match (q.get_mut(*e), h.points.get(&(*pawn, point.clone()))) {
        (Ok(mut inst), Some(m)) => {
            inst.set_transform(*m);
            true
        }
        (Ok(mut inst), None) => {
            inst.stop();
            false
        }
        _ => false,
    });
    let pending = std::mem::take(&mut h.pending);
    let Some(lf) = fx.levels.get(&fx.active).cloned() else { return };
    let level = fx.active.clone();
    for (path, pawn, point, m, attach) in pending {
        let Some(&si) = lf.by_path.get(&path.to_ascii_lowercase()) else {
            warn!("effects: animation effect {path} not decoded for {level}");
            continue;
        };
        if let Some(e) = spawn_instance(&mut commands, &mut meshes, &lf, &level, si, m, true) {
            fx.spawned_oneshots += 1;
            if attach {
                h.live.push((e, pawn, point));
            }
        }
    }
    // Points are written again next frame for what still plays.
    h.points.retain(|k, _| h.live.iter().any(|l| l.1 == k.0 && l.2 == k.1));
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

/// Soft particles (depth-biased alpha) on top of the sprite's unlit StandardMaterial: the
/// particle_soft.wgsl fragment multiplies the alpha by the scene-depth fade. One material per
/// emitter, so the sprite meshes batch exactly as before; soft.x = 0 draws a plain sprite.
#[derive(Asset, bevy::render::render_resource::AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct SoftFade {
    /// x: the depth-fade scale k (0: off), see `soft_scale`; y: the height-map erosion's
    /// multiplier (`ParticleShading::erode_scale`, 0: no erosion).
    #[uniform(100)]
    pub soft: Vec4,
}

impl bevy::pbr::MaterialExtension for SoftFade {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/particle_soft.wgsl".into()
    }
}

pub type FxMaterial = bevy::pbr::ExtendedMaterial<StandardMaterial, SoftFade>;

/// The depth fade scale of a material's depth-biased alpha (0: none), from the particle bases'
/// compiled base passes (shader_maps / fxc /dumpbin; render_depth.md "Soft particles"): with
/// `Db_Enable_Depth_Biased_Alpha` (DefaultAdditive / DefaultTranslu families, Glow, LensFlare,
/// WindSwirls, Volume_Light_Ring / _translu, ...; 36033, 35519, 35911, 6869, ...)
/// k = 1 / ((1 - Db_Depth_Biased_Alpha_Intensity) x 250); `lightCone_twoSided_PMAT` has the same
/// term under `Da_Enable_Depth_Biased_Alpha` / `Da_Depth_Biased_Alpha_Intensity` (default 0.5) with
/// 500 for 250 (shader 2726; LIKELY, read from the disassembly's constants). k is 1000 when the
/// product is below 0.001. (Volume_Light_PMAT uses 50, `volume_light.rs`.)
pub fn soft_scale(p: &dishonored_ir::MaterialParams) -> f32 {
    let scalar = |n: &str, d: f32| p.scalars.iter().find(|s| s.0.trim().eq_ignore_ascii_case(n)).map(|s| s.1).unwrap_or(d);
    let one_sided_smoke = p.base_material.as_deref().is_some_and(|b| b.rsplit('.').next().is_some_and(|n| n.eq_ignore_ascii_case("Look_Up_Smoke_translu_PMAT")));
    let (range, intensity) = if p.switch("Db_Enable_Depth_Biased_Alpha") == Some(true) {
        (250.0, scalar("Db_Depth_Biased_Alpha_Intensity", 0.05))
    } else if p.switch("Da_Enable_Depth_Biased_Alpha") == Some(true) {
        (500.0, scalar("Da_Depth_Biased_Alpha_Intensity", 0.5))
    } else if one_sided_smoke {
        // `Look_Up_Smoke_translu_PMAT` always fades (36275: x 64, `Depth_Bias`).
        (64.0, scalar("Depth_Bias", 0.0))
    } else {
        return 0.0;
    };
    let b = (1.0 - intensity) * range;
    if b - 0.001 >= 0.0 { 1.0 / b } else { 1000.0 }
}

/// A static-mesh section of a particle base with depth-biased alpha (level meshes: light cones,
/// glows, translucent props), waiting for its `FxMaterial` (spawned with a `StandardMaterial` by
/// `map_render::spawn_map`, swapped on the next update; one `FxMaterial` per source material, so
/// batches are kept).
#[derive(Component, Clone, Copy)]
pub struct PendingSoft(pub f32);

fn apply_pending_soft(
    mut commands: Commands,
    q: Query<(Entity, &PendingSoft, &MeshMaterial3d<StandardMaterial>)>,
    std_mats: Res<Assets<StandardMaterial>>,
    mut mats: ResMut<Assets<FxMaterial>>,
    mut cache: Local<HashMap<(AssetId<StandardMaterial>, u32), Handle<FxMaterial>>>,
) {
    for (e, p, m) in &q {
        let key = (m.0.id(), p.0.to_bits());
        let h = match cache.get(&key) {
            Some(h) => h.clone(),
            None => {
                let Some(base) = std_mats.get(&m.0).cloned() else { continue };
                let h = mats.add(FxMaterial { base, extension: SoftFade { soft: Vec4::new(p.0, 0.0, 0.0, 0.0) } });
                cache.insert(key, h.clone());
                h
            }
        };
        commands.entity(e).remove::<(PendingSoft, MeshMaterial3d<StandardMaterial>)>().insert(MeshMaterial3d(h));
    }
}
/// Placed emitters are simulated only within this distance of the camera (uu).
const SIM_RANGE: f32 = 9000.0;

/// Per-level decoded systems with their GPU materials.
struct LevelFx {
    systems: Vec<ParticleSystemAsset>,
    by_path: HashMap<String, usize>,
    /// (system, emitter) -> material handle.
    materials: HashMap<(usize, usize), Handle<FxMaterial>>,
    /// Mesh emitters: (system, emitter) -> (mesh, material).
    mesh_parts: HashMap<(usize, usize), (Handle<Mesh>, Handle<FxMaterial>)>,
    /// Emitters not drawn (untextured mesh emitters).
    skipped: std::collections::HashSet<(usize, usize)>,
    /// Sprite emitters whose material erodes by the particle alpha (`ParticleShading::erode`).
    erode: std::collections::HashSet<(usize, usize)>,
    /// Untextured (procedural) mesh emitters: mesh, colour x intensity, glow parameters.
    glow: HashMap<(usize, usize), (Handle<Mesh>, Vec4, Vec4)>,
    /// Mesh emitters on Volume_Light_PMAT materials: mesh (with its vertex colours) and the decoded
    /// material (volume_light.rs), whose `MeshEmitterVertexColor` each particle sets.
    volume: HashMap<(usize, usize), (Handle<Mesh>, crate::volume_light::VolumeLightMaterial)>,
    /// Emitters with distortion materials, drawn again by the distortion pass.
    distortion: HashMap<(usize, usize), DistortionSource>,
    /// [interp] Placed emitters by "package:actor" (lowercase): (system, transform,
    /// `bAutoActivate`), for matinee toggle tracks.
    placed: HashMap<String, (usize, Mat4, bool)>,
    /// The contact system's decal materials (`GAMEPLAY_DECAL_MATERIALS`) by lowercase path.
    decals: HashMap<String, Handle<FxMaterial>>,
}

/// A gameplay decal (`DisContactSub_Decal`) of the pool of `DisDecalManager`: when it was made,
/// its lifetime and when a view last saw it (`PrimitiveComponent.LastRenderTime`).
#[derive(Component)]
struct ContactDecal {
    born: f32,
    life: f32,
    seen: f32,
}

/// A distorting emitter: its decoded material, textures and (mesh emitters) the mesh's
/// triangles in Dishonored mesh space.
struct DistortionSource {
    mat: DistortionMaterial,
    texture: Handle<Image>,
    opacity: Option<Handle<Image>>,
    uv_distortion: Option<Handle<Image>>,
    mesh: Option<Arc<(Vec<Vec3>, Vec<Vec2>, Vec<u32>)>>,
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
    /// The levels' camera lens effects by spawning action (lowercase `<package>:<path>`).
    pub lens: HashMap<String, dishonored_ir::LensEffectDef>,
    rng: u64,
    pub spawned_oneshots: u64,
    /// Gameplay decals made so far (`DisContactSub_Decal`).
    pub spawned_decals: u64,
    /// The last gameplay effects asked for: (system path, spawned) (harness `effects` command).
    pub recent: Vec<(String, bool)>,
    pub live_particles: usize,
    /// Gameplay one-shot instances alive (systems still simulating).
    pub live_oneshots: usize,
    /// What the event -> effect rules remember (delayed effects, bolts, traps, shots in flight).
    rules: FxEventState,
    /// The Blink targeting indicator's instance and system while aiming.
    indicator: Option<(Entity, &'static str)>,
    /// Effects that follow a live object (`attached_effects`): (kind, key) -> instance, system.
    attached: HashMap<(u8, u32), (Entity, &'static str)>,
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
    pools: Vec<Vec<(Entity, Handle<FxMaterial>, LinearRgba)>>,
    /// Glow mesh emitters: one entity and material per live particle.
    glow_pools: Vec<Vec<(Entity, Handle<GlowMaterial>)>>,
    /// Light-beam mesh emitters: one entity and material per live particle.
    volume_pools: Vec<Vec<(Entity, Handle<crate::volume_light::VolumeLightMaterial>)>>,
    one_shot: bool,
    warmed: bool,
    /// Beam emitters with `TargetMethod` = `PEB2STM_UserSet`: the end point the game set
    /// (Dishonored world), e.g. the pistol tracer to its impact.
    beam_target: Option<Vec3>,
    /// `ParticleSystemComponent.InstanceParameters` (scalars) by lowercase name, read by
    /// `DistributionFloatParticleParameter`s (`SetFloatParameter`).
    float_params: HashMap<String, f32>,
    /// Per emitter: the rain modules' state (`DisParticleModuleRainDrops` / `RainImpacts`).
    rain: Vec<crate::rain::RainEmitterState>,
}

/// A placed `Emitter` actor's instance: its "package:actor" key (lowercase, as
/// [`placed_emitter`] takes it), so a system that moves the actor (the camera's rain box) finds
/// the instance the level spawned.
#[derive(Component, Clone, Debug)]
pub struct PlacedEmitter(pub String);

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "volume_glow.wgsl");
        bevy::asset::embedded_asset!(app, "particle_soft.wgsl");
        app.add_plugins(MaterialPlugin::<FxMaterial>::default()).add_systems(Update, apply_pending_soft);
        app.add_plugins(MaterialPlugin::<GlowMaterial>::default());
        app.init_resource::<Effects>().init_resource::<EffectSockets>().init_resource::<HandEffects>().init_resource::<PawnEffects>().init_resource::<crate::weapon_trails::WeaponTrails>().init_resource::<crate::lens_effects::LensEffects>().init_resource::<crate::rain::RainCamera>().add_systems(
            Update,
            (
                gameplay_effects.before(crate::audio::drive),
                age_decals,
                blink_indicator,
                attached_effects,
                // Camera lens effects follow the camera placed this frame.
                // (A play session's only: `--inspect` has no session view.)
                crate::lens_effects::update.after(crate::players::camera_follow).after(crate::cutscene::camera).after(crate::viewmodel::apply_fov).run_if(resource_exists::<crate::session::SessionView>),
                hand_effects.after(crate::viewmodel::animate_view_rig),
                pawn_effects.after(crate::characters::animate_characters),
                // Swing trails: the poses of this frame set their sources.
                crate::weapon_trails::drive.after(crate::characters::animate_characters).after(crate::viewmodel::animate_view_rig),
                // The player camera's rain box (moves the rain emitter before it simulates).
                crate::rain::drive.after(crate::players::camera_follow).after(crate::cutscene::camera).after(crate::viewmodel::apply_fov),
                // After everything that places the camera or sets its projection this frame
                // (`simulate` culls sprite meshes against that view).
                simulate.after(crate::players::camera_follow).after(crate::cutscene::camera).after(crate::viewmodel::apply_fov).after(crate::inspect_cam),
            )
                .chain(),
        );
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
        next_rand(&mut self.rng)
    }
}

fn material_for(map: &DishonoredMap, em: &ParticleEmitterAsset, images: &mut Assets<Image>, cache: &mut HashMap<usize, Handle<Image>>, mats: &mut Assets<FxMaterial>) -> Handle<FxMaterial> {
    let tex = em.material.and_then(|m| {
        let ma = map.materials.get(m)?;
        // A decoded particle material's sprite texture is its diffuse slot (or none).
        if map.material_params.get(m).is_some_and(|p| p.particle.is_some()) {
            return ma.diffuse;
        }
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
    // Decoded particle materials (EFFECTS.md "Particle materials"): the importer baked the
    // shader's colour x opacity sources into the diffuse slot; the shader multiplies them by
    // its constants and the vertex colour, nothing else.
    let shading = params.and_then(|p| p.particle);
    let (tint, gain) = match shading {
        Some(s) => ([s.tint[0], s.tint[1], s.tint[2], 1.0], s.opacity),
        None => (tint, gain),
    };
    // A decoded sprite with only a colour constant (no texture) draws a plain quad, as the
    // original; only procedural glows get the radial falloff.
    let plain = shading.is_some_and(|s| s.radial_power.is_none());
    let image = if plain { image } else { image.or_else(|| {
        let power = shading.and_then(|s| s.radial_power).or_else(|| params.and_then(|p| p.scalars.iter().find(|s| s.0.eq_ignore_ascii_case("SphereMask_Power")).map(|s| s.1))).unwrap_or(2.0);
        let key = usize::MAX - (power * 100.0) as usize;
        if let Some(h) = cache.get(&key) {
            return Some(h.clone());
        }
        let h = images.add(radial_falloff(power));
        cache.insert(key, h.clone());
        Some(h)
    }) };
    // Emissive bug / glow materials (`E_Emissive_Color`, alpha = intensity: fireflies x25):
    // drawn additively with the emissive colour (unlit, so it reads as the glowing body).
    let emissive = params.and_then(|p| p.vectors.iter().find(|v| v.0.eq_ignore_ascii_case("E_Emissive_Color")).map(|v| v.1)).filter(|e| e[3] > 1.0);
    let (alpha_mode, tint, gain) = match emissive {
        Some(e) => (AlphaMode::Add, [e[0], e[1], e[2], 1.0], e[3].min(4.0)),
        None => (alpha_mode, tint, gain),
    };
    // The look-up smoke erodes the raw height and applies its opacity after (soft.z).
    let smooth = shading.is_some_and(|s| s.erode && s.erode_smooth);
    let base_color = if alpha_mode == AlphaMode::Add {
        Color::linear_rgb(tint[0] * gain, tint[1] * gain, tint[2] * gain)
    } else {
        Color::linear_rgba(tint[0], tint[1], tint[2], if smooth { 1.0 } else { gain.clamp(0.0, 1.0) })
    };
    // Depth-biased alpha (soft particles): Db_Enable_Depth_Biased_Alpha permutations fade by the scene depth.
    let soft = params.map(soft_scale).unwrap_or(0.0);
    let erode = match shading.filter(|s| s.erode) {
        Some(s) if s.erode_smooth => -1.0,
        Some(s) => s.erode_scale.max(1e-3),
        None => 0.0,
    };
    mats.add(FxMaterial {
        base: StandardMaterial { base_color, base_color_texture: image, unlit: true, alpha_mode, cull_mode: None, double_sided: true, fog_enabled: alpha_mode != AlphaMode::Add, ..default() },
        extension: SoftFade { soft: Vec4::new(soft, erode, if smooth { gain.clamp(0.0, 1.0) } else { 0.0 }, 0.0) },
    })
}

/// A material's blend as the particle systems name it (the importer's `material_blend`).
fn blend_of(map: &DishonoredMap, id: usize) -> ParticleBlend {
    use dishonored_ir::MaterialBlend as B;
    match map.material_params.get(id).map(|p| p.blend) {
        Some(B::Additive) => ParticleBlend::Additive,
        Some(B::Masked) => ParticleBlend::Masked,
        Some(B::Opaque) => ParticleBlend::Opaque,
        Some(B::Modulate) => ParticleBlend::Modulate,
        _ => ParticleBlend::Translucent,
    }
}

/// How far a decal lies off its surface (uu): the engine's depth bias, `DecalDepthBias` -6e-5.
const DECAL_LIFT: f32 = 0.25;

/// A decal's quad (`DishonoredDecalComponent`: `Width` x `Height` uu about the contact, lying on
/// the surface of normal `normal`; Dishonored world in, Bevy space out). Its texture axis is the
/// direction the contact came along (`dir`, on the surface plane); without one, the reference
/// tangent of `DisContactSub_Decal`'s fire function (0x00bcf000). APPROXIMATION: the engine
/// clips the receivers' triangles to the decal's frustum (`DecalComponent`: FieldOfView 80,
/// FarPlane 300, BlendRange 89.5..180 degrees); this quad is flat.
fn decal_mesh(at: Vec3, normal: Vec3, dir: Vec3, width: f32, height: f32) -> Mesh {
    let n = normal.normalize_or(Vec3::Z);
    let reference = if n.x.abs() + n.y.abs() >= 1e-4 { Vec3::Z } else { Vec3::X };
    let t0 = (reference - n * n.dot(reference)).normalize_or(Vec3::X);
    let d = dir - n * n.dot(dir);
    let u = if d.length_squared() > 1e-6 { d.normalize() } else { t0 };
    let v = n.cross(u);
    let c = at + n * DECAL_LIFT;
    let (hw, hh) = (width * 0.5, height * 0.5);
    let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, corners.iter().map(|(x, y)| coords::pos(c + u * hw * *x + v * hh * *y).to_array()).collect::<Vec<[f32; 3]>>());
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![coords::dir(n).normalize_or_zero().to_array(); 4]);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0f32; 4]; 4]);
    m.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    m
}

/// The pool's bookkeeping: note when a view last saw each decal, and end the ones past their
/// lifetime (`ActiveDecalInfo.LifetimeRemaining`, `m_fLifetime`).
fn age_decals(time: Res<Time>, mut commands: Commands, mut q: Query<(Entity, &mut ContactDecal, &ViewVisibility)>) {
    let now = time.elapsed_secs();
    for (e, mut d, vis) in &mut q {
        if vis.get() {
            d.seen = now;
        }
        if now - d.born >= d.life {
            commands.entity(e).despawn();
        }
    }
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
pub fn spawn_level_effects(name: &str, map: &DishonoredMap, commands: &mut Commands, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, mats: &mut Assets<FxMaterial>, fx: &mut Effects) -> usize {
    for l in &map.lens_effects {
        fx.lens.insert(l.action.to_ascii_lowercase(), l.clone());
    }
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
    let mut volume = HashMap::new();
    let mut distortion = HashMap::new();
    for (si, s) in map.particle_systems.iter().enumerate() {
        for (ei, e) in s.emitters.iter().enumerate() {
            // Distortion materials (`Dist_Enable_Distortion`, e.g. the pistol muzzle's
            // `WindWave_Disto_01_inst`) are drawn again by the distortion pass (distortion.rs);
            // with `D_Disable_Diffuse` they draw nothing in the colour pass.
            let mesh_id = e.mesh_id.filter(|m| *m < map.meshes.len());
            let dmat = match mesh_id {
                Some(mid) if !(e.mesh_override_material && e.material.is_some()) => map.meshes[mid].sections.first().and_then(|s| s.material),
                _ => e.material,
            };
            let params = dmat.and_then(|m| map.material_params.get(m));
            let tex_params = dmat.and_then(|m| map.materials.get(m)).map(|m| m.texture_params.as_slice()).unwrap_or(&[]);
            if let Some(dm) = params.and_then(|p| DistortionMaterial::decode(p, tex_params)) {
                let mut img = |t: dishonored_ir::TextureId| -> Option<Handle<Image>> {
                    if let Some(h) = cache.get(&t) {
                        return Some(h.clone());
                    }
                    let h = images.add(to_image(map.textures.get(t)?));
                    cache.insert(t, h.clone());
                    Some(h)
                };
                if let Some(texture) = img(dm.texture) {
                    let opacity = dm.opacity.and_then(&mut img);
                    let uv_distortion = dm.uv_distortion.and_then(|v| img(v.0));
                    let mesh = mesh_id.map(|mid| {
                        let m = &map.meshes[mid];
                        let uv = if m.uv0.len() == m.positions.len() { m.uv0.clone() } else { vec![Vec2::ZERO; m.positions.len()] };
                        Arc::new((m.positions.clone(), uv, m.indices.clone()))
                    });
                    distortion.insert((si, ei), DistortionSource { mat: dm.clone(), texture, opacity, uv_distortion, mesh });
                }
            }
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
            // Light beams (Volume_Light_PMAT): the decoded material (volume_light.rs; the particle
            // vertex factory's base passes compute the same as the static mesh ones), the particle
            // colour as its `MeshEmitterVertexColor`.
            if let Some((vp, ma)) = eff.and_then(|m| Some((map.material_params.get(m)?, map.materials.get(m)?))).filter(|(p, _)| crate::volume_light::is_volume_light(p)) {
                if let Some(params) = crate::volume_light::VolumeLightUniform::decode(vp, crate::map_render::light_unit()) {
                    let rays = ma.texture_params.iter().find(|(k, _)| k.eq_ignore_ascii_case(crate::volume_light::RAYS_TEXTURE)).and_then(|t| {
                        if let Some(h) = cache.get(&t.1) {
                            return Some(h.clone());
                        }
                        let h = images.add(to_image(map.textures.get(t.1)?));
                        cache.insert(t.1, h.clone());
                        Some(h)
                    });
                    let mh = meshes.add(crate::volume_light::beam_mesh(&map.meshes[mid]));
                    volume.insert((si, ei), (mh, crate::volume_light::VolumeLightMaterial { params, rays }));
                    continue;
                }
            }
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
        erode: map
            .particle_systems
            .iter()
            .enumerate()
            .flat_map(|(si, s)| s.emitters.iter().enumerate().filter(|(_, e)| e.material.and_then(|m| map.material_params.get(m)).and_then(|p| p.particle).is_some_and(|p| p.erode)).map(move |(ei, _)| (si, ei)))
            .collect(),
        materials,
        mesh_parts,
        skipped,
        glow,
        volume,
        distortion,
        placed: map
            .emitters
            .iter()
            .filter_map(|e| Some((placed_key(&e.source)?, (e.system, e.transform, e.auto_activate))))
            .collect(),
        decals: map
            .named_materials
            .iter()
            .map(|(path, id)| {
                // The decal quad is a sprite of the material (the decal shader is not run).
                let em = ParticleEmitterAsset { material: Some(*id), blend: blend_of(map, *id), ..Default::default() };
                (path.to_ascii_lowercase(), material_for(map, &em, images, &mut cache, mats))
            })
            .collect(),
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
                    let tex = |t: Option<usize>| t.and_then(|t| map.textures.get(t)).map(|t| t.source.clone());
                    info!("fx permutation {} {:?}", e.name, map.material_params.get(m).map(|p| (&p.static_switches, &p.mask_channels, tex(p.opacity), map.materials.get(m).map(|x| x.texture_params.iter().map(|(n, t)| (n.clone(), tex(Some(*t)))).collect::<Vec<_>>()))));
                }
            }
        }
    }
    if std::env::var_os("DISH_FX_LIST").is_some() {
        for e in &map.emitters {
            let s = &map.particle_systems[e.system];
            let unsupported: Vec<&str> = s.emitters.iter().flat_map(|x| x.unsupported().collect::<std::collections::BTreeSet<_>>()).collect();
            info!("fx emitter {} {} at {:.0} ({} emitters, unsupported {:?})", e.source, s.source, e.position(), s.emitters.len(), unsupported);
        }
    }
    // `--no-fx` (render comparisons) also leaves the particle effects out.
    let off = std::env::args().any(|a| a == "--no-fx") || std::env::var_os("DISH_NO_PARTICLES").is_some();
    for e in map.emitters.iter().filter(|e| e.auto_activate && !off) {
        let root = spawn_instance(commands, meshes, &lf, name, e.system, e.transform, false);
        // Its "package:actor" key: an actor that drives its emitters (a wall of light, the
        // camera's rain box) finds it.
        if let (Some(root), Some(key)) = (root, placed_key(&e.source)) {
            commands.entity(root).insert(PlacedEmitter(key));
        }
        // An emitter of a Kismet-streamed sublevel leaves with it (level streaming).
        if let (Some(root), Some(tag)) = (root, crate::level::streamed_tag(&map.streamed_levels, crate::level::source_package(&e.source))) {
            commands.entity(root).insert(tag);
        }
        n += 1;
    }
    info!("effects: {name}: {} particle systems, {n} placed emitters, {} distortion emitters", map.particle_systems.len(), lf.distortion.len());
    if std::env::var_os("DISH_FX_LIST").is_some() {
        for (&(si, ei), d) in &lf.distortion {
            info!("fx distortion {} emitter {} {:?} mesh {}", lf.systems[si].source, lf.systems[si].emitters[ei].name, d.mat, d.mesh.is_some());
        }
    }
    n
}

/// The "package:actor" key (lowercase) of a placed emitter's source ("package:path.actor").
fn placed_key(source: &str) -> Option<String> {
    let (pkg, path) = source.split_once(':')?;
    Some(format!("{pkg}:{}", path.rsplit('.').next()?).to_ascii_lowercase())
}

/// [interp] A placed emitter played by a matinee's toggle track (`InterpTrackToggle`).
#[derive(Component)]
pub struct MatineeEmitter;

/// Stop an instance: it spawns no more and goes once its particles die (`DeactivateSystem`).
pub fn stop_instance(commands: &mut Commands, e: Entity) {
    commands.entity(e).try_insert(StopAtOnce);
}

/// Set (or move) the user-set end of an instance's beams (`SetBeamTargetPoint`).
pub fn set_beam_target(commands: &mut Commands, e: Entity, to: Vec3) {
    commands.entity(e).try_insert(BeamTarget(to));
}

/// Give an instance's AnimTrail emitters this update's source data (the instance moves to the
/// first edge).
pub fn feed_trail(commands: &mut Commands, e: Entity, src: TrailSource) {
    commands.entity(e).try_insert(TrailFeed(src));
}

impl FxInstance {
    /// [interp] Move the emitter (Dishonored world transform).
    pub fn set_transform(&mut self, m: Mat4) {
        self.transform = m;
    }
    /// [interp] Stop spawning; the instance goes once its particles die (`DeactivateSystem`).
    pub fn stop(&mut self) {
        self.one_shot = true;
        for s in &mut self.sims {
            s.done = true;
            s.deactivated = true;
        }
    }
    /// Deactivated (`stop`) or a one-shot.
    pub fn stopped(&self) -> bool {
        self.one_shot
    }
    /// `UParticleSystemComponent::SetFloatParameter`: a scalar instance parameter (names are
    /// case-insensitive, as FNames).
    pub fn set_float_param(&mut self, name: &str, value: f32) {
        self.float_params.insert(name.to_ascii_lowercase(), value);
    }
    pub fn float_param(&self, name: &str) -> Option<f32> {
        self.float_params.get(&name.to_ascii_lowercase()).copied()
    }
}

/// A decoded system of a loaded level.
pub fn level_system<'a>(fx: &'a Effects, level: &str, system: usize) -> Option<&'a ParticleSystemAsset> {
    fx.levels.get(level)?.systems.get(system)
}

/// [interp] A placed emitter of the loaded levels by "package:actor": (system's level, system,
/// level transform, `bAutoActivate`). Emitters based on another actor are not imported (level
/// import) and have none.
pub fn placed_emitter(fx: &Effects, key: &str) -> Option<(String, usize, Mat4, bool)> {
    let k = key.to_ascii_lowercase();
    fx.levels.iter().find_map(|(name, lf)| lf.placed.get(&k).map(|p| (name.clone(), p.0, p.1, p.2)))
}

/// [interp] Start a placed emitter (`ETTA_On`), or fire it once (`ETTA_Trigger`, `one_shot`),
/// at a Dishonored world transform. Returns the instance entity.
pub fn start_placed_emitter(commands: &mut Commands, meshes: &mut Assets<Mesh>, fx: &mut Effects, key: &str, transform: Mat4, one_shot: bool) -> Option<Entity> {
    let (level, system, _, _) = placed_emitter(fx, key)?;
    let lf = fx.levels.get(&level)?.clone();
    let e = spawn_instance(commands, meshes, &lf, &level, system, transform, one_shot)?;
    if !one_shot {
        commands.entity(e).insert((MatineeEmitter, PlacedEmitter(key.to_ascii_lowercase())));
    }
    Some(e)
}

fn spawn_instance(commands: &mut Commands, meshes: &mut Assets<Mesh>, lf: &LevelFx, level: &str, system: usize, transform: Mat4, one_shot: bool) -> Option<Entity> {
    let s = lf.systems.get(system)?;
    let mut handles = Vec::new();
    let root = commands
        .spawn((Transform::IDENTITY, Visibility::Inherited, LevelOf(level.to_string())))
        .id();
    for (ei, _) in s.emitters.iter().enumerate() {
        let mesh = meshes.add(empty_mesh());
        handles.push(mesh.clone());
        if lf.mesh_parts.contains_key(&(system, ei)) || lf.skipped.contains(&(system, ei)) || lf.glow.contains_key(&(system, ei)) || lf.volume.contains_key(&(system, ei)) {
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
        volume_pools: s.emitters.iter().map(|_| Vec::new()).collect(),
        one_shot,
        warmed: one_shot,
        beam_target: None,
        float_params: HashMap::new(),
        rain: s.emitters.iter().map(|_| Default::default()).collect(),
    });
    Some(root)
}

/// Whether an emitter's sprite mesh is read: drawn by its own entity (`spawn_instance` spawns
/// one unless the emitter is skipped, a mesh or glow emitter, or has no material) or copied by
/// the distortion pass.
fn sprite_mesh_used(lf: &LevelFx, system: usize, emitter: usize) -> bool {
    let k = (system, emitter);
    let drawn = !lf.mesh_parts.contains_key(&k) && !lf.skipped.contains(&k) && !lf.glow.contains_key(&k) && !lf.volume.contains_key(&k) && lf.materials.contains_key(&k);
    drawn || lf.distortion.get(&k).is_some_and(|d| d.mesh.is_none())
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

/// Spawn a beam system (`TargetMethod` = `PEB2STM_UserSet`) from `from` to `to` (Dishonored
/// world), as a one-shot.
pub fn spawn_beam(commands: &mut Commands, meshes: &mut Assets<Mesh>, fx: &mut Effects, path: &str, from: Vec3, to: Vec3) -> bool {
    if fx.recent.len() >= 64 {
        fx.recent.remove(0);
    }
    fx.recent.push((path.to_string(), false));
    let Some(lf) = fx.levels.get(&fx.active).cloned() else { return false };
    let Some(&si) = lf.by_path.get(&path.to_ascii_lowercase()) else { return false };
    let level = fx.active.clone();
    let m = facing(from, to - from);
    let Some(e) = spawn_instance(commands, meshes, &lf, &level, si, m, true) else { return false };
    // `simulate` copies it into the instance (spawned through commands, not yet queryable).
    commands.entity(e).insert(BeamTarget(to));
    fx.spawned_oneshots += 1;
    if let Some(r) = fx.recent.last_mut() {
        r.1 = true;
    }
    true
}

/// The user-set end of a beam instance, applied to its `FxInstance` on the next simulate.
#[derive(Component)]
struct BeamTarget(Vec3);

/// An instance deactivated before its first update (`FxRequest::EndedTrail`).
#[derive(Component)]
struct StopAtOnce;

/// AnimTrail source data for an instance's next update ([`feed_trail`]).
#[derive(Component)]
struct TrailFeed(TrailSource);

/// Spawn a ParticleSystem by path of the active level at a Dishonored world transform; a
/// looping instance (not `one_shot`) runs until stopped. Returns the instance entity.
pub fn spawn_system(commands: &mut Commands, meshes: &mut Assets<Mesh>, fx: &mut Effects, path: &str, transform: Mat4, one_shot: bool) -> Option<Entity> {
    let lf = fx.levels.get(&fx.active).cloned()?;
    let &si = lf.by_path.get(&path.to_ascii_lowercase())?;
    let level = fx.active.clone();
    spawn_instance(commands, meshes, &lf, &level, si, transform, one_shot)
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

/// Write the sprite quads of one emitter into its mesh (Bevy space, facing the camera).
/// `erode`: the material erodes its opacity map by the particle alpha
/// (`saturate((H - alpha) x Mask_Multiply)`, `ParticleShading::erode`): the vertex alpha stays 1
/// and the particle alpha rides in the second UV set, which `particle_soft.wgsl` subtracts from
/// the baked height (earlier APPROXIMATION, replaced: the
/// sprite fades where the original's shape shrinks).
fn build_mesh(em: &ParticleEmitterAsset, sim: &EmitterSim, xf: &Mat4, cam_right: Vec3, cam_up: Vec3, cam_pos: Vec3, erode: bool, beam_target: Option<Vec3>, mesh: &mut Mesh) {
    let n = sim.particles.len();
    let mut pos = Vec::with_capacity(n * 4);
    let mut uv = Vec::with_capacity(n * 4);
    let mut col = Vec::with_capacity(n * 4);
    let mut nrm = Vec::with_capacity(n * 4);
    let mut idx = Vec::with_capacity(n * 6);
    let mut uv_b: Vec<[f32; 2]> = Vec::new();
    let frames = (em.sub_h * em.sub_v).max(1);
    let look = |p: &Particle| particle_look(em, p, sim.loop_time);
    let world_of = |p: &Particle| particle_world(em, xf, p);
    if em.trail.is_some() || em.beam.is_some() {
        strip_mesh(em, sim, xf, cam_pos, &look, &world_of, beam_target, mesh);
        return;
    }
    // `SortMode`: back to front by view depth (1) or distance to the view (2), oldest (3) or
    // newest (4) first; unsorted particles draw in spawn order.
    let mut order: Vec<&Particle> = sim.particles.iter().collect();
    let fwd = cam_up.cross(cam_right);
    match em.sort_mode {
        1 => order.sort_by(|a, b| (coords::pos(world_of(b)) - cam_pos).dot(fwd).total_cmp(&(coords::pos(world_of(a)) - cam_pos).dot(fwd))),
        2 => order.sort_by(|a, b| coords::pos(world_of(b)).distance_squared(cam_pos).total_cmp(&coords::pos(world_of(a)).distance_squared(cam_pos))),
        3 => order.sort_by(|a, b| b.age.total_cmp(&a.age)),
        4 => order.sort_by(|a, b| a.age.total_cmp(&b.age)),
        _ => {}
    }
    let (axis_lock, sub_image) = (em.axis_lock(), em.sub_image());
    for p in order {
        // A decal emitter's particle lies on the surface it was placed on; one without
        // (not placed yet) is not drawn.
        let decal_normal = match (&em.decal, p.normal) {
            (Some(_), Some(n)) => Some(coords::dir(n).normalize_or_zero()),
            (Some(_), None) => continue,
            _ => None,
        };
        let rel = (p.age / p.life).clamp(0.0, 1.0);
        let (color, alpha, size) = look(p);
        let erode_alpha = alpha.clamp(0.0, 1.0);
        let alpha = if erode { 1.0 } else { alpha };
        let world = world_of(p);
        let c = coords::pos(world);
        // Only `PSA_Square` draws Size.X by Size.X (UE3 sprite dynamic data: Size.Y = Size.X for
        // square sprites); rectangle and velocity-aligned sprites (the rain streaks) use Size.Y.
        let (hw, hh) = (size.x * 0.5 * coords::UU_TO_BEVY, if em.screen_align != 0 { size.y } else { size.x } * 0.5 * coords::UU_TO_BEVY);
        let (s, co) = p.rot.sin_cos();
        // Plane of the sprite: camera facing, or locked to an emitter axis (OrientationAxisLock).
        let axis = |v: glam::Vec4| coords::dir(v.truncate()).normalize_or_zero();
        let (br, bu) = match axis_lock {
            1 | 4 => (axis(xf.y_axis), axis(xf.z_axis)),
            2 | 5 => (axis(xf.x_axis), axis(xf.z_axis)),
            3 | 6 => (axis(xf.x_axis), axis(xf.y_axis)),
            7..=9 => {
                let up = axis([xf.x_axis, xf.y_axis, xf.z_axis][(axis_lock - 7) as usize]);
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
        // (decals: flat on the surface, turned by the particle's rotation, a hair above it;
        // adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with
        // permission: the decal quad of its particles.rs mesh builder)
        let (rx, ry, c) = match decal_normal {
            Some(nb) if nb != Vec3::ZERO => {
                let t0 = nb.any_orthonormal_vector();
                let b0 = nb.cross(t0);
                (t0 * co + b0 * s, b0 * co - t0 * s, c + nb * DECAL_LIFT * coords::UU_TO_BEVY)
            }
            _ => (rx, ry, c),
        };
        let base = pos.len() as u32;
        for (ox, oy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            pos.push((c + rx * hw * ox + ry * hh * oy).to_array());
            nrm.push([0.0, 1.0, 0.0]);
            col.push([color.x.max(0.0), color.y.max(0.0), color.z.max(0.0), alpha.clamp(0.0, 1.0)]);
        }
        // `DetermineImageIndex` (0x008B5390): linear methods take `(int)SubImageIndex(relative
        // time)` clamped to the frames; random methods a random frame (`RandomImageChanges` 0:
        // drawn once); `PSUVIM_None` keeps the first frame.
        let f = match (em.sub_method, sub_image) {
            (1 | 2, Some(d)) => (d.eval1(rel, p.r[2]).max(0.0) as u32).min(frames - 1),
            (3 | 4, _) => ((p.r[2] * frames as f32) as u32).min(frames - 1),
            _ => 0,
        };
        let (fx, fy) = ((f % em.sub_h) as f32, (f / em.sub_h) as f32);
        let (w, h) = (1.0 / em.sub_h as f32, 1.0 / em.sub_v as f32);
        for (u, v) in [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)] {
            uv.push([(fx + u) * w, (fy + v) * h]);
            if erode {
                uv_b.push([erode_alpha, 0.0]);
            }
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    if pos.is_empty() {
        clear_sprite_mesh(mesh);
        return;
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    if erode {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv_b);
    }
    mesh.insert_indices(Indices::U32(idx));
}

/// A sprite mesh without particles. Never upload an empty vertex buffer: one degenerate,
/// transparent triangle.
fn clear_sprite_mesh(mesh: &mut Mesh) {
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32; 2]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0f32; 4]; 3]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
}

/// A particle's colour, alpha and size at its age: `ParticleModuleSizeScale` (Size = BaseSize x
/// scale, 0x0089E040), size over life, `SizeScaleByTime` over the particle's clock (0x0089E120)
/// and size by velocity; colour / alpha over life, scaled by `ColorScaleOverLife` (0x008BA1E0: at
/// the relative time, or the emitter time `et` with `bEmitterTime`).
fn particle_look(em: &ParticleEmitterAsset, p: &Particle, et: f32) -> (Vec3, f32, Vec3) {
    let rel = (p.age / p.life).clamp(0.0, 1.0);
    let mut size = match em.size_scale() {
        Some(d) => p.size * Vec3::from(d.eval(rel, p.r[1])),
        None => p.size,
    };
    if let Some((d, axes)) = em.size_over_life() {
        let m = d.eval(rel, p.r[1]);
        for k in 0..3 {
            if axes[k] {
                size[k] *= m[k];
            }
        }
    }
    if let Some((d, axes)) = em.size_scale_by_time() {
        let m = d.eval(p.age, p.r[1]);
        for k in 0..3 {
            if axes[k] {
                size[k] *= m[k];
            }
        }
    }
    // `ParticleModuleSizeMultiplyVelocity`: Size *= VelocityMultiplier x |Velocity| on the
    // enabled axes (every update, from the reset size; the rain streaks' length).
    if let Some((d, axes)) = em.size_mult_velocity() {
        let m = Vec3::from(d.eval(rel, p.r[1])) * p.vel.length();
        for k in 0..3 {
            if axes[k] {
                size[k] *= m[k];
            }
        }
    }
    let mut color = em.color_over_life().map(|d| Vec3::from(d.eval(rel, p.r[0]))).unwrap_or(p.color);
    let mut alpha = em.alpha_over_life().map(|d| d.eval1(rel, p.r[0])).unwrap_or(p.alpha);
    if let Some((cs, als, emitter_time)) = em.color_scale_over_life() {
        let t = if emitter_time { et } else { rel };
        if let Some(d) = cs {
            color *= Vec3::from(d.eval(t, p.r[0]));
        }
        if let Some(d) = als {
            alpha *= d.eval1(t, p.r[0]);
        }
    }
    (color, alpha, size)
}

/// A particle's drawn position (Dishonored world space). Orbit: an emitter-space offset turned
/// by the orbit rotation, added at render time.
fn particle_world(em: &ParticleEmitterAsset, xf: &Mat4, p: &Particle) -> Vec3 {
    let orb = if p.orbit_off != Vec3::ZERO { Quat::from_euler(EulerRot::ZYX, p.orbit_rot.z, p.orbit_rot.y, p.orbit_rot.x) * p.orbit_off } else { Vec3::ZERO };
    if em.local_space { xf.transform_point3(p.pos + orb) } else { p.pos + xf.transform_vector3(orb) }
}

/// Bevy-space bounding sphere of the sprites `build_mesh` draws for an emitter (each quad lies
/// within its half-diagonal of its centre, whatever its orientation), or None without particles.
fn sprite_bounds(em: &ParticleEmitterAsset, sim: &EmitterSim, xf: &Mat4) -> Option<(Vec3, f32)> {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    let mut reach = 0.0f32;
    for p in &sim.particles {
        let (_, _, size) = particle_look(em, p, sim.loop_time);
        let c = coords::pos(particle_world(em, xf, p));
        let hw = size.x.abs() * 0.5 * coords::UU_TO_BEVY;
        let hh = if em.screen_align != 0 { size.y.abs() } else { size.x.abs() } * 0.5 * coords::UU_TO_BEVY;
        lo = lo.min(c);
        hi = hi.max(c);
        reach = reach.max((hw * hw + hh * hh).sqrt());
    }
    if !lo.is_finite() || !hi.is_finite() || !reach.is_finite() {
        return None;
    }
    Some(((lo + hi) * 0.5, (hi - lo).length() * 0.5 + reach))
}

fn simulate(
    mut commands: Commands,
    mut fx: ResMut<Effects>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<FxMaterial>>,
    mut glow_mats: ResMut<Assets<GlowMaterial>>,
    mut volume_mats: ResMut<Assets<crate::volume_light::VolumeLightMaterial>>,
    mut q: Query<(Entity, &mut FxInstance, &InheritedVisibility, Option<&BeamTarget>, Option<&StopAtOnce>, Option<&TrailFeed>)>,
    cams: Query<(&GlobalTransform, &Transform, &Projection, Option<&ChildOf>), (With<Camera3d>, Without<crate::reflect::ReflectCamera>)>,
    session: Option<Res<crate::session::Session>>,
    mut dframe: ResMut<DistortionFrame>,
    mut rain_cam: ResMut<crate::rain::RainCamera>,
) {
    use bevy::camera::primitives::Sphere;
    dframe.clear();
    let Some((cam, cam_tf, cam_proj, cam_parent)) = cams.iter().next() else { return };
    // The view this frame will be drawn from (the camera's transform as set this frame; the
    // system runs after the camera systems). Sprite meshes outside it are not rebuilt: they are
    // emptied once and rebuilt from the current particles when back in view, so what is drawn
    // is unchanged. A parented camera (not used) disables this.
    let frustum = cam_parent.is_none().then(|| cam_proj.compute_frustum(&GlobalTransform::from(*cam_tf)));
    let in_view = |c: Vec3, r: f32| frustum.as_ref().is_none_or(|f| f.intersects_sphere(&Sphere { center: c.into(), radius: r * 1.05 + 0.1 }, false));
    let cam_pos_b = cam.translation();
    let t_now = time.elapsed_secs_wrapped();
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
    fx.live_oneshots = q.iter().filter(|(_, i, _, _, _, _)| i.one_shot).count();
    for (e, mut inst, vis, beam, stop_now, trail) in &mut q {
        if stop_now.is_some() {
            inst.stop();
            commands.entity(e).remove::<StopAtOnce>();
        }
        if let Some(b) = beam {
            inst.beam_target = Some(b.0);
        }
        if let Some(t) = trail {
            inst.transform = Mat4::from_translation(t.0.edges[0]);
            for s in inst.sims.iter_mut() {
                s.trail_src = Some(t.0);
            }
            commands.entity(e).remove::<TrailFeed>();
        }
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
                feed_location_sources(&sys.emitters, &mut inst.sims, &xf);
                for (em, sim) in sys.emitters.iter().zip(inst.sims.iter_mut()).filter(|(em, _)| !crate::rain::is_rain_emitter(em)) {
                    step(em, sim, &xf, 1.0 / 15.0, &mut rnd, (em.collision().is_some() || em.decal.is_some()).then_some(ray_fn));
                }
                dispatch_events(&sys.emitters, &mut inst.sims, &xf, &mut rnd);
            }
        }
        feed_location_sources(&sys.emitters, &mut inst.sims, &xf);
        for (i, (em, sim)) in sys.emitters.iter().zip(inst.sims.iter_mut()).enumerate() {
            if crate::rain::is_rain_emitter(em) {
                // Arkane's rain modules replace the spawn rate and the particles' lifetimes.
                let fp = &inst.float_params;
                let params = |n: &str| fp.get(&n.to_ascii_lowercase()).copied();
                if let Some(rs) = inst.rain.get_mut(i) {
                    crate::rain::step_emitter(em, sim, rs, &xf, dt, &mut rnd, &params, &mut rain_cam);
                }
            } else {
                step(em, sim, &xf, dt, &mut rnd, (em.collision().is_some() || em.decal.is_some()).then_some(ray_fn));
            }
            live += sim.particles.len();
            let mut sprite_culled = false;
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
                    let (world, c, a) = mesh_particle(em, p, &xf, sim.loop_time);
                    commands.entity(pool[k].0).insert(Transform::from_matrix(coords::transform(world)));
                    if let Some(mut m) = glow_mats.get_mut(&pool[k].1) {
                        let (s, _, t) = world.to_scale_rotation_translation();
                        m.sphere = coords::pos(t).extend(params.w * s.max_element() * coords::UU_TO_BEVY);
                        m.color = Vec4::new(color.x * c.x * a, color.y * c.y * a, color.z * c.z * a, 1.0);
                    }
                }
            } else if let Some((mh, vm)) = lf.volume.get(&(inst.system, i)) {
                // Light-beam mesh particles: one entity and material each, the particle colour
                // and alpha as `MeshEmitterVertexColor`.
                let pool = &mut inst.volume_pools[i];
                let want = sim.particles.len().min(64);
                while pool.len() < want {
                    let vh = volume_mats.add(vm.clone());
                    let id = commands.spawn((Mesh3d(mh.clone()), MeshMaterial3d(vh.clone()), Transform::IDENTITY, bevy::light::NotShadowCaster, bevy::light::NotShadowReceiver, FxMeshParticle, ChildOf(e))).id();
                    pool.push((id, vh));
                }
                while pool.len() > want {
                    if let Some((id, vh)) = pool.pop() {
                        volume_mats.remove(&vh);
                        commands.entity(id).despawn();
                    }
                }
                for (k, p) in sim.particles.iter().take(want).enumerate() {
                    let (world, c, a) = mesh_particle(em, p, &xf, sim.loop_time);
                    commands.entity(pool[k].0).insert(Transform::from_matrix(coords::transform(world)));
                    if let Some(mut m) = volume_mats.get_mut(&pool[k].1) {
                        m.params.mesh_color = Vec4::new(c.x, c.y, c.z, a);
                    }
                }
            } else if let Some((mh, mat)) = lf.mesh_parts.get(&(inst.system, i)) {
                // Mesh particles: one entity each, scaled by the particle size and turned by
                // its mesh rotation, its colour and alpha on its own material copy.
                let pool = &mut inst.pools[i];
                let want = sim.particles.len().min(64);
                while pool.len() < want {
                    let base = mats.get(mat).cloned().unwrap_or_default();
                    let tintc = base.base.base_color.to_linear();
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
                    let (color, alpha, size) = particle_look(em, p, sim.loop_time);
                    let alpha = alpha.clamp(0.0, 1.0);
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
                    if let Some(mut m) = mats.get_mut(&pool[k].1) {
                        m.base.base_color = if em.blend == ParticleBlend::Additive || m.base.alpha_mode == AlphaMode::Add {
                            Color::linear_rgb(color.x * alpha * pool[k].2.red, color.y * alpha * pool[k].2.green, color.z * alpha * pool[k].2.blue)
                        } else {
                            Color::linear_rgba(color.x * pool[k].2.red, color.y * pool[k].2.green, color.z * pool[k].2.blue, alpha * pool[k].2.alpha)
                        };
                    }
                }
            } else if crate::profile::perf_baseline() {
                // Before / after timings: the old per-frame rebuild of every sprite mesh.
                if let Some(mut m) = inst.meshes.get(i).and_then(|h| meshes.get_mut(h)) {
                    build_mesh(em, sim, &xf, right, up, cam_pos_b, lf.erode.contains(&(inst.system, i)), inst.beam_target, &mut m);
                    sim.mesh_empty = sim.particles.is_empty();
                }
            } else if sprite_mesh_used(&lf, inst.system, i) {
                // Rebuilding a mesh re-uploads it: skip emitters that draw nothing (no sprite
                // entity and no distortion copy), meshes that stay empty and, emptied once,
                // sprites entirely outside the view (trails and beams are always rebuilt).
                let strip = em.trail.is_some() || em.beam.is_some();
                sprite_culled = !strip && !sim.particles.is_empty() && sprite_bounds(em, sim, &xf).is_some_and(|(c, r)| !in_view(c, r));
                let empty = sim.particles.is_empty() || sprite_culled;
                if !(empty && sim.mesh_empty) {
                    if let Some(mut m) = inst.meshes.get(i).and_then(|h| meshes.get_mut(h)) {
                        if sprite_culled {
                            clear_sprite_mesh(&mut m);
                        } else {
                            build_mesh(em, sim, &xf, right, up, cam_pos_b, lf.erode.contains(&(inst.system, i)), inst.beam_target, &mut m);
                        }
                        sim.mesh_empty = empty;
                    }
                }
            }
            // The distortion pass draws the same particles with the distortion material
            // (nothing of sprites outside the view).
            if let Some(ds) = lf.distortion.get(&(inst.system, i)).filter(|d| !sim.particles.is_empty() && !(sprite_culled && d.mesh.is_none())) {
                let imgs = (&ds.texture, ds.opacity.as_ref(), ds.uv_distortion.as_ref());
                if let Some(mesh) = &ds.mesh {
                    let (pos, uv, idx) = &**mesh;
                    let tris = sim.particles.iter().take(64).flat_map(|p| {
                        let (world, _, a) = mesh_particle(em, p, &xf, sim.loop_time);
                        // The axis swap mirrors: reverse the winding (no culling, but keep it consistent).
                        idx.chunks_exact(3).flat_map(move |t| [t[0], t[2], t[1]]).filter_map(move |k| {
                            let k = k as usize;
                            Some((coords::pos(world.transform_point3(*pos.get(k)?)), *uv.get(k)?, a))
                        })
                    });
                    dframe.push(&ds.mat, imgs, tris, t_now, true);
                } else if let Some(m) = inst.meshes.get(i).and_then(|h| meshes.get(h)) {
                    use bevy::mesh::VertexAttributeValues as V;
                    if let (Some(V::Float32x3(p)), Some(V::Float32x2(uv)), Some(V::Float32x4(c)), Some(idx)) =
                        (m.attribute(Mesh::ATTRIBUTE_POSITION), m.attribute(Mesh::ATTRIBUTE_UV_0), m.attribute(Mesh::ATTRIBUTE_COLOR), m.indices())
                    {
                        let tris = idx.iter().filter_map(|k| Some((Vec3::from(*p.get(k)?), Vec2::from(*uv.get(k)?), c.get(k)?[3])));
                        dframe.push(&ds.mat, imgs, tris, t_now, false);
                    }
                }
            }
        }
        // The events the emitters generated reach the emitters that receive them (blood drops
        // that strike a surface spawn the decal emitter's particles there).
        dispatch_events(&sys.emitters, &mut inst.sims, &xf, &mut rnd);
        // One-shots end when every emitter is done and empty; an emitter that loops forever
        // (`EmitterLoops` 0, e.g. the Blink indicators) is deactivated after ONE_SHOT_LIMIT, its
        // particles killed (`bKillOnDeactivate`), so no spawned effect outlives its use.
        if inst.one_shot && one_shot_finished(&inst.sims) {
            commands.entity(e).despawn();
        }
    }
    fx.live_particles = live;
    if !dframe.batches.is_empty() && std::env::var_os("DISH_DISTORTION_DEBUG").is_some() {
        info!("distortion frame: {} batches, {} floats", dframe.batches.len(), dframe.verts.len());
    }
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
    mut hands: ResMut<HandEffects>,
    features: Option<Res<crate::map_render::RenderFeatures>>,
    decals: Query<(Entity, &ContactDecal)>,
) {
    let Some(mut view) = view else { return };
    let events = view.audio_events.clone();
    if audio.is_none() {
        // Without the audio plugin nobody else drains the presentation events.
        view.audio_events.clear();
    }
    let now = time.elapsed_secs();
    let me = session.as_ref().and_then(|s| s.local_id());
    let players: Vec<PlayerState> = view.me.iter().chain(view.others.iter()).copied().collect();
    let scene = FxScene { players: &players, me, muzzle: &sockets.muzzle, bones: &sockets.bones, contact: audio.as_ref().and_then(|a| a.contact()), world: session.as_ref().map(|s| &s.content.collision), now, projectiles: &view.projectiles };
    let requests = fx.rules.requests(&events, &scene);
    // A wall of light's shock (`m_ShockParticleSystemTemplate` at the point, the wall's rotation:
    // 0x00A3BCE0 / 0x00A3E630 / 0x00A3E7D0 spawn it there).
    if let Some(s) = session.as_ref() {
        for e in &events {
            let dishonored_sim::events::SimEvent::WallOfLightShock { wall, point, particles: true } = *e else { continue };
            let Some(d) = s.content.world_actors.walls_of_light.get(wall as usize).filter(|d| !d.shock_particles.is_empty()) else { continue };
            spawn_effect(&mut commands, &mut meshes, &mut fx, &d.shock_particles, point, d.facing);
        }
    }
    // `DisDecalManager`'s pool: (entity, made, last seen).
    let mut pool: Vec<(Entity, f32, f32)> = decals.iter().map(|(e, d)| (e, d.born, d.seen)).collect();
    let decals_on = features.as_deref().is_none_or(|f| f.dynamic_decals);
    for r in requests {
        match r {
            FxRequest::Decal { material, at, normal, dir, width, height, lifetime } => {
                if !decals_on {
                    continue;
                }
                let Some(mat) = fx.levels.get(&fx.active).and_then(|lf| lf.decals.get(&material.to_ascii_lowercase())).cloned() else {
                    warn!("effects: decal material {material} not decoded for {}", fx.active);
                    continue;
                };
                // `FUN_00bc6480`: with `MAX_DECAL_COUNT` live, the first one not rendered for more
                // than `DECAL_EVICT_AFTER` seconds goes; none to give up, no new decal.
                if pool.len() >= dishonored_audio::contact::MAX_DECAL_COUNT {
                    let victim = pool.iter().filter(|d| now - d.2 > dishonored_audio::contact::DECAL_EVICT_AFTER).min_by(|a, b| a.1.total_cmp(&b.1)).map(|d| d.0);
                    let Some(victim) = victim else { continue };
                    commands.entity(victim).despawn();
                    pool.retain(|d| d.0 != victim);
                }
                let e = commands
                    .spawn((Mesh3d(meshes.add(decal_mesh(at, normal, dir, width, height))), MeshMaterial3d(mat), Transform::IDENTITY, bevy::light::NotShadowCaster, LevelOf(fx.active.clone()), ContactDecal { born: now, life: lifetime, seen: -1000.0 }))
                    .id();
                pool.push((e, now, -1000.0));
                fx.spawned_decals += 1;
            }
            FxRequest::Effect { path, at, dir } => {
                spawn_effect(&mut commands, &mut meshes, &mut fx, &path, at, dir);
            }
            FxRequest::Beam { path, from, to } => {
                spawn_beam(&mut commands, &mut meshes, &mut fx, path, from, to);
            }
            FxRequest::EndedTrail { path, xf } => {
                let Some(lf) = fx.levels.get(&fx.active).cloned() else { continue };
                let Some(&si) = lf.by_path.get(&path.to_ascii_lowercase()) else { continue };
                let level = fx.active.clone();
                if let Some(e) = spawn_instance(&mut commands, &mut meshes, &lf, &level, si, xf, true) {
                    commands.entity(e).insert(StopAtOnce);
                }
            }
            // The local player's cast effects ride the first-person rig's hand socket.
            FxRequest::HandCast { path, at, dir } => {
                if hands.sockets.contains_key(HAND_CAST_SOCKET) {
                    hands.pending.push((path, HAND_CAST_SOCKET.into(), true));
                } else {
                    spawn_effect(&mut commands, &mut meshes, &mut fx, &path, at, dir);
                }
            }
        }
    }
}

/// Blink's targeting indicator (`Twk_Blink`, EFFECTS.md "Gameplay effects"): while the local
/// player aims Blink (`left.blink_target`), one instance of `m_pGroundPS` stands on the floor
/// under the target, or `m_pLowFallPS` floats at a target in the air; it follows the target
/// every frame and is killed when aiming ends (release, cancel, travel), as its emitters
/// (`bKillOnDeactivate`, endless loops) are deactivated by the power.
fn blink_indicator(mut commands: Commands, mut fx: ResMut<Effects>, mut meshes: ResMut<Assets<Mesh>>, view: Option<Res<SessionView>>, session: Option<Res<crate::session::Session>>, mut q: Query<&mut FxInstance>) {
    let me = view.as_ref().and_then(|v| v.me.as_ref());
    let want = session.as_ref().and_then(|s| blink_indicator_want(me, &s.content.collision, &s.content.sim.gameplay.movement));
    let current = fx.indicator.take();
    let Some((path, at)) = want else {
        if let Some((e, _)) = current {
            commands.entity(e).try_despawn();
        }
        return;
    };
    let m = Mat4::from_translation(at);
    if let Some((e, p)) = current {
        if p == path {
            if let Ok(mut inst) = q.get_mut(e) {
                inst.set_transform(m);
                fx.indicator = Some((e, p));
                return;
            }
        } else {
            commands.entity(e).try_despawn();
        }
    }
    let Some(lf) = fx.levels.get(&fx.active).cloned() else { return };
    let Some(&si) = lf.by_path.get(&path.to_ascii_lowercase()) else { return };
    let level = fx.active.clone();
    if let Some(e) = spawn_instance(&mut commands, &mut meshes, &lf, &level, si, m, false) {
        fx.indicator = Some((e, path));
    }
}

/// Effects that follow a live object, kept alive while it exists: projectile trails along the
/// flight, and Dark Vision's vision cones on the revealed NPCs while the power is on. A trail
/// whose projectile is gone stops spawning and fades out; a cone goes with the power.
fn attached_effects(mut commands: Commands, mut fx: ResMut<Effects>, mut meshes: ResMut<Assets<Mesh>>, view: Option<Res<SessionView>>, mut q: Query<&mut FxInstance>) {
    let Some(view) = view else { return };
    let want = attached_want(view.me.as_ref(), &view.others, &view.projectiles, &view.dark_vision_reveals);
    let gone: Vec<(u8, u32)> = fx.attached.keys().filter(|k| !want.iter().any(|w| w.0 == **k)).copied().collect();
    for k in gone {
        if let Some((e, _)) = fx.attached.remove(&k) {
            match q.get_mut(e) {
                // Trails fade out (their particles live on); cones are killed.
                Ok(mut inst) if k.0 == 0 => inst.stop(),
                _ => {
                    commands.entity(e).try_despawn();
                }
            }
        }
    }
    let Some(lf) = fx.levels.get(&fx.active).cloned() else { return };
    let level = fx.active.clone();
    for (k, path, m) in want {
        if let Some((e, _)) = fx.attached.get(&k) {
            if let Ok(mut inst) = q.get_mut(*e) {
                inst.set_transform(m);
                continue;
            }
        }
        let Some(&si) = lf.by_path.get(&path.to_ascii_lowercase()) else { continue };
        if let Some(e) = spawn_instance(&mut commands, &mut meshes, &lf, &level, si, m, false) {
            fx.attached.insert(k, (e, path));
        }
    }
}

/// Particle notifies of the first-person rig (`AnimNotify_PlayParticleEffect` of the player
/// sets: the Mark of the Outsider's glows on the `Tattoo` socket, the swimming splashes on the
/// fingers) and the rig's socket / bone transforms this frame (Dishonored world space, scaled
/// with the rig), filled by `viewmodel::animate_view_rig`.
#[derive(Resource, Default)]
pub struct HandEffects {
    /// (system path, socket or bone name, attached) to start.
    pub pending: Vec<(String, String, bool)>,
    /// Socket or bone name -> world transform.
    pub sockets: HashMap<String, Mat4>,
    /// Live attached instances and their socket.
    live: Vec<(Entity, String)>,
}

fn hand_effects(mut commands: Commands, mut fx: ResMut<Effects>, mut meshes: ResMut<Assets<Mesh>>, mut hands: ResMut<HandEffects>, mut q: Query<&mut FxInstance>) {
    let h = &mut *hands;
    // Attached instances follow their socket while they live (one-shots despawn themselves).
    h.live.retain(|(e, s)| match (q.get_mut(*e), h.sockets.get(s)) {
        (Ok(mut inst), Some(m)) => {
            inst.set_transform(*m);
            true
        }
        (Ok(mut inst), None) => {
            inst.stop();
            false
        }
        _ => false,
    });
    let pending = std::mem::take(&mut h.pending);
    let Some(lf) = fx.levels.get(&fx.active).cloned() else { return };
    let level = fx.active.clone();
    for (path, socket, attach) in pending {
        let Some(m) = h.sockets.get(&socket).copied() else { continue };
        let Some(&si) = lf.by_path.get(&path.to_ascii_lowercase()) else {
            warn!("effects: hand effect {path} not decoded for {level}");
            continue;
        };
        if let Some(e) = spawn_instance(&mut commands, &mut meshes, &lf, &level, si, m, true) {
            fx.spawned_oneshots += 1;
            if attach {
                h.live.push((e, socket));
            }
        }
    }
}

/// The live Blink indicator, if any: (system path, entity) (harness `effects` command).
pub fn blink_indicator_live(fx: &Effects) -> Option<&'static str> {
    fx.indicator.map(|i| i.1)
}

/// World transform (Dishonored space), colour and alpha of a mesh-emitter particle: the mesh
/// scaled by the particle size (and size over life), turned by its mesh rotation.
fn mesh_particle(em: &ParticleEmitterAsset, p: &Particle, xf: &Mat4, et: f32) -> (Mat4, Vec3, f32) {
    let (color, alpha, size) = particle_look(em, p, et);
    let rot = Mat4::from_rotation_z(p.mrot.z) * Mat4::from_rotation_y(p.mrot.y) * Mat4::from_rotation_x(p.mrot.x);
    let world = if em.local_space {
        *xf * Mat4::from_translation(p.pos) * rot * Mat4::from_scale(size)
    } else {
        let (_, r, _) = xf.to_scale_rotation_translation();
        Mat4::from_translation(p.pos) * Mat4::from_quat(r) * rot * Mat4::from_scale(size)
    };
    (world, color, alpha.clamp(0.0, 1.0))
}

/// An AnimTrail's strip (Bevy space): each knot (oldest first) spans its two edges; U runs along
/// the trail (the tiling over its length), V across (0 at the first edge). With render
/// tessellation a segment longer than the step is subdivided on the Hermite spline of its knots,
/// whose tangents are recalculated from the neighbouring knots (`bTangentRecalculationEveryFrame`).
#[allow(clippy::too_many_arguments)]
fn anim_trail_strip(t: &dishonored_ir::particles::TrailParams, sim: &EmitterSim, look: &dyn Fn(&Particle) -> (Vec3, f32, Vec3), pos: &mut Vec<[f32; 3]>, uv: &mut Vec<[f32; 2]>, col: &mut Vec<[f32; 4]>, idx: &mut Vec<u32>) {
    let knots = &sim.particles;
    if knots.len() < 2 {
        return;
    }
    let colour = |p: &Particle| {
        let (c, a, _) = look(p);
        [c.x.max(0.0), c.y.max(0.0), c.z.max(0.0), a.clamp(0.0, 1.0)]
    };
    let edge = |k: usize, i: usize| if i == 0 { knots[k].pos } else { knots[k].edge2 };
    let tangent = |k: usize, i: usize| (edge((k + 1).min(knots.len() - 1), i) - edge(k.saturating_sub(1), i)) * if k == 0 || k + 1 == knots.len() { 1.0 } else { 0.5 };
    // (first edge, second edge, colour) along the trail.
    let mut rows: Vec<(Vec3, Vec3, [f32; 4])> = Vec::with_capacity(knots.len());
    for k in 0..knots.len() {
        rows.push((edge(k, 0), edge(k, 1), colour(&knots[k])));
        if k + 1 == knots.len() {
            break;
        }
        let len = (edge(k + 1, 0) - edge(k, 0)).length().max((edge(k + 1, 1) - edge(k, 1)).length());
        let n = t.tessellation_step.filter(|s| *s > 1e-3).map_or(1, |s| (len / s).ceil().clamp(1.0, 16.0) as usize);
        let (c0, c1) = (colour(&knots[k]), colour(&knots[k + 1]));
        for j in 1..n {
            let f = j as f32 / n as f32;
            let e = |i: usize| hermite(edge(k, i), tangent(k, i), edge(k + 1, i), tangent(k + 1, i), f);
            let c = [0, 1, 2, 3].map(|q| c0[q] + (c1[q] - c0[q]) * f);
            rows.push((e(0), e(1), c));
        }
    }
    let base = pos.len() as u32;
    let total = (rows.len() - 1) as f32;
    for (k, (a, b, c)) in rows.iter().enumerate() {
        // Newest knot first along U.
        let u = (1.0 - k as f32 / total) * t.tiling.max(1e-3);
        pos.push(coords::pos(*a).to_array());
        pos.push(coords::pos(*b).to_array());
        uv.push([u, 0.0]);
        uv.push([u, 1.0]);
        col.push(*c);
        col.push(*c);
    }
    for k in 0..rows.len() as u32 - 1 {
        let a = base + k * 2;
        idx.extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
    }
}

/// Trail / ribbon and beam emitters as camera-facing strips (UE3 `TypeDataTrail2` /
/// `TypeDataRibbon`: the emitter's particles in spawn order joined into one strip, width the
/// particle size; `TypeDataBeam2`: each particle a beam from the emitter origin along its X
/// axis by `Distance` or to the `BeamTarget` point, displaced by `BeamNoise`). The texture runs
/// along the strip (U) once per segment set by the tiling.
#[allow(clippy::type_complexity)]
fn strip_mesh(em: &ParticleEmitterAsset, sim: &EmitterSim, xf: &Mat4, cam_pos: Vec3, look: &dyn Fn(&Particle) -> (Vec3, f32, Vec3), world_of: &dyn Fn(&Particle) -> Vec3, beam_target: Option<Vec3>, mesh: &mut Mesh) {
    let mut pos: Vec<[f32; 3]> = Vec::new();
    let mut uv: Vec<[f32; 2]> = Vec::new();
    let mut col: Vec<[f32; 4]> = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    if let Some(t) = em.trail.as_ref().filter(|t| t.anim) {
        anim_trail_strip(t, sim, look, &mut pos, &mut uv, &mut col, &mut idx);
    }
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
    if let Some(t) = em.trail.as_ref().filter(|t| !t.anim) {
        let pts: Vec<(Vec3, f32, [f32; 4])> = sim.particles.iter().map(point).collect();
        strip(&pts, t.tiling.max(1e-3));
    } else if em.trail.is_some() {
    } else if let Some(b) = &em.beam {
        let origin = xf.w_axis.truncate();
        for (i, p) in sim.particles.iter().enumerate() {
            let (_, hw, colr) = point(p);
            let end = match (beam_target, &b.target, &b.distance) {
                (Some(t), _, _) => t,
                (None, Some(t), _) => xf.transform_point3(Vec3::from(t.eval(sim.time, p.r[0]))),
                (None, None, Some(d)) => origin + xf.x_axis.truncate().normalize_or_zero() * d.eval1(sim.time, p.r[0]),
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
