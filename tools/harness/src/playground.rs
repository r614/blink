//! `harness playground`: the Playground's stations (maps/playground.ron, missions/playground.ron)
//! played with scripted input on the headless simulation, each mechanic checked against the
//! values measured on the original (research/dishonored/PLAYER.md, HOOK_TRIAGE.md). Movement
//! checks that fit the oracle's scenario format are spec files under
//! `fixtures/playground/scenarios/` (`dishonored_oracle::scenario`), run here on the port.

use crate::Ctx;
use crate::report::Report;
use anyhow::{Context, Result};
use dishonored_content::{MapContents, MatchContent};
use dishonored_mission::MissionDef;
use dishonored_multiplayer::game::Game;
use dishonored_multiplayer::mission::MissionMatch;
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::state::{MoveMode, PlayerState, WorldState, power};
use dishonored_sim::{spawn_player, step_world};
use dishonored_world::WorldQuery;
use glam::Vec3;

const S: &str = "Playground";
const MAP: &str = "Playground";
const ME: u32 = 1;

/// A lone pawn on the Playground's world (movement stations).
struct Solo<'a> {
    mc: &'a MatchContent,
    st: WorldState,
    events: Vec<(u64, SimEvent)>,
}

impl<'a> Solo<'a> {
    fn new(mc: &'a MatchContent, at: Vec3, yaw: f32) -> Self {
        let mut st = WorldState::default();
        spawn_player(&mut st, ME, at, yaw, &mc.sim);
        let mut s = Solo { mc, st, events: Vec::new() };
        for _ in 0..30 {
            s.step(PlayerInput { yaw, ..Default::default() });
        }
        s.events.clear();
        s
    }
    fn step(&mut self, i: PlayerInput) {
        let i = PlayerInput { tick: self.st.tick, ..i };
        let out = step_world(&mut self.st, &[(ME, i)], &self.mc.collision, &self.mc.sim);
        let t = self.st.tick;
        self.events.extend(out.events.into_iter().map(|e| (t, e)));
    }
    fn p(&self) -> &PlayerState {
        self.st.player(ME).expect("player")
    }
    fn run(&mut self, ticks: usize, i: PlayerInput) -> f32 {
        let mut max_z = f32::MIN;
        for _ in 0..ticks {
            self.step(i);
            max_z = max_z.max(self.p().position.z);
        }
        max_z
    }
}

fn fwd(yaw: f32) -> PlayerInput {
    PlayerInput { yaw, forward: 127, ..Default::default() }
}

/// A Playground mission match with the agent in it.
struct Play<'a> {
    g: Game,
    maps: &'a MapContents,
    tick: u64,
    events: Vec<(u64, SimEvent)>,
}

