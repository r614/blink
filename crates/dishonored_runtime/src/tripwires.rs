//! Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission. The wire drawn between its ends follows game/src/traps.rs; the keyhole camera and field of view follow the decoded state, not game/src/keyhole.rs.
//!
//! Tripwires (`DisTripwire`) and door keyholes as the client shows them
//! (research/dishonored/systems/interactables.md).
//!
//! - **The wire.** `MatchView::tripwires` lists the active map's wires with their ends and whether
//!   they have tripped. An armed wire is drawn as a thin taut beam between its ends; a tripped
//!   one is gone (the original plays its `m_TrippedAnimName` sequence, `TripodTrap_Fire`, which
//!   drops the rope; the skinned rope itself is not drawn, the beam stands in for it).
//! - **The keyhole.** [`peek_view`] is the local player's `PeekView` while the eye is at a keyhole:
//!   the camera sits at the keyhole socket, and the field of view is the peek's
//!   (`m_fKeyHoleFOV * (1 - Attribute_KeyholePeepingMagnification)`, 0x00A63400).

use crate::coords;
use crate::session::SessionView;
use bevy::prelude::*;
use dishonored_sim::interactables::PeekView;
use std::collections::HashMap;

/// Beam thickness (uu): the rope's diameter is a few units.
const BEAM_THICKNESS_UU: f32 = 1.5;

#[derive(Component)]
pub struct TripwireBeam {
    actor: String,
}

/// The beams spawned for the active map, by `<package>:<actor>`.
#[derive(Resource, Default)]
pub struct TripwireBeams {
    spawned: HashMap<String, Entity>,
}

/// The local player's peek, if the eye is at a keyhole.
pub fn peek_view(view: &SessionView) -> Option<PeekView> {
    let me = view.me.as_ref()?;
    view.matchv.peeks.iter().find(|(id, _)| *id == me.id).map(|(_, v)| *v)
}

/// Spawn, show and hide the wires' beams to follow `MatchView::tripwires`.
pub fn sync_beams(
    view: Res<SessionView>,
    mut beams: ResMut<TripwireBeams>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut q: Query<(&TripwireBeam, &mut Visibility)>,
) {
    let wires = &view.matchv.tripwires;
    // A map change drops the old map's beams.
    beams.spawned.retain(|actor, e| {
        let keep = wires.iter().any(|w| &w.actor == actor);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    for w in wires {
        if !beams.spawned.contains_key(&w.actor) {
            let (a, b) = (coords::pos(Vec3::from(w.a)), coords::pos(Vec3::from(w.b)));
            let len = a.distance(b);
            if len < 1e-4 {
                continue;
            }
            let mesh = meshes.add(Cuboid::new(BEAM_THICKNESS_UU * coords::UU_TO_BEVY, BEAM_THICKNESS_UU * coords::UU_TO_BEVY, 1.0));
            let material = materials.add(StandardMaterial { base_color: Color::srgb(0.16, 0.12, 0.08), perceptual_roughness: 0.9, ..default() });
            let mid = (a + b) * 0.5;
            let transform = Transform::from_translation(mid).looking_to((b - a) / len, Vec3::Y).with_scale(Vec3::new(1.0, 1.0, len));
            let e = commands.spawn((Mesh3d(mesh), MeshMaterial3d(material), transform, TripwireBeam { actor: w.actor.clone() })).id();
            beams.spawned.insert(w.actor.clone(), e);
        }
    }
    for (b, mut vis) in &mut q {
        let tripped = wires.iter().find(|w| w.actor == b.actor).is_some_and(|w| w.tripped);
        let want = if tripped { Visibility::Hidden } else { Visibility::Inherited };
        if *vis != want {
            *vis = want;
        }
    }
}

/// On the frame a peek starts the local look takes the view the door gave the pawn (the socket's
/// rotation, 0x00A38580); from then on the look is the player's own.
pub fn sync_peek_look(view: Res<SessionView>, mut local: ResMut<crate::input_map::LocalInput>, mut was: Local<bool>) {
    let peeking = peek_view(&view).is_some();
    if peeking && !*was {
        if let Some(me) = view.me.as_ref() {
            local.yaw = me.yaw;
            local.pitch = me.pitch;
        }
    }
    *was = peeking;
}
