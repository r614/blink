//! `harness kismet [--mission FILE] [--seed N]`: the original Boyle mission run by its own
//! Kismet (`missions/boyle_original.ron`, `dishonored_kismet`).
//!
//! A scripted agent plays it headless on the authoritative match: the global sequence's level
//! change into the street, the original objectives, the travel doors' Kismet into the mansion,
//! the original random switches (positions, dresses, guilty sister), a clue, the kill, and the
//! original way out. The agent is placed next to what it has to use (it does not path
//! across the maps); everything the mission does in response is the original Kismet. The
//! report lists how far the mission runs, the op coverage and activation counts.

use crate::Ctx;
use crate::report::Report;
use anyhow::{Context, Result};
use dishonored_content::MapContents;
use dishonored_kismet::{Obj, OpId};
use dishonored_mission::{MissionDef, TaskState};
use dishonored_multiplayer::game::Game;
use dishonored_multiplayer::mission::MissionMatch;
use dishonored_multiplayer::mission::kismet_host;
use dishonored_sim::input::{PlayerInput, buttons};
use glam::Vec3;

const S: &str = "Original Kismet";
const AGENT: u32 = 1;

struct Play<'a> {
    g: Game,
    maps: &'a MapContents,
    tick: u64,
    /// (time, task path, state) transitions seen.
    tasks: Vec<(f32, String, TaskState)>,
    last: Vec<(String, TaskState)>,
}

impl Play<'_> {
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
        let t = self.time();
        let m = self.m();
        let now: Vec<(String, TaskState)> = m.state.def.objectives.iter().flat_map(|o| o.tasks.iter().map(move |x| format!("{}.{}", o.id, x.id))).map(|p| {
            let s = m.state.task(&p).unwrap_or_default();
            (p, s)
        }).collect();
        let mut changes = Vec::new();
        for (p, s) in &now {
            if self.last.iter().find(|(q, _)| q == p).map(|x| x.1) != Some(*s) && !self.last.is_empty() {
                changes.push((t, p.clone(), *s));
            }
        }
        self.tasks.extend(changes);
        self.last = now;
    }

    fn idle(&mut self, secs: f32) {
        for _ in 0..(secs * 60.0) as u32 {
            self.step(PlayerInput::default());
        }
    }

    /// Run until `f` holds (or the time runs out); returns whether it held.
    fn until(&mut self, secs: f32, f: impl Fn(&MissionMatch) -> bool) -> bool {
        for _ in 0..(secs * 60.0) as u32 {
            if f(self.m()) {
                return true;
            }
            self.step(PlayerInput::default());
        }
        f(self.m())
    }

    fn time(&self) -> f32 {
        self.tick as f32 / 60.0
    }

    fn history(&self) -> Vec<String> {
        self.m().kismet.as_ref().map(|k| k.st.history.clone()).unwrap_or_default()
    }
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let path = ctx.opt("--mission").unwrap_or_else(|| "missions/boyle_original.ron".into());
    let mut def = MissionDef::load(std::path::Path::new(&path))?;
    if let (Some(s), Some(ok)) = (ctx.opt("--seed"), def.original_kismet.as_mut()) {
        ok.seed = s.parse()?;
    }
    let maps = MapContents::load(&dishonored_content::mission_maps(&def))?;
    let t0 = std::time::Instant::now();
    let setup = dishonored_content::mission_setup(&maps, def.clone()).map_err(|e| anyhow::anyhow!(e))?;
    let load_s = t0.elapsed().as_secs_f32();
    let dishonored_multiplayer::game::GameSetup::Mission(ms) = &setup else { anyhow::bail!("not a mission") };
    let ks = ms.kismet.as_ref().context("the mission has no original Kismet")?;
    let db = ks.db.clone();
    let classes = db.class_counts(&[]);
    rep.check(S, "original Kismet loaded: global sequence + both Boyle maps", db.ops.len() > 5000 && db.maps.len() == 2, format!("{} packages, {} ops of {} classes, {} level actors, {} objectives, {:.1} s", db.packages.len(), db.ops.len(), classes.len(), db.actors.len(), db.objectives.len(), load_s));
    let ext = maps.get(&def.map).context("ext")?.clone();
    maps.set.set_active(&def.map);
    let g = setup.build(&ext.sim);
    let mut p = Play { g, maps: &maps, tick: 0, tasks: Vec::new(), last: Vec::new() };
    p.g.add_player(AGENT, "agent", 0, &maps.set, &ext.sim);

    let explore: Option<f32> = ctx.opt("--explore").and_then(|s| s.parse().ok());
    if let Some(secs) = explore {
        p.idle(secs);
        dump(&p);
        return Ok(());
    }
    if ctx.opt("--route").is_some_and(|r| r != "lethal") {
        drop(p);
        return routes(ctx, rep, &setup, &maps, &def, &db);
    }
    play(&mut p, rep, &db)?;
    if ctx.args.iter().any(|a| a == "--dump") {
        dump(&p);
    }
    coverage(&p, rep, &db);
    let first = fingerprint(&p);
    drop(p);
    // Determinism: the same seed replays the same mission.
    let mut q = fresh(&setup, &maps, &def)?;
    play(&mut q, &mut Report::new(&ctx.out.join("replay")), &db)?;
    let second = fingerprint(&q);
    drop(q);
    rep.check(S, "same seed, same run (Kismet state hash, history and task timeline)", first == second, format!("hash {:016x} vs {:016x}; {} history lines", first.0, second.0, first.1));
    // The original random switches across seeds (setup only: up to the interior randomisation).
    let mut rows = Vec::new();
    let mut guilty_seen = std::collections::BTreeSet::new();
    let mut dress_sets = std::collections::BTreeSet::new();
    for seed in 1..=6u64 {
        let mut d = def.clone();
        if let Some(ok) = d.original_kismet.as_mut() {
            ok.seed = seed;
        }
        let s = dishonored_content::mission_setup(&maps, d.clone()).map_err(|e| anyhow::anyhow!(e))?;
        maps.set.set_active(&d.map);
        let mut r = Play { g: s.build(&ext.sim), maps: &maps, tick: 0, tasks: Vec::new(), last: Vec::new() };
        r.g.add_player(AGENT, "agent", 0, &maps.set, &ext.sim);
        let (guilty, dresses) = interior_setup(&mut r, &db)?;
        guilty_seen.insert(guilty.clone());
        dress_sets.insert(dresses.clone());
        rows.push(vec![seed.to_string(), guilty, dresses]);
    }
    rep.check(S, "different seeds: the original switches pick different guilty sisters and dresses", guilty_seen.len() >= 2 && dress_sets.len() >= 2, format!("guilty sisters {guilty_seen:?}; {} distinct dress assignments over 6 seeds", dress_sets.len()));
    rep.table("Interior randomisation by seed", &["seed", "guilty", "dresses (pawn: body material)"], rows);
    routes(ctx, rep, &setup, &maps, &def, &db)
}