impl<'a> Play<'a> {
    fn new(maps: &'a MapContents, def: MissionDef) -> Result<Self> {
        let setup = dishonored_content::mission_setup(maps, def).map_err(|e| anyhow::anyhow!(e))?;
        let mc = maps.get(MAP).context("playground loaded")?;
        maps.set.set_active(MAP);
        let mut g = setup.build(&mc.sim);
        g.add_player(ME, "agent", 0, &maps.set, &mc.sim);
        Ok(Play { g, maps, tick: 0, events: Vec::new() })
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
    fn step(&mut self, i: PlayerInput) {
        self.tick += 1;
        let sim = &self.maps.get(MAP).unwrap().sim;
        let out = self.g.step(&[(ME, PlayerInput { tick: self.tick, ..i })], &self.maps.set, sim);
        let t = self.tick;
        self.events.extend(out.events.into_iter().map(|e| (t, e)));
    }
    fn idle(&mut self, ticks: usize, i: PlayerInput) {
        for _ in 0..ticks {
            self.step(i);
        }
    }
    fn p(&self) -> PlayerState {
        *self.m().world.player(ME).expect("agent")
    }
    /// Put the agent somewhere (the stations are far apart).
    fn teleport(&mut self, at: Vec3, yaw: f32) {
        if let Some(p) = self.mm().world.player_mut(ME) {
            p.position = at;
            p.velocity = Vec3::ZERO;
            p.yaw = yaw;
        }
        self.idle(20, PlayerInput { yaw, ..Default::default() });
    }
    fn npc(&self, id: &str) -> u32 {
        self.m().setup.npcs.iter().find(|n| n.0 == id).map(|n| n.1.id).unwrap_or(0)
    }
    /// Press use once (and release).
    fn use_once(&mut self, yaw: f32, pitch: f32) {
        self.step(PlayerInput { yaw, pitch, buttons: buttons::USE, ..Default::default() });
        self.idle(5, PlayerInput { yaw, pitch, ..Default::default() });
    }
    /// The yaw / pitch from the agent's eye to a point.
    fn aim(&self, at: Vec3) -> (f32, f32) {
        let mc = self.maps.get(MAP).unwrap();
        let p = self.p();
        let d = at - dishonored_sim::movement::eye_position(&p, &mc.sim.gameplay.movement);
        (d.y.atan2(d.x).to_degrees(), d.z.atan2(d.truncate().length()).to_degrees())
    }
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let t0 = std::time::Instant::now();
    let def = MissionDef::load(std::path::Path::new(&ctx.opt("--mission").unwrap_or_else(|| "missions/playground.ron".into())))?;
    let errs = def.validate();
    rep.check(S, "mission file valid", errs.is_empty(), errs.join("; "));
    // Every brush draws what it collides with (and the reverse), faces wound to show from
    // outside, no blockout or dev material, and a closed boundary taller than any reachable point.
    {
        let layout = dishonored_content::playground::layout_for(MAP).context("maps/playground.ron")??;
        let content = dishonored_assets::Content::locate()?;
        let geo = dishonored_content::playground::import_layout_geometry(&content, &layout)?;
        let errs = dishonored_content::playground::audit_brushes(&layout, &geo);
        rep.check(S, "brushes: render faces = collision faces, wound outward, thin skins only without collision", errs.is_empty(), if errs.is_empty() { format!("{} brushes", layout.brushes.len()) } else { errs.join("; ") });
        let dev: Vec<String> = layout.brushes.iter().filter_map(|b| b.material.clone()).chain(std::iter::once(layout.default_material.clone())).chain(layout.meshes.iter().flat_map(|m| m.materials.iter().flatten().cloned())).filter(|m| m.to_ascii_lowercase().contains("blockout") || m.to_ascii_lowercase().contains("_blk_")).collect();
        rep.check(S, "no blockout / dev materials in the layout", dev.is_empty(), dev.join(", "));
        // The boundary: walls on all four sides of the stations, up to 3000.
        let world = dishonored_world::CollisionWorld::from_map(&geo);
        let mut misses = Vec::new();
        for (x, y, dir) in [(0.0, 0.0, Vec3::X), (0.0, 0.0, -Vec3::X), (0.0, 0.0, Vec3::Y), (0.0, 0.0, -Vec3::Y), (5500.0, -1000.0, Vec3::X), (-5500.0, 7500.0, Vec3::Y), (-5500.0, -5500.0, -Vec3::Y)] {
            for z in [100.0f32, 1700.0, 2900.0] {
                let from = Vec3::new(x, y, z);
                // The boundary's inner faces: x = +-6000, y = -6000 and 8000.
                let hit = world.raycast(from, dir, 20000.0, dishonored_world::channel::PAWN).map(|h| h.point);
                if !hit.is_some_and(|p| p.x.abs() <= 6001.0 && (-6001.0..=8001.0).contains(&p.y)) {
                    misses.push(format!("{from:.0?} along {dir:?}"));
                }
            }
        }
        rep.check(S, "boundary: a wall blocks every way out up to 3000 (above the highest point, 1600 + a 500 Blink)", misses.is_empty(), if misses.is_empty() { "21 rays from inside at 100 / 1700 / 2900 up".into() } else { misses.join("; ") });
    }
    let maps = MapContents::load(&[MAP.to_string()])?;
    let mc = maps.get(MAP).context("playground")?.clone();
    let nav = mc.collision.nav.as_ref().map(|n| n.polys.len()).unwrap_or(0);
    rep.check(
        S,
        "layout loads: collision, navigation, volumes, chain, pickups, doors",
        mc.collision.tris.len() > 1000 && nav > 1000 && mc.collision.water.as_ref().map(|w| w.len()).unwrap_or(0) == 1 && mc.collision.physics_volumes().len() == 1 && mc.collision.climbables().len() == 1 && mc.pickups.len() == 7 && mc.usables.len() == 5 && mc.collision.movers.len() == 3,
        format!(
            "{} triangles, {} nav polygons, {} water, {} pain, {} chains, {} pickups, {} usables ({} door movers), {:.1?}",
            mc.collision.tris.len(),
            nav,
            mc.collision.water.as_ref().map(|w| w.len()).unwrap_or(0),
            mc.collision.physics_volumes().len(),
            mc.collision.climbables().len(),
            mc.pickups.len(),
            mc.usables.len(),
            mc.collision.movers.len(),
            t0.elapsed()
        ),
    );
    // Where the copied doors and usables stand (their bodies' bounds against the floor at z 0).
    let rows: Vec<Vec<String>> = mc
        .usables
        .iter()
        .map(|u| {
            let tris: Vec<Vec3> = mc.collision.movers.iter().filter(|m| m.actor == u.actor).flat_map(|m| m.tris.iter().flat_map(|t| t.iter().copied())).collect();
            let (lo, hi) = tris.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), v| (a.min(*v), b.max(*v)));
            vec![u.actor.clone(), format!("{:.0?}", u.location), format!("{:.0?}", u.center), if tris.is_empty() { "-".into() } else { format!("{:.0?} .. {:.0?}", lo, hi) }, u.prompt.clone(), u.locked_prompt.clone()]
        })
        .collect();
    rep.table("Doors and usables", &["actor", "location", "use point", "door bodies (closed)", "prompt", "locked prompt"], rows);
    scenarios(rep, &mc)?;
    steps(rep, &mc);
    slopes(rep, &mc);
    mantles(rep, &mc);
    gaps(rep, &mc);
    blink(rep, &mc);
    chain(rep, &mc);
    drowning(rep, &mc);
    falls(rep, &mc);
    lean_and_tunnel(rep, &mc);
    pain(rep, &mc);
    stations_in_mission(rep, &maps, &def)?;
    powers_in_mission(rep, &maps, &def)?;
    detection(rep, &maps)?;
    rep.check(S, "suite time", true, format!("{:.1?}", t0.elapsed()));
    Ok(())
}

/// Oracle-format scenario specs of the Playground (`fixtures/playground/scenarios`), on the port.
fn scenarios(rep: &mut Report, mc: &MatchContent) -> Result<()> {
    use dishonored_oracle::scenario::{Button, System};
    use dishonored_oracle::trace::Trace;
    let dir = ["fixtures/playground/scenarios", "../../fixtures/playground/scenarios"].iter().map(std::path::PathBuf::from).find(|p| p.is_dir()).context("fixtures/playground/scenarios")?;
    let specs = dishonored_oracle::scenario::load_dir(&dir)?;
    let mut rows = Vec::new();
    for (_, s) in &specs {
        if s.system != System::Movement || !s.map.eq_ignore_ascii_case(MAP) {
            continue;
        }
        let Some(ps) = &s.setup.player else { continue };
        let mut st = WorldState::default();
        spawn_player(&mut st, ME, Vec3::from(ps.position), ps.yaw, &mc.sim);
        let chans = [("pawn.x", "uu"), ("pawn.y", "uu"), ("pawn.z", "uu"), ("pawn.vz", "uu/s"), ("pawn.speed2d", "uu/s"), ("pawn.health", "pt")];
        let mut trace = Trace::new(&s.name, "port", s.tick_hz, &chans);
        let sample = |st: &WorldState| {
            let p = st.player(ME).unwrap();
            vec![p.position.x as f64, p.position.y as f64, p.position.z as f64, p.velocity.z as f64, p.velocity.truncate().length() as f64, p.vitals.health as f64]
        };
        trace.push(0, sample(&st));
        for (t, (f, r, yaw, pitch, held)) in s.input_timeline().iter().enumerate() {
            let b = held.iter().fold(0u16, |a, b| {
                a | match b {
                    Button::Jump => buttons::JUMP,
                    Button::Crouch => buttons::CROUCH,
                    Button::Sprint => buttons::SPRINT,
                    Button::Walk => buttons::WALK,
                    Button::Primary => buttons::PRIMARY,
                    Button::Secondary => buttons::SECONDARY,
                    Button::Block => buttons::BLOCK,
                    Button::LeanLeft => buttons::LEAN_LEFT,
                    Button::LeanRight => buttons::LEAN_RIGHT,
                    Button::Use => buttons::USE,
                    Button::Reload => buttons::RELOAD,
                }
            });
            let input = PlayerInput { tick: st.tick, forward: *f, right: *r, yaw: *yaw, pitch: *pitch, buttons: b, ..Default::default() };
            step_world(&mut st, &[(ME, input)], &mc.collision, &mc.sim);
            trace.push(t as u32 + 1, sample(&st));
        }
        for c in dishonored_oracle::diff::check_checkpoints(&trace, &s.checkpoints) {
            let actual = c.actual.map(|a| format!("{a:.2}")).unwrap_or_else(|| "-".into());
            rep.check(S, &format!("{}: {}", s.name, c.checkpoint), c.pass, format!("{} = {actual}, expected {} +- {} ({})", c.metric, c.expected, c.tol, c.source));
            rows.push(vec![s.name.clone(), c.checkpoint.clone(), actual, format!("{} +- {}", c.expected, c.tol)]);
        }
    }
    rep.table("Oracle-format scenarios on the Playground", &["scenario", "checkpoint", "port", "expected"], rows);
    Ok(())
}

