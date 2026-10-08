//! Per-tick behaviour traces of the original, driven by a scenario spec
//! (`fixtures/oracle/scenarios/**.ron`, `dishonored_oracle::scenario`), written into the oracle
//! cache as a `Trace` under `Probe::Trace` keyed by the spec hash, so `oracle scenario NAME`
//! reports the first divergence against the port.
//!
//! probe_launch scenario <NAME | path.ron>... [--dir DIR] [--no-reload] [--partial]
//!                      [--channel NAME=OBJECT:PATH]... [--json OUT.json]
//!
//! The run is lockstep: the game thread is parked at every tick boundary (tick.rs in the DLL),
//! the channels are read, the input of the next tick is applied (keys through the engine's own
//! `InputKey`, the view written into `Controller.Rotation`, console commands), and exactly one
//! world tick of 1 / `tick_hz` s is released. Tick 0 is the state right after the setup, as in
//! the port's runner; the input of tick t acts during the step that produces tick t + 1.
//!
//! Recorded channels (names shared with the port's runner, tools/oracle/src/runner.rs):
//! - `Pawn`: `pawn.x/y/z` (Location), `pawn.vx/vy/vz` (Velocity), `pawn.speed2d`,
//!   `pawn.crouched` (`bIsCrouched`), `pawn.physics` (EPhysics index);
//! - `Vitals`: `pawn.health` (Health), `pawn.mana` (the player pawn's mana, +0xA60);
//! - `Camera`: `camera.x/y/z`, `camera.pitch/yaw/roll` (degrees), `camera.fov`
//!   (`PlayerCamera.CameraCache.POV`);
//! - `Npc(name)`: `npc.<name>.x/y/z/health/physics` (a pawn by name, or the pawn a spawner of
//!   that name points at);
//! - `Kismet`: events of kind `Kismet` (ProcessEvent calls on `SequenceOp` objects);
//! - `ProcessEvent(filters)`: events of kind `ProcessEvent` (functions whose `Outer.Name`
//!   contains a filter; a `Class:Name` filter traces every call on objects of that class);
//! - `SimEvents`: events of kind `Spawn` (actors that appear in the level: projectiles,
//!   emitters, pickups...).
//! `--channel NAME=OBJECT:PATH` adds any property (a Vector adds `.x/.y/.z`).
use crate::reflect::Place;
use crate::session::Session;
use anyhow::{Context, Result, bail};
use dishonored_oracle::cache::{self, OracleKey, Probe, Provenance};
use dishonored_oracle::scenario::{self, Action, Button, Record, Scenario};
use dishonored_oracle::trace::{Trace, TraceEvent};
use dishonored_probe::ReadPath;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How a channel's value is read from its bytes.
#[derive(Clone, Copy, Debug)]
enum Kind {
    F32,
    I32,
    U8,
    Bool(u32),
    /// Unreal rotator units (i32) -> degrees in [0, 360).
    RotDeg,
}

struct Chan {
    name: String,
    unit: String,
    path: Option<ReadPath>,
    kind: Kind,
    /// `pawn.speed2d`: from the two previous channels' values (indices).
    speed_of: Option<(usize, usize)>,
}

fn kind_of(pl: &Place) -> Result<Kind> {
    Ok(match pl.prop.class.as_str() {
        "FloatProperty" => Kind::F32,
        "IntProperty" => Kind::I32,
        "ByteProperty" => Kind::U8,
        "BoolProperty" => Kind::Bool(pl.prop.aux),
        k => bail!("{}: a {k} is not a scalar channel", pl.prop.name),
    })
}

fn len_of(k: Kind) -> u32 {
    if matches!(k, Kind::U8) { 1 } else { 4 }
}

fn value(k: Kind, b: &[u8]) -> f64 {
    let u = |b: &[u8]| u32::from_le_bytes(b[..4].try_into().unwrap());
    match k {
        Kind::F32 => f32::from_le_bytes(b[..4].try_into().unwrap()) as f64,
        Kind::I32 => u(b) as i32 as f64,
        Kind::U8 => b[0] as f64,
        Kind::Bool(m) => (u(b) & m != 0) as u8 as f64,
        Kind::RotDeg => ((u(b) as i32).rem_euclid(65536)) as f64 * 360.0 / 65536.0,
    }
}