/// The other routes through the original mission, each on a fresh match: every stage is a
/// check (stage, passed, detail).
fn routes(ctx: &Ctx, rep: &mut Report, setup: &dishonored_multiplayer::game::GameSetup, maps: &MapContents, def: &MissionDef, db: &dishonored_kismet::KismetDb) -> Result<()> {
    let only = ctx.opt("--route");
    let want = |r: &str| only.as_deref().is_none_or(|o| o == r);
    type Route = fn(&mut Play, &dishonored_kismet::KismetDb) -> Result<Vec<(String, bool, String)>>;
    let list: [(&str, &str, Route); 3] = [("brisby", "Original Kismet: nonlethal route (Lord Brisby)", nonlethal), ("white", "Original Kismet: Miss White's drink", miss_white), ("duel", "Original Kismet: Lord Shaw's duel", duel)];
    for (id, section, f) in list {
        if !want(id) {
            continue;
        }
        let mut p = fresh(setup, maps, def)?;
        let stages = f(&mut p, db)?;
        for (stage, ok, detail) in stages {
            rep.check(section, &stage, ok, detail);
        }
        coverage_line(&p, rep, section);
        if ctx.args.iter().any(|a| a == "--dump") {
            dump(&p);
        }
    }
    Ok(())
}

/// One line of op coverage for a route: unsupported classes reached on it.
fn coverage_line(p: &Play, rep: &mut Report, section: &str) {
    let Some(k) = &p.m().kismet else { return };
    let cov = &k.k.coverage;
    rep.check(section, "no unsupported Kismet class on the route", cov.unhandled.is_empty(), format!("{} activations of {} distinct ops; unsupported {:?}; no port system {:?}", cov.activations.values().sum::<u32>(), cov.activations.len(), cov.unhandled, k.st.not_modelled));
}

fn fresh<'a>(setup: &dishonored_multiplayer::game::GameSetup, maps: &'a MapContents, def: &MissionDef) -> Result<Play<'a>> {
    let ext = maps.get(&def.map).context("ext")?;
    maps.set.set_active(&def.map);
    let mut p = Play { g: setup.build(&ext.sim), maps, tick: 0, tasks: Vec::new(), last: Vec::new() };
    p.g.add_player(AGENT, "agent", 0, &maps.set, &ext.sim);
    Ok(p)
}

/// (Kismet state hash, history length, history text hash).
fn fingerprint(p: &Play) -> (u64, usize, u64) {
    let k = p.m().kismet.as_ref().unwrap();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in k.st.history.join("\n").bytes().chain(format!("{:?}", p.tasks).bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (k.k.state_hash(), k.st.history.len(), h)
}

/// Run a fresh match to the interior randomisation: (guilty sister, dresses).
fn interior_setup(p: &mut Play, db: &dishonored_kismet::KismetDb) -> Result<(String, String)> {
    p.until(90.0, |m| m.state.task(&format!("{BA}.DishonoredTask_Custom_12")) == Some(TaskState::Active));
    let tv = actor(db, "L_Boyle_Ext_Script:TriggerVolume_14")?;
    p.enter(tv);
    p.idle(3.0);
    let door = actor(db, "L_Boyle_Ext_P:DishonoredUsableObject_7")?;
    p.use_actor(door);
    p.idle(0.2);
    p.choose(0);
    p.until(15.0, |m| ["Esma Guilty", "Lydia Guilty", "Waverly Guilty"].iter().any(|n| matches!(var(m, "L_Boyle_Int_Script", n), Some(dishonored_kismet::Value::Bool(true)))));
    let guilty: Vec<&str> = ["Esma", "Lydia", "Waverly"].into_iter().filter(|n| matches!(var(p.m(), "L_Boyle_Int_Script", &format!("{n} Guilty")), Some(dishonored_kismet::Value::Bool(true)))).collect();
    let mut dresses: Vec<String> = p.history().iter().filter(|h| h.contains("materials of pawn")).map(|h| h.split("materials of pawn ").nth(1).unwrap_or("").split(',').next().unwrap_or("").to_string()).collect();
    dresses.sort();
    Ok((guilty.join("+"), dresses.join("; ")))
}

/// Actor index by `Package:Name`.
fn actor(db: &dishonored_kismet::KismetDb, r: &str) -> Result<u32> {
    db.find_actor(r).with_context(|| format!("actor {r}"))
}

/// Value of a Kismet variable by its `VarName` in a package.
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

impl Play<'_> {
    /// Put the agent on the floor near `pos` (at `dist` from it) facing it.
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
        let me = self.mm().world.player_mut(AGENT).expect("agent");
        me.position = best;
        me.velocity = Vec3::ZERO;
        let eye = best + Vec3::Z * 64.0;
        let to = pos - eye;
        me.yaw = to.y.atan2(to.x).to_degrees();
        me.pitch = (to.z / to.truncate().length().max(1.0)).atan().to_degrees();
    }

    /// Stand in front of a Kismet usable and press use once.
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

    fn choose(&mut self, option: usize) -> Vec<String> {
        let opts = self.m().view.mission.as_ref().and_then(|v| v.choices.iter().find(|c| c.0 == AGENT).map(|c| c.1.clone())).unwrap_or_default();
        self.step(PlayerInput { buttons: buttons::CHOICES[option], ..Default::default() });
        self.step(PlayerInput::default());
        opts
    }

    /// Move the agent into a trigger actor's shape.
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

    fn task(&self, path: &str) -> TaskState {
        self.m().state.task(path).unwrap_or_default()
    }

    fn task_text(&self, path: &str) -> String {
        let (o, t) = path.split_once('.').unwrap_or(("", ""));
        self.m().state.def.objectives.iter().find(|x| x.id == o).and_then(|x| x.tasks.iter().find(|y| y.id == t)).map(|y| y.description.clone()).unwrap_or_default()
    }
}

