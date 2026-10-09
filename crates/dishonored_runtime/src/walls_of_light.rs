//! Walls of light (`DisWallOfLight`) as the player sees them (`dishonored_sim::walls`,
//! research/dishonored/systems/walls_of_light.md):
//!
//! - **Mesh.** The wall's static mesh (the tweak's `m_pStaticMesh`, `walllight_visible_in_editor`,
//!   set by the tweaks interface's apply 0x00A29110) is the editor's stand-in: its component is
//!   `HiddenGame` (`WallOfLightDef::mesh_hidden`), so it is never drawn; it only collides. A wall
//!   whose mesh is not hidden shows while powered (the actor is placed hidden, 0x00A28D80, shown
//!   by the power callback 0x00A3EB10, hidden by 0x00A3ECA0).
//! - **Spark emitters** (`m_SparkEffects`, placed `Emitter`s): deactivated at placement
//!   (0x00A28D80), activated while powered (0x00A3EB10), deactivated when powered off
//!   (0x00A3ECA0). The level starts them (`bAutoActivate`); the wall takes them over.
//! - **Beams** (`m_Beams`, placed `Emitter`s with beam emitters): activated by the kill effect and
//!   the shock (0x00A30CA0) with their beam end at the effect's point (0x00A29240,
//!   `SetBeamTargetPoint`), deactivated when the activation timer runs out (0x00A28F80).
//! - **Cross spark** (`m_CrossSparkEffect` with the tweak's `m_CrossSparkEffectTemplate`): the
//!   wall's tick (0x00A3EEE0) picks two different spark emitters at random every
//!   `m_fMinSparkInterval`..`m_fMaxSparkInterval` s, puts the component on the first and its
//!   beam end on the second, and with `m_bDisableSystemBetweenSparks` activates it for
//!   `m_fSparkDuration` s only. It is the wall's own component: seen while the wall is shown.
//! - **Shock** particles: `effects::gameplay_effects` on `SimEvent::WallOfLightShock`.

use crate::effects::{Effects, FxInstance, PlacedEmitter};
use crate::session::{Session, SessionView};
use bevy::prelude::*;
use dishonored_ir::world_actors::WallOfLightDef;
use std::collections::HashMap;

/// A static mesh instance of a `DisWallOfLight` actor.
#[derive(Component, Clone, Debug)]
pub struct WallOfLightMesh {
    /// Actor object name (`DisWallOfLight_0`).
    pub actor: String,
    /// The instance's translation in Dishonored space (same-named walls of different sublevels).
    pub origin: glam::Vec3,
}

/// `map_render` hook: the instances of walls of light stay separate entities so they can be
/// hidden.
pub fn wall_of(source: &str, transform: glam::Mat4) -> Option<WallOfLightMesh> {
    let (path, class) = source.rsplit_once(" [")?;
    if class.trim_end_matches(']') != "DisWallOfLight" {
        return None;
    }
    let parts: Vec<&str> = path.split('.').collect();
    let actor = parts.get(parts.len().checked_sub(2)?)?.to_string();
    Some(WallOfLightMesh { actor, origin: transform.w_axis.truncate() })
}

/// The map's wall def of a wall the view lists.
fn def_of<'a>(defs: &'a [WallOfLightDef], name: &str, position: glam::Vec3, reach: f32) -> Option<&'a WallOfLightDef> {
    defs.iter().find(|d| d.name.eq_ignore_ascii_case(name) && d.position.distance(position) < reach)
}

