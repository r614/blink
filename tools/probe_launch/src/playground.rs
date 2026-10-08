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
        // PhysX: the body is made when the component (re)attaches with BlockRigidBody, so set it
        // (and the channels ragdolls and props collide with) before the mesh attaches.
        let _ = self.r.call(&mut self.c, comp, "SetBlockRigidBody", &json!([true]));
        for (k, v) in [("RBChannel", json!("RBCC_Default")), ("RBCollideWithChannels", json!({"Default": true, "Pawn": true, "DeadPawn": true, "GameplayPhysics": true, "EffectPhysics": true, "FracturedMeshPart": true, "SeveredLimbs": true}))] {
            if let Ok(pl) = self.r.place(&mut self.c, comp, k) {
                let _ = self.r.write_place(&mut self.c, &pl, &v);
            }
        }
        // The PhysX body is built once, when the mesh attaches, from the transform at that
        // moment (a later scale does not rebuild it): place, rotate and scale the empty actor
        // first, then set the mesh.
        self.r.call(&mut self.c, a, "SetDrawScale3D", &json!([[scale.x, scale.y, scale.z]]))?;
        // Spawn may have nudged the actor out of geometry: put it exactly.
        self.r.call(&mut self.c, a, "SetLocation", &json!([[loc.x, loc.y, loc.z]]))?;
        self.r.call(&mut self.c, a, "SetRotation", &json!([[units(rot[0]), units(rot[1]), units(rot[2])]]))?;
        self.r.call(&mut self.c, a, "SetCollision", &json!([true, true]))?;
        self.r.call(&mut self.c, a, "SetCollisionType", &json!(["COLLIDE_BlockAll"]))?;
        self.r.call(&mut self.c, comp, "SetStaticMesh", &json!({"NewMesh": hex(mesh)}))?;
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
        // Lights: movable point / spot light actors with the layout's colour (linear, written
        // as the sRGB bytes UE3's FColor holds), brightness, radius, falloff and shadows.
        // `from` lights are the host level's own (Boyle's moon): kept as they are.
        let mut lights = 0;
        for lt in &l.lights {
            if lt.from.is_some() {
                continue;
            }
            let at = Vec3::new(lt.at.0, lt.at.1, lt.at.2) + offset;
            let r = (|| -> Result<()> {
                let (class, rot) = match lt.kind {
                    dishonored_ir::layout::LightType::Point => ("PointLightMovable", [0.0; 3]),
                    dishonored_ir::layout::LightType::Spot => {
                        let d = Vec3::new(lt.direction.0, lt.direction.1, lt.direction.2).normalize_or_zero();
                        ("SpotLightMovable", [d.z.asin().to_degrees(), d.y.atan2(d.x).to_degrees(), 0.0])
                    }
                    dishonored_ir::layout::LightType::Directional => anyhow::bail!("directional layout lights are not built"),
                };
                let a = self.spawn_any(class, at, rot, None)?;
                let lc = self.r.place(&mut self.c, a, "LightComponent")?;
                let lc = Self::addr_of(&self.r.read_place(&mut self.c, &lc)?).context("no LightComponent")?;
                let srgb = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
                let mut set = |k: &str, v: Value| -> Result<()> {
                    let pl = self.r.place(&mut self.c, lc, k)?;
                    self.r.write_place(&mut self.c, &pl, &v)
                };
                set("Brightness", json!(lt.brightness))?;
                set("LightColor", json!({"R": srgb(lt.color.0), "G": srgb(lt.color.1), "B": srgb(lt.color.2), "A": 255}))?;
                set("Radius", json!(lt.radius))?;
                set("FalloffExponent", json!(lt.falloff_exponent))?;
                set("CastShadows", json!(lt.shadows))?;
                if let Some((inner, outer)) = lt.cone {
                    let _ = set("InnerConeAngle", json!(inner));
                    let _ = set("OuterConeAngle", json!(outer));
                }
                self.r.call(&mut self.c, a, "ReattachComponent", &json!([hex(lc)]))?;
                Ok(())
            })();
            match r {
                Ok(()) => lights += 1,
                Err(e) => errors.push(format!("light at {:?}: {e:#}", lt.at)),
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
            "layout": l.name, "offset": [offset.x, offset.y, offset.z], "boxes": boxes, "meshes": meshes, "lights": lights, "actors": actors, "volumes": volumes,
            "not_built": ["labels", "nav"], "errors": errors, "seconds": t0.elapsed().as_secs_f64(),
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

impl Session {
    /// A fresh NPC from a spawner (U-807): a `DishonoredSpawner` copied from a host spawner of
    /// the kind (`Spawn` with it as ActorTemplate: its factory and settings), put at `at` /
    /// `yaw`, given the pawn tweak and faction override when asked, then started through its
    /// Kismet handler `OnStartSpawn` (`DisSeqAct_StartSpawn`'s default with
    /// `m_bSpawnEvenIfVisible`). Waits up to 120 ticks for the pawn (`m_SpawnedPawns`).
    pub fn spawn_npc(&mut self, tweak: Option<&str>, faction: Option<&str>, guard: bool, at: Vec3, yaw: f32) -> Result<(u32, bool)> {
        self.spawn_npc_from(None, tweak, faction, guard, at, yaw)
    }

    pub fn spawn_npc_from(&mut self, from: Option<u32>, tweak: Option<&str>, faction: Option<&str>, guard: bool, at: Vec3, yaw: f32) -> Result<(u32, bool)> {
        self.spawn_npc_opts(from, tweak, faction, guard, at, yaw, false)
    }

    /// `patrol`: the spawner's `m_bPatrolUponStartup` (the pawn starts in its patrol behaviour
    /// and adopts the nearest route).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_npc_opts(&mut self, from: Option<u32>, tweak: Option<&str>, faction: Option<&str>, guard: bool, at: Vec3, yaw: f32, patrol: bool) -> Result<(u32, bool)> {
        let spawners = if from.is_some() { Vec::new() } else { self.c.find("", "DishonoredSpawner")? };
        let mut template = from;
        let mut tweak_obj = None;
        if let Some(tw) = tweak {
            let short = tw.rsplit('.').next().unwrap_or(tw);
            tweak_obj = self.c.find(short, "")?.into_iter().find(|_| true);
        }
        for sp in &spawners {
            let n = self.c.name_of(*sp)?;
            if n.contains("Default__") {
                continue;
            }
            let pl = self.r.place(&mut self.c, *sp, "m_pPawnTweaks")?;
            let tw = self.c.read_u32(pl.addr)?;
            if tw == 0 {
                continue;
            }
            let twn = self.c.name_of(tw)?;
            if tweak_obj.is_some_and(|t| t == tw) {
                template = Some(*sp);
                break;
            }
            let is_guard = twn.contains("Guard") && !twn.contains("Tallboy");
            let civ = !twn.contains("Guard") && !twn.contains("Tallboy") && !twn.contains("Weeper") && twn.contains("DisTweaks_NPCPawn ");
            if template.is_none() && ((guard && is_guard) || (!guard && civ)) {
                template = Some(*sp);
            }
        }
        let template = template.context("no host spawner of that kind to copy")?;
        let sp = self.spawn_any("DishonoredSpawner", at, [0.0, yaw, 0.0], Some(template))?;
        if let Some(t) = tweak_obj {
            let pl = self.r.place(&mut self.c, sp, "m_pPawnTweaks")?;
            self.c.write(pl.addr, &t.to_le_bytes())?;
        } else if let Some(tw) = tweak {
            anyhow::bail!("pawn tweak {tw} is not loaded");
        }
        if let Some(f) = faction {
            let fo = if f == "None" { 0 } else { self.c.find(f.rsplit('.').next().unwrap_or(f), "")?.first().copied().with_context(|| format!("faction {f} not loaded"))? };
            let pl = self.r.place(&mut self.c, sp, "m_pFactionTweakOverride")?;
            self.c.write(pl.addr, &fo.to_le_bytes())?;
        }
        for (k, v) in [("m_bCanSpawnWhenVisible", json!(true)), ("m_bCanSpawnWhenPlayerNear", json!(true)), ("m_bPatrolUponStartup", json!(patrol)), ("m_bSpawnOnBeginPlay", json!(false))] {
            let pl = self.r.place(&mut self.c, sp, k)?;
            self.r.write_place(&mut self.c, &pl, &v)?;
        }
        crate::lab::move_actor_r(&mut self.c, &mut self.r, sp, at.to_array())?;
        // The template copy carries the host spawner's `m_SpawnedPawns` (its own pawn): empty the
        // copy (data, num, max zeroed: the copied buffer is left alone rather than shared).
        let pl = self.r.place(&mut self.c, sp, "m_SpawnedPawns")?;
        self.c.write(pl.addr, &[0u8; 12])?;
        // Nor does it join the template's squad: the spawn (0x00A5E790) refuses once the
        // squad's alive / total caps are reached (reasons 3 / 2), so `m_Squad` is None.
        let pl = self.r.place(&mut self.c, sp, "m_Squad")?;
        self.c.write(pl.addr, &[0u8; 8])?;
        // Nor the template's queued spawn requests (ring m_PendingSpawns with its count, head,
        // retries and retry timer).
        for k in ["m_NumPendingSpawns", "m_FirstPendingSpawn", "m_NumRetryAttempts", "m_fRetryTimer"] {
            let pl = self.r.place(&mut self.c, sp, k)?;
            self.c.write(pl.addr, &[0u8; 4])?;
        }
        // The requester is the class default `DisSeqAct_StartSpawn` (the native OnStartSpawn
        // 0x00A5C150 dereferences it; None crashes). The spawner counts on it
        // (m_PendingSpawnCount / m_SpawnedCount / m_FailedCount), and every StartSpawn loaded
        // later starts from those values (a count above the instance's output links trips an
        // array assertion in its latent update 0x00B90050 after the next map load). So its flag
        // and counters are put back afterwards, a failed spawn's queue is cancelled (no late
        // spawn counts on it), and `open_map` clears the counters before every load.
        let act = self.r.object(&mut self.c, "Default__DisSeqAct_StartSpawn", None)?;
        let pl = self.r.place(&mut self.c, act, "m_bSpawnEvenIfVisible")?;
        let saved = self.c.read(pl.addr, 16)?;
        self.r.write_place(&mut self.c, &pl, &json!(true))?;
        let r = self.start_spawn(sp, act, at);
        let _ = self.c.write(pl.addr, &saved);
        r
    }

    /// A patrol route through `points` for a scenario NPC (U-802): a host `DishonoredRoute`
    /// with at least as many points is taken over: its `DishonoredNavPoint`s are moved to the
    /// points (static actors: Location written, components updated), their guard settings
    /// cleared, its list cut to the points, `RouteType` set (0 Linear, 1 PingPong, 2 Circle),
    /// any squad, unlimited capacity and range, active, no adopters. The patrol manager
    /// (0x00C41590) gives a starting patroller the nearest adoptable route: this one, at the
    /// scenario's place. Returns the route.
    pub fn setup_route(&mut self, points: &[Vec3], route_type: u8, used: &mut Vec<u32>) -> Result<u32> {
        let mut best = None;
        for r in self.c.find("", "DishonoredRoute")? {
            if used.contains(&r) {
                continue;
            }
            let pl = self.r.place(&mut self.c, r, "RouteList")?;
            let n = self.c.read_u32(pl.addr + 4)?;
            if n as usize >= points.len() && best.is_none_or(|(_, bn)| n < bn) {
                best = Some((r, n));
            }
        }
        let (route, _) = best.context("no host route with enough points")?;
        used.push(route);
        let pl = self.r.place(&mut self.c, route, "RouteList")?;
        let data = self.c.read_u32(pl.addr)?;
        for (i, p) in points.iter().enumerate() {
            let nav = self.c.read_u32(data + 20 * i as u32)?;
            anyhow::ensure!(nav != 0, "route point {i} has no actor");
            crate::lab::move_actor_r(&mut self.c, &mut self.r, nav, p.to_array())?;
            for (k, v) in [("m_fGuardDuration", json!(0.0)), ("m_bGuardForever", json!(false))] {
                if let Ok(pl) = self.r.place(&mut self.c, nav, k) {
                    self.r.write_place(&mut self.c, &pl, &v)?;
                }
            }
            if let Ok(pl) = self.r.place(&mut self.c, nav, "m_WatchPoints") {
                self.c.write(pl.addr + 4, &0u32.to_le_bytes())?;
            }
        }
        self.c.write(pl.addr + 4, &(points.len() as u32).to_le_bytes())?;
        let sq = self.r.place(&mut self.c, route, "m_SupportedSquads")?;
        self.c.write(sq.addr + 4, &0u32.to_le_bytes())?;
        let pl = self.r.place(&mut self.c, route, "RouteType")?;
        self.c.write(pl.addr, &[route_type])?;
        for (k, v) in [("m_Capacity", json!(-1)), ("m_AdoptionRange", json!(-1.0)), ("m_bIsActive", json!(true)), ("m_NumNPCAdopters", json!(0)), ("m_NeglectTimer", json!(0.0))] {
            let pl = self.r.place(&mut self.c, route, k)?;
            self.r.write_place(&mut self.c, &pl, &v)?;
        }
        Ok(route)
    }

    /// Zero the class default StartSpawn's counters (see `spawn_npc_from`).
    pub fn clear_start_spawn_counts(&mut self) -> Result<()> {
        let act = self.r.object(&mut self.c, "Default__DisSeqAct_StartSpawn", None)?;
        for k in ["m_PendingSpawnCount", "m_SpawnedCount", "m_FailedCount"] {
            let pl = self.r.place(&mut self.c, act, k)?;
            self.c.write(pl.addr, &[0u8; 4])?;
        }
        Ok(())
    }
    /// The pawn, and whether it had to be staged: when the spawn keeps being retried at `at`
    /// (another pawn there blocks it, e.g. a row of NPCs 84 uu apart), the spawner is moved
    /// 600 uu up after 30 ticks with a new request, the pawn spawns there and is put at `at`
    /// (spawner too).
    fn start_spawn(&mut self, sp: u32, act: u32, at: Vec3) -> Result<(u32, bool)> {
        self.r.call(&mut self.c, sp, "DishonoredSpawner.OnStartSpawn", &json!([hex(act)]))?;
        let _ = self.c.lockstep(true, 30_000);
        let mut staged = false;
        for i in 0..60 {
            if i == 15 && !staged {
                let pl = self.r.place(&mut self.c, sp, "m_NumPendingSpawns")?;
                if self.c.read_u32(pl.addr)? > 0 {
                    // The queued request keeps its own location: cancel it, move, ask again.
                    for k in ["m_NumPendingSpawns", "m_FirstPendingSpawn", "m_NumRetryAttempts", "m_fRetryTimer"] {
                        let pl = self.r.place(&mut self.c, sp, k)?;
                        self.c.write(pl.addr, &[0u8; 4])?;
                    }
                    crate::lab::move_actor_r(&mut self.c, &mut self.r, sp, (at + Vec3::Z * 600.0).to_array())?;
                    self.r.call(&mut self.c, sp, "DishonoredSpawner.OnStartSpawn", &json!([hex(act)]))?;
                    let _ = self.c.lockstep(true, 30_000);
                    staged = true;
                }
            }
            self.c.step(2, 30_000)?;
            let pl = self.r.place(&mut self.c, sp, "m_SpawnedPawns")?;
            let arr = self.c.read(pl.addr, 8)?;
            let (data, num) = (u32::from_le_bytes(arr[0..4].try_into().unwrap()), u32::from_le_bytes(arr[4..8].try_into().unwrap()));
            if num > 0 {
                let p = self.c.read_u32(data)?;
                if p != 0 {
                    if staged {
                        crate::lab::move_actor_r(&mut self.c, &mut self.r, sp, at.to_array())?;
                        crate::lab::move_actor_r(&mut self.c, &mut self.r, p, at.to_array())?;
                        self.c.step(1, 30_000)?;
                    }
                    return Ok((p, staged));
                }
            }
        }
        let mut st = Vec::new();
        for k in ["m_NumPendingSpawns", "m_NumRetryAttempts", "m_fRetryTimer", "m_fTimeSinceLastSpawn"] {
            if let Ok(pl) = self.r.place(&mut self.c, sp, k) {
                st.push(format!("{k} {}", self.r.read_place(&mut self.c, &pl).unwrap_or_default()));
            }
        }
        // Cancel what is still queued, so no pawn appears later.
        for k in ["m_NumPendingSpawns", "m_NumRetryAttempts"] {
            if let Ok(pl) = self.r.place(&mut self.c, sp, k) {
                let _ = self.c.write(pl.addr, &[0u8; 4]);
            }
        }
        anyhow::bail!("the spawner {sp:#x} made no pawn within 120 ticks ({})", st.join(", "))
    }
}