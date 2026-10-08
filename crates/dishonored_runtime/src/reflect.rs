//! Planar reflections: the level's `SceneCaptureReflectActor`s (research/dishonored/systems/
//! render_reflect.md).
//!
//! Each actor draws the scene from the viewer mirrored about its plane into a
//! `TextureRenderTarget2D` (`boyle_ext_MS.RealTime_refl`, `Hub_MS.water_RTR`, ...), which the water
//! materials sample as `C_Realtime_reflection_Texture` at the screen position (`ark_water.wgsl`,
//! `water_foam.wgsl`). Every rule below is read from the actor's data or from the executable:
//!
//! - The plane is the actor's X axis through its location (`SceneCaptureReflectComponent` vtable
//!   slot 88, 0x006E8200: the normal is the owner's rotated axis, `d = normal . Location`). All 38
//!   placed actors have pitch 16384, so the plane is horizontal at the actor's Z.
//! - Only primitives whose `ReflectionChannels` share a bit with the capture's are drawn
//!   (`MeshInstance::reflect_channels`, `ReflectCapture::channels`): the capture sets only
//!   `Group_1` (the class default BSP / Static / Dynamic / Skeletal / Particles / Decals are
//!   cleared) and a primitive carries it only where the level's artists put it (Boyle Ext: the
//!   sky dome, the canal walls, the sewer pieces, the handrails). A capture's channel set is a
//!   render layer here; the flagged primitives are in layer 0 (the main view) and in theirs.
//! - `SceneCaptureComponent` defaults: lit without shadows (`SceneCapView_LitNoShadows`: no light
//!   reaches the reflection camera, which sees lightmaps and ambient only), black clear colour, no
//!   post-process (`bEnablePostProcess`) and no fog (`bEnableFog`) except where the actor sets them.
//! - The target is a `TextureRenderTarget2D` of `PF_A8R8G8B8` with `m_ResolutionType`
//!   `TRT_HALFSIZE` (every placed target): half the back buffer in each direction.
//! - `FrameRate` limits the captures per second, `bSkipUpdateIfTextureUsersOccluded` skips the
//!   capture while no primitive that samples the target is visible, `MaxUpdateDist` (0: unlimited)
//!   skips it when the viewer is further than that from the plane, `bEnabled` gates it.
//!
//! The mirrored camera has the reflected position, forward and up. Reflecting a basis flips its
//! handedness, so the camera built from them is the true mirror view flipped in x; the water
//! shaders sample the target at `1 - u`. The near plane is the mirror plane (the projection's
//! oblique `near_clip_plane`, Lengyel 2005), so geometry below the water surface is cut off.
//!
//! Fog and post-process of the captures with `bEnableFog` (Boyle Ext, Isl Geom Master / Slave) and
//! `bEnablePostProcess` (Isl): the capture camera carries its own `ArkPost` pass with the level's DisFog
//! layers and ArkPp settings (no bloom parts; LIKELY, U-960). Kismet's `SeqAct_Toggle` on a capture actor
//! (`SceneCaptureActor.OnToggle`: the Streetsewer and Prison sewer captures start disabled) reaches the
//! cameras through `MissionView::captures` (U-962). A `SceneCapture2DActor` (the Tower Return lens TV) is
//! a capture with a `FixedView` instead of a mirror plane, drawing the whole scene (`tv_screen.rs` shows
//! it). Not rendered: `generic_TRTC`, the cube render target no actor draws into (U-961).

use crate::coords;
use crate::ark_post::ArkPostSource;
use crate::map_render::{MapGeometry, RenderFeatures};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, PerspectiveProjection, Projection, RenderTarget};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use dishonored_ir::DishonoredMap;
use std::collections::HashMap;

/// The texture parameter of the water materials that holds the capture's render target.
pub const REALTIME_PARAM: &str = "C_Realtime_reflection_Texture";

/// The capture's render target format (`TextureRenderTarget2D::Format` default `PF_A8R8G8B8`; sRGB
/// storage so the 8-bit values keep their precision in the darks and read back linear).
const TARGET_FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;

/// Render layers of a map's capture channel sets: the distinct non-empty `ReflectCapture::channels`
/// are layers 1, 2, ...; a primitive is drawn in layer `k + 1` when its channels share a bit with
/// set `k`.
#[derive(Clone, Debug, Default)]
pub struct Layers {
    masks: Vec<u32>,
}

impl Layers {
    /// Capture layers beyond this are dropped (a map has at most a handful of channel sets).
    const MAX: usize = 30;

