//! The original's light environments (`DynamicLightEnvironmentComponent`) for the placed doors
//! and usable objects, decoded from `FDynamicLightEnvironmentState` in `Dishonored.exe`
//! (research/dishonored/RENDERING.md "Light environments"):
//!
//! - **Static environment** (0x00501B70): every enabled static light whose lighting channels,
//!   with `CompositeDynamic` read as `Dynamic` (0x004DB950), overlap the owner component's, that
//!   reaches the bounds (`AffectsBounds`) and is visible from them (0x004FF270: one line check from
//!   the bounds sphere point facing the light to the light), adds `Y(L) x GetDirectIntensity`
//!   (0x004FF710; point lights `Brightness x LightColor x max(1 - d^2/R^2, 0)^FalloffExponent`,
//!   0x005306F0, spot lights times `saturate((cos - cos outer) / (cos inner - cos outer))^2`,
//!   0x00549EE0, sky lights from straight up, 0x00547370) to an SH (9 coefficients per channel,
//!   `SHBasisFunction` 0x00450860). The level's precomputed light volume is interpolated at the
//!   bounds origin (0x0053DFA0: samples within their radius, weight `(1 - d/R)/R`, each sample's
//!   SH `Y(indirect dir) x IndirectRadiance + Y(environment dir) x EnvironmentRadiance +
//!   AmbientFunction x AmbientRadiance`, 0x00529710; RGBE colours) and added normalised by the
//!   total weight; the level is the one of the floor below the bounds origin (a line check down).
//! - **Synthesized lights** (0x004F85C0): a directional light along the luminance SH's linear
//!   band (`(-L3, -L1, L2)`), coloured by the projection of the SH on `Y(dir)` (clamped at 0) and
//!   subtracted from it (0x004E1480, 0x004E1300); then an upper and a lower sky light, the
//!   projections of the remainder on `UpperSkyFunction` (1/sqrt(pi), 0, sqrt(3/pi)/2) and
//!   `LowerSkyFunction` (z negated), each subtracted in turn (0x004E16E0). The rest is dropped
//!   (no `bSynthesizeSHLight` on these props).
//!
//! Approximations (U-620): dynamic and toggleable lights are not composited (the port draws them
//! as lights); the directional light's dominant-shadow scale is taken as 1 (no dominant light
//! shadow transition); the colours are not quantised through the light components' `FColor` +
//! brightness; one visibility sample from the bounds sphere toward the light; the line checks use
//! the colliding static geometry.

use crate::content::Content;
use dishonored_ir::{DishonoredMap, LightEnvironment};
use glam::{Mat4, Vec3};
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

/// `float(PI)` as the static initialisers use it (0x00F67490: the doubles are widened floats).
const PI_F: f64 = std::f32::consts::PI as f64;

/// UE3's SH: 9 coefficients (bands 0-2) per channel.
pub type Sh = [[f32; 9]; 3];

/// `FVolumeLightingSample` of a level's `PrecomputedLightVolume` (33 bytes serialized).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeSample {
    pub position: Vec3,
    pub radius: f32,
    /// Indirect direction (theta, phi) and environment direction (theta, phi), one byte each.
    pub directions: [u8; 4],
    /// RGBE `FColor`s in memory order (B, G, R, exponent).
    pub indirect: [u8; 4],
    pub environment: [u8; 4],
    pub ambient: [u8; 4],
    pub shadowed: bool,
}

