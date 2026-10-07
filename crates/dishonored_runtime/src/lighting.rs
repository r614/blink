//! Real-time lights from the level's `_Light`/`_Env`/`_Script` packages (dominant and
//! toggleable lights; static lights are already in the lightmaps).
//!
//! Unit conversion (UE3 -> Bevy), with `U = map_render::light_unit()`:
//! - UE3 diffuse = albedo * Brightness * LightColor * N.L (no 1/pi); Bevy diffuse includes
//!   1/pi, so directional `illuminance = Brightness * pi * U`.
//! - UE3 point/spot attenuation is `max(1 - d^2 / R^2, 0)^FalloffExponent` with no inverse
//!   square (light pass `TLightPixelShader<FPointLightPolicy / FSpotLightPolicy>`: the vertex
//!   shader passes `(LightPosition - P) x InvRadius`, the pixel shader raises
//!   `max(1 - |v|^2, 0)` to `LightColorAndFalloffExponent.w`, 0 below 1e-6). Bevy's distance
//!   attenuation is replaced by that ([`Ue3FalloffPlugin`] patches `bevy_pbr`'s
//!   `pbr_lighting.wgsl`); the exponent travels in the light's `radius` as
//!   `(E + 1) / 1000`. With Bevy's Lambert `1/pi` and its `intensity / 4 pi`, the UE3 value
//!   `albedo x Brightness x LightColor x atten x N.L` needs `intensity = 4 pi^2 Brightness U`.
//! - Spot cones: UE3 `saturate((cos a - cos outer) / (cos inner - cos outer))^2`, the same
//!   form as Bevy's (half-angles in both engines).
//! - `LightColor` (an `FColor`) becomes linear as `(c / 255)^2.2` (UE3's `FLinearColor(FColor)`
//!   table; measured: the moon's (164, 209, 255) x 0.25 draws as (0.0947, 0.1614, 0.25)).

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
    4.0 * std::f32::consts::PI.powi(2) * l.brightness * light_unit()
}

/// The UE3 falloff exponent, carried to the patched shader in the light's `radius`.
pub fn falloff_radius(l: &Light) -> f32 {
    (l.falloff_exponent.max(0.0) + 1.0) * 1e-3
}

/// Replaces Bevy's inverse-square point / spot attenuation with UE3's radial falloff (see the
/// module docs) by patching the embedded `bevy_pbr::lighting` shader once it is loaded. A light
/// whose radius is 0 keeps Bevy's attenuation.
pub struct Ue3FalloffPlugin;

impl Plugin for Ue3FalloffPlugin {
    fn build(&self, app: &mut App) {
        // Patch the embedded source before the shader is read, so no pipeline is compiled twice;
        // `patch_falloff` catches a load that raced ahead of this.
        let path = std::path::Path::new("bevy_pbr/render/pbr_lighting.wgsl");
        match app.world().get_resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().map(|reg| (reg, reg.remove_asset(path))) {
            Some((reg, Some(data))) => {
                let src = String::from_utf8_lossy(data.value()).into_owned();
                let patched = patch_source(&src);
                info!("pbr_lighting.wgsl: embedded source patched: {}", patched.is_some());
                reg.insert_asset(std::path::PathBuf::from(path), path, patched.unwrap_or(src).into_bytes());
                // bevy_pbr started loading the original bytes when it was built; load again so
                // the patched source arrives before the first frame's pipelines are compiled.
                if let Some(server) = app.world().get_resource::<AssetServer>() {
                    server.reload("embedded://bevy_pbr/render/pbr_lighting.wgsl");
                }
            }
            Some((_, None)) => info!("pbr_lighting.wgsl: not in the embedded registry"),
            None => info!("no embedded asset registry"),
        }
        app.add_systems(Update, patch_falloff);
    }
}

fn patch_source(src: &str) -> Option<String> {
    if src.contains("fn ue3_attenuation") {
        return None;
    }
    src.contains(BEVY_ATTENUATION).then(|| format!("{}{UE3_FN}", src.replace(BEVY_ATTENUATION, UE3_ATTENUATION)))
}

const BEVY_ATTENUATION: &str = "let rangeAttenuation = getDistanceAttenuation(distance_square, (*light).color_inverse_square_range.w);";
const UE3_ATTENUATION: &str = "let rangeAttenuation = ue3_attenuation(distance_square, (*light).color_inverse_square_range.w, (*light).position_radius.w);";
const UE3_FN: &str = "
// dishonored-rs: UE3's point / spot light falloff, max(1 - d^2 / R^2, 0)^E (0 below 1e-6), with
// E carried in the light radius as (E + 1) / 1000; radius 0 keeps Bevy's attenuation.
fn ue3_attenuation(distance_square: f32, inverse_range_squared: f32, radius: f32) -> f32 {
    if (radius <= 0.0) {
        return getDistanceAttenuation(distance_square, inverse_range_squared);
    }
    let e = radius * 1000.0 - 1.0;
    let x = max(1.0 - distance_square * inverse_range_squared, 0.0);
    return select(0.0, pow(x, e), x - 1e-6 >= 0.0);
}
";

fn patch_falloff(asset_server: Res<AssetServer>, mut shaders: ResMut<Assets<bevy::shader::Shader>>, mut handle: Local<Option<Handle<bevy::shader::Shader>>>, mut done: Local<bool>) {
    if *done {
        return;
    }
    let h = handle.get_or_insert_with(|| asset_server.load("embedded://bevy_pbr/render/pbr_lighting.wgsl")).clone();
    let Some(s) = shaders.get(&h) else { return };
    *done = true;
    let bevy::shader::Source::Wgsl(src) = &s.source else { return };
    if src.contains("fn ue3_attenuation") {
        return;
    }
    let Some(patched) = patch_source(src) else {
        warn!("pbr_lighting.wgsl: point light attenuation line not found; UE3 falloff not applied");
        return;
    };
    let path = s.path.clone();
    let _ = shaders.insert(h.id(), bevy::shader::Shader::from_wgsl(patched, path));
    warn!("pbr_lighting.wgsl patched with UE3's light falloff after it loaded (pipelines recompile)");
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
                        radius: falloff_radius(l),
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
                        radius: falloff_radius(l),
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
