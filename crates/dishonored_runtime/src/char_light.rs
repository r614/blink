//! Characters (NPCs, the player's third-person body, ragdolls) lit by their light environment.
//!
//! The original gives every pawn a `DynamicLightEnvironmentComponent` (`MyLightEnvironment`: class
//! defaults, `bDynamic`, `bSynthesizeDirectionalLight`, plus `bSynthesizeSHLight` on
//! `DishonoredPawn`, `InvisibleUpdateTime` 5, `MinTimeBetweenFullUpdates` 1, `WorldInfo`
//! `bAllowLightEnvSphericalHarmonicLights` true) whose update (`FUN_005035d0`, research/dishonored/
//! systems/render_lighting.md "Characters") this module follows:
//!
//! - **Static environment** (`dishonored_assets::light_env::static_environment`, 0x00501B70): the
//!   level's static lights that reach the bounds and see them (line check, `TRACE_ShadowCast`:
//!   `channel::SHADOW`) plus the floor level's precomputed light volume, as an SH.
//! - **Update rate**: the environment is recomputed when `now - last update` exceeds
//!   `rand(0.8, 1.2) x (visible ? MinTimeBetweenFullUpdates : InvisibleUpdateTime) x clamp(distance
//!   to the viewpoint / (4 x bounds radius), 1, 10)`, and only if the bounds moved that tick. "Visible" is
//!   rendered within the last second. Not every frame, and nothing per entity per frame.
//! - **Blend** (0x004E18F0): the current SH moves linearly to the new target over the interval
//!   (`alpha = dt / time left`); here that runs on the GPU (`char_light.wgsl`: previous SH, target SH,
//!   start and end time), with the directional + SH-light synthesis (0x004F85C0) done per pixel
//!   on the interpolated SH, as the original does every tick.
//! - **Shading**: the SH light's base pass `TBasePassPixelShader<FSHLightLightMapPolicy>`
//!   (`char_light.wgsl`); the dominant and toggleable lights stay Bevy lights.
//!
//! Not modelled (render_lighting.md): the dynamic environment (toggleable and movable lights
//! composited into the SH: the port draws them as lights, so a character near one gets it
//! twice), `bIsCharacterLightEnvironment`'s contrast factor (off on pawns), the dominant light's
//! modulated shadow transition and `AmbientShadowColor` (Bevy's shadow maps cast the shadow).

use crate::characters::CharacterRoot;
use crate::players::GameCamera;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_assets::light_env::{EnvLight, EnvQuery, Sh, VolumeSample, static_environment};
use dishonored_ir::env_light::LightEnvWorld;
use dishonored_world::WorldQuery;

/// `DynamicLightEnvironmentComponent` class defaults (`Default__DynamicLightEnvironmentComponent`,
/// Engine; `MyLightEnvironment` of the pawns overrides neither).
pub const INVISIBLE_UPDATE_TIME: f32 = 5.0;
pub const MIN_TIME_BETWEEN_FULL_UPDATES: f32 = 1.0;
/// The owner component's lighting channels without an override (`bInitialized`, `Dynamic`).
const OWNER_CHANNELS: u32 = 1 | (1 << 3);
/// Rendered this recently counts as visible (0x005035d0: `now - LastRenderTime < 1`).
const VISIBLE_WINDOW: f32 = 1.0;
/// How often an entity's visibility is looked at (a child scan), seconds.
const VISIBILITY_POLL: f32 = 0.25;
/// `Time::elapsed_secs_wrapped`'s period (the shader's clock).
const WRAP: f32 = 3600.0;

#[derive(Clone, Copy, Debug, PartialEq, ShaderType)]
pub struct CharLightUniform {
    pub sh_a: [Vec4; 9],
    pub sh_b: [Vec4; 9],
    pub time: Vec4,
    pub spec: Vec4,
    pub spec_color: Vec4,
    pub spec_mask: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct CharLightExt {
    #[uniform(100)]
    pub params: CharLightUniform,
    #[texture(101)]
    #[sampler(102)]
    pub specular: Option<Handle<Image>>,
}

impl MaterialExtension for CharLightExt {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/char_light.wgsl".into()
    }
}