fn f32_at(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// The precomputed light volume samples of the persistent `Level` of `pkg` (None when the level
/// has none). Found from the end of the export: `bInitialized`, the bounds `FBox` (25 bytes), a
/// dword (0 on Boyle) and the sample array, followed by 28 bytes (the visibility and
/// distance-field data, empty on the Boyle levels).
pub fn read_light_volume(pkg: &Package) -> Option<Vec<VolumeSample>> {
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Level")?;
    let d = pkg.export_data(i).ok()?;
    let native = pkg.read_object(i).map(|h| h.native_offset).unwrap_or(0);
    let end = d.len().checked_sub(28)?;
    for o in (native + 33..end.saturating_sub(4)).rev() {
        let n = u32_at(d, o) as usize;
        if n == 0 || o + 4 + n * 33 != end || u32_at(d, o - 33) != 1 || d[o - 5] != 1 {
            continue;
        }
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let e = o + 4 + k * 33;
            let b = |at: usize| [d[e + at], d[e + at + 1], d[e + at + 2], d[e + at + 3]];
            out.push(VolumeSample {
                position: Vec3::new(f32_at(d, e), f32_at(d, e + 4), f32_at(d, e + 8)),
                radius: f32_at(d, e + 12),
                directions: b(16),
                indirect: b(20),
                environment: b(24),
                ambient: b(28),
                shadowed: d[e + 32] != 0,
            });
        }
        return Some(out);
    }
    None
}

/// `FColor::FromRGBE` (0x00403270, table 0x00FA3080 = `ldexp(1/255, e - 128)`): bytes (B, G, R, E).
pub fn rgbe(c: [u8; 4]) -> [f32; 3] {
    if c[3] == 0 {
        return [0.0; 3];
    }
    let s = (1.0f64 / 255.0 * 2f64.powi(c[3] as i32 - 128)) as f32;
    [c[2] as f32 * s, c[1] as f32 * s, c[0] as f32 * s]
}

/// A direction packed as (theta, phi) bytes: theta = b / 255 x pi, phi = b / 255 x 2 pi - pi
/// (tables 0x01278980 / 0x01279180 hold their sines and cosines).
pub fn unpack_direction(theta: u8, phi: u8) -> Vec3 {
    let t = theta as f32 / 255.0 * std::f32::consts::PI;
    let p = phi as f32 / 255.0 * 2.0 * std::f32::consts::PI - std::f32::consts::PI;
    Vec3::new(p.cos() * t.sin(), p.sin() * t.sin(), t.cos())
}

/// `SHBasisFunction` (0x00450860, normalisation constants 0x0126B100).
pub fn sh_basis(v: Vec3) -> [f32; 9] {
    let mut r = [0.282_095, 0.488_603, 0.488_603, 0.488_603, 0.182_091, 0.364_183, 0.630_783, 0.364_183, 0.182_091];
    let (x, y, z) = (v.x, v.y, v.z);
    let rr = x * x + y * y;
    if rr > 1e-4 {
        let k = 1.0 / rr.sqrt();
        let (c, s) = (x * k, y * k);
        r[1] *= s;
        r[3] *= c;
        r[5] *= s;
        r[7] *= c;
        r[4] *= 2.0 * c * s;
        r[8] *= 2.0 * c * c - 1.0;
    }
    let zz = z * z;
    let w = 1.0 - zz;
    let st = if w.abs() <= 1e-4 { 0.0 } else { w.sqrt() };
    r[1] = -(r[1] * st);
    r[2] *= z;
    r[3] = -(st * r[3]);
    r[4] *= (zz - 1.0) * -3.0;
    r[5] *= st * z * -3.0;
    r[6] *= zz * 3.0 * 0.5 - 0.5;
    r[7] *= st * z * -3.0;
    r[8] *= (zz - 1.0) * -3.0;
    r
}

/// `AmbientFunction` (0x01423520 = 1 / (2 sqrt(pi)) in band 0).
pub fn ambient_function() -> [f32; 9] {
    let mut f = [0.0; 9];
    f[0] = (1.0 / (2.0 * PI_F.sqrt())) as f32;
    f
}

/// `UpperSkyFunction` (0x014234C0) and `LowerSkyFunction` (0x014234F0).
pub fn sky_function(upper: bool) -> [f32; 9] {
    let mut f = [0.0; 9];
    f[0] = (1.0 / PI_F.sqrt()) as f32;
    f[2] = ((3.0 / PI_F).sqrt() * if upper { 0.5 } else { -0.5 }) as f32;
    f
}

fn add_scaled(sh: &mut Sh, f: &[f32; 9], c: [f32; 3]) {
    for ch in 0..3 {
        for k in 0..9 {
            sh[ch][k] += f[k] * c[ch];
        }
    }
}