    pub fn new(map: &DishonoredMap, features: &RenderFeatures) -> Self {
        let mut masks: Vec<u32> = Vec::new();
        if features.reflections {
            for c in &map.reflect_captures {
                if c.channels != 0 && !masks.contains(&c.channels) && masks.len() < Self::MAX {
                    masks.push(c.channels);
                }
            }
        }
        Self { masks }
    }

    /// The layers (bit `k` = layer `k + 1`) a primitive with `reflect_channels` is drawn in.
    pub fn bits(&self, reflect_channels: u32) -> u32 {
        self.masks.iter().enumerate().fold(0, |a, (k, m)| if m & reflect_channels != 0 { a | (1 << k) } else { a })
    }
}

/// Render layers of an entity drawn in the main view and in the capture layers `bits`.
pub fn render_layers(bits: u32) -> RenderLayers {
    let mut v: Vec<usize> = vec![0];
    v.extend((0..32).filter(|k| bits & (1 << k) != 0).map(|k| k + 1));
    RenderLayers::from_layers(&v)
}

/// Render layer of the screens that sample a capture's target (`tv_screen.rs`): the game cameras draw it
/// (`game_layers`), the capture cameras (layer 0 or a reflection layer) do not, so a screen never samples
/// the target it would be drawn into.
pub const SCREEN_LAYER: usize = 31;

/// The render layers of the game and inspect cameras: the main layer and the screens'.
pub fn game_layers() -> RenderLayers {
    RenderLayers::from_layers(&[0, SCREEN_LAYER])
}

/// A capture camera: one `SceneCaptureReflectActor`.
#[derive(Component)]
pub struct ReflectCamera {
    /// The render target image (shared by the actors with the same `TextureTarget`).
    pub image: Handle<Image>,
    /// Plane `normal . p = d` in Bevy space.
    pub normal: Vec3,
    pub d: f32,
    /// `m_ResolutionType` (0 fixed, 1 full, 2 half, 3 quarter) and the fixed `SizeX` / `SizeY`.
    pub resolution_type: u8,
    pub fixed_size: UVec2,
    /// `FrameRate`, seconds of the last capture.
    pub frame_rate: f32,
    pub last: f32,
    pub enabled: bool,
    pub skip_if_unseen: bool,
    /// `MaxUpdateDist` in Bevy units (0: unlimited).
    pub max_dist: f32,
    /// "package:actor" (lower case), the key of Kismet's toggles (`MissionView::captures`), and the
    /// component's own `bEnabled` they start from.
    pub key: String,
    pub level_enabled: bool,
    /// A `SceneCapture2DActor`: the fixed view (Bevy space) instead of the mirror plane.
    pub fixed: Option<FixedView>,
}

/// The fixed camera of a `SceneCapture2DComponent` in Bevy space; `fov` is the horizontal angle
/// (UE3's `FieldOfView`), `near` / `far` are in Bevy units.
#[derive(Clone, Copy, Debug)]
pub struct FixedView {
    pub position: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

/// The key of a capture actor in Kismet's `MissionView::captures`: "package:actor", lower case.
fn actor_key(source: &str) -> String {
    match source.split_once(':') {
        Some((pkg, path)) => format!("{pkg}:{}", path.rsplit('.').next().unwrap_or(path)).to_ascii_lowercase(),
        None => source.to_ascii_lowercase(),
    }
}

/// The render target of a material that samples a scene capture other than as the water's
/// reflection (the lens TV, `tv_screen.rs`).
pub fn screen_target(p: &dishonored_ir::MaterialParams) -> Option<&str> {
    p.render_targets.iter().find(|(n, _)| !n.trim().eq_ignore_ascii_case(REALTIME_PARAM)).map(|(_, t)| t.as_str())
}

/// Whether Kismet's toggles of a capture actor leave it enabled: the last explicit On / Off
/// (else the component's `bEnabled` from the level), inverted by an odd number of Toggles after it
/// (`SceneCaptureActor.OnToggle`).
pub fn toggled_enabled(level_enabled: bool, explicit: Option<bool>, flipped: bool) -> bool {
    explicit.unwrap_or(level_enabled) ^ flipped
}

/// A primitive that samples a capture's render target (the water): the capture is skipped while
/// none of its users is visible (`bSkipUpdateIfTextureUsersOccluded`).
#[derive(Component)]
pub struct ReflectUser(pub Handle<Image>);

/// The object path of the render target a material binds as `C_Realtime_reflection_Texture`.
pub fn material_target(p: &dishonored_ir::MaterialParams) -> Option<&str> {
    p.render_targets.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(REALTIME_PARAM)).map(|(_, t)| t.as_str())
}

