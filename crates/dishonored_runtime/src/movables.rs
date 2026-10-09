//! Movable props (`DishonoredMovable`) drawn where the simulation has them: the placed static mesh
//! instance of each prop follows its `MovableState` (held in front of the eye, flying after a
//! throw, at rest where it fell), and a broken one disappears.
//!
//! The original has no carry animation for props (research/dishonored/systems/movables.md M-4):
//! the arms stay on the empty-hand idle / locomotion while the prop hangs in front, so the
//! presentation of a carried prop is the prop alone, at the position the sim's hold rule gives
//! (`dishonored_sim::movables::hold_target`, moving 0.066 s from where it was grabbed). The
//! original draws a held prop in the foreground depth group; here it is drawn with the world.

use crate::coords;
use crate::session::SessionView;
use bevy::prelude::*;
use dishonored_kismet::interp::rot_matrix_deg;
use dishonored_sim::state::{MovablePhase, MovableState};

/// A static mesh instance of a `DishonoredMovable` actor (the level's mesh the prop is drawn with).
#[derive(Component, Clone, Debug)]
pub struct PropActor {
    /// Actor object name (`DishonoredMovable_29`).
    pub actor: String,
    /// The instance's translation in Dishonored space (to tell same-named actors of different
    /// sublevels apart).
    pub origin: glam::Vec3,
}

/// The instance's transform as the level placed it.
#[derive(Component, Clone, Copy)]
pub struct PropBase(pub Transform);

/// `map_render` hook: instances of movable props stay separate entities (they are not merged into
/// the static batches) so they can move.
pub fn prop_of(source: &str, transform: glam::Mat4) -> Option<PropActor> {
    let (path, class) = source.rsplit_once(" [")?;
    let class = class.trim_end_matches(']');
    if class != "DishonoredMovable" && class != "DisDLC05_Movable" {
        return None;
    }
    let parts: Vec<&str> = path.split('.').collect();
    let actor = parts.get(parts.len().checked_sub(2)?)?.to_string();
    Some(PropActor { actor, origin: transform.w_axis.truncate() })
}

/// The actor's world matrix in Dishonored space: pivot and (pitch, yaw, roll) in degrees.
fn actor_matrix(pos: glam::Vec3, rot: glam::Vec3) -> glam::Mat4 {
    glam::Mat4::from_rotation_translation(glam::Quat::from_mat3(&rot_matrix_deg(rot.x, rot.y, rot.z)), pos)
}

/// Where the mesh instance goes for a prop at `m`: the level's transform moved by what the actor
/// moved (its current matrix times the inverse of the placed one).
pub fn instance_transform(base: Transform, m: &MovableState) -> Transform {
    let delta = actor_matrix(m.position, m.rotation) * actor_matrix(m.home, m.home_rotation).inverse();
    Transform::from_matrix(coords::transform(delta) * base.to_matrix())
}

/// Props follow the simulation: held, thrown and fallen ones are drawn where the sim has them; the
/// ones at rest on their placed spot keep the level's transform; a broken one is hidden.
pub fn follow_props(view: Res<SessionView>, mut commands: Commands, mut props: Query<(Entity, &PropActor, &mut Transform, &mut Visibility, Option<&PropBase>)>) {
    for (e, pa, mut tr, mut vis, base) in &mut props {
        let Some(m) = view.movables.iter().find(|m| m.name.eq_ignore_ascii_case(&pa.actor) && m.home.distance(pa.origin) < 400.0) else { continue };
        let base = match base {
            Some(b) => b.0,
            None => {
                commands.entity(e).insert(PropBase(*tr));
                *tr
            }
        };
        let want_vis = if m.gone { Visibility::Hidden } else { Visibility::Inherited };
        if *vis != want_vis {
            *vis = want_vis;
        }
        let at_home = m.phase == MovablePhase::Rest && m.position == m.home && m.rotation == m.home_rotation;
        let want = if at_home { base } else { instance_transform(base, m) };
        if *tr != want {
            *tr = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_movable_classes_are_props() {
        let src = "L_Boyle_Int_Script.TheWorld.PersistentLevel.DishonoredMovable_29.StaticMeshComponent0 [DishonoredMovable]";
        let p = prop_of(src, glam::Mat4::from_translation(glam::Vec3::new(1.0, 2.0, 3.0))).unwrap();
        assert_eq!(p.actor, "DishonoredMovable_29");
        assert_eq!(p.origin, glam::Vec3::new(1.0, 2.0, 3.0));
        assert!(prop_of("L.TheWorld.PersistentLevel.InterpActor_1.StaticMeshComponent0 [InterpActor]", glam::Mat4::IDENTITY).is_none());
    }
}