/// The SH of one volume sample (0x00529710; `bIsCharacterLightEnvironment` off, so the world's
/// character brightness / contrast scales are 1).
pub fn sample_sh(s: &VolumeSample) -> Sh {
    let mut sh = [[0.0; 9]; 3];
    add_scaled(&mut sh, &sh_basis(unpack_direction(s.directions[0], s.directions[1])), rgbe(s.indirect));
    add_scaled(&mut sh, &sh_basis(unpack_direction(s.directions[2], s.directions[3])), rgbe(s.environment));
    add_scaled(&mut sh, &ambient_function(), rgbe(s.ambient));
    sh
}

/// Interpolate a light volume at `p` (0x0053DFA0): (weighted SH sum, total weight).
pub fn interpolate_volume(samples: &[VolumeSample], p: Vec3) -> (Sh, f32) {
    let mut sh = [[0.0; 9]; 3];
    let mut total = 0.0;
    for s in samples {
        let d2 = (s.position - p).length_squared();
        if d2 >= s.radius * s.radius {
            continue;
        }
        let inv = 1.0 / s.radius;
        let w = (1.0 - inv * d2.sqrt()) * inv;
        let x = sample_sh(s);
        for ch in 0..3 {
            for k in 0..9 {
                sh[ch][k] += x[ch][k] * w;
            }
        }
        total += w;
    }
    (sh, total)
}

/// Kind of a composited light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnvLightKind {
    Point,
    Spot { inner_deg: f32, outer_deg: f32 },
    Directional,
    Sky,
}

/// A static light as the light environment reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvLight {
    pub kind: EnvLightKind,
    pub position: Vec3,
    /// Spot / directional axis (direction the light shines).
    pub direction: Vec3,
    /// `Brightness x FLinearColor(LightColor)`.
    pub intensity: [f32; 3],
    pub radius: f32,
    pub falloff_exponent: f32,
    /// `LightingChannelContainer` bits (bit 0 `bInitialized`, 1 BSP, 2 Static, 3 Dynamic,
    /// 4 CompositeDynamic, ...).
    pub channels: u32,
    /// `CastShadows && CastStaticShadows`: the light's visibility is traced.
    pub traced: bool,
    pub source: String,
    /// `bOnlyAffectSameAndSpecifiedLevels`: the levels (package names) the light affects, its own
    /// first then `OtherLevelsToAffect` (0x00516400); None: every level.
    pub only_levels: Option<Vec<String>>,
}

/// `LightingChannelContainer` field names in bit order (`class_layout Engine LightingChannelContainer`).
const CHANNELS: [&str; 29] = [
    "bInitialized", "BSP", "Static", "Dynamic", "CompositeDynamic", "Skybox", "Unnamed_1", "Unnamed_2", "Unnamed_3", "Unnamed_4", "Unnamed_5", "Unnamed_6", "Cinematic_1", "Cinematic_2", "Cinematic_3", "Cinematic_4",
    "Cinematic_5", "Cinematic_6", "Cinematic_7", "Cinematic_8", "Cinematic_9", "Cinematic_10", "Gameplay_1", "Gameplay_2", "Gameplay_3", "Gameplay_4", "Crowd", "m_LightProbe", "m_bOutsider",
];

/// Channel bits of a `LightingChannels` struct property over `default` bits.
pub fn channel_bits(props: &[Property], default: u32) -> u32 {
    let mut bits = default;
    if let Some(Value::Struct(_, s)) = find(props, "LightingChannels").map(|p| &p.value) {
        for (i, n) in CHANNELS.iter().enumerate() {
            if let Some(Value::Bool(b)) = find(s, n).map(|p| &p.value) {
                if *b {
                    bits |= 1 << i;
                } else {
                    bits &= !(1 << i);
                }
            }
        }
    }
    bits
}