/// Step heights: walking into each step (no jump) climbs it up to `MaxStepHeight` 44.
fn steps(rep: &mut Report, mc: &MatchContent) {
    let mut rows = Vec::new();
    let mut ok = true;
    for (x, h) in [(-5400.0, 30.0), (-5000.0, 44.0), (-4600.0, 50.0), (-4200.0, 56.0), (-3800.0, 60.0)] {
        let mut s = Solo::new(mc, Vec3::new(x, -3900.0, 92.0), 90.0);
        let z0 = s.p().position.z;
        let top = s.run(110, fwd(90.0)) - z0;
        let climbed = top > h - 3.0;
        rows.push(vec![format!("{h}"), format!("{top:.1}"), climbed.to_string()]);
        if (h <= 50.0 && !climbed) || (h >= 56.0 && climbed) {
            ok = false;
        }
    }
    rep.check(S, "step heights: 30 / 44 / 50 climbed, 56 / 60 not (stepUp rises MaxStepHeight + 2.54 from 6.73 above a per-polygon floor: up to 53.3)", ok, rows.iter().map(|r| format!("{} -> {}", r[0], r[2])).collect::<Vec<_>>().join(", "));
    rep.table("Step heights (walk in, no jump)", &["step (uu)", "rise (uu)", "climbed"], rows);
}

/// Slopes: walkable up to `WalkableFloorZ` = cos 50 (0.643).
fn slopes(rep: &mut Report, mc: &MatchContent) {
    let mut rows = Vec::new();
    let mut ok = true;
    for (x, a) in [(-2800.0, 40.0f32), (-2400.0, 45.0), (-2000.0, 50.0), (-1600.0, 55.0), (-1200.0, 60.0)] {
        let h = 400.0 * a.to_radians().tan();
        let mut s = Solo::new(mc, Vec3::new(x, -3900.0, 92.0), 90.0);
        let z0 = s.p().position.z;
        let top = s.run(180, fwd(90.0)) - z0;
        let up = top > 0.9 * h;
        rows.push(vec![format!("{a}"), format!("{h:.0}"), format!("{top:.1}"), up.to_string()]);
        if (a < 50.0 && !up) || (a > 50.0 && top > 0.5 * h) {
            ok = false;
        }
    }
    rep.check(S, "slopes: 40 / 45 walked up, 55 / 60 not (WalkableFloorZ cos 50)", ok, rows.iter().map(|r| format!("{} deg -> {}", r[0], r[3])).collect::<Vec<_>>().join(", "));
    rep.table("Slopes (run up, 400 long)", &["angle", "top (uu)", "rise (uu)", "reached the top"], rows);
}

/// Mantle: jump at a ledge; edges up to `m_MantleMaxEdgeHeight` 230 above the feet are mantled.
fn mantles(rep: &mut Report, mc: &MatchContent) {
    let mut rows = Vec::new();
    let mut ok = true;
    for (x, h) in [(-400.0, 60.0), (0.0, 120.0), (400.0, 200.0), (800.0, 260.0), (1200.0, 300.0), (1600.0, 330.0), (2000.0, 360.0)] {
        let mut s = Solo::new(mc, Vec3::new(x, -3900.0, 92.0), 90.0);
        let z0 = s.p().position.z;
        let mut top = f32::MIN;
        let mut jumped = false;
        for _ in 0..180 {
            let at_wall = s.p().position.y > -3500.0 - 35.0 - 30.0;
            let b = if at_wall && !jumped { buttons::JUMP } else { 0 };
            jumped |= b != 0;
            s.step(PlayerInput { buttons: b, ..fwd(90.0) });
            top = top.max(s.p().position.z);
        }
        let mantled = s.events.iter().any(|(_, e)| matches!(e, SimEvent::Mantle { .. }));
        let on_top = s.p().position.z - z0 > h - 5.0;
        rows.push(vec![format!("{h}"), format!("{:.1}", top - z0), mantled.to_string(), on_top.to_string()]);
        if (h <= 330.0 && !on_top) || (h >= 360.0 && on_top) {
            ok = false;
        }
    }
    rep.check(S, "mantle from a standing jump: ledges 60 to 330 reached, 360 not (edges up to m_MantleMaxEdgeHeight 230 above the feet, which the jump raises by up to 113)", ok, rows.iter().map(|r| format!("{} -> {}", r[0], r[3])).collect::<Vec<_>>().join(", "));
    rep.table("Mantle ledges (run, jump at the face)", &["ledge (uu)", "highest rise", "mantle event", "on top"], rows);
}