pub type CharMaterial = ExtendedMaterial<StandardMaterial, CharLightExt>;

/// The static lights and light volumes of the active level.
#[derive(Resource, Default)]
pub struct CharLightWorld {
    lights: Vec<EnvLight>,
    /// (level package, samples).
    volumes: Vec<(String, Vec<VolumeSample>)>,
    /// The persistent level (`L_Boyle_Ext_P`): the volume when the floor's level has none.
    persistent: String,
}

impl CharLightWorld {
    pub fn from_ir(map: &str, w: &LightEnvWorld) -> Self {
        Self {
            lights: w.lights.iter().map(EnvLight::from_data).collect(),
            volumes: w.volumes.iter().map(|v| (v.level.clone(), v.samples.iter().map(VolumeSample::from_data).collect())).collect(),
            persistent: map.to_string(),
        }
    }

    /// Whether the level has anything to light characters with (without it they keep Bevy's lighting).
    pub fn active(&self) -> bool {
        !self.lights.is_empty() || !self.volumes.is_empty()
    }

    fn volume(&self, level: Option<&str>) -> Option<&[VolumeSample]> {
        let by = |name: &str| self.volumes.iter().find(|v| v.0.eq_ignore_ascii_case(name)).map(|v| v.1.as_slice());
        level.and_then(by).or_else(|| by(&self.persistent)).or_else(|| self.volumes.first().map(|v| v.1.as_slice()))
    }
}

/// A character with its light environment: the materials it is drawn with and the state of the
/// update schedule (`FDynamicLightEnvironmentState`).
#[derive(Component)]
pub struct CharLight {
    mats: Vec<Handle<CharMaterial>>,
    sphere: (Vec3, f32),
    init: bool,
    /// Seconds (virtual time) of the last full update.
    last_update: f32,
    /// `rand(0.8, 1.2) x InvisibleUpdateTime` and `x MinTimeBetweenFullUpdates` (state +0x38 / +0x3c).
    iv_invisible: f32,
    iv_visible: f32,
    /// The SH when the last interpolation started, its target and the interval it runs over.
    from: Sh,
    target: Sh,
    start: f32,
    end: f32,
    visible: bool,
    /// When a mesh of it was last in view (`LastRenderTime`).
    last_seen: f32,
    next_poll: f32,
    rng: u32,
}

impl CharLight {
    fn new(mats: Vec<Handle<CharMaterial>>, sphere: (Vec3, f32), entity: Entity, always_visible: bool) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        entity.hash(&mut h);
        CharLight {
            mats,
            sphere,
            init: false,
            last_update: 0.0,
            iv_invisible: 0.0,
            iv_visible: 0.0,
            from: [[0.0; 9]; 3],
            target: [[0.0; 9]; 3],
            start: 0.0,
            end: 0.0,
            visible: true,
            last_seen: 0.0,
            next_poll: if always_visible { f32::MAX } else { 0.0 },
            rng: (h.finish() as u32).max(1),
        }
    }

    fn rand(&mut self) -> f32 {
        // xorshift32 in [0, 1).
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / 16_777_216.0
    }

    /// `rand x 0.4 + 0.8` times a class time.
    fn jitter(&mut self, base: f32) -> f32 {
        (self.rand() * 0.4 + 0.8) * base
    }

    /// The interpolated SH at `now`: the state 0x004E18F0 holds (`alpha` = fraction of the interval gone).
    fn current(&self, now: f32) -> Sh {
        let a = if self.end > self.start { ((now - self.start) / (self.end - self.start)).clamp(0.0, 1.0) } else { 1.0 };
        let mut out = self.target;
        for c in 0..3 {
            for k in 0..9 {
                out[c][k] = self.from[c][k] + (self.target[c][k] - self.from[c][k]) * a;
            }
        }
        out
    }
}