/// `Default__LightComponent`'s channels (bInitialized, BSP, Static, Dynamic, CompositeDynamic) as
/// changed by the actor class templates (`Default__PointLight.PointLightComponent0` and the spot
/// light's: Dynamic off, m_LightProbe on).
fn template_channels(actor_class: &str) -> u32 {
    let base = 0b1_1111;
    match actor_class {
        "PointLight" | "SpotLight" | "StaticLightCollectionActor" => (base & !(1 << 3)) | (1 << 27),
        // `Default__DirectionalLight`, `Default__SkyLight` and the dominant lights' templates set
        // `m_LightProbe` (Engine.upk); the toggleable and movable templates keep the class default.
        "DirectionalLight" | "SkyLight" | "DominantDirectionalLight" | "DominantPointLight" | "DominantSpotLight" => base | (1 << 27),
        _ => base,
    }
}

/// The `m_LightProbe` lighting channel bit.
pub const CHANNEL_LIGHT_PROBE: u32 = 1 << 27;

fn bool_or(props: &[Property], name: &str, d: bool) -> bool {
    match find(props, name).map(|p| &p.value) {
        Some(Value::Bool(b)) => *b,
        _ => d,
    }
}
fn float_or(props: &[Property], name: &str, d: f32) -> f32 {
    find(props, name).and_then(|p| p.as_float()).unwrap_or(d)
}

/// The static light components of a level package that a light environment composites: plain
/// `PointLight` / `SpotLight` / `DirectionalLight` / `SkyLight` actors and the components of
/// `StaticLightCollectionActor`s (dominant, toggleable and movable lights are drawn as lights).
pub fn read_static_lights(pkg: &Package) -> Vec<EnvLight> {
    read_lights(pkg, false)
}

/// The light components of a level package's actors. `all`: every light actor (dominant,
/// toggleable and movable lights too, as the world's static and dynamic light lists hold them);
/// otherwise the static ones ([`read_static_lights`]).
pub fn read_lights(pkg: &Package, all: bool) -> Vec<EnvLight> {
    let mut out = Vec::new();
    for ci in 0..pkg.exports.len() {
        let cclass = pkg.export_class(ci);
        let known = matches!(cclass.as_str(), "PointLightComponent" | "SpotLightComponent" | "DirectionalLightComponent" | "SkyLightComponent");
        let dominant = matches!(cclass.as_str(), "DominantPointLightComponent" | "DominantSpotLightComponent" | "DominantDirectionalLightComponent");
        if !(known || (all && dominant)) {
            continue;
        }
        let outer = ObjRef::from_index(pkg.exports[ci].outer);
        let ObjRef::Export(ai) = outer else { continue };
        if pkg.obj_name(ObjRef::from_index(pkg.exports[ai].outer)) != "PersistentLevel" {
            continue;
        }
        let aclass = pkg.export_class(ai);
        if !all && !matches!(aclass.as_str(), "PointLight" | "SpotLight" | "DirectionalLight" | "SkyLight" | "StaticLightCollectionActor") {
            continue;
        }
        let Ok(ch) = pkg.read_object(ci) else { continue };
        let c = &ch.properties;
        if !bool_or(c, "bEnabled", true) {
            continue;
        }
        let world = match find(c, "CachedParentToWorld").map(|p| &p.value) {
            Some(Value::Raw(_, b)) if b.len() == 64 => {
                let f: Vec<f32> = b.chunks_exact(4).map(|x| f32::from_le_bytes([x[0], x[1], x[2], x[3]])).collect();
                // Tagged Matrix rows are stored (W, X, Y, Z) (see level::read_light).
                let row = |i: usize| glam::Vec4::new(f[4 * i + 1], f[4 * i + 2], f[4 * i + 3], f[4 * i]);
                Mat4::from_cols(row(0), row(1), row(2), row(3))
            }
            _ => pkg.read_object(ai).map(|h| crate::level::actor_transform(&h.properties)).unwrap_or(Mat4::IDENTITY),
        } * crate::level::component_transform(c);
        let color = match find(c, "LightColor").map(|p| &p.value) {
            Some(Value::Color(rgba)) => [0, 1, 2].map(|k| crate::lightmap::color_to_linear(rgba[k])),
            _ => [1.0; 3],
        };
        let brightness = float_or(c, "Brightness", 1.0);
        let kind = match cclass.as_str() {
            "SpotLightComponent" | "DominantSpotLightComponent" => EnvLightKind::Spot { inner_deg: float_or(c, "InnerConeAngle", 0.0), outer_deg: float_or(c, "OuterConeAngle", 44.0) },
            "DirectionalLightComponent" | "DominantDirectionalLightComponent" => EnvLightKind::Directional,
            "SkyLightComponent" => EnvLightKind::Sky,
            _ => EnvLightKind::Point,
        };
        out.push(EnvLight {
            kind,
            position: world.w_axis.truncate(),
            direction: world.x_axis.truncate().normalize_or(Vec3::NEG_Z),
            intensity: color.map(|x| x * brightness),
            // Directional lights: `TraceDistance` (class default 100000), the length of their position.
            radius: if kind == EnvLightKind::Directional { float_or(c, "TraceDistance", 100_000.0) } else { float_or(c, "Radius", 1024.0) },
            falloff_exponent: float_or(c, "FalloffExponent", 2.0),
            channels: channel_bits(c, template_channels(&aclass)),
            // `Default__SkyLightComponent` clears CastShadows.
            traced: bool_or(c, "CastShadows", cclass != "SkyLightComponent") && bool_or(c, "CastStaticShadows", true),
            source: format!("{}:{}", pkg.name, pkg.obj_path(ObjRef::Export(ci))),
            only_levels: bool_or(c, "bOnlyAffectSameAndSpecifiedLevels", false).then(|| {
                let mut v = vec![pkg.name.clone()];
                // `array<name>`: count, then (name index, number) pairs.
                if let Some((n, raw)) = find(c, "OtherLevelsToAffect").and_then(|p| p.array_raw()) {
                    for k in 0..n {
                        let Some(b) = raw.get(k * 8..k * 8 + 4) else { break };
                        let idx = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
                        if let Some(name) = pkg.names.get(idx) {
                            v.push(name.name.clone());
                        }
                    }
                }
                v
            }),
        });
    }
    out
}