/// Jump gaps: run (360 uu/s with the sword) and sprint (540) off a 240-high platform.
fn gaps(rep: &mut Report, mc: &MatchContent) {
    let mut rows = Vec::new();
    let mut cleared = std::collections::BTreeMap::new();
    for sprint in [false, true] {
        for (x, gap) in [(2800.0, 100.0), (3400.0, 200.0), (4000.0, 300.0), (4600.0, 400.0), (5200.0, 500.0)] {
            let mut s = Solo::new(mc, Vec3::new(x, -3850.0, 240.0 + 92.0), 90.0);
            let mut jumped = false;
            for _ in 0..200 {
                let at_edge = s.p().position.y > -3500.0 - 25.0;
                let mut b = if sprint { buttons::SPRINT } else { 0 };
                if at_edge && !jumped && s.p().mode == MoveMode::Walking {
                    b |= buttons::JUMP;
                    jumped = true;
                }
                s.step(PlayerInput { buttons: b, ..fwd(90.0) });
                if jumped && s.p().mode == MoveMode::Walking && s.p().position.y > -3500.0 + gap {
                    break;
                }
            }
            let p = s.p();
            let landed = p.position.z > 240.0 && p.position.y > -3500.0 + gap;
            let grabbed = s.events.iter().any(|(_, e)| matches!(e, SimEvent::Mantle { .. }));
            cleared.insert((sprint, gap as i32), landed && !grabbed);
            rows.push(vec![if sprint { "sprint" } else { "run" }.into(), format!("{gap}"), landed.to_string(), grabbed.to_string()]);
        }
    }
    let c = |s: bool, g: i32| cleared.get(&(s, g)).copied().unwrap_or(false);
    // Airtime 0.533 s (measured): run 360 x 0.533 = 192, sprint 540 x 0.533 = 288 uu, plus the
    // pawn's 70 uu of width between the edges.
    let ok = c(false, 100) && c(false, 200) && !c(false, 400) && c(true, 300) && !c(true, 500);
    rep.check(S, "jump gaps: run clears 200 not 400, sprint clears 300 not 500 (airtime 0.533 s)", ok, rows.iter().map(|r| format!("{} {}: {}", r[0], r[1], r[2])).collect::<Vec<_>>().join(", "));
    rep.table("Jump gaps (jump at the edge)", &["gait", "gap (uu)", "landed across", "ledge grab"], rows);
}

/// Blink: level 2 reaches 1600 uu horizontally (1100 at level 1), ending 5-19 short of the
/// target (measured); 500 straight up.
fn blink(rep: &mut Report, mc: &MatchContent) {
    let one = |level: u8, pitch: f32| -> (f32, f32, f32) {
        let mut s = Solo::new(mc, Vec3::new(-5600.0, 0.0, 96.0), 0.0);
        if let Some(p) = s.st.player_mut(ME) {
            p.inv.power_levels[power::BLINK] = level;
        }
        let from = s.p().position;
        let mana = s.p().vitals.mana;
        let aim = |b| PlayerInput { yaw: 0.0, pitch, buttons: b, left_hand: LeftHand::Blink, ..Default::default() };
        for _ in 0..10 {
            s.step(aim(buttons::SECONDARY));
        }
        for _ in 0..90 {
            s.step(aim(0));
        }
        let p = s.p();
        ((p.position - from).truncate().length(), p.position.z - from.z, mana - p.vitals.mana)
    };
    let (d2, _, cost) = one(2, 0.0);
    let (d1, _, _) = one(1, 0.0);
    // Aim 19.84 degrees up (the measured case): the target is the eye plus the range vector,
    // `t = min(500 / d.z, 1600)` = 1385.7 horizontally and +500 (HOOK_TRIAGE.md). Read while
    // aiming: after the travel the pawn falls back to the floor.
    let (flat, up) = {
        let mut s = Solo::new(mc, Vec3::new(-5600.0, 0.0, 96.0), 0.0);
        let aim = PlayerInput { yaw: 0.0, pitch: 19.84, buttons: buttons::SECONDARY, left_hand: LeftHand::Blink, ..Default::default() };
        for _ in 0..10 {
            s.step(aim);
        }
        let eye = dishonored_sim::movement::eye_position(s.p(), &mc.sim.gameplay.movement);
        let t = s.p().left.blink_target.unwrap_or(eye);
        ((t - eye).truncate().length(), t.z - eye.z)
    };
    let ok = (1575.0..=1600.0).contains(&d2) && (1075.0..=1100.0).contains(&d1) && (cost - 20.0).abs() < 0.5;
    rep.check(S, "Blink range: level 2 1600, level 1 1100, ends < 25 short (measured 5-19), cost 20", ok, format!("level 2 {d2:.1} uu, level 1 {d1:.1} uu, mana {cost:.0}"));
    rep.check(S, "Blink aimed 19.84 deg up: target 1385.7 ahead and 500 above the eye (measured)", (flat - 1385.7).abs() < 5.0 && (up - 500.0).abs() < 5.0, format!("{flat:.1} ahead, {up:.1} above the eye"));
}

/// The climbable chain: Use grabs, forward with the view up climbs at 163.8 uu/s.
fn chain(rep: &mut Report, mc: &MatchContent) {
    let mut s = Solo::new(mc, Vec3::new(-1000.0, -120.0, 92.0), 90.0);
    let z0 = s.p().position.z;
    s.step(PlayerInput { yaw: 90.0, pitch: 30.0, buttons: buttons::USE, ..Default::default() });
    s.run(5, PlayerInput { yaw: 90.0, pitch: 30.0, ..Default::default() });
    let grabbed = s.p().mode == MoveMode::Climbing;
    let top = s.run(120, PlayerInput { yaw: 90.0, pitch: 30.0, forward: 127, ..Default::default() });
    rep.check(S, "climbable chain: Use grabs, forward climbs", grabbed && top - z0 > 150.0, format!("mode {:?}, rose {:.0} uu in 2 s", s.p().mode, top - z0));
}

