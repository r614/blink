//! Our layout maps (`maps/*.ron`, `dishonored_ir::layout::MapLayout`, research/dishonored/
//! PLAYGROUND.md) built inside a running original level, so the Playground scenarios can be
//! traced on both sides. The layout is placed at `offset` (default (0, 0, 20000): above the
//! host level, under its StallZ and far over its KillZ -20000).
//!
//! - Box and stairs brushes with collision: one `InterpActor` per box with the
//!   level blockout cube `Env_Blockout.staticmeshes.Cube_100` (a 100 uu cube, corner pivot,
//!   simple box collision used for line and extent checks), scaled to the box (`DrawScale3D`)
//!   and yawed, `COLLIDE_BlockAll`. The engine's own checks then hit exact boxes.
//! - Ramps: a slab of the same cube pitched to the ramp angle whose top face is the slope (the
//!   wedge's vertical back face and its inside are not modelled: walked surfaces only).
//! - Meshes: an `InterpActor` with the original static mesh (its own collision).
//! - Actors: `Spawn(class, ActorTemplate = the original placed actor)` copies doors, usables,
//!   pickups and the chain with their properties (`bNoDelete` classes are spawned with the
//!   class default's flag cleared for the call, then restored).
//! - Water / pain boxes: an existing volume of the host level of that kind is moved and scaled
//!   to the box (its brush and convex hull scale with `DrawScale3D`).
//! - Lights, labels and the nav grid are not built (U-806: the stealth lane's light levels
//!   differ).
use crate::reflect::{hex, parse_addr};
use crate::session::Session;
use anyhow::{Context, Result, bail};
use dishonored_ir::layout::{MapLayout, Shape};
use glam::{Mat3, Vec3};
use serde_json::{Value, json};

pub const CUBE: &str = "Cube_100";
pub const DEFAULT_OFFSET: [f32; 3] = [0.0, 0.0, 20000.0];

pub fn parse_layout(text: &str) -> Result<MapLayout> {
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME | ron::extensions::Extensions::UNWRAP_NEWTYPES)
        .from_str(text)
        .map_err(|e| anyhow::anyhow!("layout parse error: {e}"))
}

/// A box to build: world centre of the local origin frame, rotation, local min corner, size.
#[derive(Debug, Clone, Copy)]
pub struct WorldBox {
    /// World position of the box's local min corner (the cube's pivot).
    pub pivot: Vec3,
    pub size: Vec3,
    /// Unreal rotator in degrees (pitch, yaw, roll).
    pub rot: [f32; 3],
}

/// Unreal rotation matrix (yaw about Z, then pitch: positive raises +X toward +Z).
fn rot_matrix(pitch: f32, yaw: f32) -> Mat3 {
    Mat3::from_rotation_z(yaw.to_radians()) * Mat3::from_rotation_y(-pitch.to_radians())
}

/// The collision boxes of a layout, in world space with `offset` added.
pub fn layout_boxes(l: &MapLayout, offset: Vec3) -> Vec<WorldBox> {
    let mut out = Vec::new();
    for b in &l.brushes {
        if !b.collide {
            continue;
        }
        let at = Vec3::new(b.at.0, b.at.1, b.at.2) + offset;
        match &b.shape {
            Shape::Ramp { width, length, angle } => {
                // The slope from (0, 0, 0) to (length, 0, h) along +X: a slab below it.
                let t = 40.0;
                let slope = length / angle.to_radians().cos();
                let r = rot_matrix(*angle, b.yaw);
                out.push(WorldBox { pivot: at + r * Vec3::new(0.0, -width / 2.0, -t), size: Vec3::new(slope, *width, t), rot: [*angle, b.yaw, 0.0] });
            }
            _ => {
                let r = rot_matrix(0.0, b.yaw);
                for (mn, mx) in b.local_boxes() {
                    out.push(WorldBox { pivot: at + r * mn, size: mx - mn, rot: [0.0, b.yaw, 0.0] });
                }
            }
        }
    }
    out
}

fn units(deg: f32) -> i32 {
    (deg / 360.0 * 65536.0).round() as i32
}

