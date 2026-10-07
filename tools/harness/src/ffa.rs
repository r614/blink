//! Headless free-for-all with test bots on the real map, as fast as the simulation runs:
//! do bots meet and kill each other, and how does everyone die? Every death is logged with
//! its position, movement mode and the physics / water volume it happened in, so world
//! deaths can be checked against the level data.
//!
//! ```text
//! harness ffa [--players 3] [--minutes 5] [--map L_Boyle_Ext_P] [--seed 1]
//! ```

use crate::Ctx;
use crate::report::Report;
use anyhow::Result;
use dishonored_bots::{Bot, BotStyle};
use dishonored_content::{DEFAULT_MAP, MatchContent, load_match};
use dishonored_multiplayer::ffa::{FfaRules, SpawnPoint};
use dishonored_multiplayer::game::GameSetup;
use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::state::MoveMode;
use dishonored_world::WorldQuery;
use std::collections::BTreeMap;

const S: &str = "Free-for-all with test bots (headless)";

pub struct Death {
    pub tick: u64,
    pub victim: u32,
    pub killer: Option<u32>,
    pub cause: DamageCause,
    pub detail: String,
}

pub struct FfaStats {
    pub deaths: Vec<Death>,
    pub player_kills: usize,
    pub scores: Vec<(String, i32, i32, i32)>,
}

/// Style of the bot in slot `i` (the autopilot styles alternate the same way).
pub fn style(i: usize) -> BotStyle {
    if i % 2 == 0 { BotStyle::Duelist } else { BotStyle::Gunner }
}

/// Where a pawn is: position, mode and the volumes around it.
pub fn where_is(mc: &MatchContent, p: &dishonored_sim::state::PlayerState) -> String {
    let w = &mc.collision;
    let pain: Vec<&str> = w.physics_volumes().iter().filter(|v| v.pain.is_some() && v.touches(p.position, 34.0, 90.0)).map(|v| v.name.as_str()).collect();
    let water = w.water_at(p.position).map(|i| format!(" water surface {:.0}", i.surface_z)).unwrap_or_default();
    format!("pos {:.0?} vel {:.0?} mode {:?}{water}{}", p.position, p.velocity, p.mode, if pain.is_empty() { String::new() } else { format!(" pain {pain:?}") })
}