struct Channels {
    list: Vec<Chan>,
}

impl Channels {
    fn add(&mut self, name: &str, unit: &str, addr: u32, kind: Kind) {
        self.list.push(Chan { name: name.into(), unit: unit.into(), path: Some(ReadPath { base: addr, offsets: vec![0], len: len_of(kind) }), kind, speed_of: None });
    }

    /// A property path; a Vector / Rotator adds per-component channels.
    fn add_path(&mut self, s: &mut Session, name: &str, unit: &str, object: u32, path: &str) -> Result<()> {
        let pl = s.r.place(&mut s.c, object, path)?;
        if pl.prop.class == "StructProperty" {
            let st = s.r.info(&mut s.c, pl.prop.aux)?.name.clone();
            let (comps, rot): (&[&str], bool) = match st.as_str() {
                "Vector" => (&["x", "y", "z"], false),
                "Rotator" => (&["pitch", "yaw", "roll"], true),
                _ => bail!("{path}: struct {st} is not a scalar, Vector or Rotator"),
            };
            for (k, c) in comps.iter().enumerate() {
                self.add(&format!("{name}.{c}"), if rot { "deg" } else { unit }, pl.addr + 4 * k as u32, if rot { Kind::RotDeg } else { Kind::F32 });
            }
            return Ok(());
        }
        let k = kind_of(&pl)?;
        self.add(name, unit, pl.addr, k);
        Ok(())
    }
}

/// Engine key of a scenario button (DefaultInput.ini `m_PCBindings`).
fn button_key(b: Button) -> Result<&'static str> {
    Ok(match b {
        Button::Jump => "SpaceBar",
        Button::Crouch => "C",
        Button::Sprint => "LeftShift",
        Button::Primary => "LeftMouseButton",
        Button::Secondary => "RightMouseButton",
        Button::Block => "LeftControl",
        Button::LeanLeft => "Q",
        Button::LeanRight => "E",
        Button::Use => "F",
        Button::Walk | Button::Reload => bail!("{b:?} has no key binding in the original (DefaultInput.ini)"),
    })
}

/// Keys held for a (forward, right) axis state: full deflection only (the original reads
/// keys; partial axes are not representable with keys).
fn move_keys(fw: i8, rt: i8) -> Result<Vec<&'static str>> {
    let mut k = Vec::new();
    for (v, pos, neg) in [(fw, "W", "S"), (rt, "D", "A")] {
        match v {
            0 => {}
            v if v.unsigned_abs() >= 120 => k.push(if v > 0 { pos } else { neg }),
            v => bail!("partial axis {v} cannot be played with keys (use -127, 0 or 127)"),
        }
    }
    Ok(k)
}

fn find_specs(names: &[String], dir: &std::path::Path) -> Result<Vec<Scenario>> {
    let mut out = Vec::new();
    let mut all: Option<Vec<(PathBuf, Scenario)>> = None;
    for n in names {
        let p = PathBuf::from(n);
        if p.extension().is_some_and(|x| x == "ron") && p.exists() {
            out.push(Scenario::load(&p)?);
            continue;
        }
        if all.is_none() {
            all = Some(scenario::load_dir(dir)?);
        }
        let s = all.as_ref().unwrap().iter().find(|(_, s)| s.name == *n || s.name.ends_with(&format!("/{n}"))).with_context(|| format!("no scenario {n} in {}", dir.display()))?;
        out.push(s.1.clone());
    }
    Ok(out)
}

/// The pawn a spawner (or a pawn of that name) stands for.
fn npc_pawn(s: &mut Session, name: &str) -> Result<u32> {
    let o = s.r.object(&mut s.c, name, None)?;
    let cls = s.r.class_of(&mut s.c, o)?;
    if s.r.field(&mut s.c, cls, "Health", false)?.is_some() {
        return Ok(o);
    }
    // A spawner: the first pointer in its fields to a live DishonoredNPCPawn.
    let pawns: std::collections::HashSet<u32> = s.c.find("", "DishonoredNPCPawn")?.into_iter().collect();
    let size = s.r.info(&mut s.c, cls)?.size;
    let raw = s.c.read(o, size)?;
    for w in raw.chunks(4) {
        if w.len() == 4 {
            let p = u32::from_le_bytes(w.try_into().unwrap());
            if pawns.contains(&p) {
                return Ok(p);
            }
        }
    }
    bail!("{name}: neither a pawn nor a spawner with a live pawn")
}