impl Session {
    /// Object address from a `"... @0xADDR"` value.
    fn addr_of(v: &Value) -> Option<u32> {
        v.as_str().and_then(|s| s.rsplit_once('@')).and_then(|x| parse_addr(x.1))
    }

    /// Spawn a class whose default may be `bNoDelete` (the flag is cleared for the call).
    fn spawn_any(&mut self, class: &str, loc: Vec3, rot: [f32; 3], template: Option<u32>) -> Result<u32> {
        let cdo = format!("Default__{class}");
        let cdo_addr = self.r.object(&mut self.c, &cdo, None)?;
        let pl = self.r.place(&mut self.c, cdo_addr, "bNoDelete")?;
        let was = self.r.read_place(&mut self.c, &pl)?;
        if was == json!(true) {
            self.r.write_place(&mut self.c, &pl, &json!(false))?;
        }
        let cls = self.r.class_named(&mut self.c, class)?;
        let pc = crate::lab::pc(&mut self.c)?;
        let mut args = serde_json::Map::new();
        args.insert("SpawnClass".into(), json!(format!("@{}", hex(cls))));
        args.insert("SpawnLocation".into(), json!([loc.x, loc.y, loc.z]));
        args.insert("SpawnRotation".into(), json!([units(rot[0]), units(rot[1]), units(rot[2])]));
        if let Some(t) = template {
            args.insert("ActorTemplate".into(), json!(format!("@{}", hex(t))));
        }
        args.insert("bNoCollisionFail".into(), json!(true));
        let r = self.r.call(&mut self.c, pc, "Actor.Spawn", &Value::Object(args));
        if was == json!(true) {
            self.r.write_place(&mut self.c, &pl, &json!(true))?;
        }
        let r = r?;
        Self::addr_of(r.get("return").unwrap_or(&Value::Null)).with_context(|| format!("Spawn {class} returned None"))
    }

    /// A static mesh actor with `mesh`, scale and rotation, blocking everything.
    fn spawn_mesh(&mut self, mesh: u32, loc: Vec3, rot: [f32; 3], scale: Vec3) -> Result<u32> {
        // An InterpActor (PHYS_Interpolating, a mover that stays put): a DynamicSMActor_Spawnable
        // turns PHYS_RigidBody and falls when a pawn steps off it.
        let a = self.spawn_any("InterpActor", loc, rot, None)?;
        let comp = self.r.place(&mut self.c, a, "StaticMeshComponent")?;
        let comp = Self::addr_of(&self.r.read_place(&mut self.c, &comp)?).context("no StaticMeshComponent")?;
        self.r.call(&mut self.c, comp, "SetStaticMesh", &json!({"NewMesh": hex(mesh)}))?;
        self.r.call(&mut self.c, a, "SetDrawScale3D", &json!([[scale.x, scale.y, scale.z]]))?;
        // Spawn may have nudged the actor out of geometry: put it exactly.
        self.r.call(&mut self.c, a, "SetLocation", &json!([[loc.x, loc.y, loc.z]]))?;
        self.r.call(&mut self.c, a, "SetRotation", &json!([[units(rot[0]), units(rot[1]), units(rot[2])]]))?;
        self.r.call(&mut self.c, a, "SetCollision", &json!([true, true]))?;
        self.r.call(&mut self.c, a, "SetCollisionType", &json!(["COLLIDE_BlockAll"]))?;
        Ok(a)
    }

    fn static_mesh(&mut self, path: &str) -> Result<u32> {
        let name = path.rsplit('.').next().unwrap_or(path);
        let found = self.c.find(name, "StaticMesh")?;
        let names = self.c.names_of(&found)?;
        if let Some(i) = names.iter().position(|n| n.ends_with(path) || n.ends_with(&format!(".{path}"))).or(if found.is_empty() { None } else { Some(0) }) {
            return Ok(found[i]);
        }
        let meta = self.r.class_named(&mut self.c, "StaticMesh")?;
        let pc = crate::lab::pc(&mut self.c)?;
        let r = self.r.call(&mut self.c, pc, "Object.DynamicLoadObject", &json!({"ObjectName": path, "ObjectClass": hex(meta)}))?;
        Self::addr_of(r.get("return").unwrap_or(&Value::Null)).with_context(|| format!("static mesh {path} not loaded and not loadable"))
    }

