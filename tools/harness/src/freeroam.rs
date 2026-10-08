//! `harness freeroam [--pair A,B] [--maps M,N...]`: free roam between the maps of one
//! campaign level (interior / exterior: Boyle Ext / Int, the Bridge parts, the Flooded parts,
//! Tower Return yard / interior...) through the original travel.
//!
//! The pairs come from the original travel data (`dishonored_content::level_group`: two maps
//! whose Kismet can each ask the global `ChangeLvl_*` handlers for the other without crossing a
//! mission boundary). For each pair the free-roam session (`dishonored_content::freeroam_def`,
//! the original Kismet started by the map's campaign entry event) starts on one map; the agent
//! then sets off the level change to the other map the way the level wires it: it uses the
//! travel door (`DisSeqEvent_Used` / `SeqEvent_Used`) or steps into the trigger
//! (`SeqEvent_Touch`) whose Kismet leads to the `ChangeLvl_*` remote event, answering a
//! confirmation with "go"; the original Kismet does the rest (travel destination, prepare /
//! commit, the arrival `PlayerStart`). Checked both ways: the arrival map, the tagged
//! `PlayerStart`, standing on the new map's floor, the inventory kept. The agent is placed at
//! the door or trigger (it does not path across the map).

use crate::Ctx;
use crate::report::Report;
use anyhow::{Context, Result};
use dishonored_content::MapContents;
use dishonored_kismet::{KismetDb, Kind, Obj, OpId};
use dishonored_multiplayer::game::Game;
use dishonored_multiplayer::mission::MissionMatch;
use dishonored_multiplayer::mission::kismet_host;
use dishonored_sim::input::PlayerInput;
use glam::Vec3;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const S: &str = "Free roam travel";
const AGENT: u32 = 1;

struct Play<'a> {
    g: Game,
    maps: &'a MapContents,
    tick: u64,
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
    fn step(&mut self) {
        self.tick += 1;
        let (yaw, pitch) = self.m().world.player(AGENT).map(|p| (p.yaw, p.pitch)).unwrap_or_default();
        let sim = &self.maps.maps[0].sim;
        self.g.step(&[(AGENT, PlayerInput { tick: self.tick, yaw, pitch, ..Default::default() })], &self.maps.set, sim);
        if let Some(cm) = self.g.current_map() {
            self.maps.set.set_active(cm);
        }
    }
    /// Run until `f` holds, answering a scripted choice with its first option ("go").
    fn until(&mut self, secs: f32, f: impl Fn(&MissionMatch) -> bool) -> bool {
        for _ in 0..(secs * 60.0) as u32 {
            if f(self.m()) {
                return true;
            }
            if self.m().kismet.as_ref().is_some_and(|k| k.choosing(AGENT)) {
                kismet_host::choice(self.mm(), AGENT, 0);
            }
            self.step();
        }
        f(self.m())
    }
    fn history(&self) -> Vec<String> {
        self.m().kismet.as_ref().map(|k| k.st.history.clone()).unwrap_or_default()
    }
}

/// Every map the global handlers can prepare (the campaign's persistent maps).
fn campaign_maps(content: &dishonored_assets::Content) -> Vec<String> {
    let mut maps: Vec<String> = Vec::new();
    for (_, d, _) in dishonored_assets::travel::global_entries(content) {
        for m in d {
            if content.has_package(&m) && !maps.iter().any(|x| x.eq_ignore_ascii_case(&m)) {
                maps.push(m);
            }
        }
    }
    maps.sort_by_key(|m| m.to_ascii_lowercase());
    maps
}

