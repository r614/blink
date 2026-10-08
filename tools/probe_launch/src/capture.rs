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
//!   `pawn.crouched` (`m_bSneakModeToggled`), `pawn.physics` (EPhysics index);
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
use dishonored_probe::{ReadPath, ScriptAction};
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
    /// A raw object pointer (hidden channels turned into events after the run).
    Ptr,
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
        Kind::Ptr => u(b) as f64,
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
    // Or one in its decoded fields (arrays of spawned pawns, structs).
    let dump = s.r.dump_object(&mut s.c, o, None)?.to_string();
    for part in dump.split("DishonoredNPCPawn ").skip(1) {
        if let Some(a) = part.split('@').nth(1).and_then(|x| crate::reflect::parse_addr(&x[..10.min(x.len())]))
            && pawns.contains(&a) {
                return Ok(a);
            }
    }
    // Or the live pawn whose spawner reference (any field) points back at it.
    for p in &pawns {
        let pc = s.r.class_of(&mut s.c, *p)?;
        let size = s.r.info(&mut s.c, pc)?.size.min(0x2000);
        if let Ok(raw) = s.c.read(*p, size)
            && raw.chunks(4).any(|w| w.len() == 4 && u32::from_le_bytes(w.try_into().unwrap()) == o) {
                return Ok(*p);
            }
    }
    bail!("{name}: neither a pawn nor a spawner with a live pawn")
}

/// Offset of a property in an object's class (or its supers).
fn field_off(s: &mut Session, obj: u32, name: &str) -> Result<u32> {
    let cls = s.r.class_of(&mut s.c, obj)?;
    Ok(s.r.field(&mut s.c, cls, name, false)?.with_context(|| format!("no property {name}"))?.offset)
}

