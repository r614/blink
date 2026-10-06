//! Water volumes of a map: every `DishonoredWaterVolume` (a `PhysicsVolume` with
//! `bWaterVolume`, class default `FluidFriction` 2.4) of the level packages, with its brush's
//! convex hulls in world space. The pawn swims while its centre is inside one (UE3
//! `APawn::physSwimming`, `StatePlayerMasterSwim`), see research/dishonored/PLAYER.md
//! "Swimming".

use crate::content::Content;
use anyhow::Result;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

/// Class defaults of `DishonoredWaterVolume` (DishonoredGame.upk) and `PhysicsVolume`.
pub const DEFAULT_FLUID_FRICTION: f32 = 2.4;
pub const DEFAULT_TERMINAL_VELOCITY: f32 = 4000.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WaterVolumeDef {
    pub name: String,
    pub package: String,
    /// Planes `(nx, ny, nz, d)`, inside when `n . p <= d` for every plane of one hull.
    pub hulls: Vec<Vec<[f32; 4]>>,
    pub min: Vec3,
    pub max: Vec3,
    pub fluid_friction: f32,
    pub terminal_velocity: f32,
    pub shallow: bool,
}

/// Bounds of a convex hull from its planes (corners where three planes meet inside the hull).
pub fn hull_bounds(planes: &[[f32; 4]]) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let n = planes.len();
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let (a, b, c) = (planes[i], planes[j], planes[k]);
                let (na, nb, nc) = (Vec3::new(a[0], a[1], a[2]), Vec3::new(b[0], b[1], b[2]), Vec3::new(c[0], c[1], c[2]));
                let det = na.dot(nb.cross(nc));
                if det.abs() < 1e-6 {
                    continue;
                }
                let p = (nb.cross(nc) * a[3] + nc.cross(na) * b[3] + na.cross(nb) * c[3]) / det;
                if planes.iter().all(|q| p.x * q[0] + p.y * q[1] + p.z * q[2] <= q[3] + 0.5) {
                    min = min.min(p);
                    max = max.max(p);
                }
            }
        }
    }
    (min.x <= max.x).then_some((min, max))
}

/// Read the water volumes of `packages` (persistent level and sublevels).
pub fn extract_water(content: &Content, packages: &[String]) -> Result<Vec<WaterVolumeDef>> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for name in packages.iter().filter(|p| content.has_package(p)) {
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "DishonoredWaterVolume" || pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            let Ok(h) = pkg.read_object(i) else { continue };
            let p = &h.properties;
            let Some(hulls) = crate::ai_level::brush_world_hulls(&pkg, p) else { continue };
            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            for hull in &hulls {
                if let Some((a, b)) = hull_bounds(hull) {
                    min = min.min(a);
                    max = max.max(b);
                }
            }
            if hulls.is_empty() || min.x > max.x {
                continue;
            }
            let float = |k: &str, d: f32| find(p, k).and_then(|x| x.as_float()).unwrap_or(d);
            out.push(WaterVolumeDef {
                name: pkg.obj_name(ObjRef::Export(i)).to_string(),
                package: name.clone(),
                hulls,
                min,
                max,
                fluid_friction: float("FluidFriction", DEFAULT_FLUID_FRICTION),
                terminal_velocity: float("TerminalVelocity", DEFAULT_TERMINAL_VELOCITY),
                shallow: matches!(find(p, "m_bShallowWater").map(|x| &x.value), Some(Value::Bool(true))),
            });
        }
    }
    Ok(out)
}


/// A non-water `PhysicsVolume` of the level (Engine `PhysicsVolume` defaults:
/// `bVelocityAffectsWalking` and `bEntryPain` true, `PainInterval` 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PhysicsVolumeDef {
    pub name: String,
    pub package: String,
    pub hulls: Vec<Vec<[f32; 4]>>,
    pub min: Vec3,
    pub max: Vec3,
    pub priority: i32,
    pub zone_velocity: Vec3,
    pub velocity_affects_walking: bool,
    pub pain_causing: bool,
    pub damage_per_sec: f32,
    pub pain_interval: f32,
    pub entry_pain: bool,
    /// `DamageType` class name (`DisDamageType_SpringRazor`), empty for the default.
    pub damage_type: String,
}

/// Read the `PhysicsVolume`s of `packages` that change movement or cause pain.
pub fn extract_physics_volumes(content: &Content, packages: &[String]) -> Result<Vec<PhysicsVolumeDef>> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for name in packages.iter().filter(|p| content.has_package(p)) {
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "PhysicsVolume" || pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            let Ok(h) = pkg.read_object(i) else { continue };
            let p = &h.properties;
            let float = |k: &str, d: f32| find(p, k).and_then(|x| x.as_float()).unwrap_or(d);
            let flag = |k: &str, d: bool| match find(p, k).map(|x| &x.value) {
                Some(Value::Bool(b)) => *b,
                _ => d,
            };
            let zone = find(p, "ZoneVelocity").and_then(|x| x.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
            let pain = flag("bPainCausing", false);
            if zone == Vec3::ZERO && !pain {
                continue;
            }
            let Some(hulls) = crate::ai_level::brush_world_hulls(&pkg, p) else { continue };
            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            for hull in &hulls {
                if let Some((a, b)) = hull_bounds(hull) {
                    min = min.min(a);
                    max = max.max(b);
                }
            }
            if hulls.is_empty() || min.x > max.x {
                continue;
            }
            let damage_type = find(p, "DamageType").and_then(|x| x.as_object()).map(|o| pkg.obj_name(ObjRef::from_index(o)).to_string()).unwrap_or_default();
            out.push(PhysicsVolumeDef {
                name: pkg.obj_name(ObjRef::Export(i)).to_string(),
                package: name.clone(),
                hulls,
                min,
                max,
                priority: match find(p, "Priority").map(|x| &x.value) {
                    Some(Value::Int(n)) => *n,
                    _ => 0,
                },
                zone_velocity: zone,
                velocity_affects_walking: flag("bVelocityAffectsWalking", true),
                pain_causing: pain,
                damage_per_sec: float("DamagePerSec", 0.0),
                pain_interval: float("PainInterval", 1.0),
                entry_pain: flag("bEntryPain", true),
                damage_type,
            });
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_hull_bounds() {
        let planes = [[0., 0., 1., 100.], [0., 0., -1., 0.], [1., 0., 0., 50.], [-1., 0., 0., 50.], [0., 1., 0., 20.], [0., -1., 0., 20.]];
        let (a, b) = hull_bounds(&planes).unwrap();
        assert_eq!(a, Vec3::new(-50., -20., 0.));
        assert_eq!(b, Vec3::new(50., 20., 100.));
    }
}