    /// Move and scale an existing volume of `class` (and not yet used) to the box.
    fn repurpose_volume(&mut self, class: &str, used: &mut Vec<u32>, min: Vec3, max: Vec3) -> Result<u32> {
        let cands = self.c.find("", class)?;
        let names = self.c.names_of(&cands)?;
        let v = cands.iter().zip(&names).find(|(a, n)| !used.contains(a) && n.contains("PersistentLevel") && !n.contains("Default__")).map(|x| *x.0).with_context(|| format!("no {class} left in the host level to reuse"))?;
        used.push(v);
        let comp = self.r.place(&mut self.c, v, "BrushComponent")?;
        let comp = Self::addr_of(&self.r.read_place(&mut self.c, &comp)?).context("volume without BrushComponent")?;
        let geom = self.r.place(&mut self.c, comp, "BrushAggGeom.ConvexElems[0].ElemBox")?;
        let eb = self.r.read_place(&mut self.c, &geom)?;
        let g = |k: &str, a: &str| eb[k][a].as_f64().unwrap_or(0.0) as f32;
        let (emin, emax) = (Vec3::new(g("Min", "X"), g("Min", "Y"), g("Min", "Z")), Vec3::new(g("Max", "X"), g("Max", "Y"), g("Max", "Z")));
        let s = (max - min) / (emax - emin).max(Vec3::splat(1e-3));
        let loc = min - s * emin;
        self.r.call(&mut self.c, v, "SetRotation", &json!([[0, 0, 0]]))?;
        self.r.call(&mut self.c, v, "SetDrawScale3D", &json!([[s.x, s.y, s.z]]))?;
        let pl = self.r.place(&mut self.c, v, "Location")?;
        self.r.write_place(&mut self.c, &pl, &json!([loc.x, loc.y, loc.z]))?;
        self.r.call(&mut self.c, v, "ForceUpdateComponents", &json!([true, false]))?;
        Ok(v)
    }

