//! `harness save [--mission FILE]`: saving and loading a mission like the original
//! (research/dishonored/SAVES.md), on the original Boyle mission run by its own Kismet
//! (`missions/boyle_original.ron`).
//!
//! A scripted agent plays the mission headless (the same stages as `harness kismet`: the
//! street, the estate entrance, the garden door into the ballroom, the guilty sister's note
//! and her death, back out to the garden, the canal path). A reference run plays it without
//! interruption and records the state hashes. Then, for each save point, a fresh run plays to
//! the point, saves to a file, loads the file into a new match built from the mission's setup
//! (the way a reload rebuilds it) and continues: every tick's world hash and the full match
//! hash (everything a save holds) must match the reference. The save points cover a save
//! made inside the mansion (the street's level state inside the save; the continuation
//! travels back out) and a save made in the street before going in (the continuation
//! travels into the mansion). The run also writes the autosaves the mission takes (each map
//! load, `DisSeqAct_AutoSave`) into the original's slots and checks the slot rotation.

use crate::Ctx;
use crate::report::Report;
use anyhow::{Context, Result};
use dishonored_content::MapContents;
use dishonored_kismet::{KismetDb, Obj};
use dishonored_mission::{MissionDef, TaskState};
use dishonored_multiplayer::game::{Game, GameSetup};
use dishonored_multiplayer::mission::{MissionMatch, MissionSetup, kismet_host, save};
use dishonored_save::slot;
use dishonored_sim::input::{PlayerInput, buttons};
use glam::Vec3;
use std::path::PathBuf;

const S: &str = "Save / load";
const AGENT: u32 = 1;
const BA: &str = "BoyleAssassination";

/// Hashes along a run: (tick, world hash) every tick, (tick, full match hash) every second
/// and at stage ends.
#[derive(Default, Clone)]
struct Trail {
    world: Vec<(u64, u64)>,
    full: Vec<(u64, u64)>,
}

struct Play<'a> {
    g: Game,
    maps: &'a MapContents,
    tick: u64,
    trail: Trail,
    /// Write the saves the mission takes into this folder.
    autosave_dir: Option<PathBuf>,
    autosaves: Vec<(i32, String, u64)>,
    requests: usize,
}

impl<'a> Play<'a> {
    fn new(g: Game, maps: &'a MapContents) -> Self {
        Play { g, maps, tick: 0, trail: Trail::default(), autosave_dir: None, autosaves: Vec::new(), requests: 0 }
    }
    fn m(&self) -> &MissionMatch {
        match &self.g {
            Game::Mission(m) => m,
            _ => unreachable!(),
        }
    }
    fn mm(&mut self) -> &mut MissionMatch {
        match &mut self.g {
            Game::Mission(m) => m,
            _ => unreachable!(),
        }
    }

    fn step(&mut self, input: PlayerInput) {
        self.tick += 1;
        let sim = &self.maps.maps[0].sim;
        let inp = PlayerInput { tick: self.tick, ..input };
        self.g.step(&[(AGENT, inp)], &self.maps.set, sim);
        if let Some(cm) = self.g.current_map() {
            self.maps.set.set_active(cm);
        }
        // The host writes the saves the mission took this step (autosaves, mission starts).
        let reqs = save::take_requests(self.mm());
        self.requests += reqs.len();
        if let Some(dir) = self.autosave_dir.clone() {
            for r in reqs {
                let s = match r {
                    save::SaveRequest::Autosave => dishonored_save::next_autosave_slot(&dishonored_save::list(&dir)),
                    save::SaveRequest::MissionStart(s) => s,
                };
                if let Ok(bytes) = save::encode(self.m(), s, "missions/boyle_original.ron") {
                    let _ = dishonored_save::write_file(&dishonored_save::slot_path(&dir, s), &bytes);
                    self.autosaves.push((s, self.m().current_map().to_string(), self.tick));
                }
            }
        }
        let t = self.tick;
        let wh = dishonored_sim::state_hash(&self.m().world);
        self.trail.world.push((t, wh));
        if t % 60 == 0 {
            let fh = save::state_hash(self.m());
            self.trail.full.push((t, fh));
        }
    }

    fn idle(&mut self, secs: f32) {
        for _ in 0..(secs * 60.0) as u32 {
            self.step(PlayerInput::default());
        }
    }

    fn until(&mut self, secs: f32, f: impl Fn(&MissionMatch) -> bool) -> bool {
        for _ in 0..(secs * 60.0) as u32 {
            if f(self.m()) {
                return true;
            }
            self.step(PlayerInput::default());
        }
        f(self.m())
    }