/// The pool: a pawn under water swims; 30 s of air, then 1 damage every 61 ticks (measured).
fn drowning(rep: &mut Report, mc: &MatchContent) {
    // Held under by looking down and swimming down (crouch held), as the sim's breath tests do:
    // an idle swimmer floats with its head out (neutral buoyancy). Drowning takes health
    // directly (0x00ABB1E0, no damage event), so the health drops are counted.
    let mut s = Solo::new(mc, Vec3::new(2600.0, 0.0, -400.0), 0.0);
    let swimming = s.p().mode == MoveMode::Swimming;
    if let Some(p) = s.st.player_mut(ME) {
        p.water.air_used = 0.0;
        p.water.drown_timer = 0.0;
    }
    let dive = PlayerInput { pitch: -60.0, buttons: buttons::CROUCH, ..Default::default() };
    let (mut under_at, mut drops) = (None, Vec::new());
    let mut last = s.p().vitals.health;
    for t in 0..2400u64 {
        s.step(dive);
        let p = s.p();
        if under_at.is_none() && p.water.under {
            under_at = Some(t);
        }
        if p.vitals.health < last {
            drops.push(t);
        }
        last = p.vitals.health;
    }
    let first = under_at.zip(drops.first()).map(|(u, d)| (d - u) as f32 / 60.0);
    let gaps: Vec<u64> = drops.windows(2).map(|w| w[1] - w[0]).collect();
    let ok = swimming && first.is_some_and(|f| (f - 30.0).abs() < 0.1) && !gaps.is_empty() && gaps.iter().all(|g| (60..=62).contains(g));
    rep.check(S, "pool: swims; held under, the first health loss after 30 s of air, then every 61 ticks", ok, format!("swimming {swimming}, head under at tick {under_at:?}, first loss after {first:?} s, gaps {gaps:?} ticks"));
}

/// Falling damage: `trunc(70 x (v - 2250) / 650)` at landing speed v; none below 2250.
fn falls(rep: &mut Report, mc: &MatchContent) {
    let mut rows = Vec::new();
    let mut ok = true;
    let fall_from = |at: Vec3, yaw: f32, label: &str, rows: &mut Vec<Vec<String>>| -> (f32, f32, bool) {
        let mut s = Solo::new(mc, at, yaw);
        let h0 = s.p().vitals.health;
        s.run(400, fwd(yaw));
        let v = s.events.iter().filter_map(|(_, e)| match e {
            SimEvent::Landed { id, speed, .. } if *id == ME => Some(*speed),
            _ => None,
        }).fold(0.0f32, f32::max);
        let p = s.p();
        let dmg = if p.alive { h0 - p.vitals.health } else { h0 };
        rows.push(vec![label.to_string(), format!("{v:.0}"), format!("{dmg:.0}"), p.alive.to_string()]);
        (v, dmg, p.alive)
    };
    // Control: an 800 uu drop (no damage, measured: 800 none, 868 one point).
    let (_, d800, _) = fall_from(Vec3::new(-3000.0, 1600.0, 800.0 + 92.0), 0.0, "800 (open floor)", &mut rows);
    ok &= d800 == 0.0;
    for (y, h) in [(550.0, 880.0), (950.0, 1040.0), (1350.0, 1200.0), (1750.0, 1360.0), (2150.0, 1520.0)] {
        let (v, dmg, alive) = fall_from(Vec3::new(5260.0, y, h + 92.0), 180.0, &format!("{h}"), &mut rows);
        let want = (70.0 * ((v - 2250.0) / 650.0).clamp(0.0, 1.0)).trunc();
        if v >= 2900.0 {
            ok &= !alive;
        } else {
            ok &= alive && (dmg - want).abs() < 0.5 && v > 2250.0;
        }
    }
    rep.check(S, "falling damage = trunc(70 (v - 2250) / 650), none at 800, death above 2900", ok, rows.iter().map(|r| format!("{}: v {} dmg {}", r[0], r[1], r[2])).collect::<Vec<_>>().join(", "));
    rep.table("Fall-damage ledges (walk off west)", &["drop (uu)", "landing speed", "damage", "alive"], rows);
}

/// Lean (Q) and the crouch tunnel (clearance 150: sneaking at 130 fits, standing at 175 not).
fn lean_and_tunnel(rep: &mut Report, mc: &MatchContent) {
    let mut s = Solo::new(mc, Vec3::new(-1300.0, 3300.0, 92.0), 0.0);
    s.run(90, PlayerInput { yaw: 0.0, buttons: buttons::LEAN_LEFT, ..Default::default() });
    let lean = s.p().lean;
    rep.check(S, "lean: holding Q leans", lean.abs() > 0.1, format!("lean {lean:.2}"));
    let mut stand = Solo::new(mc, Vec3::new(-400.0, 2850.0, 92.0), 90.0);
    stand.run(180, fwd(90.0));
    let blocked = stand.p().position.y < 3000.0;
    let mut sneak = Solo::new(mc, Vec3::new(-400.0, 2850.0, 92.0), 90.0);
    sneak.step(PlayerInput { yaw: 90.0, buttons: buttons::CROUCH, ..Default::default() });
    sneak.step(PlayerInput { yaw: 90.0, ..Default::default() });
    sneak.run(330, fwd(90.0));
    let through = sneak.p().position.y > 3850.0;
    rep.check(S, "crouch tunnel: standing is stopped at the entrance, sneaking goes through", blocked && through, format!("standing stopped at y {:.0}, sneaking reached y {:.0}", stand.p().position.y, sneak.p().position.y));
}

/// The pain volume: damage on entry (`bEntryPain`), then every `PainInterval` (1 s).
fn pain(rep: &mut Report, mc: &MatchContent) {
    let mut s = Solo::new(mc, Vec3::new(1000.0, 2450.0, 92.0), 90.0);
    s.run(80, fwd(90.0));
    s.run(200, PlayerInput { yaw: 90.0, ..Default::default() });
    let hits: Vec<(u64, f32)> = s.events.iter().filter_map(|(t, e)| match e {
        SimEvent::Damage { victim, amount, .. } if *victim == ME => Some((*t, *amount)),
        _ => None,
    }).collect();
    let gaps: Vec<u64> = hits.windows(2).map(|w| w[1].0 - w[0].0).collect();
    let ok = hits.len() >= 3 && hits.iter().all(|h| (h.1 - 10.0).abs() < 0.5) && gaps.iter().skip(1).all(|g| (59..=61).contains(g));
    rep.check(S, "pain volume: 10 on entry, then 10 a second", ok, format!("{} hits {:?}, gaps {gaps:?} ticks", hits.len(), hits.iter().map(|h| h.1).collect::<Vec<_>>()));
}