const BA: &str = "BoyleAssassination";

fn play(p: &mut Play, rep: &mut Report, db: &dishonored_kismet::KismetDb) -> Result<()> {
    // 1. Mission start: the global sequence's level change into the street.
    let ok = p.until(5.0, |m| m.kismet.as_ref().is_some_and(|k| k.st.history.iter().any(|h| h.contains("commit map change L_Boyle_Ext_P"))));
    let start = p.m().setup.tagged_starts.iter().find(|t| t.1 == "PlayerStart_SamuelDropOff").map(|t| t.2).unwrap_or_default();
    let at = p.m().world.player(AGENT).map(|x| x.position).unwrap_or_default();
    rep.check(S, "mission starts through the global Kismet: ChangeLvl_fromHub_toBoyle -> PrepareMapChange L_Boyle_Ext_P -> PlayerStart_SamuelDropOff -> CommitMapChange", ok && at.truncate().distance(start.truncate()) < 200.0, format!("{}; agent {:.0} uu from PlayerStart_SamuelDropOff", p.history().iter().take(4).cloned().collect::<Vec<_>>().join("; "), at.truncate().distance(start.truncate())));
    let t12 = format!("{BA}.DishonoredTask_Custom_12");
    let ok = p.until(90.0, |m| m.state.task(&t12) == Some(TaskState::Active));
    let objs: Vec<String> = p.m().view.mission.as_ref().map(|v| v.objectives.iter().filter(|o| !o.hidden).map(|o| format!("\"{}\": {}", o.description, o.tasks.iter().filter(|t| !t.hidden && t.state == TaskState::Active).map(|t| format!("\"{}\"", t.description)).collect::<Vec<_>>().join(", "))).collect()).unwrap_or_default();
    rep.check(S, "Samuel's intro (Spawned -> DialogInputs -> conversation -> soiree matinee) adds the original objectives with their localized text", ok && objs.iter().any(|o| o.contains("Assassinate Lady Boyle")) && p.task_text(&t12) == "Find a way inside the Boyle Estate", format!("at {:.1} s: {}", p.time(), objs.join("; ")));
    // 2. Estate grounds: the entrance trigger.
    let tv = actor(db, "L_Boyle_Ext_Script:TriggerVolume_14")?;
    p.enter(tv);
    let t1 = format!("{BA}.DishonoredTask_Custom_1");
    let ok = p.until(5.0, |m| m.state.task(&t1) == Some(TaskState::Active));
    rep.check(S, "touching the estate entrance (SeqEvent_Touch on TriggerVolume_14) completes T12 and activates T1", ok && p.task(&t12) == TaskState::Completed, format!("T12 {:?}, T1 {:?} \"{}\"", p.task(&t12), p.task(&t1), p.task_text(&t1)));
    // 3. The garden door into the ballroom: original choice, then the global level change.
    let door = actor(db, "L_Boyle_Ext_P:DishonoredUsableObject_7")?;
    p.idle(1.0);
    let used = p.use_actor(door);
    p.idle(0.2);
    let opts = p.choose(0);
    let ok = p.until(5.0, |m| m.current_map().eq_ignore_ascii_case("L_Boyle_Int_P"));
    let ballroom = p.m().setup.tagged_starts.iter().find(|t| t.0 == "L_Boyle_Int_P" && t.1 == "PlayerStart_Ballroom").map(|t| t.2).unwrap_or_default();
    let at = p.m().world.player(AGENT).map(|x| x.position).unwrap_or_default();
    rep.check(S, "the garden door's Kismet (Used -> DialogScriptedChoice -> SetPlayerTravelDestination -> ChangeLvl_* -> global PrepareMapChange/SaveLevelState/CommitMapChange) takes the agent to the ballroom", used && ok && opts.len() == 2 && at.truncate().distance(ballroom.truncate()) < 300.0, format!("choice {opts:?}; map {}; agent {:.0} uu from PlayerStart_Ballroom", p.m().current_map(), at.truncate().distance(ballroom.truncate())));
    if !ok {
        return Ok(());
    }
    // 4. Interior setup: the sisters' positions, dresses and guilt from the original switches.
    let ok = p.until(10.0, |m| ["Esma Guilty", "Lydia Guilty", "Waverly Guilty"].iter().any(|n| matches!(var(m, "L_Boyle_Int_Script", n), Some(dishonored_kismet::Value::Bool(true)))));
    let guilty: Vec<&str> = ["Esma", "Lydia", "Waverly"].into_iter().filter(|n| matches!(var(p.m(), "L_Boyle_Int_Script", &format!("{n} Guilty")), Some(dishonored_kismet::Value::Bool(true)))).collect();
    let dresses: Vec<String> = p.history().iter().filter(|h| h.contains("materials of pawn")).cloned().collect();
    rep.check(S, "the original random switches dress the three sisters and pick one guilty sister", ok && guilty.len() == 1 && dresses.len() >= 3, format!("guilty {guilty:?}; {}", dresses.join("; ")));
    let Some(sister) = guilty.first().map(|s| s.to_string()) else { return Ok(()) };
    // 5. The guilty sister's clue: the note for her dress that the Kismet placed in her bedroom.
    let notes: Vec<(String, u32)> = ["Red", "White", "Black"].iter().filter_map(|c| match var(p.m(), "L_Boyle_Int_Script", &format!("{sister}{c}Note")) {
        Some(dishonored_kismet::Value::Obj(Obj::Actor(a))) => Some((c.to_string(), a)),
        _ => None,
    }).collect();
    let placed: Vec<(String, u32)> = notes.iter().filter(|(_, a)| p.m().kismet.as_ref().is_some_and(|k| k.st.actor_pos.contains_key(a))).cloned().collect();
    let dress_task_before: Vec<String> = dress_tasks(p, &sister, TaskState::Active);
    let mut picked = false;
    if let Some((_, a)) = placed.first() {
        picked = p.use_actor(*a);
        p.idle(1.0);
    }
    let dress_task = dress_tasks(p, &sister, TaskState::Active);
    let t22 = format!("{BA}.DishonoredTask_Custom_22");
    rep.check(S, "picking up the guilty sister's clue note (placed by the original teleports) shows her dress task", picked && placed.len() == 1 && dress_task_before.is_empty() && dress_task.len() == 1, format!("notes placed {:?}; picked {picked}; active dress task {:?}; T22 {:?}", placed.iter().map(|(c, a)| format!("{sister} {c}: {}", db.actors[*a as usize].name)).collect::<Vec<_>>(), dress_task.iter().map(|t| p.task_text(t)).collect::<Vec<_>>(), p.task(&t22)));
    // 6. The kill: the guilty sister, from behind with the sword.
    let spawner = match sister.as_str() {
        "Esma" => "L_Boyle_Int_Script:DishonoredSpawner_2",
        "Lydia" => "L_Boyle_Int_Script:DishonoredSpawner_11",
        _ => "L_Boyle_Int_Script:DishonoredSpawner_4",
    };
    let sa = actor(db, spawner)?;
    let pawn = p.m().kismet.as_ref().and_then(|k| k.k.spawned.get(&sa).and_then(|v| v.first().copied()));
    let mut killed = false;
    if let Some(id) = pawn {
        for i in 0..(40 * 60) {
            let Some(t) = p.m().world.player(id).cloned() else { break };
            if !t.alive {
                killed = true;
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
            let input = PlayerInput { yaw: to.y.atan2(to.x).to_degrees(), pitch: -5.0, buttons: if i % 20 == 0 { buttons::PRIMARY } else { 0 }, ..Default::default() };
            p.step(input);
        }
    }
    p.idle(2.0);
    let t6 = format!("{BA}.DishonoredTask_Custom_6");
    let complete_flag = p.m().state.flag("Boyle_Return_To_Boat");
    rep.check(S, "killing the identified guilty sister fires her attached SeqEvent_Death: elimination complete, T6 \"escape\" active", killed && complete_flag && p.task(&t6) == TaskState::Active, format!("{sister} (pawn {pawn:?}) killed {killed}; Boyle_Return_To_Boat {complete_flag}; dress task {:?}; T6 {:?} \"{}\"", dress_task.iter().map(|t| p.task(t)).collect::<Vec<_>>(), p.task(&t6), p.task_text(&t6)));
    // 7. Out of the mansion through the ballroom door, then the street exit.
    let door = actor(db, "L_Boyle_Int_P:DishonoredUsableObject_10")?;
    p.idle(1.0);
    let used = p.use_actor(door);
    p.idle(0.2);
    let opts = p.choose(0);
    let ok = p.until(5.0, |m| m.current_map().eq_ignore_ascii_case("L_Boyle_Ext_P"));
    rep.check(S, "the ballroom door's Kismet takes the agent back to the garden (exterior state restored)", used && ok, format!("choice {opts:?}; map {}", p.m().current_map()));
    if !ok {
        return Ok(());
    }
    p.idle(2.0);
    let tv = actor(db, "L_BoyleStreet_Ext_Script:TriggerVolume_23")?;
    p.enter(tv);
    let t10 = format!("{BA}.DishonoredTask_Custom_10");
    let ok = p.until(5.0, |m| m.state.task(&t10) == Some(TaskState::Active));
    rep.check(S, "the canal path trigger (TriggerVolume_23) moves the objective to Samuel's new mooring (T10)", ok, format!("T6 {:?}, T10 {:?} \"{}\"", p.task(&t6), p.task(&t10), p.task_text(&t10)));
    let mut ended = false;
    let mut asked = Vec::new();
    for t in ["L_BoyleStreet_Ext_Script:DisTrigger_35", "L_BoyleStreet_Ext_Script:DisTrigger_0", "L_BoyleStreet_Ext_Script:DisTrigger_34"] {
        let a = actor(db, t)?;
        p.enter(a);
        // Samuel asks (a conversation choice): "let's go".
        if p.until(30.0, |m| m.state.view.choice.is_some()) {
            asked = p.choose(0);
        }
        if p.until(60.0, |m| m.state.outcome().is_some()) {
            ended = true;
            break;
        }
    }
    let outcome = p.m().state.outcome().cloned();
    let hist = p.history();
    let path: Vec<String> = hist.iter().filter(|h| h.contains("prepare map change L_Pub") || h.contains("leaves the mission")).cloned().collect();
    rep.check(S, "Samuel's boat: T10 completes, Lets_Go asks to leave, and the original level change out of Boyle (ChangeLvl_fromBoyle_toHub -> PrepareMapChange L_Pub_*) ends the mission", ended && outcome.as_ref().is_some_and(|o| o.success), format!("Samuel asks {asked:?}; outcome {outcome:?} at {:.1} s; {}; T10 {:?}; T5 {:?}", p.time(), path.join("; "), p.task(&t10), p.task(&format!("{BA}.DishonoredTask_Custom_5"))));
    Ok(())
}

/// One route stage: (stage, passed, detail).
type Stage = (String, bool, String);

fn stage(rows: &mut Vec<Stage>, name: &str, ok: bool, detail: String) -> bool {
    rows.push((name.to_string(), ok, detail));
    ok
}

/// Pawn spawned by an original spawner (`Package:Name`).
fn spawned_pawn(p: &Play, db: &dishonored_kismet::KismetDb, spawner: &str) -> Option<u32> {
    let a = db.find_actor(spawner)?;
    p.m().kismet.as_ref().and_then(|k| k.k.spawned.get(&a).and_then(|v| v.first().copied()))
}

fn sister_spawner(sister: &str) -> &'static str {
    match sister {
        "Esma" => "L_Boyle_Int_Script:DishonoredSpawner_2",
        "Lydia" => "L_Boyle_Int_Script:DishonoredSpawner_11",
        _ => "L_Boyle_Int_Script:DishonoredSpawner_4",
    }
}

