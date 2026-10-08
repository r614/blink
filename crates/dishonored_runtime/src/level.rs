//! Level presentation and level travel.
//!
//! The original swaps the persistent map on travel (`SeqAct_PrepareMapChange` ->
//! `SeqAct_CommitMapChange`) behind its loading screen. Here the renderer follows
//! `Session::content` (which the session switches to the server's active map): on a change the
//! original loading screen comes up (`crate::loading_screen`), the destination level is imported
//! on a worker thread and spawned on its first visit, or simply made visible again on later
//! visits, and the camera's post/fog settings are switched to that level's. Boyle Ext and Int share one coordinate frame, so the
//! level that was left is hidden rather than kept on screen.

use crate::map_render::{self, BakedMaterial, MapGeometry, RenderFeatures};
use crate::session::Session;
use crate::{atmosphere, lighting};
use bevy::prelude::*;
use dishonored_assets::{Content, ImportOptions, import_level, map_packages};
use dishonored_ir::{DishonoredMap, Environment};
use std::collections::HashMap;

/// Which level an entity belongs to.
#[derive(Component)]
pub struct LevelOf(pub String);

struct LevelLook {
    environment: Environment,
    env_light: Option<bevy::light::GeneratedEnvironmentMapLight>,
}

#[derive(Resource, Default)]
pub struct Levels {
    /// The level currently shown (newly spawned map entities are tagged with it).
    pub active: String,
    looks: HashMap<String, LevelLook>,
    /// A level change in progress: the target, and its import running on a worker thread.
    pending: Option<String>,
    import: Option<std::sync::Mutex<std::sync::mpsc::Receiver<Result<(DishonoredMap, map_render::PreparedMap), String>>>>,
}

pub struct LevelOptions {
    pub max_tex: u32,
    pub show_hidden: bool,
    pub legacy: bool,
}

/// Import and spawn a level; returns the parsed map (for the caller's camera/spawn needs).
#[allow(clippy::too_many_arguments)]
pub fn spawn_level(
    name: &str,
    opts: &LevelOptions,
    features: &RenderFeatures,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    baked: &mut Assets<BakedMaterial>,
    levels: &mut Levels,
    fx: &mut crate::effects::Effects,
) -> DishonoredMap {
    let content = Content::locate().expect("Dishonored install not found (set DISHONORED_DIR)");
    // [playground] A map built from a layout of original assets (`maps/<name>.ron`).
    if let Some(layout) = dishonored_content::playground::layout_for(name) {
        let layout = layout.expect("map layout");
        let (map, stats) = dishonored_assets::level::layout_import::import_layout(&content, &layout, &import_options(name, opts)).expect("layout import failed");
        for e in &stats.errors {
            warn!("layout {name}: {e}");
        }
        return spawn_imported(name, map, None, stats.instances, opts, features, commands, meshes, images, materials, baked, levels, fx);
    }
    // --legacy-geometry: the pre-fidelity package/actor coverage (prefix-matched sublevels,
    // StaticMeshActor + StaticMeshCollectionActor only), for before/after comparisons.
    let pkgs = if opts.legacy { dishonored_assets::map_packages_by_prefix(&content, name) } else { map_packages(&content, name) };
    let iopts = import_options(name, opts);
    let (map, stats) = import_level(&content, name, &pkgs, &iopts).expect("map import failed");
    info!(
        "lightmaps: {name}: {} texture lightmaps, {} pages, {} components with a dominant directional distance-field shadow map, {} with a dominant point / spot one",
        stats.lightmaps_texture, stats.lightmap_pages, stats.shadow_map_components, stats.local_shadow_map_components
    );
    info!(
        "import {name}: {} (lightmaps {})",
        stats.timings.iter().map(|(k, s)| format!("{k} {s:.1} s")).collect::<Vec<_>>().join(", "),
        if stats.lightmaps_cached { "from the cache" } else { "baked" }
    );
    spawn_imported(name, map, None, stats.instances, opts, features, commands, meshes, images, materials, baked, levels, fx)
}