/// Stations that need the mission: the bench, doors and usables, pickups, shooting, the sword
/// dummies and the stealth lane's patrol.
fn stations_in_mission(rep: &mut Report, maps: &MapContents, def: &MissionDef) -> Result<()> {
    let mut p = Play::new(maps, def.clone())?;
    p.idle(10, PlayerInput { yaw: 90.0, ..Default::default() });
    // Loadout at spawn: every power at level 2, full ammunition.
    let me = p.p();
    let mc = maps.get(MAP).unwrap().clone();
    let full = dishonored_ir::items::AmmoType::ALL.iter().all(|t| dishonored_sim::inventory::ammo(&me, *t) >= dishonored_sim::inventory::capacity(&me, *t, &mc.sim.gameplay));
    rep.check(S, "spawn loadout: every power at level 2, full ammunition, 10 elixirs", me.inv.power_levels.iter().all(|l| *l == 2) && full && me.inv.elixirs == [10, 10], format!("powers {:?}, elixirs {:?}", me.inv.power_levels, me.inv.elixirs));
    // Bench: the starting loadout back.
    p.teleport(Vec3::new(500.0, -5450.0, 92.0), 90.0);
    let (y, pi) = p.aim(Vec3::new(500.0, -5300.0, 100.0));
    p.use_once(y, pi);
    let reset = p.p();
    let ok_reset = reset.inv.power_levels[power::POSSESSION] == 0 && reset.inv.power_levels[power::BLINK] == 2 && reset.inv.elixirs == [0, 0];
    p.teleport(Vec3::new(-500.0, -5450.0, 92.0), 90.0);
    let (y, pi) = p.aim(Vec3::new(-500.0, -5300.0, 100.0));
    p.use_once(y, pi);
    let all = p.p();
    rep.check(S, "bench: starting loadout, then every power and item", ok_reset && all.inv.power_levels.iter().all(|l| *l == 2), format!("after reset {:?}, after all {:?}", reset.inv.power_levels, all.inv.power_levels));

    // Pickups (after the reset the elixirs are empty, so they can be taken).
    p.teleport(Vec3::new(500.0, -5450.0, 92.0), 90.0);
    let (y, pi) = p.aim(Vec3::new(500.0, -5300.0, 100.0));
    p.use_once(y, pi);
    let mut taken = Vec::new();
    for (name, x) in [("Pickup_HealthElixir", -600.0), ("Pickup_Rune", 200.0), ("Pickup_Coins", 600.0)] {
        p.teleport(Vec3::new(x, -4890.0, 92.0), 90.0);
        let (y, pi) = p.aim(Vec3::new(x, -4800.0, 15.0));
        let before = p.events.len();
        p.use_once(y, pi);
        let got = p.events[before..].iter().any(|(_, e)| matches!(e, SimEvent::PickedUp { id, .. } if *id == ME));
        taken.push((name, got));
    }
    let after = p.p();
    rep.check(S, "pickups: health elixir, rune and coins taken with Use", taken.iter().all(|t| t.1) && after.inv.elixirs[0] == 1 && after.inv.runes >= 1 && after.inv.coins > 0, format!("{taken:?}, elixirs {:?}, runes {}, coins {}", after.inv.elixirs, after.inv.runes, after.inv.coins));

    // Doors: closed blocks; Use opens it away from the user; the locked one stays shut.
    // Aim at each one's use point (the centre of its bodies) from within the 220 uu reach.
    let use_point = |actor: &str| mc.usables.iter().find(|u| u.actor == actor).map(|u| u.center).unwrap_or(Vec3::ZERO);
    let wood = use_point("Playground:Door_Wood");
    p.teleport(Vec3::new(wood.x, 2850.0, 92.0), 90.0);
    p.idle(60, fwd(90.0));
    let blocked = p.p().position.y < 2990.0;
    let (y, pi) = p.aim(wood);
    p.use_once(y, pi);
    let state = p.m().props.get("Playground:Door_Wood").map(|s| s.door).unwrap_or(0);
    p.idle(120, fwd(90.0));
    let through = p.p().position.y > 3080.0;
    rep.check(S, "door: closed blocks, Use opens it, then it lets the pawn through", blocked && state != 0 && through, format!("blocked {blocked}, state after use {state}, through {through} (y {:.0})", p.p().position.y));
    let locked = use_point("Playground:Door_Locked");
    p.teleport(Vec3::new(locked.x, 2850.0, 92.0), 90.0);
    p.idle(30, fwd(90.0));
    let (y, pi) = p.aim(locked);
    p.use_once(y, pi);
    let locked_state = p.m().props.get("Playground:Door_Locked").map(|s| (s.door, s.locked)).unwrap_or((9, false));
    p.idle(60, fwd(90.0));
    rep.check(S, "locked door: Use leaves it shut and it blocks", locked_state == (0, true) && p.p().position.y < 2990.0, format!("state {locked_state:?}, y {:.0}", p.p().position.y));
    let locker = use_point("Playground:Locker");
    p.teleport(Vec3::new(locker.x, locker.y - 150.0, 92.0), 90.0);
    let (y, pi) = p.aim(locker);
    p.use_once(y, pi);
    let stage = p.m().props.get("Playground:Locker").map(|s| s.stage).unwrap_or(0);
    rep.check(S, "locker: Use moves it to its next stage", stage == 1, format!("stage {stage}"));

    // Shooting gallery: pistol and crossbow at the 10 / 20 / 40 m targets.
    let mut rows = Vec::new();
    let mut hits_ok = true;
    for (weapon, hand) in [("pistol", LeftHand::Pistol), ("crossbow", LeftHand::Crossbow)] {
        for (target, x, d) in [("target_10m", 3000.0, 500.0), ("target_20m", 3800.0, 1000.0), ("target_40m", 4600.0, 2000.0)] {
            let id = p.npc(target);
            p.teleport(Vec3::new(x, 2800.0, 92.0), 90.0);
            let tp = p.m().world.player(id).map(|t| t.position).unwrap_or(Vec3::new(x, 2800.0 + d, 92.0));
            let (y, pi) = p.aim(tp + Vec3::Z * 25.0);
            // Equip and let the dispersion settle to its minimum (70 per second).
            p.idle(90, PlayerInput { yaw: y, pitch: pi, left_hand: hand, ..Default::default() });
            let before = p.events.len();
            p.step(PlayerInput { yaw: y, pitch: pi, left_hand: hand, buttons: buttons::SECONDARY, ..Default::default() });
            p.idle(60, PlayerInput { yaw: y, pitch: pi, left_hand: hand, ..Default::default() });
            let hit = p.events[before..].iter().find_map(|(_, e)| match e {
                SimEvent::Damage { victim, amount, .. } if *victim == id => Some(*amount),
                _ => None,
            });
            if d <= 1000.0 && hit.is_none() {
                hits_ok = false;
            }
            rows.push(vec![weapon.to_string(), format!("{} m", d / 50.0), hit.map(|h| format!("{h:.0}")).unwrap_or_else(|| "miss".into())]);
            p.idle(120, PlayerInput { yaw: y, pitch: pi, left_hand: hand, ..Default::default() });
        }
    }
    rep.check(S, "shooting gallery: pistol and crossbow hit the 10 and 20 m targets", hits_ok, rows.iter().map(|r| format!("{} {}: {}", r[0], r[1], r[2])).collect::<Vec<_>>().join(", "));
    rep.table("Shooting gallery (one aimed shot each)", &["weapon", "distance", "damage"], rows);

    // Sword dummies: a fast hit does 0.2 x 70 = 14 (the port's rule for NPC health, U-113).
    let id = p.npc("dummy_a");
    p.teleport(Vec3::new(0.0, 4460.0, 92.0), 90.0);
    let tp = p.m().world.player(id).map(|t| t.position).unwrap_or_default();
    let (y, pi) = p.aim(tp + Vec3::Z * 20.0);
    let before = p.events.len();
    p.step(PlayerInput { yaw: y, pitch: pi, buttons: buttons::PRIMARY, ..Default::default() });
    p.idle(60, PlayerInput { yaw: y, pitch: pi, ..Default::default() });
    let dmg: Vec<f32> = p.events[before..].iter().filter_map(|(_, e)| match e {
        SimEvent::Damage { victim, amount, .. } if *victim == id => Some(*amount),
        _ => None,
    }).collect();
    let still = p.m().world.player(id).map(|t| (t.position - tp).truncate().length()).unwrap_or(0.0);
    rep.check(S, "sword dummy: a sword hit lands (14 = 0.2 x 70) and the dummy does not fight back", dmg.first().is_some_and(|d| (*d - 14.0).abs() < 0.5) && still < 50.0, format!("damage {dmg:?}, dummy moved {still:.0} uu"));

    // The stealth lane's patrol walks the generated navigation mesh between its two points.
    let guard = p.npc("patrol");
    let g0 = p.m().world.player(guard).map(|g| g.position).unwrap_or_default();
    p.teleport(Vec3::new(0.0, -5500.0, 92.0), 90.0);
    let mut far = 0.0f32;
    for _ in 0..(30 * 60) {
        p.step(PlayerInput { yaw: 90.0, ..Default::default() });
        if let Some(g) = p.m().world.player(guard) {
            far = far.max((g.position - g0).truncate().length());
        }
    }
    rep.check(S, "stealth lane: the guard patrols along the generated navigation", far > 1500.0, format!("moved up to {far:.0} uu from its start in 30 s"));
    Ok(())
}