impl EnvLight {
    /// `ULightComponent::GetPosition`: (position, 1) for point and spot lights, (-axis, 0) for
    /// directional lights, (0, 0, 1, 0) for sky lights.
    fn position4(&self) -> (Vec3, f32) {
        match self.kind {
            EnvLightKind::Point | EnvLightKind::Spot { .. } => (self.position, 1.0),
            // `-axis x TraceDistance` (0x004E0440), carried in `radius`.
            EnvLightKind::Directional => (-self.direction * self.radius, 0.0),
            EnvLightKind::Sky => (Vec3::Z, 0.0),
        }
    }

    /// `AffectsBounds` for a bounds sphere (point: distance < radius + sphere radius; spot: the
    /// cone test of 0x005499E0; others always).
    pub fn affects(&self, origin: Vec3, sphere: f32) -> bool {
        match self.kind {
            EnvLightKind::Point => (origin - self.position).length() <= self.radius + sphere,
            EnvLightKind::Spot { inner_deg, outer_deg } => {
                if (origin - self.position).length() > self.radius + sphere {
                    return false;
                }
                let (_, outer) = cone_angles(inner_deg, outer_deg);
                let (sin_o, cos_o) = outer.sin_cos();
                // The cone's apex moved back so that it encloses the sphere.
                let apex = self.position - self.direction * (sphere / sin_o);
                let v = origin - apex;
                let a = self.direction.dot(v);
                if !(a > 0.0 && v.length_squared() * cos_o * cos_o <= a * a) {
                    return false;
                }
                let u = origin - self.position;
                let b = -self.direction.dot(u);
                b <= 0.0 || b * b < u.length_squared() * sin_o * sin_o || u.length_squared() <= sphere * sphere
            }
            EnvLightKind::Directional | EnvLightKind::Sky => true,
        }
    }