/// The importer's options. Baked lightmap pages are cached under DISHONORED_CACHE
/// (`<map>.lightmaps1.bin`; `DISHONORED_NO_LIGHTMAP_CACHE` bakes every time).
fn import_options(name: &str, opts: &LevelOptions) -> ImportOptions {
    let lightmap_cache = std::env::var_os("DISHONORED_NO_LIGHTMAP_CACHE")
        .is_none()
        .then(|| dishonored_content::cache_dir().join(format!("{name}{}.lightmaps1.bin", if opts.legacy { ".legacy" } else { "" })));
    ImportOptions { max_texture_size: opts.max_tex, other_actors: !opts.legacy, extra_particles: crate::effects::GAMEPLAY_PARTICLES.iter().map(|s| s.to_string()).collect(), lightmap_cache, ..Default::default() }
}

/// Import a level and prepare its render data (worker-thread safe): the frame that spawns it
/// then has next to nothing left to do (`map_render::prepare_map`).
fn import(name: &str, opts: &LevelOptions, features: &RenderFeatures) -> Result<(DishonoredMap, map_render::PreparedMap), String> {
    let content = Content::locate().map_err(|e| e.to_string())?;
    let map = if let Some(layout) = dishonored_content::playground::layout_for(name) {
        let layout = layout.map_err(|e| format!("{e:#}"))?;
        dishonored_assets::level::layout_import::import_layout(&content, &layout, &import_options(name, opts)).map(|r| r.0).map_err(|e| format!("{e:#}"))?
    } else {
        let pkgs = if opts.legacy { dishonored_assets::map_packages_by_prefix(&content, name) } else { map_packages(&content, name) };
        let iopts = import_options(name, opts);
        import_level(&content, name, &pkgs, &iopts).map(|r| r.0).map_err(|e| format!("{e:#}"))?
    };
    let prep = map_render::prepare_map(&map, opts.show_hidden, features);
    Ok((map, prep))
}

/// Spawn an imported level's geometry and lights and record its look.
#[allow(clippy::too_many_arguments)]
fn spawn_imported(
    name: &str,
    map: DishonoredMap,
    prepared: Option<map_render::PreparedMap>,
    instances: usize,
    opts: &LevelOptions,
    features: &RenderFeatures,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    baked: &mut Assets<BakedMaterial>,
    levels: &mut Levels,
    fx: &mut crate::effects::Effects,
) -> DishonoredMap {
    let spawned = match prepared {
        Some(p) => map_render::spawn_prepared(&map, p, commands, meshes, images, materials, baked, opts.show_hidden, features),
        None => map_render::spawn_map(&map, commands, meshes, images, materials, baked, opts.show_hidden, features),
    };
    crate::effects::spawn_level_effects(name, &map, commands, meshes, images, materials, fx);
    crate::level_props::queue(name, &map, commands);
    info!("{}: {} instances, {} meshes, {} textures, {} entities", name, instances, map.meshes.len(), map.textures.len(), spawned);
    let lights = lighting::spawn_lights(&map, commands, features);
    info!("{} map lights", lights);
    if lights == 0 {
        // Placeholder sun when real lights are disabled (or the map has none).
        commands.spawn((
            DirectionalLight { illuminance: 6000.0, shadow_maps_enabled: false, color: Color::srgb(0.75, 0.8, 1.0), affects_lightmapped_mesh_diffuse: !features.lightmaps, ..default() },
            Transform::default().looking_to(Vec3::new(-0.4, -1.0, -0.3), Vec3::Y),
            LevelOf(name.to_string()),
        ));
    }
    let env_light = if features.materials { map_render::environment_light(&map, images) } else { None };
    levels.active = name.to_string();
    levels.looks.insert(name.to_ascii_lowercase(), LevelLook { environment: map.environment.clone(), env_light });
    map
}

