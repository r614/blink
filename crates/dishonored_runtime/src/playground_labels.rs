//! [playground] In-world labels of a layout map (`maps/<name>.ron`, `labels`): each station's
//! name and what it tests, drawn as screen text at its world point while within range.

use crate::coords;
use bevy::prelude::*;

#[derive(Component)]
pub struct WorldLabel {
    /// Bevy world position.
    at: Vec3,
    /// Shown within this distance (Bevy units).
    range: f32,
}

/// Spawn the labels of the map being played, when it is a layout.
pub fn setup_labels(mut commands: Commands, args: Res<crate::Args>) {
    let Some(Ok(layout)) = dishonored_content::playground::layout_for(&args.map) else { return };
    for l in &layout.labels {
        commands.spawn((
            Text::new(l.text.clone()),
            TextFont { font_size: FontSize::Px(15.0), ..default() },
            TextColor(Color::srgb(1.0, 0.92, 0.7)),
            TextShadow::default(),
            Node { position_type: PositionType::Absolute, ..default() },
            Visibility::Hidden,
            WorldLabel { at: coords::pos(dishonored_ir::layout::v3(l.at)), range: l.range * coords::UU_TO_BEVY },
        ));
    }
}

/// Place each label over its world point (centred), hidden behind the camera or out of range.
pub fn update_labels(camera: Query<(&Camera, &GlobalTransform), With<Camera3d>>, mut labels: Query<(&WorldLabel, &mut Node, &mut Visibility, &ComputedNode)>) {
    let Some((cam, tf)) = camera.iter().find(|(c, _)| c.is_active) else { return };
    let eye = tf.translation();
    for (l, mut node, mut vis, computed) in &mut labels {
        let shown = eye.distance(l.at) <= l.range && tf.forward().dot(l.at - eye) > 0.0;
        let p = shown.then(|| cam.world_to_viewport(tf, l.at).ok()).flatten();
        match p {
            Some(p) => {
                let size = computed.size() * computed.inverse_scale_factor();
                node.left = px(p.x - size.x * 0.5);
                node.top = px(p.y - size.y * 0.5);
                if *vis != Visibility::Inherited {
                    *vis = Visibility::Inherited;
                }
            }
            None => {
                if *vis != Visibility::Hidden {
                    *vis = Visibility::Hidden;
                }
            }
        }
    }
}
