//! The persistent hooked session and its JSON command set (research/dishonored/ORACLE.md
//! "Original side: the session"). One game process is booted once and kept; maps are switched
//! with the console `open`; every request is a JSON object in, a JSON object out.
//!
//! probe_launch session start [--map M] [--hz 60]     launch (if needed), front end, map, setup
//! probe_launch session <op> ['<json args>']          one request (prints one JSON line)
//! probe_launch session repl                          JSON lines on stdin -> JSON lines on stdout
//! probe_launch session stop                          end the game
//!
//! Ops (`{"op": ...}` in repl mode; the args object otherwise): see `OPS` and ORACLE.md.
use crate::reflect::{self, Reflect, hex};
use anyhow::{Context, Result, bail};
use dishonored_probe::{Client, LineQuery};
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

pub const FRONT_END: &str = "DishonoredGameFull_P";
/// `GUseFixedTimeStep` and `GFixedDeltaTime` (double), read by the time update 0x009DDDF0.
pub const G_USE_FIXED_STEP: u32 = 0x0141_b27c;
pub const G_FIXED_DELTA: u32 = 0x0126_96b0;
/// LocalPlayer.ControllerId: -1 until the start screen's "press any key".
const LP_CONTROLLER_ID: u32 = 0x5c;
const ENGINE_GAME_PLAYERS: u32 = 0x480;

pub const OPS: &str = "status open reload setup give power equip select_power find read write dump props call signature spawn step resume key console place collide events spawns run attention shaders census psconst names";

pub struct Session {
    pub c: Client,
    pub r: Reflect,
}

fn st_map(c: &mut Client) -> Result<std::collections::HashMap<String, String>> {
    Ok(c.status()?.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect())
}

fn st_hex(m: &std::collections::HashMap<String, String>, k: &str) -> u32 {
    m.get(k).and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()).unwrap_or(0)
}

fn st_num(m: &std::collections::HashMap<String, String>, k: &str) -> u64 {
    m.get(k).and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn args_f32(v: &Value) -> Option<[f32; 3]> {
    // "x,y,z" (key=value arguments) as well as [x, y, z].
    if let Some(s) = v.as_str() {
        let p: Vec<f32> = s.split([',', ' ']).filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect();
        return (p.len() == 3).then(|| [p[0], p[1], p[2]]);
    }
    let a = v.as_array()?;
    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32])
}

impl Session {
    pub fn connect() -> Result<Session> {
        Ok(Session { c: crate::connect()?, r: Reflect::default() })
    }

    pub fn status(&mut self) -> Result<Value> {
        let s = st_map(&mut self.c)?;
        let fixed = self.c.read_u32(G_USE_FIXED_STEP)? != 0;
        let dt = f64::from_le_bytes(self.c.read(G_FIXED_DELTA, 8)?.try_into().unwrap());
        Ok(json!({
            "map": s.get("map").cloned().unwrap_or_default(),
            "pawn": hex(st_hex(&s, "pawn")),
            "pawn_class": s.get("pawn_class").cloned().unwrap_or_default(),
            "pc": hex(st_hex(&s, "local_pc")),
            "tick": st_num(&s, "game_tick"),
            "tick_hook": st_num(&s, "tick_hook") == 1,
            "lockstep": st_num(&s, "lockstep") == 1,
            "parked": st_num(&s, "parked") == 1,
            "presents": st_num(&s, "presents"),
            "fixed_step_hz": if fixed && dt > 0.0 { json!((1.0 / dt).round()) } else { Value::Null },
            "trace_dropped": st_num(&s, "trace_dropped"),
        }))
    }

    pub fn set_fixed_step(&mut self, hz: f64) -> Result<()> {
        if hz > 0.0 {
            self.c.write(G_FIXED_DELTA, &(1.0 / hz).to_le_bytes())?;
            self.c.write(G_USE_FIXED_STEP, &1u32.to_le_bytes())?;
        } else {
            self.c.write(G_USE_FIXED_STEP, &0u32.to_le_bytes())?;
        }
        Ok(())
    }