    fn place_near(&mut self, pos: Vec3, dist: f32) {
        let map = self.m().current_map().to_string();
        let world = self.maps.get(&map).map(|m| m.collision.clone());
        let mut best = pos + Vec3::new(dist, 0.0, 60.0);
        if let Some(w) = &world {
            for i in 0..12 {
                let a = i as f32 * std::f32::consts::TAU / 12.0;
                let c = pos + Vec3::new(a.cos() * dist, a.sin() * dist, 0.0);
                if let Some(f) = dishonored_ai::snap_floor(w, c + Vec3::Z * 40.0) {
                    if (f.z - pos.z).abs() < 200.0 {
                        best = f;
                        break;
                    }
                }
            }
        }
        let mv = self.maps.maps[0].sim.gameplay.movement.clone();
        let me = self.mm().world.player_mut(AGENT).expect("agent");
        me.position = best;
        me.velocity = Vec3::ZERO;
        (me.yaw, me.pitch) = dishonored_sim::crosshair::aim_at(me, &mv, pos);
    }

    fn use_actor(&mut self, a: u32) -> bool {
        let list = kismet_host::usables(self.m());
        let Some((_, pos, _)) = list.iter().find(|u| matches!(u.0, kismet_host::KUse::Actor(x) | kismet_host::KUse::Pickup(x) if x == a)).cloned() else { return false };
        self.place_near(pos, 90.0);
        let (yaw, pitch) = {
            let me = self.m().world.player(AGENT).unwrap();
            (me.yaw, me.pitch)
        };
        self.step(PlayerInput { yaw, pitch, ..Default::default() });
        self.step(PlayerInput { yaw, pitch, buttons: buttons::USE, ..Default::default() });
        self.step(PlayerInput { yaw, pitch, ..Default::default() });
        true
    }

    fn choose(&mut self, option: usize) {
        self.step(PlayerInput { buttons: buttons::CHOICES[option], ..Default::default() });
        self.step(PlayerInput::default());
    }

    fn enter(&mut self, a: u32) {
        let pos = {
            let k = &self.m().kismet.as_ref().unwrap().k;
            let act = &k.db.actors[a as usize];
            match &act.shape {
                Some(dishonored_kismet::Shape::Boxes(b)) if !b.is_empty() => {
                    let c = [(b[0][0][0] + b[0][1][0]) * 0.5, (b[0][0][1] + b[0][1][1]) * 0.5, b[0][0][2] + 60.0];
                    Vec3::from(act.location) + Vec3::new(c[0] * act.scale[0], c[1] * act.scale[1], c[2] * act.scale[2])
                }
                _ => Vec3::from(act.location),
            }
        };
        let me = self.mm().world.player_mut(AGENT).expect("agent");
        me.position = pos;
        me.velocity = Vec3::ZERO;
    }

    fn mark(&mut self) {
        let fh = save::state_hash(self.m());
        self.trail.full.push((self.tick, fh));
    }
}

fn actor(db: &KismetDb, r: &str) -> Result<u32> {
    db.find_actor(r).with_context(|| format!("actor {r}"))
}

fn var(m: &MissionMatch, pkg: &str, name: &str) -> Option<dishonored_kismet::Value> {
    let k = &m.kismet.as_ref()?.k;
    let db = &k.db;
    let p = db.packages.iter().position(|x| x.eq_ignore_ascii_case(pkg))? as u16;
    let v = db.ops.iter().find(|o| o.package == p && o.kind == dishonored_kismet::Kind::Variable && o.props.str("VarName").eq_ignore_ascii_case(name))?;
    let mut probe = k.prop(v.id, "bValue").map(|x| dishonored_kismet::Value::Bool(x.as_int().unwrap_or(0) != 0));
    if probe.is_none() {
        probe = k.prop(v.id, "ObjValue");
    }
    probe
}

fn guilty(m: &MissionMatch) -> Option<&'static str> {
    ["Esma", "Lydia", "Waverly"].into_iter().find(|n| matches!(var(m, "L_Boyle_Int_Script", &format!("{n} Guilty")), Some(dishonored_kismet::Value::Bool(true))))
}

const STAGES: &[&str] = &[
    "mission start: Samuel's intro, objectives",
    "estate entrance trigger",
    "garden door into the ballroom (travel Ext -> Int)",
    "interior setup (guilty sister), 3 s",
    "the guilty sister's note",
    "the guilty sister killed",
    "ballroom door back to the garden (travel Int -> Ext)",
    "canal path trigger (T10)",
];