impl Play<'_> {
    /// Stand next to a pawn, facing it.
    fn near_pawn(&mut self, id: u32, d: f32) {
        if let Some(pos) = self.m().world.player(id).map(|x| x.position) {
            self.place_near(pos, d);
        }
    }

    /// Answer any open conversation or scripted choice with its first option.
    fn answer(&mut self) -> bool {
        let conv = self.m().state.view.choice.as_ref().is_some_and(|c| c.player.is_none_or(|p| p == AGENT));
        let scripted = self.m().kismet.as_ref().is_some_and(|k| k.choosing(AGENT));
        if conv || scripted {
            self.choose(0);
            return true;
        }
        false
    }

    /// Use the talk usable of an original NPC (its one-shot's use hook).
    fn talk_to(&mut self, id: u32) -> bool {
        self.near_pawn(id, 110.0);
        let list = kismet_host::usables(self.m());
        let Some(i) = list.iter().position(|u| matches!(&u.0, kismet_host::KUse::Talk { pawn, .. } if *pawn == id)) else { return false };
        kismet_host::use_usable(self.mm(), AGENT, i);
        self.step(PlayerInput::default());
        true
    }

    /// Run until `f` holds, answering choices on the way.
    fn until_answering(&mut self, secs: f32, f: impl Fn(&MissionMatch) -> bool) -> bool {
        for _ in 0..(secs * 60.0) as u32 {
            if f(self.m()) {
                return true;
            }
            if !self.answer() {
                self.step(PlayerInput::default());
            }
        }
        f(self.m())
    }

    fn hist_has(&self, s: &str) -> bool {
        self.m().kismet.as_ref().is_some_and(|k| k.st.history.iter().any(|h| h.contains(s)))
    }

    fn conv_started(&self, label: &str) -> bool {
        self.m().state.conv_log.iter().any(|e| matches!(e, dishonored_conversation::runner::ConvEvent::Started { label: l, .. } if l == label))
    }
}

