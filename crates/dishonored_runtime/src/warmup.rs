// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
//! Shader / pipeline warm-up behind the loading screen (research/dishonored/RENDERING.md).
//!
//! Bevy compiles a mesh's render pipelines the first time a view queues it, and does so on
//! worker threads: until the pipeline is ready the surface is simply not drawn. Left alone, that
//! shows as late-appearing surfaces and first-view hitches as the player turns towards new
//! geometry. While a [`Warmup`] is active every `Mesh3d` is drawn whatever the view (frustum
//! culling off, which also queues it in every shadow view), through the real cameras, so each
//! material / mesh / view pipeline variant the level uses is specialised once. The warm-up ends
//! when the pipeline cache has had no pipelines waiting for a few consecutive frames
//! (`PipelineCache::waiting_pipelines`, read in the render world and handed to the main world
//! through an atomic), or after [`TIMEOUT_SECS`] as a safety net. `DISHONORED_NO_WARMUP` skips it.

use crate::loading_screen::LoadingScreen;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;
use bevy::render::render_resource::PipelineCache;
use bevy::render::{Render, RenderApp, RenderSystems};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Frames to draw before the pipeline count is trusted (materials that are swapped in a frame
/// after spawning, e.g. the `Pending*` components, queue their pipelines by then).
const MIN_FRAMES: u32 = 4;
/// Consecutive frames with nothing waiting that end the warm-up.
const SETTLED_FRAMES: u32 = 3;
/// A warm-up that has not settled by now ends anyway.
/// The translated original shaders need several hundred pipelines a level (about 20-60 s on a cold driver
/// cache), so this is only a safety net against a pipeline that never finishes.
pub const TIMEOUT_SECS: f32 = 180.0;

pub struct WarmupPlugin;

impl Plugin for WarmupPlugin {
    fn build(&self, app: &mut App) {
        let waiting = Waiting::default();
        app.insert_resource(waiting.clone()).init_resource::<Warmup>().add_systems(Update, drive);
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.insert_resource(waiting).add_systems(Render, count_waiting.in_set(RenderSystems::Cleanup));
        }
    }
}

/// Pipelines the render world is still compiling (written there, read in the main world).
#[derive(Resource, Clone, Default)]
struct Waiting(Arc<AtomicUsize>);

fn count_waiting(cache: Res<PipelineCache>, waiting: Res<Waiting>, mut last: Local<usize>) {
    let n = cache.waiting_pipelines().count();
    waiting.0.store(n, Ordering::Relaxed);
    // A compile burst has finished: persist what was compiled (`configure_pipeline_cache`).
    if *last > 0 && n == 0 {
        if let Some(bytes) = cache.save_wgpu_pipeline_cache() {
            info!("pipeline cache saved ({bytes} bytes)");
        }
    }
    *last = n;
}

/// Compiled pipelines persist across runs. On Vulkan (`WGPU_BACKEND=vulkan`, the backend with wgpu's
/// `PIPELINE_CACHE`) the vendored bevy_render keeps its pipeline cache in `<cache>/pipelines`
/// (`BLINK_PIPELINE_CACHE_DIR`); on DX12, the default, the driver's own shader cache does it.
/// Measured on L_Boyle_Ext_P: DX12 with a warm driver cache 26.8 s, Vulkan 50.7 s cold / 41.3 s
/// with the cache (the composition of the translated shaders, not the driver compile, dominates).
/// Call before the render plugin starts.
pub fn configure_pipeline_cache() {
    if std::env::var_os("DISHONORED_NO_PIPELINE_CACHE").is_some() {
        return;
    }
    let dir = dishonored_content::cache_dir().join("pipelines");
    // SAFETY: called at startup, before any other thread reads the environment.
    unsafe {
        if std::env::var_os("BLINK_PIPELINE_CACHE_DIR").is_none() {
            std::env::set_var("BLINK_PIPELINE_CACHE_DIR", &dir);
        }
    }
}

/// Meshes drawn whatever the view while warming up (only their culling flag is undone after).
#[derive(Component)]
struct WarmupCull;

#[derive(Resource, Default)]
pub struct Warmup {
    active: bool,
    pause: bool,
    end_loading: bool,
    started: Option<std::time::Instant>,
    frames: u32,
    settled: u32,
    most: usize,
}

impl Warmup {
    /// Start warming up from the next frame; false when it is switched off
    /// (`DISHONORED_NO_WARMUP`), in which case nothing runs. `pause` stops the world's clock
    /// meanwhile; `end_loading` takes the loading screen down (`LoadingScreen::end`) when done.
    pub fn begin(&mut self, pause: bool, end_loading: bool) -> bool {
        if std::env::var_os("DISHONORED_NO_WARMUP").is_some() {
            return false;
        }
        *self = Warmup { active: true, pause, end_loading, started: Some(std::time::Instant::now()), ..default() };
        true
    }

    /// No warm-up is running (none requested, or it has settled).
    pub fn done(&self) -> bool {
        !self.active
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut w: ResMut<Warmup>,
    waiting: Res<Waiting>,
    meshes: Query<Entity, (With<Mesh3d>, Without<NoFrustumCulling>)>,
    culled: Query<Entity, With<WarmupCull>>,
    mut time: ResMut<Time<Virtual>>,
    loading: Option<ResMut<LoadingScreen>>,
) {
    if !w.active {
        return;
    }
    // Everything that exists now (and whatever spawns during the warm-up) is drawn in every view.
    let mut n = 0;
    for e in &meshes {
        commands.entity(e).try_insert((NoFrustumCulling, WarmupCull));
        n += 1;
    }
    if w.frames == 0 {
        if w.pause {
            time.pause();
        }
        info!("shader warm-up: {n} meshes drawn without culling");
    }
    w.frames += 1;
    let pending = waiting.0.load(Ordering::Relaxed);
    w.most = w.most.max(pending);
    w.settled = if pending == 0 && w.frames > MIN_FRAMES { w.settled + 1 } else { 0 };
    let elapsed = w.started.map(|s| s.elapsed().as_secs_f32()).unwrap_or(0.0);
    if w.settled < SETTLED_FRAMES && elapsed < TIMEOUT_SECS {
        return;
    }
    for e in &culled {
        commands.entity(e).try_remove::<(NoFrustumCulling, WarmupCull)>();
    }
    if w.pause {
        time.unpause();
    }
    if w.end_loading {
        if let Some(mut l) = loading {
            l.end();
        }
    }
    info!("shader warm-up done in {elapsed:.1} s ({} frames, up to {} pipelines waiting)", w.frames, w.most);
    w.active = false;
}