/// Spawn the map's capture cameras (inactive until `update_cameras` runs them) and return the
/// render target images by lower-case object path. A capture with `bEnableFog` / `bEnablePostProcess`
/// gets the level's DisFog layers / ArkPp post settings on its own `ArkPost` pass (U-960).
pub fn spawn_captures(map: &DishonoredMap, layers: &Layers, features: &RenderFeatures, commands: &mut Commands, images: &mut Assets<Image>) -> HashMap<String, Handle<Image>> {
    let mut targets: HashMap<String, Handle<Image>> = HashMap::new();
    for c in &map.reflect_captures {
        // A 2D capture (the lens TV) draws the whole scene (layer 0); a reflection capture draws
        // the layer of its channel set.
        let layer = match &c.scene2d {
            Some(_) if features.reflections => 0,
            Some(_) => continue,
            None => {
                let Some(k) = layers.masks.iter().position(|m| *m == c.channels) else { continue };
                k + 1
            }
        };
        let image = targets.entry(c.target.to_ascii_lowercase()).or_insert_with(|| images.add(Image::new_target_texture(1, 1, TARGET_FORMAT, None))).clone();
        let normal = coords::dir(c.normal).normalize_or(Vec3::Y);
        let d = normal.dot(coords::pos(c.normal * c.d));
        let fixed = c.scene2d.map(|s| FixedView {
            position: coords::pos(s.location),
            forward: coords::dir(s.forward).normalize_or(Vec3::NEG_Z),
            up: coords::dir(s.up).normalize_or(Vec3::Y),
            fov: s.fov.to_radians(),
            near: s.near * coords::UU_TO_BEVY,
            far: s.far * coords::UU_TO_BEVY,
        });
        let mut e = commands.spawn((
            Camera3d::default(),
            Camera { order: -100, is_active: false, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
            RenderTarget::from(image.clone()),
            Projection::Perspective(PerspectiveProjection::default()),
            Transform::default(),
            Msaa::Off,
            Tonemapping::None,
            DebandDither::Disabled,
            RenderLayers::layer(layer),
            ReflectCamera {
                image,
                normal,
                d,
                resolution_type: c.resolution_type,
                fixed_size: UVec2::new(c.size[0], c.size[1]),
                frame_rate: c.frame_rate,
                last: f32::NEG_INFINITY,
                enabled: c.enabled,
                skip_if_unseen: c.skip_if_unseen,
                max_dist: c.max_update_dist * coords::UU_TO_BEVY,
                key: actor_key(&c.source),
                level_enabled: c.enabled,
                fixed,
            },
            MapGeometry { instance: usize::MAX },
        ));
        // The capture's own fog and post-process (`bEnableFog`, `bEnablePostProcess`): the level's
        // DisFog layers and ArkPp settings through the same pass as the game view, without the
        // bloom parts (LIKELY, U-960: the original's capture path for them is not decoded).
        let fog_on = c.fog && features.fog && !map.environment.fog.is_empty();
        let post_on = c.post_process && features.post;
        if fog_on || post_on {
            let src = ArkPostSource { fog: map.environment.fog.clone(), post: map.environment.post.clone(), fog_on, post_on, display_gamma: 2.2, capture: true, ..Default::default() };
            let u = src.uniform(0.0);
            e.insert((src, u));
        }
    }
    targets
}
/// The size of a render target: `m_ResolutionType` of the back buffer (`TRT_FIXEDSIZE` keeps the
/// object's size, 1x1 when it has none: half size).
fn target_size(cam: &ReflectCamera, back_buffer: UVec2) -> UVec2 {
    let div = match cam.resolution_type {
        1 => 1,
        3 => 4,
        0 if cam.fixed_size.x > 1 && cam.fixed_size.y > 1 => return cam.fixed_size,
        _ => 2,
    };
    (back_buffer / div).max(UVec2::ONE)
}

/// Mirror the viewer about each capture's plane, throttle and cull the captures, and size their
/// targets. Runs after transform propagation and before the cameras' projections are computed, so
/// the mirrored view and its oblique near plane are those of this frame. A 2D capture
/// (`FixedView`) keeps its own view. Kismet's `SeqAct_Toggle`s (`MissionView::captures`) set each
/// capture's `bEnabled` first.
pub fn update_cameras(
    time: Res<Time>,
    session: Option<Res<crate::session::SessionView>>,
    mains: Query<(&Camera, &GlobalTransform, &Projection), (With<Camera3d>, Without<ReflectCamera>)>,
    users: Query<(&ReflectUser, &ViewVisibility)>,
    mut caps: Query<(Entity, &mut ReflectCamera, &mut Camera, &mut Transform, &mut GlobalTransform, &mut Projection)>,
    mut images: ResMut<Assets<Image>>,
) {
    if caps.is_empty() {
        return;
    }
    // Kismet's toggles of the capture actors (`SceneCaptureActor.OnToggle`).
    if let Some(mv) = session.as_ref().and_then(|s| s.matchv.mission.as_ref()) {
        for (_, mut rc, ..) in &mut caps {
            let on = mv.captures.iter().find(|c| c.0.eq_ignore_ascii_case(&rc.key)).map_or(rc.level_enabled, |(_, set, flipped)| toggled_enabled(rc.level_enabled, *set, *flipped));
            if rc.enabled != on {
                rc.enabled = on;
            }
        }
    }
    let main = mains.iter().find(|(c, ..)| c.is_active);
    let Some((mcam, mgt, Projection::Perspective(mp))) = main else {
        for (.., mut cam, _, _, _) in &mut caps {
            cam.is_active = false;
        }
        return;
    };
    let back_buffer = mcam.physical_target_size().unwrap_or(UVec2::new(1280, 720));
    let now = time.elapsed_secs();
    let p = mgt.translation();
    let seen: std::collections::HashSet<AssetId<Image>> = users.iter().filter(|(_, v)| v.get()).map(|(u, _)| u.0.id()).collect();
    // Per target, the capture whose plane is the nearest below the viewer is the one that draws
    // (several actors share a target in the levels with water at different heights).
    let mut best: HashMap<AssetId<Image>, (Entity, f32)> = HashMap::new();
    for (e, rc, ..) in &caps {
        let height = rc.normal.dot(p) - rc.d;
        let in_range = rc.fixed.is_some() || (height > 0.0 && (rc.max_dist <= 0.0 || height <= rc.max_dist));
        let eligible = rc.enabled && in_range && (!rc.skip_if_unseen || seen.contains(&rc.image.id()));
        if eligible && best.get(&rc.image.id()).is_none_or(|(_, h)| height < *h) {
            best.insert(rc.image.id(), (e, height));
        }
    }
    for (e, mut rc, mut cam, mut tf, mut gt, mut proj) in &mut caps {
        let due = rc.frame_rate <= 0.0 || now - rc.last >= 1.0 / rc.frame_rate;
        if best.get(&rc.image.id()).is_none_or(|(b, _)| *b != e) || !due {
            cam.is_active = false;
            continue;
        }
        rc.last = now;
        cam.is_active = true;
        // The target follows the back buffer.
        let size = target_size(&rc, back_buffer);
        if images.get(&rc.image).is_some_and(|i| i.size() != size) {
            if let Some(mut i) = images.get_mut(&rc.image) {
                i.resize(Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 });
            }
        }
        let aspect = size.x as f32 / size.y as f32;
        if let Some(v) = rc.fixed {
            // A 2D capture: the actor's own view (UE3's field of view is the horizontal one).
            let t = Transform::from_translation(v.position).looking_to(Dir3::new(v.forward).unwrap_or(Dir3::NEG_Z), Dir3::new(v.up).unwrap_or(Dir3::Y));
            *tf = t;
            *gt = GlobalTransform::from(t);
            let fov = 2.0 * ((v.fov * 0.5).tan() / aspect).atan();
            *proj = Projection::Perspective(PerspectiveProjection { fov, near: v.near, far: v.far, aspect_ratio: aspect, near_clip_plane: Vec4::new(0.0, 0.0, -1.0, -v.near) });
            continue;
        }
        // The mirrored viewer: position, forward and up reflected about the plane.
        let n = rc.normal;
        let height = n.dot(p) - rc.d;
        let reflect = |v: Vec3| v - 2.0 * n.dot(v) * n;
        let (f, u) = (reflect(mgt.forward().as_vec3()), reflect(mgt.up().as_vec3()));
        let t = Transform::from_translation(p - 2.0 * height * n).looking_to(Dir3::new(f).unwrap_or(Dir3::NEG_Z), Dir3::new(u).unwrap_or(Dir3::Y));
        *tf = t;
        *gt = GlobalTransform::from(t);
        // The plane in view space, pointing away from the camera: only what is above the water
        // is drawn (`near_clip_plane`: normal and the negative distance from the camera).
        let c = t.to_matrix().transpose() * n.extend(-rc.d);
        let c = c / c.truncate().length().max(1e-6);
        *proj = Projection::Perspective(PerspectiveProjection { fov: mp.fov, near: mp.near, far: mp.far, aspect_ratio: aspect, near_clip_plane: c });
    }
}
pub struct ReflectPlugin;

impl Plugin for ReflectPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            update_cameras.after(bevy::transform::TransformSystems::Propagate).before(bevy::camera::CameraUpdateSystems),
        );
    }
}