/// The guilty sister's clue note, picked up (her dress task shows).
fn pick_clue(p: &mut Play, sister: &str) -> Option<u32> {
    let note = ["Red", "White", "Black"].iter().find_map(|c| match var(p.m(), "L_Boyle_Int_Script", &format!("{sister}{c}Note")) {
        Some(dishonored_kismet::Value::Obj(Obj::Actor(a))) if p.m().kismet.as_ref().is_some_and(|k| k.st.actor_pos.contains_key(&a)) => Some(a),
        _ => None,
    })?;
    p.use_actor(note);
    p.idle(1.0);
    Some(note)
}

/// Out of the mansion and away on Samuel's boat (the lethal route's last steps): the ballroom
/// door to the garden, the canal trigger, Samuel's `Lets_Go`, the level change out of Boyle.
fn leave_boyle(p: &mut Play, db: &dishonored_kismet::KismetDb) -> Result<(bool, String)> {
    let door = actor(db, "L_Boyle_Int_P:DishonoredUsableObject_10")?;
    p.idle(1.0);
    p.use_actor(door);
    p.idle(0.2);
    p.choose(0);
    if !p.until(5.0, |m| m.current_map().eq_ignore_ascii_case("L_Boyle_Ext_P")) {
        return Ok((false, format!("still on {}", p.m().current_map())));
    }
    p.idle(2.0);
    let tv = actor(db, "L_BoyleStreet_Ext_Script:TriggerVolume_23")?;
    p.enter(tv);
    p.until(5.0, |m| m.state.task(&format!("{BA}.DishonoredTask_Custom_10")) == Some(TaskState::Active));
    for t in ["L_BoyleStreet_Ext_Script:DisTrigger_35", "L_BoyleStreet_Ext_Script:DisTrigger_0", "L_BoyleStreet_Ext_Script:DisTrigger_34"] {
        let a = actor(db, t)?;
        p.enter(a);
        if p.until(30.0, |m| m.state.view.choice.is_some()) {
            p.choose(0);
        }
        if p.until(60.0, |m| m.state.outcome().is_some()) {
            break;
        }
    }
    let outcome = p.m().state.outcome().cloned();
    Ok((outcome.as_ref().is_some_and(|o| o.success), format!("outcome {outcome:?} at {:.1} s", p.time())))
}

