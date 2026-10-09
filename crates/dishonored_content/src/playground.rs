//! Maps built from a layout of original assets (`maps/<name>.ron`, `dishonored_ir::layout`): the
//! Playground. The layout goes through the level importer (`dishonored_assets::level::
//! layout_import`) like a cooked level, and this module derives what the simulation needs from
//! it: collision (brushes, meshes, the copied doors and usables), water and pain volumes,
//! climbable chains, pickups, natively usable doors, the AI's factions and light probes, and a
//! navigation mesh generated over the walkable floors. research/dishonored/PLAYGROUND.md.

use super::*;
use dishonored_assets::level::layout_import::{import_layout, placed_actor};
use dishonored_ir::layout::{MapLayout, NavGrid, v3};
use dishonored_ir::nav::{NavData, NavEdge, NavEdgeClass, NavEdgeType, NavMesh, NavPoly, NavPylon};
use dishonored_multiplayer::mission::UsableDef;
use dishonored_world::{PawnBox, WorldQuery, channel};
use std::collections::HashMap;

/// The pawn a generated navigation mesh is for.
pub struct NavAgent {
    pub collision_radius: f32,
    pub collision_half_height: f32,
    pub walkable_floor_z: f32,
    pub max_step_height: f32,
}

/// Bumped when the derived data below changes shape.
const DERIVED_VERSION: u32 = 6; // 2: brushes collide as the original's Cube_100 (simple box collision); 3: stealth light environment (Boyle's light range, layout lights as stealth lights); 4: Boyle's sky light in the stealth lights; 5: stealth lights unoccluded (InterpActor layout); 6: the collision world carries `channel::SHADOW` (the InterpActor cubes do not cast it), AiLevel without the unoccluded flag

/// The layout file of a map, when the map is a layout (`maps/<name>.ron`, lower case): next to the
/// working directory, the executable, or the repository root of a `target/<profile>` build;
/// `DISHONORED_MAPS` names another directory.
pub fn layout_path(map: &str) -> Option<PathBuf> {
    let file = format!("{}.ron", map.to_ascii_lowercase());
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("DISHONORED_MAPS") {
        dirs.push(PathBuf::from(d));
    }
    dirs.push(PathBuf::from("maps"));
    dirs.push(PathBuf::from("../../maps"));
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        dirs.push(d.join("maps"));
        dirs.push(d.join("../../maps"));
    }
    dirs.into_iter().map(|d| d.join(&file)).find(|p| p.is_file())
}

/// Whether `map` is a layout map.
pub fn is_layout_map(map: &str) -> bool {
    layout_path(map).is_some()
}

/// The layout of a map (None: an original map).
pub fn layout_for(map: &str) -> Option<Result<MapLayout>> {
    layout_path(map).map(|p| load_layout(&p))
}

pub fn load_layout(path: &Path) -> Result<MapLayout> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_layout(&text).with_context(|| format!("{}", path.display()))
}

pub fn parse_layout(text: &str) -> Result<MapLayout> {
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME | ron::extensions::Extensions::UNWRAP_NEWTYPES)
        .from_str(text)
        .map_err(|e| anyhow::anyhow!("layout parse error: {e}"))
}