/// One stage of the scripted run.
fn stage(p: &mut Play, db: &KismetDb, i: usize) -> Result<()> {
    match i {
        0 => {
            let t12 = format!("{BA}.DishonoredTask_Custom_12");
            p.until(90.0, |m| m.state.task(&t12) == Some(TaskState::Active));
        }
        1 => {
            p.enter(actor(db, "L_Boyle_Ext_Script:TriggerVolume_14")?);
            let t1 = format!("{BA}.DishonoredTask_Custom_1");
            p.until(5.0, |m| m.state.task(&t1) == Some(TaskState::Active));
            p.idle(1.0);
        }
        2 => {
            let door = actor(db, "L_Boyle_Ext_P:DishonoredUsableObject_7")?;
            p.use_actor(door);
            p.idle(0.2);
            p.choose(0);
            p.until(5.0, |m| m.current_map().eq_ignore_ascii_case("L_Boyle_Int_P"));
        }
        3 => {
            p.until(10.0, |m| guilty(m).is_some());
            p.idle(3.0);
        }
        4 => {
            let sister = guilty(p.m()).unwrap_or("Esma");
            let note = ["Red", "White", "Black"].iter().find_map(|c| match var(p.m(), "L_Boyle_Int_Script", &format!("{sister}{c}Note")) {
                Some(dishonored_kismet::Value::Obj(Obj::Actor(a))) if p.m().kismet.as_ref().is_some_and(|k| k.st.actor_pos.contains_key(&a)) => Some(a),
                _ => None,
            });
            if let Some(a) = note {
                p.use_actor(a);
            }
            p.idle(1.0);
        }
        5 => {
            let sister = guilty(p.m()).unwrap_or("Esma");
            let spawner = match sister {
                "Esma" => "L_Boyle_Int_Script:DishonoredSpawner_2",
                "Lydia" => "L_Boyle_Int_Script:DishonoredSpawner_11",
                _ => "L_Boyle_Int_Script:DishonoredSpawner_4",
            };
            let sa = actor(db, spawner)?;
            let pawn = p.m().kismet.as_ref().and_then(|k| k.k.spawned.get(&sa).and_then(|v| v.first().copied()));
            if let Some(id) = pawn {
                for i in 0..(40 * 60) {
                    let Some(t) = p.m().world.player(id).cloned() else { break };
                    if !t.alive {
                        break;
                    }
                    let back = Vec3::new(t.yaw.to_radians().cos(), t.yaw.to_radians().sin(), 0.0);
                    let spot = t.position - back * 60.0;
                    {
                        let me = p.mm().world.player_mut(AGENT).unwrap();
                        me.position = spot;
                        me.velocity = Vec3::ZERO;
                    }
                    let to = t.position - spot;
                    p.step(PlayerInput { yaw: to.y.atan2(to.x).to_degrees(), pitch: -5.0, buttons: if i % 20 == 0 { buttons::PRIMARY } else { 0 }, ..Default::default() });
                }
            }
            p.idle(2.0);
        }
        6 => {
            p.idle(1.0);
            let door = actor(db, "L_Boyle_Int_P:DishonoredUsableObject_10")?;
            p.use_actor(door);
            p.idle(0.2);
            p.choose(0);
            p.until(5.0, |m| m.current_map().eq_ignore_ascii_case("L_Boyle_Ext_P"));
            p.idle(2.0);
        }
        7 => {
            p.enter(actor(db, "L_BoyleStreet_Ext_Script:TriggerVolume_23")?);
            let t10 = format!("{BA}.DishonoredTask_Custom_10");
            p.until(5.0, |m| m.state.task(&t10) == Some(TaskState::Active));
            p.idle(3.0);
        }
        _ => {}
    }
    p.mark();
    Ok(())
}

fn fresh<'a>(setup: &GameSetup, maps: &'a MapContents, def: &MissionDef) -> Result<Play<'a>> {
    let ext = maps.get(&def.map).context("start map")?;
    maps.set.set_active(&def.map);
    let mut p = Play::new(setup.build(&ext.sim), maps);
    p.g.add_player(AGENT, "agent", 0, &maps.set, &ext.sim);
    Ok(p)
}

