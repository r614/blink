//! Walls of light (`DisWallOfLight`) shown while powered: the wall is placed hidden and its power
//! callbacks show it on (0x00A3EB10) and hide it off (0x00A3ECA0); the match view carries the
//! power state (`dishonored_sim::walls`, research/dishonored/systems/walls_of_light.md).

use crate::session::SessionView;
use bevy::prelude::*;

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

/// Each wall's mesh is visible while the wall is powered; walls the view does not list (no
/// mission, or a map whose walls were not read) keep the level's visibility.
pub fn show_walls(view: Res<SessionView>, mut walls: Query<(&WallOfLightMesh, &mut Visibility)>) {
    for (w, mut vis) in &mut walls {
        let Some(v) = view.matchv.walls.iter().find(|v| v.name.eq_ignore_ascii_case(&w.actor) && glam::Vec3::from(v.position).distance(w.origin) < 400.0) else { continue };
        let want = if v.powered { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
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
}