fn exe_hash() -> Result<[u8; 32]> {
    cache::file_sha256(&cache::root(), &cache::exe_path(&dishonored_oracle::install_dir()))
}

pub fn run(args: &[String]) -> Result<()> {
    let mut names = Vec::new();
    let mut dir = PathBuf::from("fixtures").join("oracle").join("scenarios");
    let (mut reload, mut partial) = (true, false);
    let mut extra: Vec<(String, String, String)> = Vec::new();
    let mut json_out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                dir = PathBuf::from(args.get(i + 1).context("--dir DIR")?);
                i += 1;
            }
            "--no-reload" => reload = false,
            "--partial" => partial = true,
            "--json" => {
                json_out = Some(PathBuf::from(args.get(i + 1).context("--json OUT")?));
                i += 1;
            }
            "--channel" => {
                let v = args.get(i + 1).context("--channel NAME=OBJECT:PATH")?;
                let (n, rest) = v.split_once('=').context("--channel NAME=OBJECT:PATH")?;
                let (o, p) = rest.split_once(':').context("--channel NAME=OBJECT:PATH")?;
                extra.push((n.into(), o.into(), p.into()));
                i += 1;
            }
            a => names.push(a.to_string()),
        }
        i += 1;
    }
    if names.is_empty() {
        bail!("probe_launch scenario <scenario name | file.ron>... [--no-reload] [--partial] [--channel N=OBJ:PATH] [--json OUT]");
    }
    let specs = find_specs(&names, &dir)?;
    let mut s = Session::connect()?;
    for spec in &specs {
        let r = capture(&mut s, spec, reload, partial, &extra)?;
        let (summary, trace) = r;
        if let Some(p) = &json_out {
            std::fs::write(p, serde_json::to_string(&trace)?)?;
        }
        println!("{}", serde_json::to_string(&summary)?);
    }
    Ok(())
}