    /// Build a layout in the current level. Returns what was built and what failed.
    pub fn build_layout(&mut self, l: &MapLayout, offset: Vec3) -> Result<Value> {
        let t0 = std::time::Instant::now();
        let _ = self.c.lockstep(true, 30_000);
        let cube = self.static_mesh(CUBE)?;
        let mut boxes = 0;
        let mut errors = Vec::new();
        for b in layout_boxes(l, offset) {
            let s = b.size / 100.0;
            match self.spawn_mesh(cube, b.pivot, b.rot, s) {
                Ok(_) => boxes += 1,
                Err(e) => errors.push(format!("box at {:?}: {e:#}", b.pivot)),
            }
        }
        let mut meshes = 0;
        for m in &l.meshes {
            if !m.collide {
                continue;
            }
            let at = Vec3::new(m.at.0, m.at.1, m.at.2) + offset;
            let r = (|| -> Result<()> {
                let mesh = self.static_mesh(&m.mesh)?;
                self.spawn_mesh(mesh, at, [m.rotation.0, m.rotation.1, m.rotation.2], Vec3::new(m.scale.0, m.scale.1, m.scale.2))?;
                Ok(())
            })();
            match r {
                Ok(()) => meshes += 1,
                Err(e) => errors.push(format!("mesh {}: {e:#}", m.mesh)),
            }
        }
        let mut actors = Vec::new();
        for a in &l.actors {
            let at = Vec3::new(a.at.0, a.at.1, a.at.2) + offset;
            let r = (|| -> Result<Value> {
                let (pkg, name) = a.from.split_once(':').context("actor `from` is <package>:<name>")?;
                let found = self.c.find(name, "")?;
                let names = self.c.names_of(&found)?;
                let (src, class, note) = match names.iter().position(|n| n.contains(&format!(" {pkg}.")) && n.ends_with(&format!(".{name}"))) {
                    Some(i) => (found[i], names[i].split(' ').next().unwrap_or("").to_string(), None),
                    None => {
                        // Not in the host level: the first placed actor of the same class.
                        let class = name.rsplit_once('_').map(|x| x.0).unwrap_or(name).to_string();
                        let alt = self.c.find("", &class)?;
                        let alt_names = self.c.names_of(&alt)?;
                        let k = alt_names.iter().position(|n| n.contains("PersistentLevel") && !n.contains("Default__")).with_context(|| format!("{} is not loaded in this level and no {class} is", a.from))?;
                        (alt[k], class, Some(format!("stand-in {}", alt_names[k])))
                    }
                };
                let rot = self.r.place(&mut self.c, src, "Rotation")?;
                let rot = self.r.read_place(&mut self.c, &rot)?;
                let p = rot["Pitch"].as_f64().unwrap_or(0.0) as f32 * 360.0 / 65536.0;
                let ro = rot["Roll"].as_f64().unwrap_or(0.0) as f32 * 360.0 / 65536.0;
                let new = self.spawn_any(&class, at, [p, a.yaw, ro], Some(src))?;
                Ok(json!({"from": a.from, "actor": crate::reflect::object_value(&mut self.c, new)?, "note": note}))
            })();
            match r {
                Ok(v) => actors.push(v),
                Err(e) => errors.push(format!("actor {}: {e:#}", a.from)),
            }
        }
        let mut used = Vec::new();
        let mut volumes = Vec::new();
        for w in &l.water {
            let (mn, mx) = (Vec3::new(w.min.0, w.min.1, w.min.2) + offset, Vec3::new(w.max.0, w.max.1, w.max.2) + offset);
            match self.repurpose_volume("DishonoredWaterVolume", &mut used, mn, mx).or_else(|_| self.repurpose_volume("WaterVolume", &mut used, mn, mx)) {
                Ok(v) => volumes.push(json!({"water": w.name, "volume": crate::reflect::object_value(&mut self.c, v)?})),
                Err(e) => errors.push(format!("water {}: {e:#}", w.name)),
            }
        }
        for p in &l.pain {
            let (mn, mx) = (Vec3::new(p.min.0, p.min.1, p.min.2) + offset, Vec3::new(p.max.0, p.max.1, p.max.2) + offset);
            match self.repurpose_volume("PhysicsVolume", &mut used, mn, mx) {
                Ok(v) => {
                    for (k, val) in [("bPainCausing", json!(true)), ("DamagePerSec", json!(p.damage_per_sec)), ("PainInterval", json!(if p.interval > 0.0 { p.interval } else { 1.0 }))] {
                        if let Ok(pl) = self.r.place(&mut self.c, v, k) {
                            let _ = self.r.write_place(&mut self.c, &pl, &val);
                        }
                    }
                    volumes.push(json!({"pain": p.name, "volume": crate::reflect::object_value(&mut self.c, v)?}));
                }
                Err(e) => errors.push(format!("pain {}: {e:#}", p.name)),
            }
        }
        Ok(json!({
            "layout": l.name, "offset": [offset.x, offset.y, offset.z], "boxes": boxes, "meshes": meshes, "actors": actors, "volumes": volumes,
            "not_built": ["lights", "labels", "nav"], "errors": errors, "seconds": t0.elapsed().as_secs_f64(),
        }))
    }
}

/// The layout file of a map name (`Playground` -> maps/playground.ron), if any.
pub fn layout_file(map: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new("maps").join(format!("{}.ron", map.to_ascii_lowercase()));
    p.exists().then_some(p)
}

pub fn host_map() -> &'static str {
    "L_Boyle_Ext_P"
}

pub fn check_offset(v: &Value) -> Result<Vec3> {
    match v.as_array() {
        None => Ok(Vec3::from(DEFAULT_OFFSET)),
        Some(a) if a.len() == 3 => Ok(Vec3::new(a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32)),
        _ => bail!("offset [x, y, z]"),
    }
}