/// Every left-hand power and gadget at the sword dummies, cast through the mission match (the
/// playground's own path): its cost is paid (mana or ammunition) and its effect happens in the
/// simulation (events, pushed dummy, rats, time bends, projectiles, explosions, traps).
fn powers_in_mission(rep: &mut Report, maps: &MapContents, def: &MissionDef) -> Result<()> {
    use dishonored_ir::items::AmmoType;
    use dishonored_sim::state::PawnKind;
    let mut p = Play::new(maps, def.clone())?;
    p.idle(10, PlayerInput { yaw: 90.0, ..Default::default() });
    let dummy = p.npc("dummy_a");
    let spot = Vec3::new(0.0, 4300.0, 92.0);
    let mut rows = Vec::new();
    for (name, hand, hold) in [
        ("Blink", LeftHand::Blink, 10usize),
        ("Windblast", LeftHand::Windblast, 1),
        ("Dark Vision", LeftHand::DarkVision, 1),
        ("Possession", LeftHand::Possession, 15),
        ("Bend Time", LeftHand::BendTime, 1),
        ("Devouring Swarm", LeftHand::DevouringSwarm, 1),
        ("Grenade", LeftHand::Grenade, 1),
        ("Sticky grenade", LeftHand::StickyGrenade, 1),
        ("Spring Razor", LeftHand::SpringRazor, 1),
    ] {
        p.teleport(spot, 90.0);
        if let Some(me) = p.mm().world.player_mut(ME) {
            me.vitals.mana = 100.0;
        }
        let tp = p.m().world.player(dummy).map(|t| t.position).unwrap_or(spot + Vec3::Y * 300.0);
        // Aim at the dummy (Possession, Windblast); Blink a little to the side of it.
        let (y, pi) = if hand == LeftHand::Blink { p.aim(spot + Vec3::new(400.0, 200.0, -60.0)) } else { p.aim(tp + Vec3::Z * 20.0) };
        let idle = PlayerInput { yaw: y, pitch: pi, left_hand: hand, ..Default::default() };
        p.idle(45, idle);
        let (mana0, me0) = (p.p().vitals.mana, p.p());
        let before = p.events.len();
        for _ in 0..hold {
            p.step(PlayerInput { buttons: buttons::SECONDARY, ..idle });
        }
        // Watch for 5 s: rats, time bends and the dummy's displacement.
        let (mut rats, mut bends, mut pushed) = (0usize, 0usize, 0.0f32);
        for _ in 0..150 {
            p.step(idle);
            rats = rats.max(p.m().world.players.iter().filter(|q| q.kind == PawnKind::Rat).count());
            bends = bends.max(p.m().world.time_bends.len());
            if let Some(t) = p.m().world.player(dummy) {
                pushed = pushed.max((t.position - tp).length());
            }
        }
        let ev: Vec<&SimEvent> = p.events[before..].iter().map(|(_, e)| e).collect();
        let me1 = p.p();
        let spent = mana0 - me1.vitals.mana;
        let ammo = |m: &PlayerState, t: AmmoType| dishonored_sim::inventory::ammo(m, t) as i32;
        let (ok, what) = match hand {
            LeftHand::Blink => (ev.iter().any(|e| matches!(e, SimEvent::BlinkEnd { id } if *id == ME)) && spent > 0.0, format!("moved {:.0} uu", (me1.position - me0.position).length())),
            LeftHand::Windblast => (ev.iter().any(|e| matches!(e, SimEvent::WindblastCast { id } if *id == ME)) && pushed > 20.0, format!("dummy pushed {pushed:.0} uu")),
            LeftHand::DarkVision => (ev.iter().any(|e| matches!(e, SimEvent::DarkVision { id, active: true } if *id == ME)), format!("remaining {:.1} s", me1.vitals.dark_vision_remaining)),
            LeftHand::Possession => (ev.iter().any(|e| matches!(e, SimEvent::PossessStart { id, .. } if *id == ME)), format!("stage {:?}", me1.possess.stage)),
            LeftHand::BendTime => (ev.iter().any(|e| matches!(e, SimEvent::BendTimeStart { id, .. } if *id == ME)) && bends > 0, format!("{bends} time bends")),
            LeftHand::DevouringSwarm => (ev.iter().any(|e| matches!(e, SimEvent::SwarmSummoned { id, .. } if *id == ME)) && rats > 0, format!("{rats} rats")),
            LeftHand::Grenade | LeftHand::StickyGrenade => {
                let t = if hand == LeftHand::Grenade { AmmoType::Grenade } else { AmmoType::StickyGrenade };
                let used = ammo(&me0, t) - ammo(&me1, t);
                (used == 1 && ev.iter().any(|e| matches!(e, SimEvent::Explosion { owner, .. } if *owner == ME)), format!("ammo used {used}, exploded {}", ev.iter().any(|e| matches!(e, SimEvent::Explosion { .. }))))
            }
            _ => {
                let used = ammo(&me0, AmmoType::SpringRazor) - ammo(&me1, AmmoType::SpringRazor);
                (used == 1 && ev.iter().any(|e| matches!(e, SimEvent::TrapPlaced { id, .. } if *id == ME)), format!("ammo used {used}"))
            }
        };
        rep.check(S, &format!("power {name}: cast in the mission, cost paid and its effect happens"), ok, format!("mana spent {spent:.0}, {what}"));
        rows.push(vec![name.to_string(), format!("{spent:.0}"), what, if ok { "ok".into() } else { "FAIL".into() }]);
        // End what keeps running (possession, bend time) before the next one.
        if matches!(hand, LeftHand::Possession | LeftHand::BendTime) {
            p.step(PlayerInput { buttons: buttons::SECONDARY, ..idle });
            p.idle(60, idle);
        }
        p.idle(60, PlayerInput { yaw: y, pitch: pi, ..Default::default() });
    }
    rep.table("Powers and gadgets in the mission match", &["item", "mana spent", "result", ""], rows);
    Ok(())
}