/// The NPC's AI brain state per tick (DishonoredAIBrain via Pawn.Controller.m_pAIBrain):
/// `npc.<n>.suspicion`, `.top_attn_level`, `.top_enemy_attn_level`, the player attention entry
/// (`.player_attn.level` / `.value`), the first four attention entries (`.attn<i>.level` /
/// `.value`), and hidden pointer channels for the current behaviour and the 19 behaviour-stack
/// slots (turned into `NpcBehavior` events).
fn add_brain_channels(s: &mut Session, ch: &mut Channels, name: &str, np: u32) -> Result<()> {
    let ctrl_off = field_off(s, np, "Controller")?;
    let ctrl = s.c.read_u32(np + ctrl_off)?;
    anyhow::ensure!(ctrl != 0, "no controller");
    let brain_off = field_off(s, ctrl, "m_pAIBrain")?;
    let brain = s.c.read_u32(ctrl + brain_off)?;
    anyhow::ensure!(brain != 0, "no brain");
    let b = |s: &mut Session, f: &str| field_off(s, brain, f);
    let (susp, top, top_enemy, cur, stack, attn) = (b(s, "m_SuspicionLevel")?, b(s, "m_TopAttnTargetAttentionLevel")?, b(s, "m_TopEnemyAttnTargetAttentionLevel")?, b(s, "m_pCurrentBehavior")?, b(s, "m_ActiveBehaviorStack")?, b(s, "m_pAttentionProcess")?);
    let path = |offs: Vec<u32>, len: u32| Some(ReadPath { base: np, offsets: offs, len });
    let mut push = |name: String, unit: &str, offs: Vec<u32>, kind: Kind| ch.list.push(Chan { name, unit: unit.into(), path: path(offs, len_of(kind)), kind, speed_of: None });
    push(format!("npc.{name}.suspicion"), "enum", vec![ctrl_off, brain_off, susp], Kind::U8);
    push(format!("npc.{name}.top_attn_level"), "enum", vec![ctrl_off, brain_off, top], Kind::U8);
    push(format!("npc.{name}.top_enemy_attn_level"), "enum", vec![ctrl_off, brain_off, top_enemy], Kind::U8);
    push(format!("npc.{name}.brain.current"), "ptr", vec![ctrl_off, brain_off, cur], Kind::Ptr);
    for k in 0..19u32 {
        push(format!("npc.{name}.brain.slot{k}"), "ptr", vec![ctrl_off, brain_off, stack + 4 * k], Kind::Ptr);
    }
    let proc_ = s.c.read_u32(brain + attn)?;
    if proc_ != 0 {
        let infos = field_off(s, proc_, "m_AttentionInfos")?;
        let player = field_off(s, proc_, "m_pPlayerAttentionInfo")?;
        // DisAttentionInfo_Complex: m_AttentionAccumulator.m_fCurrentValue +0x8C, level +0xAC.
        push(format!("npc.{name}.player_attn.level"), "enum", vec![ctrl_off, brain_off, attn, player, 0xac], Kind::U8);
        push(format!("npc.{name}.player_attn.value"), "", vec![ctrl_off, brain_off, attn, player, 0x8c], Kind::F32);
        push(format!("npc.{name}.attn_count"), "", vec![ctrl_off, brain_off, attn, infos + 4], Kind::I32);
        for i in 0..4u32 {
            push(format!("npc.{name}.attn{i}.level"), "enum", vec![ctrl_off, brain_off, attn, infos, 4 * i, 0xac], Kind::U8);
            push(format!("npc.{name}.attn{i}.value"), "", vec![ctrl_off, brain_off, attn, infos, 4 * i, 0x8c], Kind::F32);
        }
    }
    Ok(())
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
    // A layout map (maps/<map>.ron, e.g. the Playground) is built in the host level at an
    // offset; positions in and out of the trace are the layout's own.
    let layout = crate::playground::layout_file(&spec.map);
    let off = if layout.is_some() { glam::Vec3::from(crate::playground::DEFAULT_OFFSET) } else { glam::Vec3::ZERO };
    if let Some(file) = &layout {
        let l = crate::playground::parse_layout(&std::fs::read_to_string(file)?)?;
        let (lt, c) = s.open_map(crate::playground::host_map(), Duration::from_secs(600))?;
        s.setup(spec.tick_hz as f64, spec.setup.keep_alive)?;
        let built = s.build_layout(&l, off)?;
        if let Some(errs) = built["errors"].as_array().filter(|e| !e.is_empty()) {
            warnings.push(format!("layout: {} parts not built: {}", errs.len(), errs.iter().filter_map(|e| e.as_str()).collect::<Vec<_>>().join("; ")));
        }
        warnings.push("layout: lights, labels and the nav grid are not built in the original (U-806)".into());
        load = json!({"load_s": lt, "control_s": c, "layout": built});
    } else if reload || !cur.eq_ignore_ascii_case(&spec.map) {
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
        // Port item names -> the original's inventory tweaks (a weapon tweak brings its ammo).
        let name = it.split(':').next().unwrap_or(it);
        let tweaks: &[&str] = match name {
            n if n.starts_with("Twk_") => &[],
            "FullAmmo" => &["Twk_Inv_PistolCorvo", "Twk_Inv_CrossbowCorvo", "Twk_Inv_GrenadeCorvo", "Twk_Inv_SpringRazorCorvo"],
            "Bullet" => &["Twk_Inv_PistolCorvo"],
            "Bolt" => &["Twk_Inv_CrossbowCorvo"],
            "Grenade" => &["Twk_Inv_GrenadeCorvo"],
            "SpringRazor" => &["Twk_Inv_SpringRazorCorvo"],
            "Reset" => &[],
            _ => {
                gap(format!("item {it} has no mapping to an original tweak"), &mut warnings)?;
                &[]
            }
        };
        if name.starts_with("Twk_") {
            crate::lab::give(&mut s.c, name, true)?;
        }
        for t in tweaks {
            crate::lab::give(&mut s.c, t, true)?;
        }
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
    // NPCs: an existing pawn (by spawner / pawn name) or, for a placement by position, one of the
    // host level's live NPCs of the kind (guards: `*Guard*` tweaks, civilians: the others)
    // teleported there. `Passive` unpossesses it (no AI: a dummy that takes damage); `Idle`
    // keeps its own AI; other states, factions, routes and tweaks are not applied yet (U-802).
    let mut placed: HashMap<String, u32> = HashMap::new();
    let mut used: Vec<u32> = Vec::new();
    for (k, n) in spec.setup.npcs.iter().enumerate() {
        let id = n.id.clone().or_else(|| n.spawner.clone()).unwrap_or_else(|| format!("npc{k}"));
        let pawn = match &n.spawner {
            Some(name) => npc_pawn(s, name)?,
            None => {
                let guard = n.kind.as_deref().unwrap_or("Guard").eq_ignore_ascii_case("Guard");
                let cand = crate::lab::npcs(&mut s.c)?.into_iter().find(|c| c.health > 0 && !used.contains(&c.addr) && c.tweak.contains("Guard") == guard && !c.tweak.contains("Tallboy") && !c.tweak.contains("Boatman") && !c.tweak.contains("LordShaw"));
                match cand {
                    Some(c) => c.addr,
                    None => {
                        gap(format!("NPC {id}: no free live {} in the host level to place", if guard { "guard" } else { "civilian" }), &mut warnings)?;
                        continue;
                    }
                }
            }
        };
        used.push(pawn);
        if n.tweak.is_some() {
            gap(format!("NPC {id}: tweak {:?} not applied (the host level's pawn keeps its own)", n.tweak), &mut warnings)?;
        }
        if !n.route.is_empty() {
            gap(format!("NPC {id}: patrol route not applied"), &mut warnings)?;
        }
        if let Some(fac) = &n.faction {
            // DishonoredNPCPawn.m_pCurFactionTweak: a faction tweak by name, or none.
            let tw = if fac == "None" { 0 } else {
                let name = fac.rsplit('.').next().unwrap_or(fac);
                match s.c.find(name, "")?.first() {
                    Some(a) => *a,
                    None => {
                        gap(format!("NPC {id}: faction tweak {fac} not loaded"), &mut warnings)?;
                        u32::MAX
                    }
                }
            };
            if tw != u32::MAX {
                let pl = s.r.place(&mut s.c, pawn, "m_pCurFactionTweak")?;
                s.c.write(pl.addr, &tw.to_le_bytes())?;
            }
        }
        if let Some(p) = n.position {
            let at = glam::Vec3::from(p) + off;
            let pl = s.r.place(&mut s.c, pawn, "Location")?;
            s.r.write_place(&mut s.c, &pl, &json!([at.x, at.y, at.z]))?;
            let pl = s.r.place(&mut s.c, pawn, "Velocity")?;
            s.r.write_place(&mut s.c, &pl, &json!([0, 0, 0]))?;
            let yaw = ((n.yaw / 360.0 * 65536.0).round() as i32) & 0xffff;
            s.c.write(pawn + 0xd4, &yaw.to_le_bytes())?;
            let _ = s.r.call(&mut s.c, pawn, "SetLocation", &json!([[at.x, at.y, at.z]]));
        }
        match n.state.as_deref() {
            None | Some("Idle") => {}
            Some("Passive") => {
                let co = field_off(s, pawn, "Controller")?;
                let ctrl = s.c.read_u32(pawn + co)?;
                if ctrl != 0 {
                    s.r.call(&mut s.c, ctrl, "UnPossess", &Value::Null)?;
                }
            }
            Some("Alerted") => {
                // Approximation: the guard's attention on the player filled (1.0); its own
                // attention process then raises the level and starts combat.
                let info = crate::oracle_stealth::attention_info(&mut s.c, pawn)?;
                s.c.write(info + 0x8c, &1.0f32.to_le_bytes())?;
                warnings.push(format!("NPC {id}: Alerted approximated by filling its player attention (1.0)"));
            }
            Some(st) => gap(format!("NPC {id}: AI state {st} not applied"), &mut warnings)?,
        }
        placed.insert(id, pawn);
    }
    let p = spec.setup.player.clone().unwrap_or_default();
    if let Some(ps) = &spec.setup.player {
        if ps.sword_drawn {
            crate::lab::give(&mut s.c, "Twk_Inv_SwordCorvo", true)?;
        }
        s.place((glam::Vec3::from(ps.position) + off).to_array(), Some(ps.yaw), Some(ps.pitch), ps.snap_to_floor)?;
        if spec.setup.settle > 0 {
            // A walking pawn does not look for its floor until it moves: start the settle
            // falling (PHYS_Falling written directly; `SetPhysics` is ignored for the player)
            // so it lands on its floor and hovers like the port's settled pawn.
            let pawn = crate::lab::pawn(&mut s.c)?;
            let pl = s.r.place(&mut s.c, pawn, "Physics")?;
            s.r.write_place(&mut s.c, &pl, &json!("PHYS_Falling"))?;
        }
        if let Some(h) = ps.health {
            let pawn = crate::lab::pawn(&mut s.c)?;
            s.c.write(pawn + 0x344, &(h.round() as i32).to_le_bytes())?;
        }
        if let Some(m) = ps.mana {
            let pawn = crate::lab::pawn(&mut s.c)?;
            s.c.write(pawn + crate::oracle_powers::PAWN_MANA, &(m.round() as i32).to_le_bytes())?;
        }
    }
    // The left hand at the start: a power (DisItemPowers on its component) or a weapon.
    if let Some(h) = &spec.setup.left_hand {
        let power = match h.as_str() {
            "Blink" => Some("DishonoredActivePowerComponent_Blink"),
            "Windblast" | "WindBlast" => Some("DishonoredActivePowerComponent_WindBlast"),
            "DarkVision" => Some("DisActivePowerComponent_DarkVision"),
            "Possession" | "Possess" => Some("DishonoredActivePowerComponent_Possess"),
            "BendTime" => Some("DishonoredActivePowerComponent_BendTime"),
            "DevouringSwarm" => Some("DishonoredActivePowerComponent_DevouringSwarm"),
            _ => None,
        };
        let weapon = match h.as_str() {
            "Pistol" => Some(("Twk_Inv_PistolCorvo", "DishonoredWepPistol")),
            "Crossbow" | "SleepDart" | "IncendiaryBolt" => Some(("Twk_Inv_CrossbowCorvo", "DisWepCrossbow")),
            "Grenade" | "StickyGrenade" => Some(("Twk_Inv_GrenadeCorvo", "DisWepGrenade")),
            "SpringRazor" => Some(("Twk_Inv_SpringRazorCorvo", "DisGadget_SpringRazor")),
            _ => None,
        };
        if let Some(comp) = power {
            crate::oracle_powers::select_power(&mut s.c, comp)?;
        } else if let Some((tw, cls)) = weapon {
            crate::lab::give(&mut s.c, tw, true)?;
            crate::lab::equip(&mut s.c, cls, 2)?;
        } else {
            gap(format!("left hand {h}"), &mut warnings)?;
        }
    }
    for st in &spec.input {
        if matches!(st.action, Action::Hand(_) | Action::Drink(_)) {
            gap(format!("input {:?} at tick {} (hand switches and drinks are not scripted on the original yet)", st.action, st.tick), &mut warnings)?;
        }
    }
    // `settle`: ticks run before tick 0 (the pawn lands and equips).
    if spec.setup.settle > 0 {
        s.c.lockstep(true, 30_000)?;
        s.c.step(spec.setup.settle, 600_000)?;
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
        // Dishonored's sneak is its own toggle (`bIsCrouched` stays false while sneaking).
        ch.add_path(s, "pawn.crouched", "bool", pawn, "m_bSneakModeToggled")?;
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
            let np = match placed.get(name) {
                Some(p) => *p,
                None => npc_pawn(s, name)?,
            };
            ch.add_path(s, &format!("npc.{name}"), "uu", np, "Location")?;
            ch.add_path(s, &format!("npc.{name}.health"), "pt", np, "Health")?;
            ch.add_path(s, &format!("npc.{name}.physics"), "enum", np, "Physics")?;
            if let Err(e) = add_brain_channels(s, &mut ch, name, np) {
                warnings.push(format!("npc {name}: no AI brain channels ({e:#})"));
            }
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
    let to_values = |row: &[Option<Vec<u8>>]| -> Vec<f64> {
        let mut v = vec![f64::NAN; ch.list.len()];
        for ((i, _), b) in paths.iter().zip(row) {
            if let Some(b) = b {
                v[*i] = value(ch.list[*i].kind, b);
            }
        }
        for (i, c) in ch.list.iter().enumerate() {
            if let Some((a, b)) = c.speed_of {
                v[i] = (v[a] * v[a] + v[b] * v[b]).sqrt();
            }
        }
        v
    };
    s.c.lockstep(true, 30_000)?;
    if tracing {
        s.c.event_fetch()?; // drop what the setup produced
    }
    if spawns {
        s.c.spawn_fetch()?;
    }
    // The input as a tick script: key changes, view writes, console commands per tick.
    let timeline = spec.input_timeline();
    let mut held: Vec<&'static str> = Vec::new();
    let (mut yaw, mut pitch) = (p.yaw, p.pitch);
    let rot = |deg: f32| ((deg / 360.0 * 65536.0).round() as i32) & 0xffff;
    let mut actions: Vec<(u32, ScriptAction)> = Vec::new();
    for (t, (fw, rt, y, pt, buttons)) in timeline.iter().enumerate() {
        let t = t as u32;
        let mut want_keys = move_keys(*fw, *rt)?;
        for b in buttons {
            want_keys.push(button_key(*b)?);
        }
        for k in held.iter().filter(|k| !want_keys.contains(k)) {
            actions.push((t, ScriptAction::Key(k.to_string(), false)));
        }
        for k in want_keys.iter().filter(|k| !held.contains(k)) {
            actions.push((t, ScriptAction::Key(k.to_string(), true)));
        }
        held = want_keys;
        if (*y, *pt) != (yaw, pitch) || t == 0 {
            (yaw, pitch) = (*y, *pt);
            actions.push((t, ScriptAction::Write(pc + 0xd0, rot(pitch).to_le_bytes().to_vec())));
            actions.push((t, ScriptAction::Write(pc + 0xd4, rot(yaw).to_le_bytes().to_vec())));
        }
    }
    for st in &spec.input {
        if let Action::Console(c) = &st.action {
            actions.push((st.tick, ScriptAction::Console(c.clone())));
        }
    }
    let t_run = Instant::now();
    let (base, frames, errors) = s.c.run_script(spec.ticks, &read_paths, &actions, 3_600_000)?;
    let run_s = t_run.elapsed().as_secs_f64();
    if !errors.is_empty() {
        warnings.push(format!("script: {errors}"));
    }
    for (k, row) in frames.iter().enumerate() {
        trace.push(k as u32, to_values(row));
    }
    // Back to the layout's coordinates.
    if off != glam::Vec3::ZERO {
        for (i, c) in trace.channels.clone().iter().enumerate() {
            let is_pos = c.name.starts_with("pawn.") || c.name.starts_with("camera.") || c.name.starts_with("npc.");
            let k = match c.name.rsplit('.').next() {
                Some("x") if is_pos => 0,
                Some("y") if is_pos => 1,
                Some("z") if is_pos => 2,
                _ => continue,
            };
            for fr in trace.frames.iter_mut() {
                fr.values[i] -= off[k] as f64;
            }
        }
    }
    let raw_events = if tracing { s.c.event_fetch()? } else { Vec::new() };
    let raw_spawns = if spawns { s.c.spawn_fetch()? } else { Vec::new() };
    // world.dt: the game time each tick advanced (1 / tick_hz under the fixed step).
    if let Some(ti) = trace.channel("world.time") {
        trace.channels.push(dishonored_oracle::trace::Channel { name: "world.dt".into(), unit: "s".into() });
        let mut prev = f64::NAN;
        for fr in trace.frames.iter_mut() {
            let t = fr.values[ti];
            fr.values.push(t - prev);
            prev = t;
        }
        let stalled = trace.frames.iter().skip(1).filter(|f| f.values.last().is_some_and(|d| *d <= 0.0)).count();
        if stalled > 0 {
            warnings.push(format!("the world clock did not advance on {stalled} ticks (paused?): this trace is not valid"));
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
    // Hidden pointer channels -> `NpcBehavior` events (the class of the current behaviour and
    // of each occupied slot of the active stack, whenever it changes), then dropped.
    let hidden: Vec<usize> = trace.channels.iter().enumerate().filter(|(_, c)| c.unit == "ptr").map(|(i, _)| i).collect();
    let mut class_names: HashMap<u32, String> = HashMap::new();
    for &i in &hidden {
        let cname = trace.channels[i].name.clone();
        let (npc, what) = cname.strip_prefix("npc.").and_then(|r| r.split_once(".brain.")).map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default();
        let mut prev = f64::NAN;
        for fr in &trace.frames {
            let v = fr.values[i];
            if v == prev || (v.is_nan() && prev.is_nan()) {
                continue;
            }
            let first = prev.is_nan() && fr.tick == trace.frames[0].tick;
            prev = v;
            let obj = if v.is_nan() { 0 } else { v as u32 };
            if first && obj == 0 {
                continue; // an empty slot at the start
            }
            let cls = if obj == 0 {
                "None".to_string()
            } else {
                let cl = s.c.read_u32(obj + 0x30).unwrap_or(0);
                class_names.entry(cl).or_insert_with(|| s.c.name_of(cl).ok().map(|n| n.rsplit('.').next().unwrap_or("").to_string()).unwrap_or_default()).clone()
            };
            trace.events.push(TraceEvent { tick: fr.tick, kind: "NpcBehavior".into(), object: format!("npc.{npc}"), name: cls, detail: what.clone() });
        }
    }
    for &i in hidden.iter().rev() {
        trace.channels.remove(i);
        for fr in trace.frames.iter_mut() {
            fr.values.remove(i);
        }
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

/// The `run` session op: a tick script with arbitrary channels, keys and console commands,
/// answered as JSON. `{"ticks": N, "channels": {"mana": "pawn:m_Mana", "pos": "pawn:Location"},
/// "keys": [[tick, "RightMouseButton", true], ...], "console": [[tick, "cmd"], ...],
/// "events": {"functions": [..], "classes": [..]}, "spawns": true, "every": k}`. Frame k is the
/// end-of-tick state (ORACLE.md "Sampling point"); a Vector / Rotator adds .x/.y/.z
/// (.pitch/.yaw/.roll) channels.
pub fn run_reads(s: &mut Session, a: &Value) -> Result<Value> {
    let ticks = a.get("ticks").and_then(Value::as_u64).context("ticks")? as u32;
    let every = a.get("every").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
    let mut ch = Channels { list: Vec::new() };
    if let Some(m) = a.get("channels").and_then(Value::as_object) {
        for (name, spec) in m {
            let spec = spec.as_str().context("channel spec \"object:path\"")?;
            let (o, p) = spec.split_once(':').context("channel spec \"object:path\"")?;
            let obj = s.r.object(&mut s.c, o, None)?;
            ch.add_path(s, name, "", obj, p)?;
        }
    }
    let wi = s.r.object(&mut s.c, "worldinfo", None)?;
    ch.add_path(s, "world.time", "s", wi, "TimeSeconds")?;
    let mut actions: Vec<(u32, ScriptAction)> = Vec::new();
    for k in a.get("keys").and_then(Value::as_array).into_iter().flatten() {
        let t = k.get(0).and_then(Value::as_u64).context("key [tick, name, down]")? as u32;
        let n = k.get(1).and_then(Value::as_str).context("key [tick, name, down]")?;
        actions.push((t, ScriptAction::Key(n.into(), k.get(2).and_then(Value::as_bool).unwrap_or(true))));
    }
    for k in a.get("console").and_then(Value::as_array).into_iter().flatten() {
        let t = k.get(0).and_then(Value::as_u64).context("console [tick, cmd]")? as u32;
        actions.push((t, ScriptAction::Console(k.get(1).and_then(Value::as_str).context("console [tick, cmd]")?.into())));
    }
    let ev = a.get("events");
    let strs = |k: &str| -> Vec<String> { ev.and_then(|e| e.get(k)).and_then(Value::as_array).map(|v| v.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    let (funcs, classes) = (strs("functions"), strs("classes"));
    let tracing = !funcs.is_empty() || !classes.is_empty();
    let spawns = a.get("spawns").and_then(Value::as_bool).unwrap_or(false);
    s.c.lockstep(true, 30_000)?;
    if tracing {
        s.c.event_filter(&funcs, &classes, &[], ev.and_then(|e| e.get("parms")).and_then(Value::as_u64).unwrap_or(0) as u32, 1_000_000)?;
    }
    if spawns {
        s.c.spawn_watch(100_000)?;
    }
    // "damage": true logs every call of the pawn damage path (0x00B5CCD0, oracle.rs).
    let damage = a.get("damage").and_then(Value::as_bool).unwrap_or(false);
    let dh = if damage { Some(crate::oracle_combat::damage_hook(&mut s.c)?) } else { None };
    let paths: Vec<ReadPath> = ch.list.iter().filter_map(|c| c.path.clone()).collect();
    let (base, frames, errors) = s.c.run_script(ticks, &paths, &actions, 3_600_000)?;
    let damage_recs = match dh {
        Some(h) => {
            let r = s.c.hook_fetch(h)?;
            let f = crate::oracle::FN_PAWN_DAMAGE;
            let _ = s.c.hook_set(f.0, f.1, 0, 0, &[]);
            r
        }
        None => Vec::new(),
    };
    let mut out_frames = Vec::new();
    for (k, row) in frames.iter().enumerate() {
        if k % every != 0 && k + 1 != frames.len() {
            continue;
        }
        let mut v = vec![json!(k)];
        for (c, b) in ch.list.iter().zip(row) {
            v.push(b.as_ref().map(|b| json!(value(c.kind, b))).unwrap_or(Value::Null));
        }
        out_frames.push(Value::Array(v));
    }
    let mut out = json!({"base_tick": base, "channels": ch.list.iter().map(|c| c.name.clone()).collect::<Vec<_>>(), "frames": out_frames, "errors": errors});
    if damage {
        // The frame whose world time first reaches the call's time is the tick it happened in.
        let ti = ch.list.iter().position(|c| c.name == "world.time").unwrap();
        let times: Vec<f64> = frames.iter().map(|row| row[ti].as_ref().map(|b| value(Kind::F32, b)).unwrap_or(f64::NAN)).collect();
        let mut v = Vec::new();
        for r in &damage_recs {
            let tick = times.iter().position(|t| *t + 1e-4 >= r.time as f64).unwrap_or(times.len());
            let n = s.c.names_of(&[r.ecx, r.args.get(1).copied().unwrap_or(0), r.args.get(6).copied().unwrap_or(0)])?;
            v.push(json!({
                "tick": tick, "victim": short_path(&n[0]), "amount": r.u32_at(0) as i32, "health_before": r.u32_at(16) as i32,
                "momentum": [r.f32_at(4), r.f32_at(8), r.f32_at(12)], "instigator": short_path(&n[1]), "type": n[2].rsplit('.').next().unwrap_or(""),
            }));
        }
        out["damage"] = Value::Array(v);
    }
    if tracing {
        let recs = s.c.event_fetch()?;
        s.c.event_filter(&[], &[], &[], 0, 0)?;
        let mut v = Vec::new();
        for r in recs {
            let n = s.c.names_of(&[r.object, r.func])?;
            v.push(json!({"tick": r.tick.wrapping_sub(base), "object": short_path(&n[0]), "function": n[1].rsplit(' ').next().unwrap_or(""), "parms": r.parms.iter().map(|b| format!("{b:02x}")).collect::<String>()}));
        }
        out["events"] = Value::Array(v);
    }
    if spawns {
        let recs = s.c.spawn_fetch()?;
        s.c.spawn_watch(0)?;
        let mut v = Vec::new();
        for r in recs {
            let n = s.c.names_of(&[r.object])?;
            v.push(json!({"tick": r.tick.wrapping_sub(base), "object": format!("{}@{}", n[0], crate::reflect::hex(r.object))}));
        }
        out["spawns"] = Value::Array(v);
    }
    Ok(out)
}