/// The extension of a section material before its first environment (the specular terms are the
/// material's; the SH is zero and the environment off until the first update).
fn neutral_ext(p: Option<&dishonored_ir::MaterialParams>, textures: &[Handle<Image>]) -> CharLightExt {
    let (spec, spec_color, spec_mask, specular) = crate::prop_light::specular_terms(p, textures);
    CharLightExt { params: CharLightUniform { sh_a: [Vec4::ZERO; 9], sh_b: [Vec4::ZERO; 9], time: Vec4::ZERO, spec, spec_color, spec_mask }, specular }
}

fn sh_vec4(sh: &Sh, unit: f32) -> [Vec4; 9] {
    std::array::from_fn(|k| Vec4::new(sh[0][k] * unit, sh[1][k] * unit, sh[2][k] * unit, 0.0))
}

/// Dishonored world position (uu) of a Bevy point.
fn to_ue(p: Vec3) -> glam::Vec3 {
    glam::Vec3::new(p.x, p.z, p.y) / crate::coords::UU_TO_BEVY
}

pub struct CharLightPlugin;

impl Plugin for CharLightPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "char_light.wgsl");
        app.add_plugins(MaterialPlugin::<CharMaterial>::default()).init_resource::<CharLightWorld>().add_systems(Update, (upgrade_characters, update_characters, light_view_rig).chain());
    }
}

/// Swap the materials of freshly spawned characters for light-environment ones (their meshes are
/// matched with the model's sections; weapons and accessories keep theirs).
#[allow(clippy::too_many_arguments)]
fn upgrade_characters(
    mut commands: Commands,
    world: Res<CharLightWorld>,
    features: Option<Res<crate::map_render::RenderFeatures>>,
    lib: Option<Res<crate::characters::CharacterLibrary>>,
    std_mats: Res<Assets<StandardMaterial>>,
    mut mats: ResMut<Assets<CharMaterial>>,
    roots: Query<(Entity, &CharacterRoot, &Children), Without<CharLight>>,
    meshes: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>)>,
) {
    let Some(lib) = lib else { return };
    let on = features.as_deref().is_none_or(|f| f.materials && f.dynamic_lights && !f.debug_lighting);
    if !on || !world.active() {
        return;
    }
    for (root, cr, children) in &roots {
        let Some(gpu) = lib.models.get(cr.archetype) else { continue };
        let mut handles = Vec::new();
        for child in children.iter() {
            let Ok((mesh, mat)) = meshes.get(child) else { continue };
            let Some((pi, si)) = gpu.parts.iter().enumerate().find_map(|(pi, p)| p.sections.iter().position(|s| s.0 == mesh.0).map(|si| (pi, si))) else { continue };
            let Some(base) = std_mats.get(&mat.0) else { continue };
            let params = gpu.parts[pi].section_materials.get(si).copied().flatten().and_then(|m| gpu.material_params.get(m));
            let ext = neutral_ext(params, &gpu.textures);
            let h = mats.add(CharMaterial { base: base.clone(), extension: ext });
            commands.entity(child).remove::<MeshMaterial3d<StandardMaterial>>().insert(MeshMaterial3d(h.clone()));
            handles.push(h);
        }
        if handles.is_empty() {
            continue;
        }
        commands.entity(root).insert(CharLight::new(handles, gpu.sphere, root, false));
    }
}

