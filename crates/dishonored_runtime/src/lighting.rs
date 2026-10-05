//! Real-time lights from the level's `_Light`/`_Env`/`_Script` packages (dominant and
//! toggleable lights; static lights are already in the lightmaps).
//!
//! Unit conversion (UE3 -> Bevy), with `U = map_render::light_unit()`:
//! - UE3 diffuse = albedo * Brightness * LightColor * N.L (no 1/pi); Bevy diffuse includes
//!   1/pi, so directional `illuminance = Brightness * pi * U`.
//! - UE3 point/spot attenuation is `(1 - (d/R)^2)^FalloffExponent` with no inverse square.
//!   APPROXIMATION: Bevy's inverse-square light (windowed to `range = R`) is matched to the
//!   UE3 value at d = R/4: `intensity = 4 pi^2 (R/4)^2 * Brightness * atten(R/4) * U`
//!   (Bevy space metres). Closer than R/4 Bevy is brighter, beyond it falls off faster.
//! - Spot cone angles are half-angles in both engines.

use crate::coords;
use crate::map_render::{RenderFeatures, light_unit};
use bevy::light::CascadeShadowConfigBuilder;
use bevy::prelude::*;
use dishonored_ir::{DishonoredMap, Light, LightKind};

/// Upper bound on spawned non-directional lights (clustered forward copes, but shadowed
/// point lights are expensive: only dominant ones cast shadows).
pub const MAX_LIGHTS: usize = 64;

#[derive(Component)]
pub struct MapLight {
    pub source: String,
}

fn color(l: &Light) -> Color {
    Color::linear_rgb(l.color[0], l.color[1], l.color[2])
}

fn local_intensity(l: &Light) -> f32 {
    let r = l.radius * 0.25 * coords::UU_TO_BEVY;
    let atten = (1.0f32 - 0.0625).powf(l.falloff_exponent.max(0.0));
    4.0 * std::f32::consts::PI.powi(2) * r * r * l.brightness * atten * light_unit()
}

/// Spawn the map's lights. Returns how many were spawned.
pub fn spawn_lights(map: &DishonoredMap, commands: &mut Commands, features: &RenderFeatures) -> usize {
    if !features.dynamic_lights {
        return 0;
    }
    let mut n = 0;
    let mut local = 0;
    for l in &map.lights {
        let dir = coords::dir(l.direction).normalize_or(Vec3::NEG_Y);
        let up = if dir.abs().dot(Vec3::Y) > 0.99 { Vec3::Z } else { Vec3::Y };
        let at = Transform::from_translation(coords::pos(l.position)).looking_to(dir, up);
        let shadows = l.casts_shadows && l.dominant && std::env::var("DISHONORED_NO_SHADOWS").is_err();
        match l.kind {
            LightKind::Directional => {
                commands.spawn((
                    DirectionalLight {
                        color: color(l),
                        illuminance: l.brightness * std::f32::consts::PI * light_unit(),
                        shadow_maps_enabled: shadows,
                        // Dominant light: its direct light is not in the lightmaps.
                        affects_lightmapped_mesh_diffuse: true,
                        ..default()
                    },
                    {
                        let env = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
                        CascadeShadowConfigBuilder { num_cascades: env("DISHONORED_CASCADES", 3.0) as usize, first_cascade_far_bound: env("DISHONORED_CASCADE_FIRST", 15.0), maximum_distance: env("DISHONORED_SHADOW_DIST", 200.0), ..default() }.build()
                    },
                    at,
                    MapLight { source: l.source.clone() },
                ));
            }
            _ if local >= MAX_LIGHTS => continue,
            LightKind::Point => {
                local += 1;
                commands.spawn((
                    PointLight {
                        color: color(l),
                        intensity: local_intensity(l),
                        range: l.radius * coords::UU_TO_BEVY,
                        radius: 0.1,
                        // Cube shadow maps cost six scene passes; the one dominant point light
                        // (sewer) is left unshadowed for frame time.
                        shadow_maps_enabled: false,
                        affects_lightmapped_mesh_diffuse: true,
                        ..default()
                    },
                    at,
                    MapLight { source: l.source.clone() },
                ));
            }
            LightKind::Spot { inner_cone_deg, outer_cone_deg } => {
                local += 1;
                commands.spawn((
                    SpotLight {
                        color: color(l),
                        intensity: local_intensity(l),
                        range: l.radius * coords::UU_TO_BEVY,
                        radius: 0.1,
                        inner_angle: inner_cone_deg.min(outer_cone_deg).to_radians(),
                        outer_angle: outer_cone_deg.clamp(1.0, 89.0).to_radians(),
                        shadow_maps_enabled: shadows,
                        affects_lightmapped_mesh_diffuse: true,
                        ..default()
                    },
                    at,
                    MapLight { source: l.source.clone() },
                ));
            }
        }
        n += 1;
    }
    n
}