/// `all_spawns`: every PlayerStart of the map (islands and the scripted drop-off included),
/// to exercise the bots' hazard avoidance instead of the FFA spawn choice.
pub fn run_headless(mc: &MatchContent, players: usize, minutes: f32, seed: u64, all_spawns: bool) -> FfaStats {
    let spawns: Vec<SpawnPoint> = if all_spawns { mc.spawns.iter().map(|s| SpawnPoint { position: s.position, yaw: s.yaw }).collect() } else { dishonored_content::ffa_spawns(mc) };
    let rules = FfaRules { warmup_time: 1.0, min_players: 1, time_limit: minutes * 60.0 + 60.0, score_limit: 1000, ..Default::default() };
    let mut g = GameSetup::Ffa { rules, spawns }.build(&mc.sim);
    let mut bots: Vec<Bot> = (0..players).map(|i| Bot::new(i as u32 + 1, style(i), seed * 100 + i as u64)).collect();
    for b in &bots {
        g.add_player(b.id, &format!("bot{}", b.id), dishonored_net::server::PLAYER_ARCHETYPES[b.id as usize % 3], &mc.collision, &mc.sim);
    }
    let debug = std::env::var_os("FFA_DEBUG").is_some();
    let mut st = FfaStats { deaths: Vec::new(), player_kills: 0, scores: Vec::new() };
    let mut last: BTreeMap<u32, (glam::Vec3, Option<glam::Vec3>)> = BTreeMap::new();
    for tick in 0..((minutes * 60.0 * 60.0) as u64) {
        let snap = g.world().players.clone();
        let inputs: Vec<_> = bots
            .iter_mut()
            .filter_map(|b| {
                let me = snap.iter().find(|p| p.id == b.id)?;
                Some((b.id, b.think(me, &snap, &mc.collision)))
            })
            .collect();
        if debug && tick % 600 == 0 {
            for b in &bots {
                if let Some(me) = snap.iter().find(|p| p.id == b.id) {
                    eprintln!("t={:.0}s bot{} alive {} {} target {:?} hunt {:?} goal {:.0?}", tick as f32 / 60.0, b.id, me.alive, where_is(mc, me), b.target, b.hunt, b.goal());
                }
            }
        }
        let out = g.step(&inputs, &mc.collision, &mc.sim);
        // Where each pawn last stood on the ground, and where it went into the water.
        for p in &g.world().players {
            let e = last.entry(p.id).or_insert((p.position, None));
            match p.mode {
                MoveMode::Walking => *e = (p.position, None),
                MoveMode::Swimming if e.1.is_none() => e.1 = Some(p.position),
                _ => {}
            }
        }
        for e in &out.events {
            if let SimEvent::Death { victim, killer, cause } = *e {
                let Some(v) = g.world().player(victim) else { continue };
                let (stood, swam) = last.get(&victim).copied().unwrap_or((v.position, None));
                let detail = format!("{}; last stood at {stood:.0?}{}", where_is(mc, v), swam.map(|s| format!(", went into the water at {s:.0?}")).unwrap_or_default());
                if killer.is_some_and(|k| k != victim) {
                    st.player_kills += 1;
                }
                if debug {
                    eprintln!("t={:.1}s bot{victim} died {cause:?} killer {killer:?} {detail}", tick as f32 / 60.0);
                }
                st.deaths.push(Death { tick, victim, killer, cause, detail });
            }
        }
    }
    st.scores = g.view().scores.iter().map(|s| (s.name.clone(), s.score, s.kills, s.deaths)).collect();
    st
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let players: usize = ctx.opt("--players").and_then(|s| s.parse().ok()).unwrap_or(3);
    let minutes: f32 = ctx.opt("--minutes").and_then(|s| s.parse().ok()).unwrap_or(5.0);
    let seed: u64 = ctx.opt("--seed").and_then(|s| s.parse().ok()).unwrap_or(1);
    let map = ctx.opt("--map").unwrap_or_else(|| DEFAULT_MAP.into());
    let mc = load_match(&map)?;
    let spawns = dishonored_content::ffa_spawns(&mc);
    rep.check(S, "FFA spawns: PlayerStarts on the navmesh, walkable between", spawns.len() >= 2, format!("{} of {} PlayerStarts: {:.0?}", spawns.len(), mc.spawns.len(), spawns.iter().map(|s| s.position).collect::<Vec<_>>()));
    for (all, mins) in [(false, minutes), (true, (minutes * 0.6).max(1.0))] {
        let label = if all { "every PlayerStart" } else { "FFA spawns" };
        let t = std::time::Instant::now();
        let st = run_headless(&mc, players, mins, seed, all);
        let mut by_cause: BTreeMap<String, usize> = BTreeMap::new();
        for d in &st.deaths {
            let key = if d.killer.is_some_and(|k| k != d.victim) { format!("kill: {:?}", d.cause) } else { format!("self / world: {:?}", d.cause) };
            *by_cause.entry(key).or_default() += 1;
        }
        rep.table(
            &format!("Deaths in {mins} min, {players} bots on {map}, {label} (simulated in {:.0?})", t.elapsed()),
            &["cause", "count"],
            by_cause.iter().map(|(k, n)| vec![k.clone(), n.to_string()]).collect(),
        );
        let env: Vec<&Death> = st.deaths.iter().filter(|d| d.killer.is_none_or(|k| k == d.victim)).collect();
        rep.table(
            &format!("Environmental deaths, {label}"),
            &["time", "victim", "cause", "where"],
            env.iter().map(|d| vec![format!("{:.1}s", d.tick as f32 / 60.0), d.victim.to_string(), format!("{:?}", d.cause), d.detail.clone()]).collect(),
        );
        rep.table(&format!("Final scores, {label}"), &["player", "score", "kills", "deaths"], st.scores.iter().map(|s| vec![s.0.clone(), s.1.to_string(), s.2.to_string(), s.3.to_string()]).collect());
        if all {
            // Island starts: some bots never meet; what matters is that they stay alive.
            rep.check(S, "bots from every PlayerStart keep off deadly drops and water", env.len() <= 1, format!("{} environmental deaths of {} in {mins} min", env.len(), st.deaths.len()));
        } else {
            let per_5 = st.player_kills as f32 * 5.0 / mins.max(0.1);
            rep.check(S, "bots find and kill each other", per_5 >= 3.0, format!("{} player kills in {mins} min ({per_5:.1} per 5 min, need 3)", st.player_kills));
            rep.check(S, "no environmental deaths with the FFA spawns", env.is_empty(), format!("{} of {} deaths", env.len(), st.deaths.len()));
        }
    }
    Ok(())
}