    /// `GetDirectIntensity` at `p`.
    pub fn intensity_at(&self, p: Vec3) -> [f32; 3] {
        let radial = |r: f32, e: f32| {
            let x = (1.0 - (p - self.position).length_squared() / (r * r)).max(0.0);
            x.powf(e)
        };
        let k = match self.kind {
            EnvLightKind::Point => radial(self.radius, self.falloff_exponent),
            EnvLightKind::Spot { inner_deg, outer_deg } => {
                let (inner, outer) = cone_angles(inner_deg, outer_deg);
                let (cos_o, cos_i) = (outer.cos(), inner.cos());
                let l = (p - self.position).normalize_or_zero();
                let s = ((self.direction.dot(l) - cos_o) / (cos_i - cos_o)).clamp(0.0, 1.0);
                s * s * radial(self.radius, self.falloff_exponent)
            }
            EnvLightKind::Directional | EnvLightKind::Sky => 1.0,
        };
        self.intensity.map(|c| c * k)
    }
}

/// Spot cone half-angles in radians as 0x00549EE0 clamps them: inner = clamp(inner, 0, 89) deg,
/// outer = clamp(outer, inner + 0.001 rad, 1.554343 rad).
fn cone_angles(inner_deg: f32, outer_deg: f32) -> (f32, f32) {
    let d2r = 0.017_453_292_f32;
    let inner = inner_deg.clamp(0.0, 89.0);
    let lo = inner * d2r + 0.001;
    // UE3's Clamp(X, Min, Max): Min below Min, else at most Max.
    let o = outer_deg * d2r;
    (inner * d2r, if o < lo { lo } else { o.min(1.554_343) })
}

/// The light environment's channel test (0x004DB950): the light's channels with
/// `CompositeDynamic` read as `Dynamic` (and `Dynamic` itself cleared) against the owner's,
/// `bInitialized` ignored.
pub fn channels_overlap(light: u32, owner: u32) -> bool {
    let mut ch = light & !(1 << 3);
    if light & (1 << 4) != 0 {
        ch = (ch & !(1 << 4)) | (1 << 3);
    }
    owner & ch & !1 != 0
}

/// The player's stealth light environment's channels: `Default__DishonoredPawn.StealthLightingDLE`
/// overrides the owner's (`bOverrideOwnerLightingChannels`) with `bInitialized`,
/// `CompositeDynamic` and `m_LightProbe`. `CompositeDynamic` never matches (a light's is read
/// as `Dynamic`), so only `m_LightProbe` lights contribute.
pub const STEALTH_DLE_CHANNELS: u32 = 1 | (1 << 4) | CHANNEL_LIGHT_PROBE;

/// Inputs of one light environment.
pub struct EnvQuery<'a> {
    pub origin: Vec3,
    pub sphere_radius: f32,
    /// Owner component's lighting channels.
    pub channels: u32,
    pub lights: &'a [EnvLight],
    pub volume: Option<&'a [VolumeSample]>,
    /// Print the contributions (`DISHONORED_LIGHT_ENV_DEBUG=<actor substring>`).
    pub debug: bool,
}

/// The static environment SH of a light environment; `visible(from, to)` is the line check.
pub fn static_environment(q: &EnvQuery, mut visible: impl FnMut(Vec3, Vec3) -> bool) -> Sh {
    let mut sh = [[0.0; 9]; 3];
    for l in q.lights {
        // CompositeDynamic stands for Dynamic in the light environment's channel test.
        if !channels_overlap(l.channels, q.channels) || !l.affects(q.origin, q.sphere_radius) {
            continue;
        }
        let (pos, w) = l.position4();
        let to_light = (pos - q.origin * w).normalize_or_zero();
        let vis = !l.traced || {
            let from = q.origin + to_light * q.sphere_radius;
            let to = if w > 0.0 { pos } else { from + to_light * 1.0e5 };
            visible(from, to)
        };
        if q.debug {
            eprintln!("  light {} vis {vis} dir {to_light:.2} intensity {:.3?}", l.source, l.intensity_at(q.origin));
        }
        if !vis {
            continue;
        }
        add_scaled(&mut sh, &sh_basis(to_light), l.intensity_at(q.origin));
    }
    if let Some(v) = q.volume {
        let (vsh, w) = interpolate_volume(v, q.origin);
        if q.debug {
            eprintln!("  volume weight {w:.5} band0 {:.4?}", [vsh[0][0] / w.max(1e-9), vsh[1][0] / w.max(1e-9), vsh[2][0] / w.max(1e-9)]);
        }
        if w > 1e-4 {
            for ch in 0..3 {
                for k in 0..9 {
                    sh[ch][k] += vsh[ch][k] / w;
                }
            }
        }
    }
    sh
}