/// A guard notices a player standing in the light sooner than one in the dark (the seen value
/// adds `0.1 x light01`, HOOK_TRIAGE.md).
fn detection(rep: &mut Report, maps: &MapContents) -> Result<()> {
    let mc = maps.get(MAP).unwrap().clone();
    let mut rows = Vec::new();
    let mut times = Vec::new();
    for (label, guard, player) in [("lit", Vec3::new(-4400.0, 7000.0, 92.0), Vec3::new(-3500.0, 7000.0, 92.0)), ("dark", Vec3::new(1500.0, 7000.0, 92.0), Vec3::new(2400.0, 7000.0, 92.0))] {
        let text = format!(
            r#"MissionDef(title: "Detection", map: "Playground",
                objectives: [ObjectiveDef(id: "t", description: "t", tasks: [TaskDef(id: "t", description: "t", initial: Active)])],
                markers: [MarkerDef(id: "start", at: At({:.1}, {:.1}, {:.1}))],
                npcs: [NpcDef(id: "watcher", kind: Guard, at: At({:.1}, {:.1}, {:.1}), behavior: Idle, faction: "DisFaction_Defaults.Faction_Guard_Default")],
                player_start: "start",
                rules: [RuleDef(on: Timer(999.0), then: [Complete("end")])])"#,
            player.x, player.y, player.z, guard.x, guard.y, guard.z
        );
        let def = MissionDef::from_ron(&text)?;
        let mut p = Play::new(maps, def)?;
        let id = p.npc("watcher");
        let light = dishonored_ai::light_at_with(&p.m().crowd.lights, p.m().crowd.sky_light, player, &mc.collision);
        let yaw = 180.0;
        let (mut noticed, mut engaged) = (None, None);
        for t in 0..(20 * 60) {
            p.step(PlayerInput { yaw, ..Default::default() });
            if let Some(b) = p.m().crowd.brains.iter().find(|b| b.id == id) {
                if noticed.is_none() && b.suspicion_level() >= 1 {
                    noticed = Some(t as f32 / 60.0);
                }
                if engaged.is_none() && b.state == dishonored_ai::BrainState::Combat {
                    engaged = Some(t as f32 / 60.0);
                    break;
                }
            }
        }
        rows.push(vec![label.to_string(), format!("{light:.3}"), format!("{noticed:?}"), format!("{engaged:?}")]);
        times.push((light, engaged.unwrap_or(f32::INFINITY), noticed.unwrap_or(f32::INFINITY)));
    }
    let ok = times.len() == 2 && times[0].0 > times[1].0 && times[0].1 < f32::INFINITY && (times[0].1 < times[1].1 || times[0].2 < times[1].2);
    rep.check(S, "detection: a guard 900 uu away spots a player in the light sooner than in the dark", ok, rows.iter().map(|r| format!("{}: light {}, noticed {}, engaged {}", r[0], r[1], r[2], r[3])).collect::<Vec<_>>().join("; "));
    rep.table("Detection, standing still 900 uu in front of a guard", &["where", "light probe", "noticed (s)", "engaged (s)"], rows);
    Ok(())
}
