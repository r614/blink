//! The level's reflection cubemap (`map_render::environment_light`) is prefiltered once.
//!
//! Bevy's `GeneratedEnvironmentMapLight` re-runs its whole filter chain every frame (copy,
//! mip downsampling, the radiance convolution of every specular mip and the irradiance
//! convolution), meant for cubemaps that change. Ours is a static texture and the filter is
//! deterministic (fixed blue-noise texture, no frame index), so every frame produced the same
//! result. Once the render world has run the filter for a camera, the camera's
//! `GeneratedEnvironmentMapLight` is removed: the `EnvironmentMapLight` it created keeps the
//! filtered diffuse / specular textures and draws exactly as before. A new environment light
//! (level travel, `level::apply_look`) is filtered again the same way.

use bevy::light::{EnvironmentMapLight, GeneratedEnvironmentMapLight};
use bevy::pbr::generate::{GeneratorBindGroups, GeneratorPipelines, RenderEnvironmentMap, filtering_system};
use bevy::prelude::*;
use bevy::render::render_resource::PipelineCache;
use bevy::render::sync_world::MainEntity;
use bevy::render::{Render, RenderApp};
use std::sync::{Arc, Mutex};

/// Main-world entities whose environment map the render world has filtered.
#[derive(Resource, Clone, Default)]
struct Filtered(Arc<Mutex<Vec<Entity>>>);

pub struct EnvMapOncePlugin;

impl Plugin for EnvMapOncePlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("DISHONORED_ENV_EVERY_FRAME").is_some() {
            return;
        }
        let shared = Filtered::default();
        app.insert_resource(shared.clone()).add_systems(Update, freeze);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(shared).add_systems(Render, report.after(filtering_system));
        }
    }
}

/// Render world: the views whose filter passes ran this frame (bind groups prepared and every
/// generator pipeline compiled, the conditions under which Bevy's systems dispatch them).
fn report(views: Query<&MainEntity, (With<RenderEnvironmentMap>, With<GeneratorBindGroups>)>, pipelines: Option<Res<GeneratorPipelines>>, cache: Res<PipelineCache>, out: Res<Filtered>) {
    let Some(p) = pipelines else { return };
    let ready = [p.copy, p.downsample_first, p.downsample_second, p.radiance, p.irradiance].into_iter().all(|id| cache.get_compute_pipeline(id).is_some());
    if !ready || views.is_empty() {
        return;
    }
    let mut v = out.0.lock().unwrap();
    v.extend(views.iter().map(|m| m.id()));
}

/// Main world: stop regenerating the filtered maps (unless the environment light was replaced
/// since, which is filtered again first).
fn freeze(mut commands: Commands, filtered: Res<Filtered>, q: Query<Ref<GeneratedEnvironmentMapLight>, With<EnvironmentMapLight>>) {
    let done: Vec<Entity> = std::mem::take(&mut *filtered.0.lock().unwrap());
    for e in done {
        if q.get(e).is_ok_and(|g| !g.is_changed()) {
            commands.entity(e).remove::<GeneratedEnvironmentMapLight>();
        }
    }
}