/// Cache key of a layout's derived data (its content and the derivation version).
fn layout_key(layout: &MapLayout) -> u64 {
    let text = format!("{DERIVED_VERSION}:{}", ron::to_string(layout).unwrap_or_default());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// What the simulation needs besides the collision and the navigation (cached as JSON).
#[derive(Serialize, Deserialize)]
struct Derived {
    water: Vec<dishonored_world::WaterVolume>,
    volumes: Vec<dishonored_world::PhysicsVolume>,
    climbables: Vec<dishonored_world::Climbable>,
    pickups: Vec<dishonored_ir::items::PlacedPickup>,
    usables: Vec<UsableDef>,
    ai_level: dishonored_ir::ai::AiLevel,
}

/// Geometry of a layout without textures (the simulation's import).
pub fn import_layout_geometry(content: &Content, layout: &MapLayout) -> Result<DishonoredMap> {
    let opts = ImportOptions { load_textures: false, ..Default::default() };
    let (m, stats) = import_layout(content, layout, &opts)?;
    for e in &stats.errors {
        eprintln!("warning: layout {}: {e}", layout.name);
    }
    Ok(m)
}

/// Check the imported brushes of a layout against the layout: every colliding brush draws the
/// same triangles it collides with; a brush without collision is a thin surface skin (water,
/// a floor decal: at most 4 thick), never a wall; every triangle is wound as the cooked meshes
/// are (UE3 clockwise front faces: (b - a) x (c - a) into the solid, the normal out of it), or it
/// would be culled from outside and show the solid's inside. Returns the problems found.
pub fn audit_brushes(layout: &MapLayout, map: &DishonoredMap) -> Vec<String> {
    let mut errs = Vec::new();
    let prefix = format!("{}:Brush_", layout.name);
    let mut seen = vec![false; layout.brushes.len()];
    for inst in &map.instances {
        let Some(i) = inst.source.strip_prefix(&prefix).and_then(|s| s.parse::<usize>().ok()) else { continue };
        let Some(b) = layout.brushes.get(i) else {
            errs.push(format!("{}: no such brush", inst.source));
            continue;
        };
        seen[i] = true;
        let mesh = &map.meshes[inst.mesh];
        let tris = mesh.indices.len() / 3;
        if tris == 0 {
            errs.push(format!("brush {i}: no visible faces"));
        }
        if b.collide {
            if !inst.collides || mesh.collision_triangles.len() != tris {
                errs.push(format!("brush {i}: draws {tris} triangles but collides with {} (instance collides {})", mesh.collision_triangles.len(), inst.collides));
            }
        } else {
            let thick = (mesh.bounds_max - mesh.bounds_min).min_element();
            if inst.collides || !mesh.collision_triangles.is_empty() {
                errs.push(format!("brush {i}: marked collide: false but collides"));
            }
            if thick > 4.0 {
                errs.push(format!("brush {i}: visible without collision and {thick:.0} thick (only thin skins may skip collision)"));
            }
        }
        for t in mesh.indices.chunks_exact(3) {
            let (a, bb, c) = (mesh.positions[t[0] as usize], mesh.positions[t[1] as usize], mesh.positions[t[2] as usize]);
            let n = mesh.normals[t[0] as usize];
            if (bb - a).cross(c - a).dot(n) >= 0.0 {
                errs.push(format!("brush {i}: a triangle is wound against its normal (shows from inside)"));
                break;
            }
        }
    }
    for (i, s) in seen.iter().enumerate() {
        if !s {
            errs.push(format!("brush {i}: not imported"));
        }
    }
    errs
}

/// The match content of a layout map (cached under `DISHONORED_CACHE`, keyed by the layout).
pub fn load_layout_match(content: &Content, sim: SimConfig, layout: &MapLayout) -> Result<MatchContent> {
    let key = layout_key(layout);
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).ok();
    let coll_path = dir.join(format!("{}.{key:016x}.collision.bin", layout.name));
    let nav_path = dir.join(format!("{}.{key:016x}.nav.bin", layout.name));
    let data_path = dir.join(format!("{}.{key:016x}.derived.json", layout.name));
    let cached = (CollisionWorld::load(&coll_path).ok(), std::fs::read_to_string(&data_path).ok().and_then(|t| serde_json::from_str::<Derived>(&t).ok()));
    let (mut collision, derived) = match cached {
        (Some(c), Some(d)) => (c, d),
        _ => {
            let (c, d) = derive(content, &sim, layout)?;
            c.save(&coll_path).ok();
            if let Ok(t) = serde_json::to_string(&d) {
                std::fs::write(&data_path, t).ok();
            }
            (c, d)
        }
    };
    collision.water = Some(std::sync::Arc::new(derived.water));
    collision.volumes = Some(std::sync::Arc::new(derived.volumes));
    collision.climbables = Some(std::sync::Arc::new(derived.climbables));
    if let Some(g) = &layout.nav {
        let nav = match dishonored_world::nav::NavWorld::load(&nav_path) {
            Ok(n) if !n.polys.is_empty() => n,
            _ => {
                let m = &sim.gameplay.movement;
                let n = generate_nav(&collision, g, &NavAgent { collision_radius: m.collision_radius, collision_half_height: m.collision_half_height, walkable_floor_z: m.walkable_floor_z, max_step_height: m.max_step_height });
                n.save(&nav_path).ok();
                n
            }
        };
        collision.nav = Some(std::sync::Arc::new(nav));
    }
    let spawns: Vec<Spawn> = layout.starts.iter().map(|s| Spawn { position: v3(s.at), yaw: s.yaw }).collect();
    let mut markers: Vec<dishonored_assets::npcs::LevelMarker> = layout.points.iter().map(|p| dishonored_assets::npcs::LevelMarker { name: p.name.clone(), class: "TargetPoint".into(), package: layout.name.clone(), position: v3(p.at), yaw: p.yaw }).collect();
    for (i, s) in layout.starts.iter().enumerate() {
        markers.push(dishonored_assets::npcs::LevelMarker { name: format!("PlayerStart_{i}"), class: "PlayerStart".into(), package: layout.name.clone(), position: v3(s.at), yaw: s.yaw });
    }
    let tagged_starts = layout
        .starts
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.tag.is_empty())
        .map(|(i, s)| dishonored_assets::travel::TaggedStart { package: layout.name.clone(), name: format!("PlayerStart_{i}"), tag: s.tag.clone(), position: v3(s.at), yaw: s.yaw })
        .collect();
    Ok(MatchContent {
        map_name: layout.name.clone(),
        collision,
        levels: Default::default(),
        spawns,
        sim,
        ai_points: Vec::new(),
        guard_spawns: Vec::new(),
        markers,
        travel: Vec::new(),
        tagged_starts,
        flee_points: Vec::new(),
        spawners: Vec::new(),
        routes: Vec::new(),
        ai_level: derived.ai_level,
        pickups: derived.pickups,
        usables: derived.usables,
        world_actors: Default::default(),
        interactables: Default::default(),
    })
}