/// `FUN_005035d0` for one dynamic environment: the schedule, the static environment and the
/// hand-over to the shader. `origin` / `radius` are the owner's bounds (uu, Dishonored space).
#[allow(clippy::too_many_arguments)]
fn step(st: &mut CharLight, now: f32, origin: glam::Vec3, radius: f32, eyes: &[glam::Vec3], world: &CharLightWorld, collision: &dishonored_world::CollisionWorld, mats: &mut Assets<CharMaterial>, unit: f32) {
    let first = !st.init;
    // Distance scale: the nearest viewpoint's distance in 4 bounds radii, 1 .. 10 (0x005035d0).
    let d2 = eyes.iter().map(|e| e.distance_squared(origin)).fold(f32::MAX, f32::min);
    let d = if eyes.is_empty() { 0.0 } else { d2.sqrt() };
    let scale = (d / (radius * 4.0).max(1e-5)).clamp(1.0, 10.0);
    let interval = if st.visible { st.iv_visible } else { st.iv_invisible } * scale;
    if !first && now - st.last_update <= interval {
        return;
    }
    // The interval elapsed: the clock restarts and the intervals are rolled again (0x005035d0
    // does both whether or not the bounds moved).
    st.last_update = now;
    st.iv_visible = st.jitter(MIN_TIME_BETWEEN_FULL_UPDATES);
    st.iv_invisible = st.jitter(INVISIBLE_UPDATE_TIME);
    // 0x005035d0 recomputes the static environment only if the bounds changed that tick. A
    // posed pawn's bounds (the skeletal mesh component's) follow its animation and change
    // every frame, so the recompute is always due here (a corpse at rest would keep its
    // environment: LIKELY the same result, its light does not change).
    let interval = if st.visible { st.iv_visible } else { st.iv_invisible } * scale;
    let level = collision.hit_package(origin, -glam::Vec3::Z, 262_144.0);
    let q = EnvQuery { origin, sphere_radius: radius, channels: OWNER_CHANNELS, lights: &world.lights, volume: world.volume(level.as_deref()), debug: false, level: level.as_deref() };
    let target = static_environment(&q, |a, b| {
        let v = b - a;
        let len = v.length();
        len < 1e-3 || collision.raycast(a, v / len, len - 1.0, dishonored_world::channel::SHADOW).is_none()
    });
    st.from = if first { target } else { st.current(now) };
    st.target = target;
    st.start = now;
    st.end = now + if first { 0.0 } else { interval };
    st.init = true;
    let (a, b, time) = (sh_vec4(&st.from, unit), sh_vec4(&st.target, unit), Vec4::new(st.start % WRAP, st.end % WRAP, 0.0, 1.0));
    for h in &st.mats {
        if let Some(mut m) = mats.get_mut(h) {
            let p = &m.extension.params;
            let params = CharLightUniform { sh_a: a, sh_b: b, time, spec: p.spec, spec_color: p.spec_color, spec_mask: p.spec_mask };
            m.extension.params = params;
        }
    }
}

/// The characters' dynamic environments.
#[allow(clippy::too_many_arguments)]
fn update_characters(
    time: Res<Time>,
    world: Res<CharLightWorld>,
    session: Option<Res<crate::session::Session>>,
    cams: Query<&GlobalTransform, With<GameCamera>>,
    mut roots: Query<(&GlobalTransform, &Children, &mut CharLight, Option<&crate::npc_anim::PawnAnim>), Without<crate::viewmodel::ViewRoot>>,
    views: Query<&ViewVisibility>,
    mut mats: ResMut<Assets<CharMaterial>>,
) {
    let Some(session) = session else { return };
    let now = time.elapsed_secs();
    let unit = crate::map_render::light_unit();
    let eyes: Vec<glam::Vec3> = cams.iter().map(|c| to_ue(c.translation())).collect();
    for (gt, children, mut st, anim) in &mut roots {
        // The bounds: the bind pose's sphere about the pawn; a body in its ragdoll (a corpse, a
        // thrown pawn) is bounded where its bodies are (LIKELY: the original's bounds follow the
        // posed mesh).
        let centre = anim.and_then(|a| a.ragdoll.as_ref()).map_or(st.sphere.0, |r| r.center());
        let origin = to_ue(gt.transform_point(crate::coords::pos(centre)));
        let radius = st.sphere.1;
        if now >= st.next_poll {
            st.next_poll = now + VISIBILITY_POLL;
            if children.iter().any(|c| views.get(c).is_ok_and(|v| v.get())) {
                st.last_seen = now;
            }
            st.visible = now - st.last_seen < VISIBLE_WINDOW;
        }
        step(&mut st, now, origin, radius, &eyes, &world, &session.content.collision, &mut mats, unit);
    }
}

