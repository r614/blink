//! Level presentation and level travel.
//!
//! The original swaps the persistent map on travel (`SeqAct_PrepareMapChange` ->
//! `SeqAct_CommitMapChange`) behind its loading screen. Here the renderer follows
//! `Session::content` (which the session switches to the server's active map): on a change a
//! loading overlay is shown for a frame, then the destination level is imported and spawned on
//! its first visit, or simply made visible again on later visits, and the camera's post/fog
//! settings are switched to that level's. Boyle Ext and Int share one coordinate frame, so the
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
    /// A level change waiting for the loading overlay to be on screen (frames to wait).
    pending: Option<(String, u8)>,
}

#[derive(Component)]
pub struct LoadingOverlay;

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
) -> DishonoredMap {
    let content = Content::locate().expect("Dishonored install not found (set DISHONORED_DIR)");
    // --legacy-geometry: the pre-fidelity package/actor coverage (prefix-matched sublevels,
    // StaticMeshActor + StaticMeshCollectionActor only), for before/after comparisons.
    let pkgs = if opts.legacy { dishonored_assets::map_packages_by_prefix(&content, name) } else { map_packages(&content, name) };
    let iopts = ImportOptions { max_texture_size: opts.max_tex, other_actors: !opts.legacy, ..Default::default() };
    let (map, stats) = import_level(&content, name, &pkgs, &iopts).expect("map import failed");
    let spawned = map_render::spawn_map(&map, commands, meshes, images, materials, baked, opts.show_hidden, features);
    info!("{}: {} instances, {} meshes, {} textures, {} entities", name, stats.instances, stats.meshes, stats.textures, spawned);
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

/// Follow the session's active map (level travel).
#[allow(clippy::too_many_arguments)]
pub fn follow_level(
    mut commands: Commands,
    session: Res<Session>,
    mut levels: ResMut<Levels>,
    opts: Res<crate::LevelSettings>,
    features: Res<RenderFeatures>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut baked: ResMut<Assets<BakedMaterial>>,
    mut vis: Query<(&LevelOf, &mut Visibility)>,
    camera: Query<Entity, With<Camera3d>>,
    overlay: Query<Entity, With<LoadingOverlay>>,
) {
    let want = session.content.map_name.clone();
    if levels.pending.is_none() {
        if levels.active.is_empty() || levels.active.eq_ignore_ascii_case(&want) {
            return;
        }
        // Show the loading screen first; the swap happens once it has been presented.
        commands.spawn((
            LoadingOverlay,
            Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), justify_content: JustifyContent::FlexEnd, align_items: AlignItems::FlexEnd, padding: UiRect::all(Val::Px(48.0)), ..default() },
            BackgroundColor(Color::BLACK),
            GlobalZIndex(100),
            children![(Text::new("Loading..."), TextFont { font_size: bevy::text::FontSize::Px(28.0), ..default() }, TextColor(Color::srgb(0.85, 0.8, 0.7)))],
        ));
        levels.pending = Some((want, 2));
        return;
    }
    let (target, wait) = levels.pending.clone().unwrap();
    if wait > 0 {
        levels.pending = Some((target, wait - 1));
        return;
    }
    levels.pending = None;
    for (l, mut v) in &mut vis {
        *v = if l.0.eq_ignore_ascii_case(&target) { Visibility::Inherited } else { Visibility::Hidden };
    }
    if !levels.looks.contains_key(&target.to_ascii_lowercase()) {
        spawn_level(&target, &opts.0, &features, &mut commands, &mut meshes, &mut images, &mut materials, &mut baked, &mut levels);
    }
    levels.active = target.clone();
    if let Ok(cam) = camera.single() {
        apply_look(&levels, &target, cam, &mut commands, &features);
    }
    for e in &overlay {
        commands.entity(e).despawn();
    }
    info!("level travel: now showing {target}");
}
