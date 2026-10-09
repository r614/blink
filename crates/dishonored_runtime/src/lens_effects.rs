//! Camera lens effects (`DisEmitterCameraLensEffect`, spawned by the Kismet's
//! `DisSeqAct_SpawnCameraLensEffect`): rain and water on the lens, the sickness overlays, lightning
//! flashes. Each is its particle system held before the camera (UE3
//! `EmitterCameraLensEffectBase::UpdateLocation`): at the camera's rotation, `DistFromCamera` along
//! its view, pulled in by `tan(BaseFOV / 2) / tan(FOV / 2)` for a wider view. A `LifeSpan` ends it;
//! "Stop Looping" stops spawning (the particles finish).

use crate::players::GameCamera;
use crate::session::SessionView;
use bevy::prelude::*;

/// The lens effects this client drew: the last action id seen, and the live instances
/// (id, entity, age, life span).
#[derive(Resource, Default)]
pub struct LensEffects {
    seen: u32,
    live: Vec<(u32, Entity, f32, f32)>,
}

/// The Dishonored world transform of a lens effect `distance` before the camera.
pub fn lens_transform(cam: &GlobalTransform, distance: f32) -> Mat4 {
    let swap = |v: Vec3| Vec3::new(v.x, v.z, v.y);
    let eye = swap(cam.translation()) / crate::coords::UU_TO_BEVY;
    let x = swap(*cam.forward()).normalize_or_zero();
    let z = swap(*cam.up()).normalize_or_zero();
    let y = z.cross(x).normalize_or_zero();
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), (eye + x * distance).extend(1.0))
}

/// `DistFromCamera` adjusted for the view's horizontal FOV (degrees) against `BaseFOV`.
pub fn fov_distance(distance: f32, base_fov: f32, fov: f32) -> f32 {
    if base_fov <= 0.0 || fov <= 0.0 {
        return distance;
    }
    distance * (base_fov.to_radians() * 0.5).tan() / (fov.to_radians() * 0.5).tan()
}

#[allow(clippy::too_many_arguments)]
pub fn update(
    view: Res<SessionView>,
    time: Res<Time>,
    fx: Option<ResMut<crate::effects::Effects>>,
    mut st: ResMut<LensEffects>,
    cams: Query<(&GlobalTransform, &Projection), With<GameCamera>>,
    mut q: Query<&mut crate::effects::FxInstance>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some(mut fx) = fx else { return };
    let Some((cam, proj)) = cams.iter().next() else { return };
    let hfov = match proj {
        Projection::Perspective(p) => (2.0 * ((p.fov * 0.5).tan() * p.aspect_ratio).atan()).to_degrees(),
        _ => 90.0,
    };
    let dt = time.delta_secs();
    let seen = st.seen;
    for l in view.matchv.lens_effects.iter().filter(|l| l.id > seen) {
        st.seen = st.seen.max(l.id);
        let Some(def) = fx.lens.get(&l.action.to_ascii_lowercase()).cloned() else { continue };
        let xf = lens_transform(cam, fov_distance(def.distance, def.base_fov, hfov));
        if let Some(e) = crate::effects::spawn_system(&mut commands, &mut meshes, &mut fx, &def.system, xf, false) {
            st.live.push((l.id, e, 0.0, def.life_span));
        }
    }
    let mut keep = Vec::new();
    for (id, e, age, life) in std::mem::take(&mut st.live) {
        let Ok(mut inst) = q.get_mut(e) else {
            // Not spawned yet (commands pending) or gone.
            if age == 0.0 {
                keep.push((id, e, age + dt, life));
            }
            continue;
        };
        let age = age + dt;
        let stopped = view.matchv.lens_effects.iter().find(|l| l.id == id).is_none_or(|l| l.stopped);
        let def = view.matchv.lens_effects.iter().find(|l| l.id == id).and_then(|l| fx.lens.get(&l.action.to_ascii_lowercase()));
        let distance = def.map_or(0.0, |d| fov_distance(d.distance, d.base_fov, hfov));
        inst.set_transform(lens_transform(cam, distance));
        if stopped || (life > 0.0 && age >= life) {
            inst.stop();
            continue;
        }
        keep.push((id, e, age, life));
    }
    st.live = keep;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wider_view_pulls_the_effect_in() {
        assert_eq!(fov_distance(100.0, 0.0, 120.0), 100.0, "no base FOV: as placed");
        assert!((fov_distance(100.0, 90.0, 90.0) - 100.0).abs() < 1e-3);
        assert!(fov_distance(100.0, 90.0, 110.0) < 100.0);
    }
}