/// The nonlethal route (BOYLE_MISSION.md "Nonlethal path, Lord Brisby"): Brisby's approach
/// hook starts his quest, he walks to his private spot, his use hook gives the guilty
/// sister's KO task, she is choked unconscious and carried to the cellar boat, Brisby takes
/// her away (`Thanks`, the gate closes, `CompleteMercyPath`), and the mission ends on
/// Samuel's boat.
fn nonlethal(p: &mut Play, db: &dishonored_kismet::KismetDb) -> Result<Vec<Stage>> {
    let mut rows: Vec<Stage> = Vec::new();
    let (guilty, _) = interior_setup(p, db)?;
    let sister = guilty.clone();
    let clue = pick_clue(p, &sister);
    let dress = dress_tasks(p, &sister, TaskState::Active);
    if !stage(&mut rows, "interior: the guilty sister is picked and her clue note shows her dress task", clue.is_some() && dress.len() == 1, format!("guilty {guilty}; dress task {:?} at {:.1} s", dress.iter().map(|t| p.task_text(t)).collect::<Vec<_>>(), p.time())) {
        return Ok(rows);
    }
    // Brisby: his approach hook (close band) starts `Lord_Brisby_Quest_Start`; its output
    // `Join_Brisby_in_Private` sends him to TargetPoint_120 (AIGoToActor), T26 shows.
    let Some(bris) = spawned_pawn(p, db, "L_Boyle_Int_Script:DishonoredSpawner_9") else {
        stage(&mut rows, "Brisby is spawned", false, "no pawn".into());
        return Ok(rows);
    };
    p.near_pawn(bris, 150.0);
    let started = p.until_answering(30.0, |m| m.state.conv_log.iter().any(|e| matches!(e, dishonored_conversation::runner::ConvEvent::Started { label, .. } if label == "Lord_Brisby_Quest_Start")));
    let t26 = format!("{BA}.DishonoredTask_Custom_26");
    let in_pos = p.until_answering(90.0, |m| m.state.flag("Boyle_BrisbyInPosition"));
    let goto_ok = p.hist_has("Brisby") || in_pos;
    if !stage(&mut rows, "Brisby's approach hook starts Lord_Brisby_Quest_Start; Join_Brisby_in_Private walks him to his spot (AIGoToActor Success): Boyle_BrisbyInPosition, T26", started && in_pos && goto_ok, format!("quest start {started}; in position {in_pos}; T26 {:?} at {:.1} s", p.task(&t26), p.time())) {
        return Ok(rows);
    }
    // His use hook: `Brisby_Boyle_Invite`, then the guilty sister's KO conversation, whose
    // soiree starts `GetBrisbyQuest` (Boyle_BrisbyTalked, the KO task for her).
    let mut talked = false;
    for _ in 0..4 {
        talked |= p.talk_to(bris);
        if p.until_answering(60.0, |m| m.state.flag("Boyle_BrisbyTalked")) {
            break;
        }
    }
    let ko_n = match sister.as_str() {
        "Esma" => 2,
        "Lydia" => 3,
        _ => 4,
    };
    let ko_task = format!("{BA}.DishonoredTask_Custom_{ko_n}");
    p.until(5.0, |m| m.state.task(&ko_task) == Some(TaskState::Active));
    if !stage(&mut rows, "Brisby's use hook: Brisby_Boyle_Invite, then her KO conversation -> GetBrisbyQuest: Boyle_BrisbyTalked and her KO task", talked && p.m().state.flag("Boyle_BrisbyTalked") && p.task(&ko_task) == TaskState::Active, format!("talked {talked}; Boyle_BrisbyTalked {}; {ko_task} {:?} \"{}\" at {:.1} s", p.m().state.flag("Boyle_BrisbyTalked"), p.task(&ko_task), p.task_text(&ko_task), p.time())) {
        return Ok(rows);
    }
    // Knock her out from behind (choke) and lift her.
    let Some(her) = spawned_pawn(p, db, sister_spawner(&sister)) else {
        stage(&mut rows, "the guilty sister is spawned", false, "no pawn".into());
        return Ok(rows);
    };
    let mut carrying = false;
    for _ in 0..(40 * 60) {
        let Some(t) = p.m().world.player(her).cloned() else { break };
        let me = *p.m().world.player(AGENT).unwrap();
        if me.carrying == Some(her) {
            carrying = true;
            break;
        }
        if p.answer() {
            continue;
        }
        if me.choke.phase == dishonored_sim::state::ChokePhase::None && t.condition == dishonored_sim::state::Condition::Normal {
            let back = Vec3::new(t.yaw.to_radians().cos(), t.yaw.to_radians().sin(), 0.0);
            let spot = t.position - back * 80.0;
            let m = p.mm().world.player_mut(AGENT).unwrap();
            m.position = spot;
            m.velocity = Vec3::ZERO;
            m.yaw = t.yaw;
        }
        let yaw = p.m().world.player(AGENT).unwrap().yaw;
        // Release once before the first press so the press is an edge.
        let b = if me.choke.phase == dishonored_sim::state::ChokePhase::None && me.prev_buttons & buttons::USE != 0 { 0 } else { buttons::USE };
        p.step(PlayerInput { yaw, pitch: 0.0, buttons: b, ..Default::default() });
    }
    let her_state = p.m().world.player(her).map(|t| (t.alive, t.condition));
    let boat_target = p.m().kismet.as_ref().and_then(|k| k.st.targets.get(&ko_task).cloned());
    let knocked = p.hist_has(&format!("knock-out of pawn {her}"));
    if !stage(&mut rows, "she is choked unconscious (DisSeqEvent_KnockedOut retargets the KO task to the cellar boat) and carried", carrying && knocked && boat_target.is_some(), format!("{sister} {her_state:?}; carrying {carrying}; KO task target {boat_target:?} at {:.1} s", p.time())) {
        return Ok(rows);
    }
    // Into the cellar: DisTrigger_23 tests the body (TestBoyleBody_Touch), the body goes on the
    // boat (TriggerVolume_10, "Lady On Boat?"), Brisby thanks the player and the gate closes.
    let trig = actor(db, "L_Boyle_Int_Script:DisTrigger_23")?;
    p.enter(trig);
    p.idle(1.0);
    let boat = actor(db, "L_Boyle_Int_Script:TriggerVolume_10")?;
    let boat_pos = Vec3::from(db.actors[boat as usize].location);
    // Drop her on the boat.
    {
        let me = p.mm().world.player_mut(AGENT).unwrap();
        me.position = boat_pos + Vec3::new(-60.0, 0.0, 40.0);
        me.velocity = Vec3::ZERO;
        me.yaw = 0.0;
    }
    p.step(PlayerInput::default());
    p.step(PlayerInput { buttons: buttons::USE, ..Default::default() });
    p.idle(1.0);
    if let Some(b) = p.mm().world.player_mut(her) {
        b.position = boat_pos + Vec3::Z * 20.0;
        b.velocity = Vec3::ZERO;
    }
    let tv8 = actor(db, "L_Boyle_Int_Script:TriggerVolume_8")?;
    p.enter(tv8);
    let done = p.until_answering(150.0, |m| m.state.flag("BoyleMercyPathComplete"));
    let t6 = format!("{BA}.DishonoredTask_Custom_6");
    let thanks = p.conv_started("Thanks");
    if !stage(&mut rows, "the cellar boat: TestBoyleBody_Touch, Thanks (its soiree closes the gate) -> CompleteMercyPath: BoyleMercyPathComplete, KO task completed, T6 escape", done && p.task(&ko_task) == TaskState::Completed && p.task(&t6) == TaskState::Active, format!("Thanks {thanks}; BoyleMercyPathComplete {done}; {ko_task} {:?}; T6 {:?}; SF_Global_PlyrMercyBoyle {} at {:.1} s", p.task(&ko_task), p.task(&t6), p.m().state.flag("SF_Global_PlyrMercyBoyle"), p.time())) {
        return Ok(rows);
    }
    let (ended, detail) = leave_boyle(p, db)?;
    stage(&mut rows, "the way out as on the lethal route: Samuel's boat ends the mission (nonlethal)", ended, detail);
    Ok(rows)
}