/// Planes of an axis-aligned box (`n . p <= d` inside).
fn box_planes(min: Vec3, max: Vec3) -> Vec<[f32; 4]> {
    vec![[1.0, 0.0, 0.0, max.x], [-1.0, 0.0, 0.0, -min.x], [0.0, 1.0, 0.0, max.y], [0.0, -1.0, 0.0, -min.y], [0.0, 0.0, 1.0, max.z], [0.0, 0.0, -1.0, -min.z]]
}

fn bool_prop(props: &[ue3_package::properties::Property], name: &str) -> Option<bool> {
    match ue3_package::properties::find(props, name).map(|p| &p.value) {
        Some(ue3_package::properties::Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// The original interaction text of a usable tweak (`m_InteractText` of the tweak or of its
/// `pInteractableTweaks`), else `fallback`.
fn tweak_text(tweak: Option<&str>, key: &str, fallback: &str) -> String {
    let loc = localization();
    tweak
        .and_then(|t| loc.get(t, key).or_else(|| loc.get(&format!("{t}.pInteractableTweaks"), key)))
        .map(|s| dishonored_assets::localization::with_keys(s, "F"))
        .unwrap_or_else(|| fallback.to_string())
}

/// Collision and the rest of the derived data of a layout.
fn derive(content: &Content, sim: &SimConfig, layout: &MapLayout) -> Result<(CollisionWorld, Derived)> {
    use dishonored_assets::collision_audit::mask;
    let map = import_layout_geometry(content, layout)?;
    let base = CollisionWorld::from_map(&map);
    let mut extra: Vec<(String, u8, u8, Vec<Vec<[Vec3; 3]>>)> = Vec::new();
    let mut movers = Vec::new();
    let mut climbables = Vec::new();
    let mut pickups = Vec::new();
    let mut usables = Vec::new();
    let mut by_pkg: HashMap<String, PackageData> = HashMap::new();
    for a in &layout.actors {
        let (pkg_name, actor_name) = a.source();
        let (_, _, class, placed, props) = match placed_actor(content, &a.from) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("warning: layout {}: {e:#}", layout.name);
                continue;
            }
        };
        let delta = a.delta(placed);
        let name = a.actor_name(&layout.name);
        let pkgs = vec![pkg_name.to_string()];
        let data = by_pkg.entry(pkg_name.to_ascii_lowercase()).or_insert_with(|| PackageData::load(content, &pkgs));
        let full = format!("{pkg_name}:{actor_name}");
        let xf = |t: &[Vec<[Vec3; 3]>]| -> Vec<Vec<[Vec3; 3]>> { t.iter().map(|e| e.iter().map(|tri| tri.map(|v| delta.transform_point3(v))).collect()).collect() };
        if dishonored_assets::usable_props::is_usable_class(&class) {
            let Some(prop) = data.props.iter().find(|p| p.actor.eq_ignore_ascii_case(&full)).cloned() else {
                eprintln!("warning: layout {}: usable {} has no skeletal mesh", layout.name, a.from);
                continue;
            };
            let src = data.sources.iter().find(|s| s.actor.eq_ignore_ascii_case(actor_name) && s.component_class == "SkeletalMeshComponent");
            let tweak_path = src.and_then(|s| s.tweak.clone());
            let tw = tweak_path.as_ref().and_then(|p| data.tweaks.as_ref()?.try_resolve(p));
            let tflag = |n: &str| tw.as_ref().is_some_and(|t| matches!(t.get(n), Some(dishonored_assets::tweaks::TVal::B(true))));
            let has = |n: &str| src.is_some_and(|s| s.extra.iter().any(|e| e == n));
            let is_door = class == "DisDoor" && prop.door.is_some();
            // Vision answers as in `add_level_blockers`: doors and tweaks that trace as world
            // geometry are seen unless transparent for vision.
            let transparent = if has("m_bOverrideDefaultVisionTransparency") { has("m_bTransparentForVisionChecks") } else { tflag("m_bTransparentForVisionChecks") };
            let trace = src.map_or(mask::MOVE, |s| s.trace_mask);
            let seen = !transparent && (is_door || src.is_some_and(|s| s.world_geometry) || tflag("m_bShouldTraceAsWorldGeometry"));
            let channels = dishonored_world::channel::from_trace_mask(if seen { trace } else { trace & !mask::VISION });
            let blocks = src.is_none_or(|s| s.active() && s.block && s.actor_block);
            let mut closed: Vec<Vec<[Vec3; 3]>> = Vec::new();
            let initial = a.door_state.or(prop.door.as_ref().map(|d| d.initial)).unwrap_or(0);
            if is_door {
                match dishonored_assets::prop_collision::door_states(content, &pkgs, &prop) {
                    Ok(states) => {
                        closed = xf(&states.iter().find(|s| s.0 == 0).map(|s| s.1.clone()).unwrap_or_default());
                        if blocks {
                            let info = dishonored_world::TriInfo { channels, shape: dishonored_world::shape::ALL, kind: dishonored_world::hit_kind::OTHER, elem: u32::MAX };
                            let mut tris = Vec::new();
                            let mut infos = Vec::new();
                            for (ei, e) in closed.iter().enumerate() {
                                tris.extend_from_slice(e);
                                infos.extend(std::iter::repeat_n(dishonored_world::TriInfo { elem: ei as u32, ..info }, e.len()));
                            }
                            movers.push(dishonored_world::MoverCollision {
                                actor: name.clone(),
                                actor_to_world: delta * prop.actor_to_world,
                                tris,
                                info: infos,
                                poses: states.into_iter().filter(|s| s.0 != 0).map(|(k, t)| (k, xf(&t))).collect(),
                                initial,
                            });
                        }
                    }
                    Err(e) => eprintln!("warning: door collision of {}: {e:#}", a.from),
                }
            } else {
                match dishonored_assets::prop_collision::usable_start_bodies(content, &pkgs, &prop, tw.as_ref()) {
                    Ok(b) => {
                        closed = xf(&b.tris);
                        if blocks {
                            extra.push((name.clone(), channels, dishonored_world::hit_kind::OTHER, closed.clone()));
                        }
                    }
                    Err(e) => eprintln!("warning: collision of {}: {e:#}", a.from),
                }
            }
            let pts: Vec<Vec3> = closed.iter().flatten().flat_map(|t| t.iter().copied()).collect();
            let location = delta.transform_point3(placed.w_axis.truncate());
            let center = if pts.is_empty() { location + Vec3::Z * 60.0 } else { pts.iter().copied().sum::<Vec3>() / pts.len() as f32 };
            let stages = match tw.as_ref().and_then(|t| t.get("m_Stages")) {
                Some(dishonored_assets::tweaks::TVal::List(l)) => (l.len() as i32).max(2),
                _ => 2,
            };
            let locked = a.locked.or(bool_prop(&props, "m_bLocked")).unwrap_or(false);
            usables.push(UsableDef {
                actor: name.clone(),
                map: layout.name.clone(),
                location,
                center,
                // 0x00A2D2E0's reference axis: the component's X axis in world space (the mesh's
                // `m_OriginTransform` inverse applied first; identity for these meshes, LIKELY).
                door_axis: is_door.then(|| (delta * prop.transform).x_axis.truncate().normalize_or(Vec3::X)),
                locked,
                initial,
                stages,
                prompt: tweak_text(tweak_path.as_deref(), "m_InteractText", "Use"),
                locked_prompt: tweak_text(tweak_path.as_deref(), "m_LockedMessage", "Locked"),
            });
        } else if class == "DisClimbable" {
            match data.climbables.iter().find(|c| c.name.eq_ignore_ascii_case(actor_name)) {
                Some(c) => climbables.push(dishonored_world::Climbable {
                    name: name.clone(),
                    anchor: delta.transform_point3(c.anchor),
                    bones: c.bones.iter().map(|b| delta.transform_point3(*b)).collect(),
                    sphere_radius: c.sphere_radius,
                    body_half_width: c.body_half_width,
                    top: delta.transform_point3(c.top),
                    bottom: delta.transform_point3(c.bottom),
                }),
                None => eprintln!("warning: layout {}: climbable {} not found", layout.name, a.from),
            }
        } else if let Some(p) = data.pickups.iter().find(|p| p.name.eq_ignore_ascii_case(actor_name)) {
            let mut p = p.clone();
            p.position = delta.transform_point3(p.position);
            p.name = name.rsplit(':').next().unwrap_or(&name).to_string();
            p.package = layout.name.clone();
            p.stealable_from = None;
            pickups.push(p);
        }
    }
    let mut collision = base.with_extra_kinds(&extra);
    collision.movers.extend(movers);
    let water = layout
        .water
        .iter()
        .map(|w| dishonored_world::WaterVolume { name: w.name.clone(), hulls: vec![box_planes(v3(w.min), v3(w.max))], min: v3(w.min), max: v3(w.max), fluid_friction: w.fluid_friction, terminal_velocity: dishonored_assets::water::DEFAULT_TERMINAL_VELOCITY, shallow: w.shallow })
        .collect();
    let volumes = layout
        .pain
        .iter()
        .map(|p| dishonored_world::PhysicsVolume {
            name: p.name.clone(),
            hulls: vec![box_planes(v3(p.min), v3(p.max))],
            min: v3(p.min),
            max: v3(p.max),
            priority: 0,
            zone_velocity: Vec3::ZERO,
            velocity_affects_walking: true,
            pain: Some(dishonored_world::VolumePain { damage_per_sec: p.damage_per_sec, interval: p.interval, entry: p.entry, damage_type: p.damage_type.clone() }),
        })
        .collect();
    // The AI's factions come from the original tweaks of the layout's packages; its light probe
    // samples the layout's lights (no zones, bells or audio cells: the layout has none).
    let mut ai_level = dishonored_assets::ai_level::extract_ai_level(content, &layout.packages, &[], &sim.gameplay.ai.loudness_distances).unwrap_or_default();
    ai_level.zones.clear();
    ai_level.alarms.clear();
    ai_level.audio = Default::default();
    // The original's Playground is built inside L_Boyle_Ext_P (at +20000 z, ORACLE.md "Layout
    // maps in the original"), so its visibility component normalizes with Boyle's map range.
    ai_level.player_light_range = dishonored_assets::ai_level::map_light_range(content, "L_Boyle_Ext_P");
    ai_level.lights = map.lights.iter().map(dishonored_assets::ai_level::probe_light).collect();
    // The stealth light environment: the layout's lights (built as `PointLight`s, in the
    // `m_LightProbe` channel by that template), no light volume (the layout has none).
    ai_level.stealth_lights = ai_level.lights.clone();
    // Boyle's sky light (`L_Boyle_Ext_Env` `SkyLight_0_LC`, `m_LightProbe`) reaches the original's
    // Playground too: captured at 0.1515 of the linear light at every lane point (ORACLE.md
    // "Stealth light environment"). The layout's directional stands for Boyle's dominant one.
    if let Ok(pkg) = content.package("L_Boyle_Ext_Env") {
        for l in dishonored_assets::light_env::read_lights(&pkg, true) {
            let p = dishonored_assets::ai_level::stealth_light(&l);
            if p.kind == 3 && dishonored_assets::light_env::channels_overlap(l.channels, dishonored_assets::light_env::STEALTH_DLE_CHANNELS) {
                ai_level.stealth_lights.push(p);
            }
        }
    }
    ai_level.light_volumes.clear();
    ai_level.sky_light = layout.sky_light;
    Ok((collision, Derived { water, volumes, climbables, pickups, usables, ai_level }))
}

/// What a package gives the actors copied from it, read once.
struct PackageData {
    props: Vec<dishonored_ir::SkeletalProp>,
    sources: Vec<dishonored_assets::collision_audit::CollisionSource>,
    tweaks: Option<dishonored_assets::tweaks::TweakDb>,
    climbables: Vec<dishonored_assets::climbable::ClimbableDef>,
    pickups: Vec<dishonored_ir::items::PlacedPickup>,
}

impl PackageData {
    fn load(content: &Content, pkgs: &[String]) -> Self {
        PackageData {
            props: dishonored_assets::usable_props::read_props(content, pkgs),
            sources: dishonored_assets::collision_audit::collision_sources(content, pkgs),
            tweaks: dishonored_assets::tweaks::TweakDb::load_with(content, pkgs).ok(),
            climbables: dishonored_assets::climbable::extract_climbables(content, pkgs).unwrap_or_default(),
            pickups: dishonored_assets::pickups::extract_pickups(content, pkgs).unwrap_or_default(),
        }
    }
}

/// A navigation mesh over a layout's walkable floors (the original's are authored in the editor;
/// a layout has none). Grid cells where a standing pawn fits on a walkable floor become square
/// polygons at the floor's height; neighbours link when the step between them is at most
/// `MaxStepHeight` and nothing blocks the way at waist height. Several floors over one cell
/// (platforms, the pool's basin) each get a polygon.
pub fn generate_nav(world: &CollisionWorld, g: &NavGrid, mv: &NavAgent) -> dishonored_world::nav::NavWorld {
    let cell = g.cell.max(25.0);
    let nx = (((g.max.0 - g.min.0) / cell).ceil().max(1.0)) as i32;
    let ny = (((g.max.1 - g.min.1) / cell).ceil().max(1.0)) as i32;
    let top = world.bounds.as_ref().map(|b| b.max.z).unwrap_or(5000.0) + 10.0;
    let (r, hh) = (mv.collision_radius, mv.collision_half_height);
    // Nodes per cell: floor heights.
    let mut nodes: Vec<(i32, i32, f32)> = Vec::new();
    let mut at: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    let mut contacts = Vec::new();
    'cells: for ix in 0..nx {
        for iy in 0..ny {
            let c = glam::Vec2::new(g.min.0 + (ix as f32 + 0.5) * cell, g.min.1 + (iy as f32 + 0.5) * cell);
            let mut z = top;
            for _ in 0..6 {
                let Some(h) = world.raycast(Vec3::new(c.x, c.y, z), Vec3::NEG_Z, 50000.0, channel::PAWN) else { break };
                let fz = h.point.z;
                if h.normal.z >= mv.walkable_floor_z {
                    contacts.clear();
                    world.box_contacts(&PawnBox::new(Vec3::new(c.x, c.y, fz + hh + 12.0), r, hh), channel::PAWN, &mut contacts);
                    if contacts.is_empty() {
                        if nodes.len() >= 16000 {
                            eprintln!("warning: generated navigation capped at 16000 polygons (u16 vertex indices)");
                            break 'cells;
                        }
                        at.entry((ix, iy)).or_default().push(nodes.len());
                        nodes.push((ix, iy, fz));
                    }
                }
                z = fz - 1.0;
            }
        }
    }
    let corner = |ix: i32, iy: i32, z: f32| Vec3::new(g.min.0 + ix as f32 * cell, g.min.1 + iy as f32 * cell, z);
    let mut mesh = NavMesh { source: "generated".into(), version: (0, 0), ..Default::default() };
    for &(ix, iy, z) in &nodes {
        let v = mesh.verts.len() as u16;
        mesh.verts.extend([corner(ix, iy, z), corner(ix + 1, iy, z), corner(ix + 1, iy + 1, z), corner(ix, iy + 1, z)]);
        let min = corner(ix, iy, z);
        let max = corner(ix + 1, iy + 1, z);
        mesh.polys.push(NavPoly { verts: vec![v, v + 1, v + 2, v + 3], edges: Vec::new(), center: (min + max) * 0.5, normal: Vec3::Z, min, max, height: 2.0 * hh });
    }
    // Links to the +X and +Y neighbours, through the shared side of the first polygon.
    for (a, &(ix, iy, za)) in nodes.iter().enumerate() {
        for (dx, dy) in [(1, 0), (0, 1)] {
            let Some(list) = at.get(&(ix + dx, iy + dy)) else { continue };
            for &b in list {
                let zb = nodes[b].2;
                if (za - zb).abs() > mv.max_step_height {
                    continue;
                }
                let from = Vec3::new(mesh.polys[a].center.x, mesh.polys[a].center.y, za.max(zb) + 60.0);
                let to = Vec3::new(mesh.polys[b].center.x, mesh.polys[b].center.y, from.z);
                if world.raycast(from, (to - from).normalize(), cell, channel::PAWN).is_some() {
                    continue;
                }
                let base = a as u16 * 4;
                let (v0, v1) = if dx == 1 { (base + 1, base + 2) } else { (base + 2, base + 3) };
                let e = mesh.edges.len() as u16;
                let center = (mesh.verts[v0 as usize] + mesh.verts[v1 as usize]) * 0.5;
                mesh.edges.push(NavEdge { class: NavEdgeClass::Base, kind: NavEdgeType::Normal, v0, v1, poly0: a as u16, poly1: b as u16, length: cell, effective_length: cell, center, path_object: None });
                mesh.polys[a].edges.push(e);
                mesh.polys[b].edges.push(e);
            }
        }
    }
    let (lo, hi) = mesh.verts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
    mesh.min = lo;
    mesh.max = hi;
    let pylon = NavPylon { name: "GeneratedPylon".into(), package: "layout".into(), object_ref: 0, location: (lo + hi) * 0.5, nav_guid: [0; 4], tall_boy: false, mesh, obstacle: None };
    dishonored_world::nav::NavWorld::from_data(&NavData { pylons: vec![pylon], path_objects: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Mat4;

    #[test]
    fn layout_parses_with_implicit_some() {
        let l = parse_layout(
            r#"(
                name: "T",
                packages: ["P"],
                brushes: [(at: (0, 0, -64), shape: Box((1000, 1000, 64)), material: "M")],
                actors: [(from: "P:DisDoor_1", at: (10, 0, 0), yaw: 90, locked: true)],
                water: [(name: "pool", min: (0, 0, -500), max: (100, 100, -30))],
                nav: (min: (-500, -500), max: (500, 500)),
            )"#,
        )
        .unwrap();
        assert_eq!(l.brushes.len(), 1);
        assert_eq!(l.brushes[0].material.as_deref(), Some("M"));
        assert_eq!(l.actors[0].locked, Some(true));
        assert_eq!(l.water[0].fluid_friction, 2.4);
        assert_eq!(l.nav.as_ref().unwrap().cell, 100.0);
    }

    #[test]
    fn generated_nav_walks_a_floor_and_stops_at_a_wall() {
        // A 1000 x 1000 floor with a wall across x = 0 (open for y > 300).
        let floor = dishonored_world::box_triangles(Mat4::from_translation(Vec3::new(0.0, 0.0, -32.0)) * Mat4::from_scale(Vec3::new(500.0, 500.0, 32.0)));
        let wall = dishonored_world::box_triangles(Mat4::from_translation(Vec3::new(0.0, -100.0, 200.0)) * Mat4::from_scale(Vec3::new(20.0, 400.0, 200.0)));
        let w = CollisionWorld::from_triangles(Vec::new(), Vec::new()).with_extra(&[("floor".into(), channel::ALL, floor), ("wall".into(), channel::ALL, wall)]);
        let mv = NavAgent { collision_radius: 35.0, collision_half_height: 87.5, walkable_floor_z: 0.643, max_step_height: 44.0 };
        let nav = generate_nav(&w, &NavGrid { min: (-500.0, -500.0), max: (500.0, 500.0), cell: 100.0 }, &mv);
        assert!(nav.polys.len() > 60, "{} polys", nav.polys.len());
        // From one side of the wall to the other: around its open end.
        let path = nav.find_path(Vec3::new(-300.0, -300.0, 90.0), Vec3::new(300.0, -300.0, 90.0), &Default::default()).expect("path around the wall");
        assert!(path.iter().any(|p| p.y > 300.0), "{path:?}");
    }
}
