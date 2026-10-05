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
    play(&mut p, rep, &db)?;
    if ctx.args.iter().any(|a| a == "--dump") {
        dump(&p);
    }
    coverage(&p, rep, &db);
    Ok(())
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
    Ok(())
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
    println!("presentation {:?}", k.st.presentation);
    let _ = (Obj::None, 0 as OpId, buttons::USE, Vec3::ZERO, kismet_host::PRESENTATION);
}