/// The player's arms (`DishonoredPlayerPawn.pMesh`, first person) share the pawn's light
/// environment: `BoundsMethod` `DLEB_ManualOverride`, the bounds the pawn copies from its collision
/// cylinder every tick (the stealth light environment's: stealth.md section 10), so the origin is
/// the pawn's position and the sphere the cylinder's box (extent (r, r, half height)).
#[allow(clippy::too_many_arguments)]
fn light_view_rig(
    mut commands: Commands,
    time: Res<Time>,
    world: Res<CharLightWorld>,
    features: Option<Res<crate::map_render::RenderFeatures>>,
    session: Option<Res<crate::session::Session>>,
    view: Option<Res<crate::session::SessionView>>,
    rig: Option<Res<crate::viewmodel::ViewRig>>,
    cams: Query<&GlobalTransform, With<GameCamera>>,
    std_mats: Res<Assets<StandardMaterial>>,
    mut mats: ResMut<Assets<CharMaterial>>,
    mut roots: Query<(Entity, &Children, Option<&mut CharLight>), With<crate::viewmodel::ViewRoot>>,
    meshes: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>)>,
) {
    let (Some(rig), Some(session), Some(view)) = (rig, session, view) else { return };
    let on = features.as_deref().is_none_or(|f| f.materials && f.dynamic_lights && !f.debug_lighting);
    if !on || !world.active() {
        return;
    }
    let Some(me) = view.me.as_ref() else { return };
    let mv = &session.content.sim.gameplay.movement;
    let (r, h) = (mv.collision_radius, mv.collision_half_height);
    let radius = (2.0 * r * r + h * h).sqrt();
    let now = time.elapsed_secs();
    let unit = crate::map_render::light_unit();
    let eyes: Vec<glam::Vec3> = cams.iter().map(|c| to_ue(c.translation())).collect();
    for (root, children, st) in &mut roots {
        match st {
            Some(mut st) => step(&mut st, now, me.position, radius, &eyes, &world, &session.content.collision, &mut mats, unit),
            None => {
                let gpu = rig.arms();
                let mut handles = Vec::new();
                for child in children.iter() {
                    let Ok((mesh, mat)) = meshes.get(child) else { continue };
                    let Some((pi, si)) = gpu.parts.iter().enumerate().find_map(|(pi, p)| p.sections.iter().position(|s| s.0 == mesh.0).map(|si| (pi, si))) else { continue };
                    let Some(base) = std_mats.get(&mat.0) else { continue };
                    let params = gpu.parts[pi].section_materials.get(si).copied().flatten().and_then(|m| gpu.material_params.get(m));
                    let h = mats.add(CharMaterial { base: base.clone(), extension: neutral_ext(params, &gpu.textures) });
                    commands.entity(child).remove::<MeshMaterial3d<StandardMaterial>>().insert(MeshMaterial3d(h.clone()));
                    handles.push(h);
                }
                if !handles.is_empty() {
                    // Always drawn: visible.
                    commands.entity(root).insert(CharLight::new(handles, (Vec3::ZERO, radius), root, true));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blend_is_linear_over_the_interval() {
        let mut s = CharLight {
            mats: Vec::new(),
            sphere: (Vec3::ZERO, 1.0),
            init: true,
            last_update: 0.0,
            iv_invisible: 5.0,
            iv_visible: 1.0,
            from: [[0.0; 9]; 3],
            target: [[2.0; 9]; 3],
            start: 10.0,
            end: 12.0,
            visible: true,
            last_seen: 0.0,
            next_poll: 0.0,
            rng: 1,
        };
        assert_eq!(s.current(9.0)[0][0], 0.0);
        assert!((s.current(11.0)[1][3] - 1.0).abs() < 1e-6);
        assert_eq!(s.current(13.0)[2][8], 2.0);
        // The jitter is 0.8 .. 1.2 of the class time.
        for _ in 0..100 {
            let j = s.jitter(1.0);
            assert!((0.8..=1.2).contains(&j));
        }
    }
}