/// Apply a level's post-processing, fog and environment light to the camera.
pub fn apply_look(levels: &Levels, name: &str, camera: Entity, commands: &mut Commands, features: &RenderFeatures) {
    let Some(look) = levels.looks.get(&name.to_ascii_lowercase()) else { return };
    let mut cam = commands.entity(camera);
    match &look.env_light {
        Some(e) => {
            cam.insert(e.clone());
        }
        None => {
            cam.remove::<bevy::light::GeneratedEnvironmentMapLight>();
        }
    }
    let lite = DishonoredMap { environment: look.environment.clone(), ..Default::default() };
    let fog = atmosphere::setup_camera(&lite, &mut cam, features);
    commands.insert_resource(fog);
}

/// Tag freshly spawned level entities with the level they belong to.
pub fn tag_level_entities(mut commands: Commands, levels: Res<Levels>, q: Query<Entity, (Or<(With<MapGeometry>, With<lighting::MapLight>)>, Without<LevelOf>)>) {
    for e in &q {
        commands.entity(e).insert(LevelOf(levels.active.clone()));
    }
}

/// Follow the session's active map (level travel): loading screen up, import on a worker
/// thread (first visit) or re-show (later visits), switch the camera's look, screen down.
#[allow(clippy::too_many_arguments)]
pub fn follow_level(
    mut commands: Commands,
    session: Res<Session>,
    mut levels: ResMut<Levels>,
    mut loading: ResMut<crate::loading_screen::LoadingScreen>,
    opts: Res<crate::LevelSettings>,
    features: Res<RenderFeatures>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut baked: ResMut<Assets<BakedMaterial>>,
    mut vis: Query<(&LevelOf, &mut Visibility)>,
    mut fx: ResMut<crate::effects::Effects>,
    camera: Query<Entity, (With<Camera3d>, Without<crate::reflect::ReflectCamera>)>,
    mut warm: ResMut<crate::warmup::Warmup>,
) {
    let want = session.content.map_name.clone();
    let Some(target) = levels.pending.clone() else {
        if levels.active.is_empty() || levels.active.eq_ignore_ascii_case(&want) {
            return;
        }
        loading.begin(&mut commands, &mut images, &want);
        if !levels.looks.contains_key(&want.to_ascii_lowercase()) {
            let (tx, rx) = std::sync::mpsc::channel();
            let name = want.clone();
            let o = LevelOptions { max_tex: opts.0.max_tex, show_hidden: opts.0.show_hidden, legacy: opts.0.legacy };
            let f = features.clone();
            std::thread::Builder::new()
                .name(format!("import {name}"))
                .spawn(move || {
                    let _ = tx.send(import(&name, &o, &f));
                })
                .ok();
            levels.import = Some(std::sync::Mutex::new(rx));
        }
        levels.pending = Some(want);
        return;
    };
    // First visit: wait for the worker's import, then spawn it.
    let mut new_level = false;
    if let Some(rx) = levels.import.take() {
        let got = rx.lock().ok().and_then(|r| r.try_recv().ok());
        match got {
            None => {
                levels.import = Some(rx);
                return;
            }
            Some(Ok((map, prep))) => {
                let n = map.instances.len();
                new_level = true;
                spawn_imported(&target, map, Some(prep), n, &opts.0, &features, &mut commands, &mut meshes, &mut images, &mut materials, &mut baked, &mut levels, &mut fx);
            }
            Some(Err(e)) => error!("importing {target}: {e}"),
        }
    }
    levels.pending = None;
    for (l, mut v) in &mut vis {
        *v = if l.0.eq_ignore_ascii_case(&target) { Visibility::Inherited } else { Visibility::Hidden };
    }
    levels.active = target.clone();
    fx.set_active(&target);
    if let Ok(cam) = camera.single() {
        apply_look(&levels, &target, cam, &mut commands, &features);
    }
    // A level seen for the first time compiles its pipelines behind the loading screen, which
    // the warm-up takes down when none are waiting; a revisit has them cached already.
    if !(new_level && warm.begin(true, true)) {
        loading.end();
    }
    info!("level travel: now showing {target}");
}