/// Capture one scenario's trace and store it in the cache.
pub fn capture(s: &mut Session, spec: &Scenario, reload: bool, partial: bool, extra: &[(String, String, String)]) -> Result<(Value, Trace)> {
    if spec.sweep.is_some() {
        bail!("{}: a map sweep, not a scripted scenario (use `collide`)", spec.name);
    }
    let t_all = Instant::now();
    let mut warnings: Vec<String> = Vec::new();
    let gap = |w: String, warnings: &mut Vec<String>| -> Result<()> {
        if partial {
            warnings.push(w);
            Ok(())
        } else {
            bail!("{}: {w} (not supported by the original-side runner yet; --partial records it as a warning)", spec.name)
        }
    };
    // ---- map and setup ----
    let st = s.status()?;
    let cur = st["map"].as_str().unwrap_or("").to_string();
    let mut load = json!(null);
    if reload || !cur.eq_ignore_ascii_case(&spec.map) {
        let (l, c) = s.open_map(&spec.map, Duration::from_secs(600))?;
        load = json!({"load_s": l, "control_s": c});
    }
    let t_setup = Instant::now();
    s.c.lockstep(true, 30_000)?;
    s.setup(spec.tick_hz as f64, spec.setup.keep_alive)?;
    if spec.setup.kismet {
        // The original always runs the level's Kismet; nothing to switch.
    } else {
        warnings.push("kismet: false cannot be applied to the original (its Kismet always runs)".into());
    }
    if spec.setup.seed != 0 {
        warnings.push("seed: the original's random streams are not seeded by the runner".into());
    }
    for it in &spec.setup.items {
        crate::lab::give(&mut s.c, it, true)?;
    }
    for (cls, hand) in &spec.setup.equip {
        crate::lab::equip(&mut s.c, cls, *hand)?;
    }
    for (p, lvl) in &spec.setup.powers {
        crate::lab::power(&mut s.c, p, *lvl as i32)?;
    }
    for f in &spec.setup.story_flags {
        gap(format!("story flag {f:?}"), &mut warnings)?;
    }
    for n in &spec.setup.npcs {
        match (&n.spawner, &n.tweak, &n.state) {
            (Some(name), None, None) => {
                if let Some(p) = n.position {
                    let pawn = npc_pawn(s, name)?;
                    let mut b = Vec::new();
                    for v in p {
                        b.extend(v.to_le_bytes());
                    }
                    s.c.write(pawn + 0xc4, &b)?;
                    let yaw = ((n.yaw / 360.0 * 65536.0).round() as i32) & 0xffff;
                    s.c.write(pawn + 0xd4, &yaw.to_le_bytes())?;
                }
            }
            _ => gap(format!("NPC setup {n:?} (only moving an existing pawn / spawner's pawn is supported)"), &mut warnings)?,
        }
    }
    let p = spec.setup.player.clone().unwrap_or_default();
    if let Some(ps) = &spec.setup.player {
        if ps.sword_drawn {
            crate::lab::give(&mut s.c, "Twk_Inv_SwordCorvo", true)?;
        }
        s.place(ps.position, Some(ps.yaw), Some(ps.pitch), ps.snap_to_floor)?;
        if let Some(h) = ps.health {
            let pawn = crate::lab::pawn(&mut s.c)?;
            s.c.write(pawn + 0x344, &(h.round() as i32).to_le_bytes())?;
        }
        if let Some(m) = ps.mana {
            let pawn = crate::lab::pawn(&mut s.c)?;
            s.c.write(pawn + crate::oracle_powers::PAWN_MANA, &(m.round() as i32).to_le_bytes())?;
        }
    }
    // Release every key the script uses (a previous run may have left one down).
    for k in ["W", "S", "A", "D", "SpaceBar", "C", "LeftShift", "LeftMouseButton", "RightMouseButton", "LeftControl", "Q", "E", "F"] {
        let _ = s.c.key(k, false);
    }
    // ---- channels ----
    let pawn = crate::lab::pawn(&mut s.c)?;
    let pc = crate::lab::pc(&mut s.c)?;
    let mut ch = Channels { list: Vec::new() };
    let want = |r: &Record| spec.record.iter().any(|x| std::mem::discriminant(x) == std::mem::discriminant(r));
    if want(&Record::Pawn) {
        ch.add_path(s, "pawn", "uu", pawn, "Location")?;
        ch.add_path(s, "pawn.v", "uu/s", pawn, "Velocity")?;
        // Port names: pawn.vx / vy / vz.
        let n = ch.list.len();
        for (k, c) in ["vx", "vy", "vz"].iter().enumerate() {
            ch.list[n - 3 + k].name = format!("pawn.{c}");
        }
        ch.list.push(Chan { name: "pawn.speed2d".into(), unit: "uu/s".into(), path: None, kind: Kind::F32, speed_of: Some((n - 3, n - 2)) });
        ch.add_path(s, "pawn.crouched", "bool", pawn, "bIsCrouched")?;
        ch.add_path(s, "pawn.physics", "enum", pawn, "Physics")?;
    }
    if want(&Record::Vitals) {
        ch.add_path(s, "pawn.health", "pt", pawn, "Health")?;
        ch.add_path(s, "pawn.mana", "pt", pawn, "m_Mana")?;
    }
    if want(&Record::Camera) {
        let cam = s.r.object(&mut s.c, "camera", None)?;
        ch.add_path(s, "camera", "uu", cam, "CameraCache.POV.Location")?;
        ch.add_path(s, "camera", "deg", cam, "CameraCache.POV.Rotation")?;
        ch.add_path(s, "camera.fov", "deg", cam, "CameraCache.POV.FOV")?;
    }
    for r in &spec.record {
        if let Record::Npc(name) = r {
            let np = npc_pawn(s, name)?;
            ch.add_path(s, &format!("npc.{name}"), "uu", np, "Location")?;
            ch.add_path(s, &format!("npc.{name}.health"), "pt", np, "Health")?;
            ch.add_path(s, &format!("npc.{name}.physics"), "enum", np, "Physics")?;
        }
    }
    // The world clock of every sample (`world.dt` is derived after the run): frame-timing
    // differences show up here instead of as behaviour differences.
    let wi = s.r.object(&mut s.c, "worldinfo", None)?;
    ch.add_path(s, "world.time", "s", wi, "TimeSeconds")?;
    for (n, o, path) in extra {
        let obj = s.r.object(&mut s.c, o, None)?;
        ch.add_path(s, n, "", obj, path)?;
    }
    // ---- events ----
    let mut funcs: Vec<String> = Vec::new();
    let mut classes: Vec<String> = Vec::new();
    for r in &spec.record {
        match r {
            Record::Kismet => classes.push("SequenceOp".into()),
            Record::ProcessEvent(f) => {
                for x in f {
                    match x.strip_prefix("Class:") {
                        Some(c) => classes.push(c.into()),
                        None => funcs.push(x.clone()),
                    }
                }
            }
            _ => {}
        }
    }
    let tracing = !funcs.is_empty() || !classes.is_empty();
    if tracing {
        s.c.event_filter(&funcs, &classes, &[], 0, 1_000_000)?;
    }
    let spawns = want(&Record::SimEvents);
    if spawns {
        s.c.spawn_watch(100_000)?;
    }
    let setup_s = t_setup.elapsed().as_secs_f64();
    // ---- run ----
    let mut trace = Trace::new(&spec.name, "original", spec.tick_hz, &ch.list.iter().map(|c| (c.name.as_str(), c.unit.as_str())).collect::<Vec<_>>());
    let paths: Vec<(usize, ReadPath)> = ch.list.iter().enumerate().filter_map(|(i, c)| c.path.clone().map(|p| (i, p))).collect();
    let read_paths: Vec<ReadPath> = paths.iter().map(|x| x.1.clone()).collect();
    let sample = |s: &mut Session| -> Result<Vec<f64>> {
        let r = s.c.reads(&read_paths)?;
        let mut v = vec![f64::NAN; ch.list.len()];
        for ((i, _), b) in paths.iter().zip(r) {
            if let Some(b) = b {
                v[*i] = value(ch.list[*i].kind, &b);
            }
        }
        for (i, c) in ch.list.iter().enumerate() {
            if let Some((a, b)) = c.speed_of {
                v[i] = (v[a] * v[a] + v[b] * v[b]).sqrt();
            }
        }
        Ok(v)
    };
    let base = s.c.lockstep(true, 30_000)?;
    if tracing {
        s.c.event_fetch()?; // drop what the setup produced
    }
    if spawns {
        s.c.spawn_fetch()?;
    }
    let t_run = Instant::now();
    trace.push(0, sample(s)?);
    let timeline = spec.input_timeline();
    let mut held: Vec<&'static str> = Vec::new();
    let (mut yaw, mut pitch) = (p.yaw, p.pitch);
    let rot = |deg: f32| ((deg / 360.0 * 65536.0).round() as i32) & 0xffff;
    let mut raw_events = Vec::new();
    let mut raw_spawns = Vec::new();
    let consoles: HashMap<u32, Vec<String>> = spec.input.iter().filter_map(|st| if let Action::Console(c) = &st.action { Some((st.tick, c.clone())) } else { None }).fold(HashMap::new(), |mut m, (t, c)| {
        m.entry(t).or_default().push(c);
        m
    });
    for (t, (fw, rt, y, pt, buttons)) in timeline.iter().enumerate() {
        let mut want_keys = move_keys(*fw, *rt)?;
        for b in buttons {
            want_keys.push(button_key(*b)?);
        }
        for k in held.iter().filter(|k| !want_keys.contains(k)) {
            s.c.key(k, false)?;
        }
        for k in want_keys.iter().filter(|k| !held.contains(k)) {
            s.c.key(k, true)?;
        }
        held = want_keys;
        if (*y, *pt) != (yaw, pitch) || t == 0 {
            (yaw, pitch) = (*y, *pt);
            s.c.write(pc + 0xd0, &rot(pitch).to_le_bytes())?;
            s.c.write(pc + 0xd4, &rot(yaw).to_le_bytes())?;
        }
        if let Some(cmds) = consoles.get(&(t as u32)) {
            for c in cmds {
                s.c.console(c)?;
            }
        }
        s.c.step(1, 60_000)?;
        trace.push(t as u32 + 1, sample(s)?);
        if tracing && (t % 30 == 29 || t + 1 == timeline.len()) {
            raw_events.extend(s.c.event_fetch()?);
        }
        if spawns && (t % 30 == 29 || t + 1 == timeline.len()) {
            raw_spawns.extend(s.c.spawn_fetch()?);
        }
    }
    let run_s = t_run.elapsed().as_secs_f64();
    // world.dt: the game time each tick advanced (1 / tick_hz under the fixed step).
    if let Some(ti) = trace.channel("world.time") {
        trace.channels.push(dishonored_oracle::trace::Channel { name: "world.dt".into(), unit: "s".into() });
        let mut prev = f64::NAN;
        for fr in trace.frames.iter_mut() {
            let t = fr.values[ti];
            fr.values.push(t - prev);
            prev = t;
        }
    }
    for k in &held {
        let _ = s.c.key(k, false);
    }
    if tracing {
        s.c.event_filter(&[], &[], &[], 0, 0)?;
    }
    if spawns {
        s.c.spawn_watch(0)?;
    }
    s.c.lockstep(false, 1000)?;
    // ---- events into the trace ----
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut name = |s: &mut Session, a: u32| -> String {
        names.entry(a).or_insert_with(|| s.c.name_of(a).unwrap_or_default()).clone()
    };
    let mut kismet_memo: HashMap<u32, bool> = HashMap::new();
    for e in &raw_events {
        let obj = name(s, e.object);
        let func = name(s, e.func);
        let cls = s.r.class_of(&mut s.c, e.object).unwrap_or(0);
        let is_kismet = match kismet_memo.get(&cls) {
            Some(&v) => v,
            None => {
                let mut v = false;
                let mut c = cls;
                while c != 0 && !v {
                    let info = s.r.info(&mut s.c, c)?;
                    v = info.name == "SequenceOp";
                    c = info.super_struct;
                }
                kismet_memo.insert(cls, v);
                v
            }
        };
        let fname = func.rsplit(' ').next().unwrap_or("").to_string();
        trace.events.push(TraceEvent {
            tick: e.tick.wrapping_sub(base) + 1,
            kind: if is_kismet { "Kismet" } else { "ProcessEvent" }.into(),
            object: short_path(&obj),
            name: fname.rsplit('.').next().unwrap_or("").to_string(),
            detail: fname,
        });
    }
    for r in &raw_spawns {
        let obj = name(s, r.object);
        let cls = obj.split(' ').next().unwrap_or("").to_string();
        trace.events.push(TraceEvent { tick: r.tick.wrapping_sub(base), kind: "Spawn".into(), object: short_path(&obj), name: cls, detail: String::new() });
    }
    trace.events.sort_by_key(|e| e.tick);
    // ---- store ----
    let exe = exe_hash()?;
    let key = OracleKey { exe_sha256: exe, subject: spec.name.clone(), probe: Probe::Trace, query_hash: spec.spec_hash() };
    let prov = Provenance {
        captured_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        source: format!("probe_launch scenario {} (lockstep, {} Hz){}", spec.name, spec.tick_hz, if warnings.is_empty() { String::new() } else { format!("; warnings: {}", warnings.join("; ")) }),
        capture_seconds: t_all.elapsed().as_secs_f64(),
    };
    let path = cache::store(&cache::root(), &key, &(prov, trace.clone()))?;
    let summary = json!({
        "scenario": spec.name,
        "spec_hash": format!("{:016x}", spec.spec_hash()),
        "cache": path.display().to_string(),
        "frames": trace.frames.len(),
        "channels": trace.channels.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
        "events": trace.events.len(),
        "map_load": load,
        "setup_s": setup_s,
        "run_s": run_s,
        "ticks_per_s": (spec.ticks as f64) / run_s.max(1e-6),
        "total_s": t_all.elapsed().as_secs_f64(),
        "warnings": warnings,
    });
    Ok((summary, trace))
}

/// `Class Pkg.TheWorld.PersistentLevel.Obj` -> `Pkg:Obj` (the oracle's object key).
fn short_path(full: &str) -> String {
    let path = full.split_once(' ').map(|x| x.1).unwrap_or(full);
    match path.split_once('.') {
        Some((pkg, rest)) => format!("{pkg}:{}", rest.trim_start_matches("TheWorld.PersistentLevel.")),
        None => path.to_string(),
    }
}