/// First difference between a continuation and the reference (None: identical).
fn diverge(a: &[(u64, u64)], reference: &[(u64, u64)]) -> (usize, Option<(u64, u64, u64)>) {
    let mut n = 0;
    for x in a {
        if let Some(r) = reference.iter().find(|r| r.0 == x.0) {
            n += 1;
            if r.1 != x.1 {
                return (n, Some((x.0, x.1, r.1)));
            }
        }
    }
    (n, None)
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let path = ctx.opt("--mission").unwrap_or_else(|| "missions/boyle_original.ron".into());
    let def = MissionDef::load(std::path::Path::new(&path))?;
    let maps = MapContents::load(&dishonored_content::mission_maps(&def))?;
    let setup = dishonored_content::mission_setup(&maps, def.clone()).map_err(|e| anyhow::anyhow!(e))?;
    let GameSetup::Mission(ms) = &setup else { anyhow::bail!("not a mission") };
    let ms: MissionSetup = (**ms).clone();
    let db = ms.kismet.as_ref().context("the mission has no original Kismet")?.db.clone();
    let dir = ctx.out.join("SaveData");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;

    // 1. Reference: the whole run, uninterrupted (writing the autosaves the mission takes).
    let mut r = fresh(&setup, &maps, &def)?;
    r.autosave_dir = Some(dir.clone());
    let mut stage_ticks = Vec::new();
    for i in 0..STAGES.len() {
        stage(&mut r, &db, i)?;
        stage_ticks.push((r.tick, r.m().current_map().to_string()));
    }
    let reference = r.trail.clone();
    let ref_end = (r.tick, save::state_hash(r.m()), dishonored_sim::state_hash(&r.m().world), r.m().kismet.as_ref().map(|k| k.k.state_hash()).unwrap_or(0));
    let back_out = r.m().current_map().eq_ignore_ascii_case("L_Boyle_Ext_P") && r.m().state.task(&format!("{BA}.DishonoredTask_Custom_10")) == Some(TaskState::Active);
    rep.check(S, "reference run: the scripted mission reaches the canal path after going into the mansion and back out", back_out, format!("{} ticks; stages end at {:?}", r.tick, stage_ticks));
    let autosaves = r.autosaves.clone();
    let autosave_ops = r.m().autosave.clone();
    let blocked = save::save_blocked(r.m());
    drop(r);

    // 2. Save points: save, write, read, load into a fresh match, continue, compare.
    let points: [(usize, i32, &str); 2] = [
        (3, slot::QUICKSAVE, "inside the mansion, mid-mission (the street's saved level state inside the save); continuation travels back out"),
        (1, slot::MANUAL0, "in the street before going in; continuation travels into the mansion and back"),
    ];
    let mut rows = Vec::new();
    for (after, s, what) in points {
        let mut p = fresh(&setup, &maps, &def)?;
        for i in 0..=after {
            stage(&mut p, &db, i)?;
        }
        let before = dishonored_save::state_bytes(p.m());
        let before_hash = save::state_hash(p.m());
        let t0 = std::time::Instant::now();
        let bytes = save::encode(p.m(), s, &path)?;
        let enc_ms = t0.elapsed().as_secs_f32() * 1000.0;
        let file = dishonored_save::slot_path(&dir, s);
        dishonored_save::write_file(&file, &bytes)?;
        let saved_map = p.m().current_map().to_string();
        let saved_levels = saved_map.clone();
        let tick = p.tick;
        drop(p);
        // Load: a new match from the mission's setup and the file.
        let read = std::fs::read(&file)?;
        let t1 = std::time::Instant::now();
        let (h, m) = save::restore(&read, ms.clone())?;
        let dec_ms = t1.elapsed().as_secs_f32() * 1000.0;
        maps.set.set_active(m.current_map());
        let after_bytes = dishonored_save::state_bytes(&m);
        let mut q = Play::new(Game::Mission(Box::new(m)), &maps);
        q.tick = tick;
        rep.check(S, &format!("save {what}: the loaded match is bit-identical to the saved one"), after_bytes == before && h.state_hash == before_hash && h.slot == s && h.map == saved_levels, format!("slot {s} ({}), map {}, tick {}, {} KB on disk ({} KB state), encode {enc_ms:.0} ms, load {dec_ms:.0} ms, hash {:016x}", file.file_name().unwrap().to_string_lossy(), h.map, h.tick, read.len() / 1024, before.len() / 1024, h.state_hash));
        for i in after + 1..STAGES.len() {
            stage(&mut q, &db, i)?;
        }
        let (nw, dw) = diverge(&q.trail.world, &reference.world);
        let (nf, df) = diverge(&q.trail.full, &reference.full);
        let end = (q.tick, save::state_hash(q.m()), dishonored_sim::state_hash(&q.m().world), q.m().kismet.as_ref().map(|k| k.k.state_hash()).unwrap_or(0));
        rep.check(
            S,
            &format!("save {what}: the continued run hashes identically to the uninterrupted run"),
            dw.is_none() && df.is_none() && end == ref_end && nw > 0,
            format!("{nw} ticks of world hashes and {nf} full match hashes compared{}{}; end tick {} map {}; full {:016x} vs {:016x}; world {:016x} vs {:016x}; Kismet {:016x} vs {:016x}", dw.map(|d| format!("; world diverges at tick {} ({:016x} vs {:016x})", d.0, d.1, d.2)).unwrap_or_default(), df.map(|d| format!("; full diverges at tick {} ({:016x} vs {:016x})", d.0, d.1, d.2)).unwrap_or_default(), end.0, q.m().current_map(), end.1, ref_end.1, end.2, ref_end.2, end.3, ref_end.3),
        );
        rows.push(vec![format!("{} ({})", STAGES[after], saved_map), s.to_string(), format!("{}", read.len() / 1024), format!("{enc_ms:.0} / {dec_ms:.0}"), if dw.is_none() && df.is_none() && end == ref_end { "identical".into() } else { "DIVERGED".into() }]);
        drop(q);
    }
    rep.table("Save points", &["saved after", "slot", "KB", "encode / load ms", "continuation vs uninterrupted"], rows);

    // 3. A save only loads into the setup it was made with.
    let file = dishonored_save::slot_path(&dir, slot::QUICKSAVE);
    let mut other = ms.clone();
    if let Some(k) = other.kismet.as_mut() {
        k.seed ^= 1;
    }
    let refused = save::restore(&std::fs::read(&file)?, other).err().map(|e| e.to_string());
    rep.check(S, "a save made with another mission setup (Kismet seed) is refused", refused.as_ref().is_some_and(|e| e.contains("different version")), refused.unwrap_or_else(|| "loaded".into()));

    // 4. Autosaves and slots: every map load autosaves (two slots in turn), the slot files
    //    have the original names, the last save is the newest.
    let list = dishonored_save::list(&dir);
    let names: Vec<String> = list.iter().map(|(p, h)| format!("{} (slot {}, {})", p.file_name().unwrap().to_string_lossy(), h.slot, h.map)).collect();
    let loads = autosaves.iter().filter(|a| a.0 == slot::AUTOSAVE0 || a.0 == slot::AUTOSAVE1).count();
    let alternate = autosaves.windows(2).all(|w| w[0].0 != w[1].0 || !matches!(w[0].0, slot::AUTOSAVE0 | slot::AUTOSAVE1));
    rep.check(S, "an autosave is taken on every map load (mission start, into the mansion, back out), alternating DisAutoSave0 / DisAutoSave1", loads >= 3 && alternate && list.iter().any(|x| x.1.slot == slot::AUTOSAVE0) && list.iter().any(|x| x.1.slot == slot::AUTOSAVE1), format!("autosaves (slot, map, tick): {autosaves:?}; autosaves taken / dropped: {} / {}; at the end pending {} (saving blocked: {blocked:?})", autosave_ops.written, autosave_ops.denied, autosave_ops.pending_active));
    let last = dishonored_save::resolve_load_slot(&list, slot::LAST_SAVE);
    rep.check(S, "save files carry the original slot names and the last save resolves to the newest file", list.iter().any(|x| x.0.ends_with("DisQuickSave.SAV")) && list.iter().any(|x| x.0.ends_with("DISHONORED0.SAV")) && last == list.first().map(|x| x.1.slot), format!("{}; last save = slot {last:?}", names.join(", ")));
    // An autosave loads too (into the map it was taken on).
    if let Some((p0, h0)) = list.iter().find(|x| x.1.slot == slot::AUTOSAVE0 || x.1.slot == slot::AUTOSAVE1) {
        let ok = save::restore(&std::fs::read(p0)?, ms.clone()).map(|(_, m)| m.current_map().to_string());
        rep.check(S, "an autosave file loads back into its map", ok.as_ref().is_ok_and(|m| *m == h0.map), format!("{}: {:?}", p0.display(), ok.map_err(|e| e.to_string())));
    }
    rep.artifact("save folder", &dir);
    Ok(())
}