/// Each wall's mesh: hidden when its component is `HiddenGame` (every shipped wall), else
/// visible while the wall is powered. Walls the view does not list (no mission, or a map whose
/// walls were not read) keep the level's visibility.
pub fn show_walls(view: Res<SessionView>, session: Option<Res<Session>>, mut walls: Query<(&WallOfLightMesh, &mut Visibility)>) {
    let defs: &[WallOfLightDef] = session.as_ref().map_or(&[], |s| &s.content.world_actors.walls_of_light);
    for (w, mut vis) in &mut walls {
        let Some(v) = view.matchv.walls.iter().find(|v| v.name.eq_ignore_ascii_case(&w.actor) && glam::Vec3::from(v.position).distance(w.origin) < 400.0) else { continue };
        let hidden = def_of(defs, &v.name, glam::Vec3::from(v.position), 1.0).is_some_and(|d| d.mesh_hidden);
        let want = if v.powered && !hidden { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
    }
}

/// What the presentation runs for one wall.
#[derive(Default)]
pub struct WallFx {
    sparks: Vec<Entity>,
    beams: Vec<Entity>,
    beam_end: Option<glam::Vec3>,
    /// The cross spark's instance while it arcs, `m_fSparkTimer`, `m_fSparkDurationTimer`.
    cross: Option<Entity>,
    spark_timer: f32,
    spark_duration: f32,
}

/// The walls' emitters, by "package:name" (lowercase), and the cross spark's `rand()`.
#[derive(Resource, Default)]
pub struct WallsFx {
    walls: HashMap<String, WallFx>,
    rng: u32,
}

impl WallsFx {
    /// `rand()` (MSVC: `x = x * 214013 + 2531011`, 15 bits).
    fn rand(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(214013).wrapping_add(2531011);
        (self.rng >> 16) & 0x7FFF
    }
}

fn stop_all(commands: &mut Commands, list: &mut Vec<Entity>) {
    for e in list.drain(..) {
        crate::effects::stop_instance(commands, e);
    }
}

/// Drive the walls' emitters from the view: the spark emitters while powered, the beams while
/// the kill effect runs, the cross spark's arcs.
#[allow(clippy::too_many_arguments)]
pub fn drive_wall_emitters(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut fx: ResMut<Effects>,
    mut state: ResMut<WallsFx>,
    view: Res<SessionView>,
    session: Option<Res<Session>>,
    time: Res<Time>,
    placed: Query<(Entity, &PlacedEmitter)>,
    live: Query<(), With<FxInstance>>,
) {
    let Some(session) = session else { return };
    let defs = &session.content.world_actors.walls_of_light;
    if defs.is_empty() {
        return;
    }
    // The level started the walls' emitters (`bAutoActivate`): the walls own them (placement,
    // 0x00A28D80, deactivates the spark emitters; the beams only run with the kill effect).
    let owned: std::collections::HashSet<String> = defs.iter().flat_map(|d| d.spark_effects.iter().chain(d.beams.iter())).map(|k| k.to_ascii_lowercase()).collect();
    for (e, p) in &placed {
        if owned.contains(&p.0) {
            crate::effects::stop_instance(&mut commands, e);
            commands.entity(e).remove::<PlacedEmitter>();
        }
    }
    let dt = time.delta_secs().min(0.1);
    let st = &mut *state;
    for v in &view.matchv.walls {
        let Some(d) = def_of(defs, &v.name, glam::Vec3::from(v.position), 1.0) else { continue };
        let key = format!("{}:{}", d.package, d.name).to_ascii_lowercase();
        let w = st.walls.entry(key).or_default();
        // Instances a level change took away.
        w.sparks.retain(|e| live.contains(*e));
        w.beams.retain(|e| live.contains(*e));
        if w.cross.is_some_and(|e| !live.contains(e)) {
            w.cross = None;
        }
        // The spark emitters (0x00A3EB10 / 0x00A3ECA0).
        if v.powered && w.sparks.is_empty() {
            for k in &d.spark_effects {
                if let Some((_, _, m, _)) = crate::effects::placed_emitter(&fx, k) {
                    w.sparks.extend(crate::effects::start_placed_emitter(&mut commands, &mut meshes, &mut fx, k, m, false));
                }
            }
        } else if !v.powered && !w.sparks.is_empty() {
            stop_all(&mut commands, &mut w.sparks);
        }
        // The beams (0x00A30CA0 / 0x00A28F80), their end at the effect's point.
        match v.effect.map(glam::Vec3::from) {
            Some(end) => {
                if w.beams.is_empty() {
                    for k in &d.beams {
                        if let Some((_, _, m, _)) = crate::effects::placed_emitter(&fx, k) {
                            w.beams.extend(crate::effects::start_placed_emitter(&mut commands, &mut meshes, &mut fx, k, m, false));
                        }
                    }
                    w.beam_end = None;
                }
                if w.beam_end != Some(end) {
                    for &e in &w.beams {
                        crate::effects::set_beam_target(&mut commands, e, end);
                    }
                    w.beam_end = Some(end);
                }
            }
            None => {
                stop_all(&mut commands, &mut w.beams);
                w.beam_end = None;
            }
        }
        // The cross spark (0x00A3EEE0): the wall's component, seen while it is shown.
        if !v.powered || d.cross_spark.is_empty() || d.spark_effects.is_empty() {
            if let Some(e) = w.cross.take() {
                crate::effects::stop_instance(&mut commands, e);
            }
            w.spark_timer = 0.0;
            continue;
        }
        if w.spark_timer > 0.0 {
            w.spark_timer -= dt;
            w.spark_duration -= dt;
            if w.spark_duration <= 0.0 {
                // The arc ends (0x00A29240 with 1), the system off with `m_bDisableSystemBetweenSparks`.
                if let Some(e) = w.cross.take() {
                    crate::effects::stop_instance(&mut commands, e);
                }
                w.spark_duration = f32::MAX;
            }
            continue;
        }
        let n = d.spark_effects.len() as u32;
        let a = st.rand() % n;
        let mut b = st.rand() % n;
        if a == b {
            b = if b + 1 >= n { 0 } else { b + 1 };
        }
        let w = st.walls.get_mut(&format!("{}:{}", d.package, d.name).to_ascii_lowercase()).expect("wall entry");
        let at = |k: &str| crate::effects::placed_emitter(&fx, k).map(|p| p.2);
        if let (Some(ma), Some(mb)) = (at(&d.spark_effects[a as usize]), at(&d.spark_effects[b as usize])) {
            if let Some(e) = w.cross.take() {
                crate::effects::stop_instance(&mut commands, e);
            }
            if let Some(e) = crate::effects::spawn_system(&mut commands, &mut meshes, &mut fx, &d.cross_spark, glam::Mat4::from_translation(ma.w_axis.truncate()), false) {
                crate::effects::set_beam_target(&mut commands, e, mb.w_axis.truncate());
                w.cross = Some(e);
            }
        }
        // `rand() / 32767` between the intervals; the arc lasts `m_fSparkDuration`.
        let r = st.rand() as f32 * 3.051851e-05;
        let w = st.walls.get_mut(&format!("{}:{}", d.package, d.name).to_ascii_lowercase()).expect("wall entry");
        w.spark_timer = (d.spark_interval[1] - d.spark_interval[0]) * r + d.spark_interval[0];
        w.spark_duration = d.spark_duration;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_walls_of_light_are_tagged() {
        let src = "L_Streets2_Script.TheWorld.PersistentLevel.DisWallOfLight_0.StaticMeshComponent0 [DisWallOfLight]";
        let w = wall_of(src, glam::Mat4::from_translation(glam::Vec3::new(1.0, 2.0, 3.0))).unwrap();
        assert_eq!(w.actor, "DisWallOfLight_0");
        assert!(wall_of("L.TheWorld.PersistentLevel.InterpActor_1.StaticMeshComponent0 [InterpActor]", glam::Mat4::IDENTITY).is_none());
    }

    #[test]
    fn rand_is_msvcs() {
        let mut s = WallsFx { rng: 1, ..Default::default() };
        assert_eq!((s.rand(), s.rand(), s.rand()), (41, 18467, 6334));
    }
}