/// Miss White's drink (BOYLE_MISSION.md section 3 "How the player learns the dress", 2): her
/// `Hits_On_Player` conversation asks for a drink (T25), the drink comes from the party's
/// drinks (`DisSeqAct_GivePickup` of the party drink), handing it over runs
/// `MissWhiteRevealsColors`: she names Waverly's and Lydia's colours, `WhiteToldAllColors`,
/// and the guilty sister's dress task shows.
fn miss_white(p: &mut Play, db: &dishonored_kismet::KismetDb) -> Result<Vec<Stage>> {
    let mut rows: Vec<Stage> = Vec::new();
    let (guilty, _) = interior_setup(p, db)?;
    let Some(white) = spawned_pawn(p, db, "L_Boyle_Int_Script:DishonoredSpawner_20") else {
        stage(&mut rows, "Miss White is spawned", false, "no pawn".into());
        return Ok(rows);
    };
    let t25 = format!("{BA}.DishonoredTask_Custom_25");
    // Approach her (her approach hooks), then talk (use hook): `Hits_On_Player`.
    p.near_pawn(white, 200.0);
    let mut asked = p.until_answering(20.0, |m| m.state.task(&format!("{BA}.DishonoredTask_Custom_25")) == Some(TaskState::Active));
    for _ in 0..3 {
        if asked {
            break;
        }
        p.talk_to(white);
        asked = p.until_answering(40.0, |m| m.state.task(&format!("{BA}.DishonoredTask_Custom_25")) == Some(TaskState::Active));
    }
    if !stage(&mut rows, "Miss White's Hits_On_Player conversation asks for a drink: Quest For the Drink -> T25 active", asked, format!("Hits_On_Player {}; T25 {:?} \"{}\" at {:.1} s", p.conv_started("Hits_On_Player"), p.task(&t25), p.task_text(&t25), p.time())) {
        return Ok(rows);
    }
    // The drink: the Kismet usable that gives it (`Got Yourself a Drink` / the party drink).
    let drink = "boylepartydrink_AbsItm";
    let mut got = p.m().state.items.get(drink).copied().unwrap_or(0) > 0;
    let mut used: Vec<String> = Vec::new();
    if !got {
        let target = p.m().kismet.as_ref().and_then(|k| k.st.targets.get(&t25).cloned());
        let tpos = match &target {
            Some(Obj::Actor(a)) => Some(Vec3::from(db.actors[*a as usize].location)),
            _ => db.find_actor("L_Boyle_Int_Script:TargetPoint_108").map(|a| Vec3::from(db.actors[a as usize].location)),
        };
        let mut cands: Vec<(u32, f32)> = kismet_host::usables(p.m())
            .into_iter()
            .filter_map(|(u, pos, _)| match u {
                kismet_host::KUse::Actor(a) | kismet_host::KUse::Pickup(a) => Some((a, tpos.map(|t| t.distance(pos)).unwrap_or(0.0))),
                _ => None,
            })
            .collect();
        cands.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (a, _) in cands.into_iter().take(6) {
            used.push(db.actors[a as usize].name.clone());
            p.use_actor(a);
            p.until_answering(3.0, |m| m.state.items.get(drink).copied().unwrap_or(0) > 0);
            if p.m().state.items.get(drink).copied().unwrap_or(0) > 0 {
                got = true;
                break;
            }
        }
    }
    if !stage(&mut rows, "the party drink: the drinks usable gives it (DisSeqAct_GivePickup BoylePartyDrink_twk)", got, format!("used {used:?}; items {:?} at {:.1} s", p.m().state.items, p.time())) {
        return Ok(rows);
    }
    // Hand it over: talk to her again; `Takes_Drink`, then the colour conversations.
    let mut told = false;
    for _ in 0..3 {
        p.talk_to(white);
        if p.until_answering(90.0, |m| m.state.flag("WhiteToldAllColors")) {
            told = true;
            break;
        }
    }
    let gone = p.m().state.items.get(drink).copied().unwrap_or(0) == 0;
    let convs: Vec<&str> = ["Takes_Drink", "WaverlyRed", "WaverlyWhite", "WaverlyBlack", "LydiaRed", "LydiaWhite", "LydiaBlack"].into_iter().filter(|c| p.conv_started(c)).collect();
    if !stage(&mut rows, "handing her the drink: Takes_Drink, RemoveAbstractItem, MissWhiteRevealsColors (Waverly's then Lydia's colour), WhiteToldAllColors, T25 completed", told && gone && p.task(&t25) == TaskState::Completed, format!("conversations {convs:?}; drink removed {gone}; WhiteToldAllColors {told}; T25 {:?} at {:.1} s", p.task(&t25), p.time())) {
        return Ok(rows);
    }
    p.idle(3.0);
    let dress = dress_tasks(p, &guilty, TaskState::Active);
    stage(&mut rows, "her colours identify the guilty sister: her dress task shows (Set New Objective Based on Clues)", dress.len() == 1, format!("guilty {guilty}; dress task {:?}", dress.iter().map(|t| p.task_text(t)).collect::<Vec<_>>()));
    Ok(rows)
}

/// Lord Shaw's duel (BOYLE_MISSION.md PN tasks): delivering Pendleton's note to Shaw in the
/// garden starts the duel; Shaw is followed to the duel ground, both take their marks and the
/// duel is fought.
fn duel(p: &mut Play, db: &dishonored_kismet::KismetDb) -> Result<Vec<Stage>> {
    let mut rows: Vec<Stage> = Vec::new();
    p.until(90.0, |m| m.state.task(&format!("{BA}.DishonoredTask_Custom_12")) == Some(TaskState::Active));
    let pn0 = "PendeltonsNote.DishonoredTask_Custom_0".to_string();
    let pn2 = "PendeltonsNote.DishonoredTask_Custom_2".to_string();
    // The note from Pendleton (given in the previous mission).
    *p.mm().state.items.entry("Pendleton_Shaw_Note".into()).or_insert(0) += 1;
    stage(&mut rows, "Pendleton's note task (PN T0) is active", p.task(&pn0) == TaskState::Active, format!("PN T0 {:?} \"{}\"", p.task(&pn0), p.task_text(&pn0)));
    let tv = actor(db, "L_Boyle_Ext_Script:TriggerVolume_14")?;
    p.enter(tv);
    p.idle(2.0);
    let Some(shaw) = spawned_pawn(p, db, "L_Boyle_Ext_Script:DishonoredSpawner_45") else {
        stage(&mut rows, "Lord Shaw is spawned", false, "no pawn".into());
        return Ok(rows);
    };
    // Give him the note: walk up to him (his note trigger is attached to his head) and talk.
    let mut read = false;
    for _ in 0..4 {
        p.near_pawn(shaw, 80.0);
        p.step(PlayerInput::default());
        p.talk_to(shaw);
        if p.until_answering(30.0, |m| m.state.task(&"PendeltonsNote.DishonoredTask_Custom_2".to_string()) == Some(TaskState::Active) || m.state.task(&"PendeltonsNote.DishonoredTask_Custom_0".to_string()) == Some(TaskState::Completed)) {
            read = true;
            break;
        }
    }
    stage(&mut rows, "the note delivered: Shaw reads it, PN T0 completes and PN T2 \"Follow Lord Shaw\" activates", read && p.task(&pn2) == TaskState::Active, format!("PN T0 {:?}; PN T2 {:?} at {:.1} s", p.task(&pn0), p.task(&pn2), p.time()));
    Ok(rows)
}