/// Projection of each channel on `f` (clamped at 0, 0x004E1300), subtracted from `sh`.
fn extract(sh: &mut Sh, f: &[f32; 9]) -> [f32; 3] {
    let ff: f32 = f.iter().map(|x| x * x).sum();
    let mut c = [0.0; 3];
    for ch in 0..3 {
        c[ch] = (sh[ch].iter().zip(f).map(|(a, b)| a * b).sum::<f32>() / ff).max(0.0);
    }
    if c.iter().any(|x| *x > 0.0) {
        add_scaled(sh, f, c.map(|x| -x));
    }
    c
}

/// The synthesized directional and sky lights of an environment SH (0x004F85C0).
pub fn synthesize(mut sh: Sh) -> LightEnvironment {
    let lum: Vec<f32> = (0..9).map(|k| 0.3 * sh[0][k] + 0.59 * sh[1][k] + 0.11 * sh[2][k]).collect();
    let d = Vec3::new(-lum[3], -lum[1], lum[2]);
    let mut env = LightEnvironment::default();
    if d.length_squared() >= 1e-10 {
        let dir = d.normalize();
        let c = extract(&mut sh, &sh_basis(dir));
        if c.iter().any(|x| *x > 0.0) {
            env.direction = dir;
            env.color = c;
        }
    }
    env.upper_sky = extract(&mut sh, &sky_function(true));
    env.lower_sky = extract(&mut sh, &sky_function(false));
    env
}

/// World bounds of a placed skeletal mesh in its reference pose: (box centre, the farthest vertex
/// from it). The component's bounds come from its physics asset's bodies around the posed bones;
/// the reference-pose vertices cover the same parts (APPROXIMATION, U-620).
pub fn skeletal_bounds(content: &Content, mesh: &(String, String), transform: Mat4) -> Option<(Vec3, f32)> {
    let pkg = content.package(&mesh.0).ok()?;
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SkeletalMesh" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&mesh.1))?;
    let raw = crate::skeletal_mesh::read_skeletal_mesh(&pkg, i).ok()?;
    // Mesh -> component: R(RotOrigin) x (v + MeshOrigin), as characters::load_model.
    let r = crate::util::rotator_matrix(raw.rot_origin[0], raw.rot_origin[1], raw.rot_origin[2]);
    let to_world = transform * r * Mat4::from_translation(raw.mesh_origin);
    let pts: Vec<Vec3> = raw.positions.iter().map(|p| to_world.transform_point3(*p)).collect();
    let (lo, hi) = pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    if pts.is_empty() {
        return None;
    }
    let c = (lo + hi) * 0.5;
    Some((c, pts.iter().map(|p| p.distance(c)).fold(0.0, f32::max)))
}