    fn wait_status(&mut self, what: &str, timeout: Duration, mut pred: impl FnMut(&std::collections::HashMap<String, String>) -> bool) -> Result<()> {
        let t0 = Instant::now();
        loop {
            if let Ok(s) = st_map(&mut self.c)
                && pred(&s) {
                    return Ok(());
                }
            if t0.elapsed() > timeout {
                bail!("timed out after {:.0} s waiting for {what}", t0.elapsed().as_secs_f64());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Wait for the front end after a launch (the start map running for 200 world ticks).
    pub fn wait_front_end(&mut self, timeout: Duration) -> Result<()> {
        self.wait_status("the front end", timeout, |s| s.get("map").is_some_and(|m| m == FRONT_END) && (st_num(s, "game_tick") > 200 || st_num(s, "presents") > 200))
    }

    fn release_keys(&mut self) {
        for k in ["W", "S", "A", "D", "SpaceBar", "C", "LeftShift", "Q", "E", "F", "LeftMouseButton", "RightMouseButton", "LeftControl", "Enter"] {
            let _ = self.c.key(k, false);
        }
    }

    /// Switch to `map` (console `open`) and wait until the player is in control: the level is
    /// up with a new pawn, the local player owns controller 0, the briefing is dismissed and a
    /// held W moves the pawn (or reaches `PlayerInput.aBaseY`). Returns (load s, control s).
    pub fn open_map(&mut self, map: &str, timeout: Duration) -> Result<(f64, f64)> {
        let _ = self.c.lockstep(false, 1000);
        self.release_keys();
        let _ = self.clear_start_spawn_counts();
        let t0 = Instant::now();
        let s = st_map(&mut self.c)?;
        let old_pawn = st_hex(&s, "pawn");
        let _ = self.c.console(&format!("open {map}"));
        let want = map.to_ascii_lowercase();
        self.wait_status(&format!("the level {map}"), timeout, |s| s.get("map").is_some_and(|m| m.to_ascii_lowercase() == want) && st_hex(s, "pawn") != 0 && st_hex(s, "pawn") != old_pawn)?;
        let load = t0.elapsed().as_secs_f64();
        self.r = Reflect::default();
        self.claim_controller()?;
        let t1 = Instant::now();
        let mut last_err = String::new();
        loop {
            // The game thread can stay busy for seconds after the load (streaming): job
            // timeouts mean "not yet".
            match self.in_control() {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => last_err = format!("{e:#}"),
            }
            if t1.elapsed() > timeout {
                bail!("{map}: loaded but not in control after {:.0} s ({last_err})", t1.elapsed().as_secs_f64());
            }
            // The briefing (and skippable intros) wait for a key.
            let r = (|| -> Result<()> {
                self.c.key("Enter", true)?;
                self.c.step(2, 30_000)?;
                self.c.key("Enter", false)?;
                self.c.step(30, 60_000)?;
                Ok(())
            })();
            if let Err(e) = r {
                last_err = format!("{e:#}");
                let _ = self.c.key("Enter", false);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        Ok((load, t1.elapsed().as_secs_f64()))
    }

    fn claim_controller(&mut self) -> Result<()> {
        let s = st_map(&mut self.c)?;
        let engine = st_hex(&s, "gengine");
        let players = self.c.read_u32(engine + ENGINE_GAME_PLAYERS)?;
        let lp = self.c.read_u32(players)?;
        self.c.write(lp + LP_CONTROLLER_ID, &0i32.to_le_bytes())?;
        Ok(())
    }

    /// Hold W for 10 ticks: in control when the pawn moves or the forward axis is fed.
    fn in_control(&mut self) -> Result<bool> {
        let s = st_map(&mut self.c)?;
        let (pawn, input) = (st_hex(&s, "pawn"), st_hex(&s, "player_input"));
        if pawn == 0 || input == 0 {
            return Ok(false);
        }
        let a = reflect_vec(&mut self.c, pawn + 0xc4)?;
        let mut fed = false;
        let held = (|| -> Result<()> {
            self.c.key("W", true)?;
            for _ in 0..10 {
                self.c.step(1, 30_000)?;
                // PlayerInput.aBaseY (+0x138).
                fed |= self.c.read_f32(input + 0x138)? != 0.0;
            }
            Ok(())
        })();
        self.c.key("W", false)?;
        held?;
        self.c.step(5, 30_000)?;
        let b = reflect_vec(&mut self.c, pawn + 0xc4)?;
        let moved = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        Ok(moved > 5.0 || fed)
    }

    /// The measurement kit without items: tutorials off, the player kept alive, fixed step.
    pub fn setup(&mut self, hz: f64, keep_alive: bool) -> Result<()> {
        for a in self.c.find("pGlobalUIManager", "DisGlobalUIManager")? {
            if self.c.name_of(a)?.contains("PersistentLevel") {
                let v = self.c.read_u32(a + 0x304)? & !0x18;
                self.c.write(a + 0x304, &v.to_le_bytes())?;
            }
        }
        // A real focus loss (another window activated) opens the pause menu despite the focus
        // faking (`Engine.bPauseOnLossOfFocus` -> `Dis_OpenPauseMenu`, WorldInfo.Paused): off.
        let engine = self.r.object(&mut self.c, "engine", None)?;
        let pl = self.r.place(&mut self.c, engine, "bPauseOnLossOfFocus")?;
        self.r.write_place(&mut self.c, &pl, &json!(false))?;
        self.unpause()?;
        if keep_alive {
            let p = crate::lab::pawn(&mut self.c)?;
            self.c.write(p + 0x594, &1i32.to_le_bytes())?;
        }
        self.set_fixed_step(hz)
    }

    /// Close the pause menu if it is open (`WorldInfo.Paused`: Enter on its highlighted
    /// "Resume"). Returns whether the world was paused.
    pub fn unpause(&mut self) -> Result<bool> {
        let wi = self.r.object(&mut self.c, "worldinfo", None)?;
        let pl = self.r.place(&mut self.c, wi, "Paused")?;
        if self.r.read_place(&mut self.c, &pl)? != json!(true) {
            return Ok(false);
        }
        for _ in 0..5 {
            self.c.key("Enter", true)?;
            self.c.step(2, 30_000)?;
            self.c.key("Enter", false)?;
            self.c.step(10, 30_000)?;
            if self.r.read_place(&mut self.c, &pl)? != json!(true) {
                return Ok(true);
            }
        }
        bail!("the world stays paused (a menu is open)")
    }

    /// Put the pawn at `loc` (feet snapped to the floor below when `snap`: the engine's own
    /// line check down from 100 uu above, centre at floor + CollisionHeight + 1 as the port's
    /// runner does), zero its velocity and set the view.
    pub fn place(&mut self, loc: [f32; 3], yaw: Option<f32>, pitch: Option<f32>, snap: bool) -> Result<Value> {
        let pawn = crate::lab::pawn(&mut self.c)?;
        let pc = crate::lab::pc(&mut self.c)?;
        let mut at = loc;
        let mut floor = Value::Null;
        if snap {
            let q = LineQuery { start: [loc[0], loc[1], loc[2] + 100.0], end: [loc[0], loc[1], loc[2] - 1900.0], extent: [0.0; 3], flags: TRACE_WORLD, source: pawn };
            let r = self.c.line_checks(&[q])?;
            if let Some(h) = r.first().filter(|h| h.hit) {
                let height = self.read_value("pawn", "CylinderComponent.CollisionHeight")?.as_f64().unwrap_or(0.0) as f32;
                at[2] = h.location[2] + height + 1.0;
                floor = json!(h.location[2]);
            }
        }
        let at = crate::lab::move_actor_r(&mut self.c, &mut self.r, pawn, at)?;
        self.c.write(pawn + 0x1b4, &[0u8; 12])?;
        let rot = |deg: f32| ((deg / 360.0 * 65536.0).round() as i32) & 0xffff;
        if let Some(y) = yaw {
            self.c.write(pc + 0xd4, &rot(y).to_le_bytes())?;
            crate::lab::set_actor_yaw(&mut self.c, &mut self.r, pawn, y)?;
        }
        if let Some(p) = pitch {
            self.c.write(pc + 0xd0, &rot(p).to_le_bytes())?;
        }
        Ok(json!({"location": at, "floor_z": floor}))
    }

    pub fn read_value(&mut self, object: &str, path: &str) -> Result<Value> {
        let o = self.r.object(&mut self.c, object, None)?;
        let pl = self.r.place(&mut self.c, o, path)?;
        self.r.read_place(&mut self.c, &pl)
    }

    /// One JSON request.
    pub fn handle(&mut self, op: &str, a: &Value) -> Result<Value> {
        let s = |k: &str| a.get(k).and_then(Value::as_str);
        let num = |k: &str| a.get(k).and_then(Value::as_f64);
        Ok(match op {
            "status" => self.status()?,
            "open" | "reload" => {
                let map = match s("map") {
                    Some(m) => m.to_string(),
                    None => st_map(&mut self.c)?.get("map").cloned().context("no current map")?,
                };
                let (load, control) = self.open_map(&map, Duration::from_secs(num("timeout").unwrap_or(300.0) as u64))?;
                let hz = num("hz").unwrap_or(60.0);
                self.setup(hz, a.get("keep_alive").and_then(Value::as_bool).unwrap_or(true))?;
                json!({"map": map, "load_s": load, "control_s": control, "fixed_step_hz": hz})
            }
            "setup" => {
                self.setup(num("hz").unwrap_or(60.0), a.get("keep_alive").and_then(Value::as_bool).unwrap_or(true))?;
                json!({"ok": true})
            }
            "give" => {
                crate::lab::give(&mut self.c, s("item").context("item (a tweak object, e.g. Twk_Inv_PistolCorvo)")?, a.get("equip").and_then(Value::as_bool).unwrap_or(true))?;
                json!({"ok": true})
            }
            "power" => {
                crate::lab::power(&mut self.c, s("name").context("name")?, num("level").unwrap_or(1.0) as i32)?;
                json!({"ok": true})
            }
            "equip" => {
                crate::lab::equip(&mut self.c, s("class").context("class")?, num("hand").unwrap_or(1.0) as u8)?;
                json!({"ok": true})
            }
            "find" => {
                let found = self.c.find(s("name").unwrap_or(""), s("class").unwrap_or(""))?;
                let limit = num("limit").unwrap_or(200.0) as usize;
                let names = self.c.names_of(&found[..found.len().min(limit)])?;
                json!({"count": found.len(), "objects": found.iter().zip(names).map(|(a, n)| json!({"addr": hex(*a), "name": n})).collect::<Vec<_>>()})
            }
            "read" => {
                let o = self.r.object(&mut self.c, s("object").unwrap_or("pawn"), s("class"))?;
                match s("path") {
                    Some(p) => {
                        let pl = self.r.place(&mut self.c, o, p)?;
                        let v = self.r.read_place(&mut self.c, &pl)?;
                        json!({"object": hex(o), "path": p, "type": pl.prop.class, "addr": hex(pl.addr), "value": v})
                    }
                    None => json!({"object": hex(o), "name": self.c.name_of(o)?, "fields": self.r.dump_object(&mut self.c, o, s("filter"))?}),
                }
            }
            "write" => {
                let o = self.r.object(&mut self.c, s("object").unwrap_or("pawn"), s("class"))?;
                let p = s("path").context("path")?;
                let pl = self.r.place(&mut self.c, o, p)?;
                let v = a.get("value").context("value")?;
                self.r.write_place(&mut self.c, &pl, v)?;
                // A transform written on an actor goes through the engine so its components
                // (collision bounds, light environments, touches) follow.
                let top = p.split(['.', '[']).next().unwrap_or("");
                let mut via = Value::Null;
                if matches!(top, "Location" | "Rotation" | "DrawScale" | "DrawScale3D") && { let oc = self.r.class_of(&mut self.c, o)?; self.r.field(&mut self.c, oc, "Location", false)?.is_some() } {
                    match top {
                        "Location" => {
                            let l = crate::lab::read_vec(&mut self.c, o + 0xc4)?;
                            crate::lab::move_actor_r(&mut self.c, &mut self.r, o, l)?;
                            via = json!("SetLocation");
                        }
                        "Rotation" => {
                            let b = self.c.read(o + 0xd0, 12)?;
                            let r: Vec<i32> = b.chunks(4).map(|w| i32::from_le_bytes(w.try_into().unwrap())).collect();
                            self.r.call(&mut self.c, o, "Actor.SetRotation", &json!([r]))?;
                            via = json!("SetRotation");
                        }
                        _ => {
                            self.r.call(&mut self.c, o, "Actor.ForceUpdateComponents", &json!([false, false]))?;
                            via = json!("ForceUpdateComponents");
                        }
                    }
                }
                json!({"object": hex(o), "path": p, "value": self.r.read_place(&mut self.c, &pl)?, "updated_by": via})
            }
            "dump" => {
                // The objects a request names: by class and / or name, each with its fields
                // (filtered by `filter`), keyed by `<package>:<object path>`.
                let found = self.c.find(s("name").unwrap_or(""), s("class").unwrap_or(""))?;
                let limit = num("limit").unwrap_or(50.0) as usize;
                let mut objs = Vec::new();
                for o in found.iter().take(limit) {
                    let full = self.c.name_of(*o)?;
                    let (cls, path) = full.split_once(' ').unwrap_or(("", &full));
                    let key = match path.split_once('.') {
                        Some((pkg, rest)) => format!("{pkg}:{}", rest.trim_start_matches("TheWorld.PersistentLevel.")),
                        None => path.to_string(),
                    };
                    objs.push(json!({"path": key, "class": cls, "addr": hex(*o), "fields": self.r.dump_object(&mut self.c, *o, s("filter"))?}));
                }
                json!({"count": found.len(), "objects": objs})
            }
            "props" => {
                let st = match s("class") {
                    Some(cl) => self.r.class_named(&mut self.c, cl)?,
                    None => {
                        let o = self.r.object(&mut self.c, s("object").unwrap_or("pawn"), None)?;
                        self.r.class_of(&mut self.c, o)?
                    }
                };
                let f = s("filter").map(str::to_ascii_lowercase);
                let mut out = Vec::new();
                for p in self.r.properties(&mut self.c, st)? {
                    if f.as_ref().is_some_and(|f| !p.name.to_ascii_lowercase().contains(f.as_str())) {
                        continue;
                    }
                    out.push(json!({"name": p.name, "type": p.class, "offset": hex(p.offset), "size": p.element_size, "dim": p.array_dim}));
                }
                if a.get("functions").and_then(Value::as_bool).unwrap_or(false) {
                    let mut fs = Vec::new();
                    let mut cur = st;
                    while cur != 0 {
                        let info = self.r.info(&mut self.c, cur)?.clone();
                        for x in info.fields.iter().filter(|x| x.class == "Function") {
                            if f.as_ref().is_none_or(|f| x.name.to_ascii_lowercase().contains(f.as_str())) {
                                fs.push(json!(format!("{}.{}", info.name, x.name)));
                            }
                        }
                        cur = info.super_struct;
                    }
                    json!({"properties": out, "functions": fs})
                } else {
                    json!({"properties": out})
                }
            }
            "call" => {
                let o = self.r.object(&mut self.c, s("object").unwrap_or("pawn"), s("class"))?;
                let f = s("function").context("function")?;
                self.r.call(&mut self.c, o, f, a.get("args").unwrap_or(&Value::Null))?
            }
            "signature" => {
                let st = match s("class") {
                    Some(cl) => self.r.class_named(&mut self.c, cl)?,
                    None => {
                        let o = self.r.object(&mut self.c, s("object").unwrap_or("pawn"), None)?;
                        self.r.class_of(&mut self.c, o)?
                    }
                };
                let fname = s("function").context("function")?;
                let f = self.r.field(&mut self.c, st, fname, true)?.with_context(|| format!("no function {fname}"))?;
                json!({"function": fname, "native": f.aux & 0x400 != 0, "params": self.r.signature(&mut self.c, f.addr)?})
            }
            "spawn" => {
                let cls = s("class").context("class")?;
                let cls_addr = match self.r.class_named(&mut self.c, cls) {
                    Ok(a) => a,
                    Err(_) => {
                        // Not loaded yet: DynamicLoadObject("<Package>.<Class>", class'Class').
                        let pkg = s("package").context("class not loaded; give \"package\" to load it")?;
                        let meta = self.r.class_named(&mut self.c, "Class")?;
                        let pc = crate::lab::pc(&mut self.c)?;
                        let r = self.r.call(&mut self.c, pc, "Object.DynamicLoadObject", &json!({"ObjectName": format!("{pkg}.{cls}"), "ObjectClass": hex(meta)}))?;
                        r.get("return").and_then(Value::as_str).and_then(|x| x.rsplit_once('@')).and_then(|x| reflect::parse_addr(x.1)).with_context(|| format!("could not load {pkg}.{cls}"))?
                    }
                };
                let spawner = self.r.object(&mut self.c, s("spawner").unwrap_or("pc"), None)?;
                let loc = a.get("location").and_then(args_f32).context("location [x, y, z]")?;
                let rot = a.get("rotation").and_then(args_f32).unwrap_or([0.0; 3]);
                let u = |d: f32| (d / 360.0 * 65536.0).round() as i32;
                let mut args = Map::new();
                args.insert("SpawnClass".into(), json!(format!("@{}", hex(cls_addr))));
                args.insert("SpawnLocation".into(), json!(loc));
                args.insert("SpawnRotation".into(), json!([u(rot[0]), u(rot[1]), u(rot[2])]));
                if let Some(t) = s("tag") {
                    args.insert("SpawnTag".into(), json!(t));
                }
                args.insert("bNoCollisionFail".into(), json!(a.get("no_collision_fail").and_then(Value::as_bool).unwrap_or(true)));
                let r = self.r.call(&mut self.c, spawner, "Actor.Spawn", &Value::Object(args))?;
                let actor = r.get("return").cloned().unwrap_or(Value::Null);
                if actor.is_null() {
                    bail!("Spawn returned None: the class may be bStatic / bNoDelete (e.g. Emitter: use EmitterSpawnable), abstract, or the spot blocked");
                }
                json!({"actor": actor})
            }
            "step" => {
                let n = num("n").unwrap_or(1.0) as u32;
                let t0 = Instant::now();
                let tick = if a.get("free").and_then(Value::as_bool).unwrap_or(false) {
                    let _ = self.c.lockstep(false, 1000);
                    self.c.step(n, 600_000)?
                } else {
                    self.c.lockstep(true, 30_000)?;
                    self.c.step(n, 600_000)?
                };
                json!({"tick": tick, "seconds": t0.elapsed().as_secs_f64()})
            }
            "resume" => {
                let t = self.c.lockstep(false, 1000)?;
                json!({"tick": t})
            }
            "key" => {
                let k = s("key").context("key (engine name: W, SpaceBar, LeftShift, C, LeftMouseButton...)")?;
                let h = match s("event").unwrap_or("tap") {
                    "down" => self.c.key(k, true)?,
                    "up" => self.c.key(k, false)?,
                    _ => {
                        let h = self.c.key(k, true)?;
                        self.c.step(num("ticks").unwrap_or(1.0).max(1.0) as u32, 60_000)?;
                        self.c.key(k, false)? || h
                    }
                };
                json!({"handled": h})
            }
            "console" => json!({"result": self.c.console(s("command").context("command")?)?}),
            "place" => {
                let loc = a.get("location").and_then(args_f32).context("location [x, y, z]")?;
                self.place(loc, num("yaw").map(|v| v as f32), num("pitch").map(|v| v as f32), a.get("snap").and_then(Value::as_bool).unwrap_or(false))?
            }
            "collide" => {
                // {"queries": [[sx,sy,sz, ex,ey,ez, hx,hy,hz, flags], ...], "source": "pawn"|"none"}
                let src = if s("source") == Some("none") { 0 } else { crate::lab::pawn(&mut self.c).unwrap_or(0) };
                let q: Vec<LineQuery> = a
                    .get("queries")
                    .and_then(Value::as_array)
                    .context("queries")?
                    .iter()
                    .map(|x| {
                        let v: Vec<f64> = x.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                        let g = |i: usize| v.get(i).copied().unwrap_or(0.0) as f32;
                        LineQuery { start: [g(0), g(1), g(2)], end: [g(3), g(4), g(5)], extent: [g(6), g(7), g(8)], flags: v.get(9).copied().unwrap_or(TRACE_WORLD as f64) as u32, source: src }
                    })
                    .collect();
                let r = self.c.line_checks(&q)?;
                let mut out = Vec::new();
                for x in r {
                    let (actor, comp) = if x.hit { (reflect::object_value(&mut self.c, x.actor)?, reflect::object_value(&mut self.c, x.component)?) } else { (Value::Null, Value::Null) };
                    out.push(json!({"hit": x.hit, "time": x.time, "location": x.location, "normal": x.normal, "actor": actor, "component": comp}));
                }
                json!({"results": out})
            }
            "events" => {
                // {"functions": ["Touch", "Pawn.Landed"], "classes": ["SequenceOp"], "max": N}
                // starts a trace; {"fetch": true} returns the records since.
                if a.get("fetch").and_then(Value::as_bool).unwrap_or(false) {
                    let recs = self.c.event_fetch()?;
                    let mut out = Vec::new();
                    for r in recs {
                        let n = self.c.names_of(&[r.object, r.func])?;
                        out.push(json!({"tick": r.tick, "time": r.time, "object": n[0], "function": n[1].rsplit(' ').next().unwrap_or(""), "parms": r.parms.iter().map(|b| format!("{b:02x}")).collect::<String>()}));
                    }
                    json!({"events": out})
                } else {
                    let strs = |k: &str| -> Vec<String> { a.get(k).and_then(Value::as_array).map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() };
                    let n = self.c.event_filter(&strs("functions"), &strs("classes"), &[], num("parms").unwrap_or(0.0) as u32, num("max").unwrap_or(100_000.0) as u32)?;
                    json!({"functions_traced": n})
                }
            }
            "spawns" => {
                if a.get("fetch").and_then(Value::as_bool).unwrap_or(false) {
                    let recs = self.c.spawn_fetch()?;
                    let mut out = Vec::new();
                    for r in recs {
                        let n = self.c.names_of(&[r.object])?;
                        out.push(json!({"tick": r.tick, "object": format!("{}@{}", n[0], hex(r.object))}));
                    }
                    json!({"spawns": out})
                } else {
                    self.c.spawn_watch(num("max").unwrap_or(10_000.0) as u32)?;
                    json!({"ok": true})
                }
            }
            "select_power" => {
                // The left hand on an active power: `DisItemPowers` equipped as the secondary
                // item, its current power pointed at the pawn's component of that class.
                let cls = s("component").context("component (DisActivePowerComponent_DarkVision, DishonoredActivePowerComponent_Possess...)")?;
                let comp = crate::oracle_powers::select_power(&mut self.c, cls)?;
                json!({"component": reflect::object_value(&mut self.c, comp)?})
            }
            "run" => crate::capture::run_reads(self, a)?,
            "lightenv" => self.light_env(a)?,
            "lights" => self.list_lights(a)?,
            "spawn_npc" => {
                let loc = a.get("location").and_then(args_f32).context("location")?;
                let (p, staged) = self.spawn_npc(s("tweak"), s("faction"), s("kind").unwrap_or("Guard") == "Guard", glam::Vec3::from(loc), num("yaw").unwrap_or(0.0) as f32)?;
                json!({"pawn": reflect::object_value(&mut self.c, p)?, "staged": staged, "location": crate::lab::read_vec(&mut self.c, p + 0xc4)?})
            }
            "darkness" => {
                // The player's chaos (DisDarknessManager, pawn.m_pDarknessManager): `add` runs the
                // engine's own add (0x00C41B50, thiscall (int): score += n, clamped at 0, as
                // DisSeqAct_AddDarkness's Activated does), `set` writes the score; the answer has
                // the score (0x00A446B0) and the level the engine derives (0x00C487D0, as
                // DisSeqVar_DarknessLevel reads it).
                let pawn = crate::lab::pawn(&mut self.c)?;
                let pl = self.r.place(&mut self.c, pawn, "m_pDarknessManager")?;
                let mgr = self.c.read_u32(pl.addr)?;
                anyhow::ensure!(mgr != 0, "no darkness manager");
                if let Some(n) = num("add") {
                    self.c.native_call(0x00c4_1b50, mgr, &[n as i32 as u32], false)?;
                }
                if let Some(n) = num("set") {
                    self.c.write(mgr + 0x38, &(n as i32).to_le_bytes())?;
                }
                let score = self.c.native_call(0x00a4_46b0, mgr, &[], false)?.0 as i32;
                let level = self.c.native_call(0x00c4_87d0, mgr, &[], false)?.0 as i32;
                let deeds = self.r.dump_object(&mut self.c, mgr, Some("m_DeedCounters"))?;
                json!({"score": score, "level": level, "deeds": deeds["m_DeedCounters"]})
            }
            "enum" => {
                // The value names of an enum (`eDisAmmoType`).
                let n = s("name").context("name")?;
                let e = *self.c.find(n, "Enum")?.first().with_context(|| format!("no enum {n}"))?;
                json!({"enum": n, "names": self.r.enum_names(&mut self.c, e)?.clone()})
            }
            "layout" => {
                // Build a layout map (maps/<name>.ron) in the current level (`reload`: open the
                // host level fresh first). Positions in the layout are then at +offset.
                let file = match (s("file"), s("map")) {
                    (Some(f), _) => std::path::PathBuf::from(f),
                    (None, Some(m)) => crate::playground::layout_file(m).with_context(|| format!("no maps/{}.ron (run from the repository root)", m.to_ascii_lowercase()))?,
                    _ => bail!("file or map"),
                };
                let l = crate::playground::parse_layout(&std::fs::read_to_string(&file)?)?;
                let offset = crate::playground::check_offset(a.get("offset").unwrap_or(&Value::Null))?;
                if a.get("reload").and_then(Value::as_bool).unwrap_or(true) {
                    let host = s("host").unwrap_or(crate::playground::host_map()).to_string();
                    self.open_map(&host, Duration::from_secs(600))?;
                    self.setup(num("hz").unwrap_or(60.0), true)?;
                }
                self.build_layout(&l, offset)?
            }
            "shaders" => {
                // Bytecode of the shaders the game created from id `since` on, written to
                // $DISHONORED_CACHE/oracle/shaders/<exe>/<kind>s_<hash>.bin (retail-derived:
                // never committed).
                let since = num("since").unwrap_or(0.0) as u32;
                let dir = shader_dir()?;
                std::fs::create_dir_all(&dir)?;
                let v = self.c.shader_dump(since)?;
                let mut list = Vec::new();
                let mut next = since;
                for sh in &v {
                    let kind = if sh.kind == b'v' { "vs" } else { "ps" };
                    let file = dir.join(format!("{kind}_{:016x}.bin", sh.hash));
                    // Content-addressed, so sessions share the store; written through a
                    // per-process temporary name so parallel sessions never see a partial file.
                    if !file.exists() {
                        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
                        std::fs::write(&tmp, &sh.code)?;
                        if std::fs::rename(&tmp, &file).is_err() {
                            let _ = std::fs::remove_file(&tmp);
                        }
                    }
                    next = next.max(sh.id + 1);
                    list.push(json!({"id": sh.id, "kind": kind, "hash": format!("0x{:016x}", sh.hash), "tick": sh.tick, "size": sh.code.len(), "ctab": sh.ctab, "file": file.display().to_string()}));
                }
                json!({"next": next, "dir": dir.display().to_string(), "shaders": list})
            }
            "census" => {
                // Pixel shaders drawn over the next frames (the world runs freely meanwhile).
                let _ = self.c.lockstep(false, 1000);
                let frames = num("frames").unwrap_or(10.0) as u32;
                let names: std::collections::HashMap<u64, String> = self.c.shaders()?.into_iter().map(|s| (s.hash, s.ctab)).collect();
                let v = self.c.draw_census(frames)?;
                json!({"frames": frames, "shaders": v.iter().map(|(h, d, p)| json!({"hash": format!("0x{h:016x}"), "draws": d, "prims": p, "ctab": names.get(h).cloned().unwrap_or_default()})).collect::<Vec<_>>()})
            }
            "psconst" => {
                // Live constants of the next `draws` draws with a pixel shader, by its CTAB
                // parameters (`name=c12x2`: registers c12..c13).
                let _ = self.c.lockstep(false, 1000);
                let h = s("hash").context("hash 0x...")?;
                let hash = u64::from_str_radix(h.trim_start_matches("0x"), 16)?;
                let draws = num("draws").unwrap_or(1.0) as u32;
                let names: std::collections::HashMap<u64, String> = self.c.shaders()?.into_iter().map(|s| (s.hash, s.ctab)).collect();
                let ctab = names.get(&hash).cloned().unwrap_or_default();
                let mut out = Vec::new();
                for (frame, _, regs) in self.c.ps_constants(hash, draws, "")? {
                    let mut m = Map::new();
                    for p in ctab.split_whitespace() {
                        let Some((name, reg)) = p.split_once('=') else { continue };
                        let Some(r) = reg.strip_prefix('c') else { continue };
                        let Some((i, n)) = r.split_once('x') else { continue };
                        let (i, n): (usize, usize) = (i.parse().unwrap_or(0), n.parse().unwrap_or(0));
                        let v: Vec<[f32; 4]> = regs.iter().skip(i).take(n).copied().collect();
                        m.insert(name.to_string(), json!(v));
                    }
                    out.push(json!({"frame": frame, "params": m}));
                }
                json!({"hash": h, "ctab": ctab, "draws": out})
            }
            "attention" => {
                // The NPC's attention record about the player (`PlayerAttnInfo`,
                // DisAttentionInfo_Complex under its controller).
                let npc = self.r.object(&mut self.c, s("npc").context("npc")?, None)?;
                let a = crate::oracle_stealth::attention_info(&mut self.c, npc)?;
                json!({"attention": reflect::object_value(&mut self.c, a)?, "addr": hex(a)})
            }
            "names" => {
                let v: Vec<u32> = a.get("addrs").and_then(Value::as_array).map(|v| v.iter().filter_map(|x| x.as_str().and_then(reflect::parse_addr)).collect()).unwrap_or_default();
                json!({"names": self.c.names_of(&v)?})
            }
            _ => bail!("unknown op {op}; ops: {OPS}"),
        })
    }
}

/// The player's movement checks (`UWorld::MoveActor` of the pawn, COLLISION.md): 0x20DF
/// (`TRACE_Blocking`), the trace type taken from the source pawn. Used to snap placements to
/// the floor and as the default flags of `collide` queries.
pub const TRACE_WORLD: u32 = 0x0000_20df;

/// `$DISHONORED_CACHE/oracle/shaders/<exe sha256, 16 hex>`.
pub fn shader_dir() -> Result<std::path::PathBuf> {
    use dishonored_oracle::cache;
    let root = cache::root();
    let exe = cache::file_sha256(&root, &cache::exe_path(&dishonored_oracle::install_dir()))?;
    Ok(root.join("shaders").join(&cache::hex(&exe)[..16]))
}

fn reflect_vec(c: &mut Client, a: u32) -> Result<[f32; 3]> {
    crate::lab::read_vec(c, a)
}

/// Request arguments: one JSON object (`'{"n": 5}'`), `@file.json`, or `key=value` pairs
/// whose values are JSON when they parse as JSON and strings otherwise (`n=5 path=Location.Z
/// location=[1,2,3]`): PowerShell 5 strips the quotes inside a JSON argument.
pub fn parse_args(args: &[String]) -> Result<Value> {
    match args.first() {
        None => Ok(json!({})),
        Some(j) if j.trim_start().starts_with('{') => serde_json::from_str(j).with_context(|| format!("arguments must be a JSON object: {j}")),
        Some(f) if f.starts_with('@') => Ok(serde_json::from_str(&std::fs::read_to_string(&f[1..])?)?),
        Some(_) => {
            // A token without `=` continues the previous value (`command=adddarkness 11`).
            let mut pairs: Vec<(String, String)> = Vec::new();
            for a in args {
                match a.split_once('=') {
                    Some((k, v)) if !k.is_empty() && !k.contains(' ') => pairs.push((k.to_string(), v.to_string())),
                    _ => match pairs.last_mut() {
                        Some(p) => {
                            p.1.push(' ');
                            p.1.push_str(a);
                        }
                        None => bail!("expected key=value, got {a}"),
                    },
                }
            }
            let mut m = Map::new();
            for (k, v) in pairs {
                m.insert(k, serde_json::from_str(&v).unwrap_or_else(|_| json!(v)));
            }
            Ok(Value::Object(m))
        }
    }
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string(v).unwrap_or_default());
}

pub fn run(args: &[String]) -> Result<()> {
    let verb = args.first().map(String::as_str).unwrap_or("");
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    match verb {
        "start" => {
            // Launch if no live session, wait for the front end, open the map, set up.
            let t0 = Instant::now();
            let mut launched = false;
            if crate::connect().and_then(|mut c| Ok(c.ping()?)).is_err() {
                crate::launch_session(None, Vec::new())?;
                launched = true;
            }
            let mut s = Session::connect()?;
            let launch_s = t0.elapsed().as_secs_f64();
            s.wait_front_end(Duration::from_secs(600))?;
            let front_s = t0.elapsed().as_secs_f64();
            let hz: f64 = opt("--hz").and_then(|v| v.parse().ok()).unwrap_or(60.0);
            let mut out = json!({"launched": launched, "launch_s": launch_s, "front_end_s": front_s});
            if let Some(map) = opt("--map") {
                let (load, control) = s.open_map(&map, Duration::from_secs(600))?;
                s.setup(hz, true)?;
                out["map"] = json!(map);
                out["load_s"] = json!(load);
                out["control_s"] = json!(control);
            } else {
                s.set_fixed_step(hz)?;
            }
            out["total_s"] = json!(t0.elapsed().as_secs_f64());
            out["status"] = s.status()?;
            print(&out);
        }
        "stop" => {
            crate::kill_session()?;
            print(&json!({"ok": true}));
        }
        "repl" => {
            let mut s = Session::connect()?;
            let stdin = std::io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                if stdin.read_line(&mut line)? == 0 {
                    break;
                }
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                let t0 = Instant::now();
                let r = serde_json::from_str::<Value>(t).map_err(anyhow::Error::from).and_then(|v| {
                    let op = v.get("op").and_then(Value::as_str).context("missing \"op\"")?.to_string();
                    s.handle(&op, &v)
                });
                let mut v = match r {
                    Ok(v) => json!({"ok": true, "result": v}),
                    Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
                };
                v["ms"] = json!((t0.elapsed().as_secs_f64() * 1000.0).round());
                print(&v);
                use std::io::Write;
                std::io::stdout().flush()?;
            }
        }
        "" | "help" => {
            println!("probe_launch session start [--map M] [--hz 60] | stop | repl | <op> ['<json>']\nops: {OPS}");
        }
        op => {
            let a = parse_args(&args[1..])?;
            let mut s = Session::connect()?;
            let t0 = Instant::now();
            match s.handle(op, &a) {
                Ok(v) => print(&json!({"ok": true, "result": v, "ms": (t0.elapsed().as_secs_f64() * 1000.0).round()})),
                Err(e) => {
                    print(&json!({"ok": false, "error": format!("{e:#}")}));
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