/// Kismet events that lead to an op, through output links, remote events and sequence
/// entries: the doors (`Used`) and triggers (`Touch`) with their actor, nearest first.
fn origins(db: &KismetDb, from: &[OpId]) -> Vec<(OpId, u32)> {
    let mut incoming: BTreeMap<OpId, Vec<OpId>> = BTreeMap::new();
    let mut remote: BTreeMap<String, Vec<OpId>> = BTreeMap::new();
    for o in &db.ops {
        if o.class == "SeqAct_ActivateRemoteEvent" {
            remote.entry(o.props.str("EventName").to_ascii_lowercase()).or_default().push(o.id);
        }
    }
    for o in &db.ops {
        for l in &o.outputs {
            for (t, _) in &l.links {
                incoming.entry(*t).or_default().push(o.id);
            }
            if let Some(f) = l.linked_op {
                incoming.entry(o.id).or_default().push(f);
            }
        }
        if o.kind == Kind::Sequence {
            for l in &o.inputs {
                if let Some(e) = l.linked_op {
                    incoming.entry(e).or_default().push(o.id);
                }
            }
        }
        if db.is_a(&o.class, "SeqEvent_RemoteEvent") {
            if let Some(src) = remote.get(&o.props.str("EventName").to_ascii_lowercase()) {
                incoming.entry(o.id).or_default().extend(src.iter().copied());
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut q: VecDeque<OpId> = from.iter().copied().collect();
    while let Some(o) = q.pop_front() {
        if !seen.insert(o) || seen.len() > 5000 {
            continue;
        }
        let op = db.op(o);
        let door = ["DisSeqEvent_Used", "SeqEvent_Used", "SeqEvent_Touch"].iter().any(|c| db.is_a(&op.class, c));
        if door {
            if let Obj::Actor(a) = op.props.obj("Originator") {
                out.push((o, a));
            }
        }
        q.extend(incoming.get(&o).cloned().unwrap_or_default());
    }
    out
}

/// A point inside a trigger actor's touch shape.
fn shape_pos(db: &KismetDb, a: u32) -> Vec3 {
    let act = &db.actors[a as usize];
    match &act.shape {
        Some(dishonored_kismet::Shape::Boxes(b)) if !b.is_empty() => {
            let c = [(b[0][0][0] + b[0][1][0]) * 0.5, (b[0][0][1] + b[0][1][1]) * 0.5, b[0][0][2] + 60.0];
            Vec3::from(act.location) + Vec3::new(c[0] * act.scale[0], c[1] * act.scale[1], c[2] * act.scale[2])
        }
        _ => Vec3::from(act.location),
    }
}

/// Set off the level change from the current map to `to`; returns what was tried.
fn travel(p: &mut Play, db: &KismetDb, events: &[String], to: &str) -> (bool, String) {
    let from = p.m().current_map().to_string();
    let pk: Vec<u16> = db.maps.iter().find(|m| m.0.eq_ignore_ascii_case(&from)).map(|m| m.1.clone()).unwrap_or_default();
    let senders: Vec<OpId> = db.ops.iter().filter(|o| pk.contains(&o.package) && o.class == "SeqAct_ActivateRemoteEvent" && events.iter().any(|e| e.eq_ignore_ascii_case(&o.props.str("EventName")))).map(|o| o.id).collect();
    let found = origins(db, &senders);
    let mut tried = Vec::new();
    // Doors first, then triggers.
    let mut order: Vec<(OpId, u32)> = found.iter().copied().filter(|(e, _)| !db.op_is_a(*e, "SeqEvent_Touch")).collect();
    order.extend(found.iter().copied().filter(|(e, _)| db.op_is_a(*e, "SeqEvent_Touch")));
    let arrived = |m: &MissionMatch| m.current_map().eq_ignore_ascii_case(to);
    for (e, a) in order.into_iter().take(6) {
        let act = db.actors[a as usize].clone();
        tried.push(format!("{} {}:{}", db.op(e).class, act.package, act.name));
        if db.op_is_a(e, "SeqEvent_Touch") {
            let pos = shape_pos(db, a);
            let me = p.mm().world.player_mut(AGENT).expect("agent");
            me.position = pos;
            me.velocity = Vec3::ZERO;
        } else {
            // Stand at the door and use it.
            let pos = p.m().kismet.as_ref().map(|k| k.actor_pos(a)).unwrap_or(Vec3::from(act.location));
            let w = p.maps.get(&from).map(|m| m.collision.clone());
            let spot = w.as_ref().and_then(|w| (0..12).find_map(|i| {
                let ang = i as f32 * std::f32::consts::TAU / 12.0;
                dishonored_ai::snap_floor(w, pos + Vec3::new(ang.cos() * 110.0, ang.sin() * 110.0, 40.0)).filter(|f| (f.z - pos.z).abs() < 200.0)
            }));
            let me = p.mm().world.player_mut(AGENT).expect("agent");
            if let Some(s) = spot {
                me.position = s;
            }
            me.velocity = Vec3::ZERO;
            if let Some(run) = p.mm().kismet.as_mut() {
                run.st.uses.push((AGENT, kismet_host::KUse::Actor(a)));
            }
        }
        if p.until(20.0, arrived) {
            return (true, tried.join(", "));
        }
    }
    if tried.is_empty() {
        tried.push(format!("no door or trigger in {from}'s Kismet leads to {}", events.join(" / ")));
    }
    (false, tried.join(", "))
}

/// The tag of the last commit into `map` (`commit map change <map> at <tag>`).
fn commit_tag(h: &[String], map: &str) -> Option<String> {
    let key = format!("commit map change {} at ", map.to_ascii_lowercase());
    h.iter().rev().find_map(|l| l.to_ascii_lowercase().find(&key).map(|i| l[i + key.len()..].trim().to_string()))
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let content = dishonored_assets::Content::locate()?;
    let maps: Vec<String> = match ctx.opt("--maps") {
        Some(m) => m.split(',').map(str::to_string).collect(),
        None => campaign_maps(&content),
    };
    // Campaign levels and their int / ext pairs.
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut groups: Vec<Vec<String>> = Vec::new();
    for m in &maps {
        if groups.iter().any(|g| g.iter().any(|x| x.eq_ignore_ascii_case(m))) {
            continue;
        }
        let g = dishonored_content::level_group(&content, m);
        // A pair: each map's Kismet can travel to the other (both directions exist).
        let goes = |x: &str, y: &str| dishonored_content::map_exits(&content, x).into_iter().any(|e| !e.mission_boundary && e.destinations.iter().any(|d| d.eq_ignore_ascii_case(y)));
        for (i, a) in g.iter().enumerate() {
            for b in &g[i + 1..] {
                if goes(a, b) && goes(b, a) {
                    let key = if a.to_ascii_lowercase() < b.to_ascii_lowercase() { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) };
                    if !pairs.contains(&key) {
                        pairs.push(key);
                    }
                }
            }
        }
        groups.push(g);
    }
    rep.table("Campaign levels (maps reachable from each other without a mission boundary)", &["level", "maps"], groups.iter().filter(|g| g.len() > 1).enumerate().map(|(i, g)| vec![(i + 1).to_string(), g.join(", ")]).collect());
    rep.table("Interior / exterior pairs (from the maps' travel data)", &["map", "map"], pairs.iter().map(|(a, b)| vec![a.clone(), b.clone()]).collect());
    rep.check(S, "the original travel data gives the campaign levels' map pairs", !pairs.is_empty(), format!("{} pairs in {} multi-map levels over {} maps", pairs.len(), groups.iter().filter(|g| g.len() > 1).count(), maps.len()));
    if let Some(only) = ctx.opt("--pair") {
        let (a, b) = only.split_once(',').context("--pair A,B")?;
        pairs.retain(|p| (p.0.eq_ignore_ascii_case(a) && p.1.eq_ignore_ascii_case(b)) || (p.0.eq_ignore_ascii_case(b) && p.1.eq_ignore_ascii_case(a)));
    }
    for (a, b) in pairs {
        if let Err(e) = pair(rep, &content, &a, &b) {
            rep.check(S, &format!("{a} <-> {b}"), false, format!("{e:#}"));
        }
    }
    Ok(())
}

/// One pair both ways: a free-roam session on `a` (its level's other maps limited to `b`).
fn pair(rep: &mut Report, content: &dishonored_assets::Content, a: &str, b: &str) -> Result<()> {
    let mut def = dishonored_content::freeroam_def(a, 1)?;
    def.maps = vec![b.to_string()];
    def.original_npcs = vec![a.to_string(), b.to_string()];
    let start = def.original_kismet.as_ref().map(|k| k.start_event.clone()).unwrap_or_default();
    let maps = MapContents::load(&dishonored_content::mission_maps(&def))?;
    let setup = dishonored_content::mission_setup(&maps, def.clone()).map_err(|e| anyhow::anyhow!(e))?;
    let dishonored_multiplayer::game::GameSetup::Mission(ms) = &setup else { anyhow::bail!("not a mission") };
    let db = ms.kismet.as_ref().context("no Kismet")?.db.clone();
    let mc = maps.get(a).context("map a")?.clone();
    maps.set.set_active(a);
    let mut p = Play { g: setup.build(&mc.sim), maps: &maps, tick: 0 };
    p.g.add_player(AGENT, "agent", 0, &maps.set, &mc.sim);
    let started = p.until(20.0, |m| m.kismet.as_ref().is_some_and(|k| k.st.history.iter().any(|h| h.to_ascii_lowercase().contains(&format!("commit map change {}", a.to_ascii_lowercase())))));
    rep.check(S, &format!("free roam starts on {a} through {start}"), started && p.m().current_map().eq_ignore_ascii_case(a), format!("on {}; {}", p.m().current_map(), p.history().iter().filter(|h| h.contains("map change") || h.contains("leaves")).take(3).cloned().collect::<Vec<_>>().join("; ")));
    if !started {
        return Ok(());
    }
    p.until(2.0, |_| false);
    for (from, to) in [(a, b), (b, a)] {
        if !p.m().current_map().eq_ignore_ascii_case(from) {
            break;
        }
        // Inventory to carry across.
        if let Some(me) = p.mm().world.player_mut(AGENT) {
            me.inv.coins = 1234;
        }
        let events: Vec<String> = dishonored_content::map_exits(content, from).into_iter().filter(|e| !e.mission_boundary && e.destinations.iter().any(|d| d.eq_ignore_ascii_case(to))).map(|e| e.change_event).collect();
        let h0 = p.history().len();
        let (ok, how) = travel(&mut p, &db, &events, to);
        let tag = commit_tag(&p.history(), to).unwrap_or_default();
        let at = p.m().world.player(AGENT).map(|x| x.position).unwrap_or_default();
        let start = p.m().setup.tagged_starts.iter().find(|t| t.0.eq_ignore_ascii_case(to) && t.1.eq_ignore_ascii_case(&tag)).map(|t| t.2);
        let d = start.map(|s| s.truncate().distance(at.truncate()));
        p.until(1.0, |_| false);
        let z1 = p.m().world.player(AGENT).map(|x| x.position.z).unwrap_or_default();
        let coins = p.m().world.player(AGENT).map(|x| x.inv.coins).unwrap_or(0);
        // What the Kismet did meanwhile (the level change, or why it did not happen).
        let hist = p.history();
        let since = &hist[h0.min(hist.len())..];
        // A first visit can start the level's own intro (a matinee holding the player, or
        // cinematic mode): the player is moved by the script, not falling.
        let scripted = p.m().kismet.as_ref().is_some_and(|k| k.locked(AGENT)) || since.iter().any(|l| l.contains("cinematic mode on") || (l.contains("matinee") && l.contains("Player")));
        let not_falling = z1 > at.z - 60.0 || scripted;
        let pass = ok && d.is_some_and(|d| d < 250.0) && not_falling && coins == 1234;
        let locked = !ok && since.iter().any(|l| l.contains("while locked"));
        if locked {
            rep.check(S, &format!("{from} -> {to}: the way is locked by the level's own state (DisSeqAct_Lock; the original's rule), not taken"), true, format!("via {how}; {}", since.iter().filter(|l| l.contains("locked")).cloned().collect::<Vec<_>>().join(" | ")));
            continue;
        }
        let kept: Vec<&String> = hist[h0.min(hist.len())..].iter().filter(|l| !l.contains("not modelled") && !l.contains("unsupported") && !l.contains("spawned ")).collect();
        let tail: Vec<String> = kept[kept.len().saturating_sub(if pass { 3 } else { 10 })..].iter().map(|s| s.to_string()).collect();
        rep.check(
            S,
            &format!("{from} -> {to}: arrives at the tagged PlayerStart, on the floor, inventory kept"),
            pass,
            format!("via {how}; now on {}; tag {tag:?}, {:.0} uu from it; z {:.0} -> {:.0} after 1 s{}; coins {coins}; Kismet: {}", p.m().current_map(), d.unwrap_or(f32::NAN), at.z, z1, if scripted { " (the level's intro moves the player)" } else { "" }, tail.join(" | ")),
        );
    }
    Ok(())
}