/// Dress tasks of a sister (T13..T21) in a state.
fn dress_tasks(p: &Play, sister: &str, s: TaskState) -> Vec<String> {
    let first = match sister {
        "Esma" => 13,
        "Lydia" => 16,
        _ => 19,
    };
    (first..first + 3).map(|n| format!("{BA}.DishonoredTask_Custom_{n}")).filter(|t| p.task(t) == s).collect()
}

fn coverage(p: &Play, rep: &mut Report, db: &dishonored_kismet::KismetDb) {
    let Some(k) = &p.m().kismet else { return };
    let cov = &k.k.coverage;
    let executed: std::collections::BTreeSet<String> = cov.activations.keys().map(|o| k.k.class(*o).to_string()).collect();
    let total_classes = db.class_counts(&[]).into_iter().filter(|(c, _)| !c.starts_with("SeqVar") && !c.starts_with("DisSeqVar") && c != "SequenceFrame" && c != "SequenceFrameWrapped" && c != "InterpData").count();
    let activations: u32 = cov.activations.values().sum();
    rep.check(S, "coverage report", true, format!("{} op classes executed of {} op classes in the data; {} activations of {} distinct ops; {} host-handled / {} presentation / {} unsupported activations ({} unsupported classes)", executed.len(), total_classes, activations, cov.activations.len(), cov.total_handled(), k.st.presentation.values().sum::<u32>(), cov.total_unhandled(), cov.unhandled.len()));
    let mut rows: Vec<Vec<String>> = cov.handled.iter().map(|(c, n)| vec![c.clone(), n.to_string(), "run".into()]).collect();
    rows.extend(k.st.presentation.iter().map(|(c, n)| vec![c.clone(), n.to_string(), "presentation (no-op)".into()]));
    rows.extend(k.st.not_modelled.iter().map(|(c, n)| vec![c.clone(), n.to_string(), "no port system (decoded, auto outputs)".into()]));
    rows.extend(cov.unhandled.iter().map(|(c, n)| vec![c.clone(), n.to_string(), "UNSUPPORTED (logged, auto outputs)".into()]));
    rows.sort_by(|a, b| b[1].parse::<u32>().unwrap_or(0).cmp(&a[1].parse::<u32>().unwrap_or(0)));
    rep.table("Kismet op classes activated", &["class", "activations", "handling"], rows);
    let mut top: Vec<(OpId, u32)> = cov.activations.iter().map(|(o, n)| (*o, *n)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    rep.table("Most activated ops", &["op", "class", "activations"], top.into_iter().take(25).map(|(o, n)| vec![k.k.db.describe(o), k.k.class(o).to_string(), n.to_string()]).collect());
    rep.table("Mission history (host)", &["event"], k.st.history.iter().map(|h| vec![h.clone()]).collect());
    rep.table("Task transitions", &["time", "task", "state", "text"], p.tasks.iter().map(|(t, path, s)| vec![format!("{t:.1}"), path.clone(), format!("{s:?}"), p.task_text(path)]).collect());
}

fn dump(p: &Play) {
    let m = p.m();
    let Some(k) = &m.kismet else { return };
    println!("--- history");
    for h in &k.st.history {
        println!("{h}");
    }
    println!("--- interpreter log");
    for l in &k.k.log {
        println!("{l}");
    }
    println!("--- tasks");
    for (t, path, s) in &p.tasks {
        println!("[{t:.1}] {path} -> {s:?}");
    }
    if let Some(v) = &m.view.mission {
        for o in &v.objectives {
            if o.hidden {
                continue;
            }
            println!("objective \"{}\"", o.description);
            for t in &o.tasks {
                if !t.hidden {
                    println!("   [{:?}] {}", t.state, t.description);
                }
            }
        }
        println!("messages {:?}", v.messages);
        println!("usables {}", v.usables.len());
    }
    println!("map {} players at {:?}", m.current_map(), m.world.player(AGENT).map(|p| p.position));
    println!("unhandled {:?}", k.k.coverage.unhandled);
    println!("conversation errors {:?}", m.state.conv_errors);
    for e in &m.state.conv_log {
        match e {
            dishonored_conversation::runner::ConvEvent::Line { .. } | dishonored_conversation::runner::ConvEvent::Flag { .. } => {}
            other => println!("conv {other:?}"),
        }
    }
    // KISMET_TRACE=Package:Export,... : activation counts and state of chosen ops.
    if let Ok(t) = std::env::var("KISMET_TRACE") {
        for item in t.split(',') {
            let Some((pkg, e)) = item.split_once(':') else { continue };
            let Some(id) = e.parse().ok().and_then(|e| k.k.db.find_export(pkg, e)) else { continue };
            let rt = &k.k.ops[id as usize];
            println!("trace {} {}: activations {} active {} trigger {} enabled {} props {:?}", k.k.db.describe(id), k.k.class(id), k.k.coverage.activations.get(&id).copied().unwrap_or(0), rt.active, rt.trigger_count, rt.enabled, rt.props.0);
        }
        for (s, o) in k.k.pending() {
            println!("pending in {}: {} {}", k.k.db.describe(s), k.k.db.describe(o), k.k.class(o));
        }
    }
    println!("presentation {:?}", k.st.presentation);
    let _ = (Obj::None, 0 as OpId, buttons::USE, Vec3::ZERO, kismet_host::PRESENTATION);
}
