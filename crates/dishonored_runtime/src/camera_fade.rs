//! The camera fade (`SeqAct_CameraFade` -> `PlayerController.ClientSetCameraFade`, replicated in
//! `MatchView::camera_fade`): the screen fade the Kismet plays around its teleports.
//!
//! UE3 draws an enabled camera fade as the scene view's overlay colour (`FadeColor`, alpha
//! `FadeAmount`): the scene is blended towards the colour, the HUD drawn over it (here: a
//! full-screen node under the HUD's Scaleform views, which sit at z -2 / -1). The camera runs the
//! fade on: `Camera::UpdateCamera` (0x005E5EB0) lowers `FadeTimeRemaining` and lerps
//! `FadeAmount` from `FadeAlpha.X` to `.Y` ([`dishonored_multiplayer::CameraFadeView::advance`]);
//! between snapshots the client advances its copy the same way.

use crate::session::SessionView;
use bevy::prelude::*;
use dishonored_multiplayer::CameraFadeView;

/// Below the HUD views.
const FADE_Z: i32 = -3;

#[derive(Component)]
pub struct CameraFadeOverlay;

/// The fade drawn: the last replicated fade and the local copy advanced since.
#[derive(Resource, Default)]
pub struct CameraFade {
    last: Option<CameraFadeView>,
    local: Option<CameraFadeView>,
}

impl CameraFade {
    /// Take a snapshot's fade (a new one, or a changed one, replaces the local copy) and advance
    /// the copy by `dt`. Returns the overlay (colour, alpha) to draw.
    pub fn step(&mut self, replicated: Option<&CameraFadeView>, dt: f32) -> Option<(Vec3, f32)> {
        if replicated != self.last.as_ref() {
            self.last = replicated.cloned();
            self.local = replicated.cloned();
        } else if let Some(l) = self.local.as_mut() {
            l.advance(dt);
        }
        let l = self.local.as_ref()?;
        Some((Vec3::from_array(l.color), l.amount.clamp(0.0, 1.0)))
    }
}

pub struct CameraFadePlugin;

impl Plugin for CameraFadePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraFade>().add_systems(Update, update.run_if(resource_exists::<SessionView>));
    }
}

fn update(mut commands: Commands, view: Res<SessionView>, time: Res<Time>, mut st: ResMut<CameraFade>, mut q: Query<(Entity, &mut BackgroundColor), With<CameraFadeOverlay>>) {
    let draw = st.step(view.matchv.camera_fade.as_ref(), time.delta_secs()).filter(|(_, a)| *a > 0.0);
    match (draw, q.single_mut()) {
        (Some((c, a)), Ok((_, mut bg))) => bg.0 = Color::srgba(c.x, c.y, c.z, a),
        (Some((c, a)), Err(_)) => {
            commands.spawn((
                Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
                BackgroundColor(Color::srgba(c.x, c.y, c.z, a)),
                GlobalZIndex(FADE_Z),
                Pickable::IGNORE,
                CameraFadeOverlay,
            ));
        }
        (None, Ok((e, _))) => commands.entity(e).despawn(),
        (None, Err(_)) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_runs_the_fade_between_snapshots() {
        let mut f = CameraFade::default();
        let v = CameraFadeView { id: 1, color: [0.0; 3], alpha: [0.0, 1.0], time: 1.0, remaining: 1.0, amount: 0.0 };
        assert_eq!(f.step(Some(&v), 0.1), Some((Vec3::ZERO, 0.0)));
        let (_, a) = f.step(Some(&v), 0.25).unwrap();
        assert!((a - 0.25).abs() < 1e-5);
        // A newer snapshot replaces the local copy.
        let v2 = CameraFadeView { remaining: 0.5, amount: 0.5, ..v.clone() };
        assert_eq!(f.step(Some(&v2), 0.1).unwrap().1, 0.5);
        // Disabled: nothing drawn.
        assert_eq!(f.step(None, 0.1), None);
    }
}