/// Compute the light environment of every skeletal prop of `map`. `instance_level[i]` is the
/// level (index into `volumes`) of instance `i`; `volumes[level]` its light volume.
pub fn light_props(content: &Content, map: &mut DishonoredMap, instance_level: &[usize], volumes: &[Option<Vec<VolumeSample>>], lights: &[EnvLight], prop_channels: &[u32]) {
    use dishonored_world::WorldQuery;
    if map.skeletal_props.is_empty() {
        return;
    }
    let world = dishonored_world::CollisionWorld::from_map(map);
    // Collision source k is the k-th colliding instance (CollisionWorld::from_map).
    let colliding: Vec<usize> = map.instances.iter().enumerate().filter(|(_, i)| i.collides).map(|(k, _)| k).collect();
    let persistent = volumes.first().and_then(|v| v.as_deref());
    for (pi, prop) in map.skeletal_props.iter_mut().enumerate() {
        let Some((origin, radius)) = skeletal_bounds(content, &prop.mesh, prop.transform) else { continue };
        // The floor's level: a line check straight down from the bounds origin.
        let level = world
            .raycast(origin, Vec3::NEG_Z, 262_144.0, dishonored_world::channel::ALL)
            .and_then(|h| world.tris.get(h.tri as usize))
            .and_then(|t| colliding.get(t.source as usize))
            .and_then(|&inst| instance_level.get(inst).copied());
        let volume = level.and_then(|l| volumes.get(l)).and_then(|v| v.as_deref()).or(persistent);
        let debug = std::env::var("DISHONORED_LIGHT_ENV_DEBUG").is_ok_and(|d| !d.is_empty() && prop.actor.contains(&d));
        if debug {
            eprintln!("light environment of {}: origin {origin:.0} radius {radius:.0} floor level {level:?}", prop.actor);
        }
        let q = EnvQuery { origin, sphere_radius: radius, channels: prop_channels.get(pi).copied().unwrap_or(1 | 1 << 3), lights, volume, debug };
        let sh = static_environment(&q, |a, b| {
            let d = b - a;
            let len = d.length();
            len < 1e-3 || world.raycast(a, d / len, len - 1.0, dishonored_world::channel::ALL).is_none()
        });
        prop.light_env = Some(synthesize(sh));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basis_matches_the_real_sh_with_the_condon_shortley_phase() {
        let d = Vec3::new(0.3, -0.5, 0.81).normalize();
        let y = sh_basis(d);
        let (x, yy, z) = (d.x, d.y, d.z);
        let want = [0.282_095, -0.488_603 * yy, 0.488_603 * z, -0.488_603 * x, 1.092_548 * x * yy, -1.092_548 * yy * z, 0.315_392 * (3.0 * z * z - 1.0), -1.092_548 * x * z, 0.546_274 * (x * x - yy * yy)];
        for k in 0..9 {
            assert!((y[k] - want[k]).abs() < 1e-4, "{k}: {} vs {}", y[k], want[k]);
        }
        // Sum of squares over bands 0-2 is 9 / (4 pi) for any direction.
        let s: f32 = y.iter().map(|v| v * v).sum();
        assert!((s - 9.0 / (4.0 * std::f32::consts::PI)).abs() < 1e-4);
    }

    #[test]
    fn rgbe_and_directions() {
        assert_eq!(rgbe([0, 0, 255, 128]), [1.0, 0.0, 0.0]);
        assert_eq!(rgbe([10, 20, 30, 0]), [0.0; 3]);
        let up = unpack_direction(0, 0);
        assert!((up - Vec3::Z).length() < 1e-6);
        let d = unpack_direction(128, 191);
        assert!((d.length() - 1.0).abs() < 1e-5);
    }

    /// One directional light: all of it comes back as the synthesized directional light.
    #[test]
    fn a_single_light_is_synthesized_back() {
        let dir = Vec3::new(0.2, 0.4, 0.89).normalize();
        let mut sh = [[0.0; 9]; 3];
        add_scaled(&mut sh, &sh_basis(dir), [2.0, 1.0, 0.5]);
        let e = synthesize(sh);
        assert!((e.direction - dir).length() < 1e-4);
        for (c, w) in e.color.iter().zip([2.0, 1.0, 0.5]) {
            assert!((c - w).abs() < 1e-4, "{c} vs {w}");
        }
        assert!(e.upper_sky.iter().chain(&e.lower_sky).all(|v| v.abs() < 1e-4));
    }

    /// A constant (ambient) environment has no linear band: it all goes to the sky lights.
    #[test]
    fn ambient_goes_to_the_sky_lights() {
        let mut sh = [[0.0; 9]; 3];
        add_scaled(&mut sh, &ambient_function(), [1.0, 1.0, 1.0]);
        let e = synthesize(sh);
        assert_eq!(e.color, [0.0; 3]);
        assert!(e.upper_sky[0] > 0.0 && e.lower_sky[0] > 0.0);
    }

    #[test]
    fn channels_read_over_the_template() {
        // A static light keeps BSP / Static / CompositeDynamic from Default__LightComponent and
        // gets Dynamic off from the PointLight template.
        let c = template_channels("PointLight");
        assert_eq!(c & 0b1_1111, 0b1_0111);
        assert_eq!(channel_bits(&[], c), c);
    }
}
