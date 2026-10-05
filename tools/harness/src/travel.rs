//! `harness travel [--mission FILE]`: level travel through the original doors.
//!
//! The mission is played on Boyle Ext with Boyle Int reachable (as in the original). The agent
//! walks up to the original travel door (`DishonoredUsableObject_*` with an `LTD_*` tweak),
//! presses use, and must arrive at the `PlayerStart` whose tag the original Kismet passes to
//! `SetPlayerTravelDestination`, on the destination map, standing on its floor (the collision
//! world follows the active map). It then travels back and checks the level state of the map it
//! left was kept (`SaveLevelState`): an NPC killed before leaving is still dead, the others are
//! where they were.

use crate::Ctx;
use crate::report::Report;
use anyhow::{Context, Result};
use dishonored_content::MapContents;
use dishonored_mission::MissionDef;
use dishonored_multiplayer::game::Game;
use dishonored_multiplayer::mission::TravelDoor;
use dishonored_sim::input::{PlayerInput, buttons};
use dishonored_sim::state::PawnKind;
use dishonored_world::WorldQuery;
use glam::Vec3;

const S: &str = "Level travel";
const AGENT: u32 = 1;
const EXT: &str = "L_Boyle_Ext_P";
const INT: &str = "L_Boyle_Int_P";

/// A floor spot in front of a door from which the eye sees the door's use point.
fn approach(world: &dyn WorldQuery, d: &TravelDoor) -> Option<(Vec3, f32, f32)> {
    let target = d.position + Vec3::Z * 60.0;
    let mut best = None;
    for i in 0..16 {
        let a = i as f32 * std::f32::consts::TAU / 16.0;
        for r in [110.0, 150.0] {
            let p = d.position + Vec3::new(a.cos(), a.sin(), 0.0) * r;
            let Some(f) = dishonored_ai::snap_floor(world, p) else { continue };
            let eye = f + Vec3::Z * 64.0;
            let to = target - eye;
            if to.length() > 200.0 || (f.z - d.position.z).abs() > 150.0 {
                continue;
            }
            let blocked = world.raycast(eye, to.normalize(), to.length() - 20.0, dishonored_world::channel::PAWN).is_some();
            if !blocked {
                let yaw = to.y.atan2(to.x).to_degrees();
                let pitch = (to.z / to.truncate().length()).atan().to_degrees();
                if best.as_ref().is_none_or(|b: &(Vec3, f32, f32, f32)| to.length() < b.3) {
                    best = Some((f, yaw, pitch, to.length()));
                }
            }
        }
    }
    best.map(|b| (b.0, b.1, b.2))
}

struct Travelled {
    map: String,
    pos: Vec3,
    z_after: f32,
    detail: String,
    /// Options the door asked (from the match view), if any.
    asked: Vec<String>,
}

/// Stand at a door, press use, answer its confirmation with option `answer` (1 = go), then let
/// the game run a second.
fn use_door(g: &mut Game, maps: &MapContents, door: &TravelDoor, answer: usize, tick: &mut u64) -> Result<Travelled> {
    let world = maps.get(&door.map).context("door map")?;
    let (spot, yaw, pitch) = approach(&world.collision, door).context("no clear approach to the door")?;
    let Game::Mission(m) = g else { anyhow::bail!("not a mission") };
    let me = m.world.player_mut(AGENT).context("agent")?;
    me.position = spot;
    me.velocity = Vec3::ZERO;
    me.yaw = yaw;
    me.pitch = pitch;
    let mut z_after = 0.0;
    let mut pos = Vec3::ZERO;
    let mut asked = Vec::new();
    for i in 0..90 {
        *tick += 1;
        let b = match i {
            3 => buttons::USE,
            10 => buttons::CHOICES[answer - 1],
            _ => 0,
        };
        let inp = PlayerInput { tick: *tick, yaw, pitch, buttons: b, ..Default::default() };
        let sim = &maps.maps[0].sim;
        g.step(&[(AGENT, inp)], &maps.set, sim);
        if let Some(cm) = g.current_map() {
            maps.set.set_active(cm);
        }
        if i == 6 {
            asked = g.view().mission.as_ref().and_then(|m| m.choices.iter().find(|c| c.0 == AGENT).map(|c| c.1.clone())).unwrap_or_default();
        }
        if i == 11 {
            pos = g.world().player(AGENT).map(|p| p.position).unwrap_or_default();
        }
        z_after = g.world().player(AGENT).map(|p| p.position.z).unwrap_or_default();
    }
    let map = g.current_map().unwrap_or_default().to_string();
    Ok(Travelled { map, pos, z_after, detail: format!("from ({:.0},{:.0},{:.0}) yaw {yaw:.0} pitch {pitch:.0}", spot.x, spot.y, spot.z), asked })
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let path = ctx.opt("--mission").unwrap_or_else(|| "missions/boyle_masked_host.ron".into());
    let mut def = MissionDef::load(std::path::Path::new(&path))?;
    def.map = EXT.into();
    if !def.maps.iter().any(|m| m.eq_ignore_ascii_case(INT)) {
        def.maps.push(INT.into());
    }
    def.settings.respawn = true;
    let maps = MapContents::load(&dishonored_content::mission_maps(&def))?;
    let ext = maps.get(EXT).context("ext")?.clone();
    let int = maps.get(INT).context("int")?.clone();
    rep.check(S, "original travel doors extracted for both maps", !ext.travel.is_empty() && !int.travel.is_empty(), format!("Ext {} doors, Int {} doors; tagged starts Ext {}, Int {}", ext.travel.len(), int.travel.len(), ext.tagged_starts.len(), int.tagged_starts.len()));
    let setup = dishonored_content::mission_setup(&maps, def.clone()).map_err(|e| anyhow::anyhow!(e))?;
    let dishonored_multiplayer::game::GameSetup::Mission(ms) = &setup else { anyhow::bail!("mission setup") };
    let doors = ms.doors.clone();
    let localized = doors.iter().filter(|d| !d.prompt.contains("to travel")).count();
    rep.check(S, "door prompts come from the original localization", localized == doors.len() && !doors.is_empty(), doors.iter().take(3).map(|d| format!("{}: \"{}\"", d.door, d.prompt)).collect::<Vec<_>>().join("; "));
    rep.table("Travel doors (original Kismet)", &["map", "door", "prompt", "destination", "tag"], doors.iter().map(|d| vec![d.map.clone(), d.door.clone(), d.prompt.clone(), d.dest_map.clone(), d.dest_tag.clone()]).collect());

    maps.set.set_active(EXT);
    let mut g = setup.build(&ext.sim);
    g.add_player(AGENT, "agent", 0, &maps.set, &ext.sim);
    let mut tick = 0u64;
    for _ in 0..30 {
        tick += 1;
        g.step(&[(AGENT, PlayerInput { tick, ..Default::default() })], &maps.set, &ext.sim);
    }
    // Change the level state before leaving: kill one NPC outright.
    let (victim, before) = {
        let Game::Mission(m) = &mut g else { unreachable!() };
        let v = m.world.players.iter_mut().find(|p| p.kind != PawnKind::Human && p.alive).context("an NPC on Ext")?;
        v.alive = false;
        v.vitals.health = 0.0;
        let id = v.id;
        let before: Vec<(u32, bool)> = m.world.players.iter().filter(|p| p.kind != PawnKind::Human).map(|p| (p.id, p.alive)).collect();
        (id, before)
    };

    // Ext -> Int through the first door leading inside.
    let door_in = doors.iter().find(|d| d.map.eq_ignore_ascii_case(EXT) && d.dest_map.eq_ignore_ascii_case(INT)).cloned().context("Ext->Int door")?;
    // The original asks first ("Go to ..." / "Stay in ..."); staying keeps the player outside.
    let t = use_door(&mut g, &maps, &door_in, 2, &mut tick)?;
    rep.check(S, &format!("{} asks the original confirmation; \"stay\" keeps the player on {EXT}", door_in.door), t.asked.len() == 2 && t.asked == door_in.choices && t.map.eq_ignore_ascii_case(EXT), format!("asked {:?}; still on {}", t.asked, t.map));
    let t = use_door(&mut g, &maps, &door_in, 1, &mut tick)?;
    let start_in = int.tagged_starts.iter().find(|s| s.tag.eq_ignore_ascii_case(&door_in.dest_tag)).context("destination start")?;
    let d_in = t.pos.truncate().distance(start_in.position.truncate());
    rep.check(S, &format!("use {} -> arrive at {} on {INT}", door_in.door, door_in.dest_tag), t.map.eq_ignore_ascii_case(INT) && d_in < 200.0, format!("{}; now on {}, {:.0} uu from the PlayerStart", t.detail, t.map, d_in));
    rep.check(S, "collision follows the active map (agent stands on the Int floor)", (t.z_after - t.pos.z).abs() < 60.0 && maps.set.active_name().eq_ignore_ascii_case(INT), format!("z {:.0} -> {:.0} after 1.4 s, active {}", t.pos.z, t.z_after, maps.set.active_name()));
    let npcs_int = g.world().players.iter().filter(|p| p.kind != PawnKind::Human).count();
    // First visit: exactly the interior's begin-play spawners (no alarm squads, no pursuers).
    let originals = ms.original.clone();
    let expect_int = originals.iter().filter(|o| o.map.eq_ignore_ascii_case(INT) && o.props.on_begin_play && o.props.travel_tags.is_empty() && o.props.at_suspicion.is_none()).count();
    let squads = originals.iter().filter(|o| o.map.eq_ignore_ascii_case(INT) && !o.props.on_begin_play && o.props.at_suspicion.is_some()).count();
    let pursuers_ballroom = originals.iter().filter(|o| o.props.travel_tags.iter().any(|t| t.eq_ignore_ascii_case(&door_in.dest_tag))).count();
    rep.check(S, "first visit populates the interior from its original begin-play spawners", expect_int > 0 && npcs_int == expect_int, format!("{npcs_int} NPCs present, {expect_int} begin-play spawners ({} spawners on Int: {squads} alarm-squad, {pursuers_ballroom} pursuers for {})", originals.iter().filter(|o| o.map.eq_ignore_ascii_case(INT)).count(), door_in.dest_tag));
    let view_map = g.view().map.clone();
    let int_doors_usable = g.view().mission.as_ref().map(|m| m.usables.iter().filter(|u| doors.iter().any(|d| d.map.eq_ignore_ascii_case(INT) && d.prompt == u.1)).count()).unwrap_or(0);
    rep.check(S, "match view reports the new map and its doors", view_map.eq_ignore_ascii_case(INT) && int_doors_usable > 0, format!("view.map {view_map}, {int_doors_usable} Int doors usable, {npcs_int} NPCs present"));

    // Int -> Ext back through a door leading out.
    let door_out = doors.iter().find(|d| d.map.eq_ignore_ascii_case(INT) && d.dest_map.eq_ignore_ascii_case(EXT)).cloned().context("Int->Ext door")?;
    let t = use_door(&mut g, &maps, &door_out, 1, &mut tick)?;
    let start_out = ext.tagged_starts.iter().find(|s| s.tag.eq_ignore_ascii_case(&door_out.dest_tag)).context("return start")?;
    let d_out = t.pos.truncate().distance(start_out.position.truncate());
    rep.check(S, &format!("use {} -> arrive at {} on {EXT}", door_out.door, door_out.dest_tag), t.map.eq_ignore_ascii_case(EXT) && d_out < 200.0, format!("{}; now on {}, {:.0} uu from the PlayerStart", t.detail, t.map, d_out));
    let after: Vec<(u32, bool)> = g.world().players.iter().filter(|p| p.kind != PawnKind::Human).map(|p| (p.id, p.alive)).collect();
    let victim_dead = after.iter().any(|(id, alive)| *id == victim && !alive);
    let same_set = before.iter().all(|b| after.iter().any(|a| a.0 == b.0));
    rep.check(S, "level state kept: the NPC killed on Ext is still dead, the rest still there", victim_dead && same_set, format!("{} NPCs before, {} after; victim #{victim} dead: {victim_dead}", before.len(), after.len()));

    // Pursuit: a guard on Ext hunts the agent, who escapes through the ballroom door; the
    // ballroom's `DisTravelSpawner`s (DAISL_Suspecting) send pursuers in after the player.
    let hunter = {
        let Game::Mission(m) = &mut g else { unreachable!() };
        let me = m.world.player(AGENT).map(|p| p.position).unwrap_or_default();
        let gid = m.world.players.iter().filter(|p| p.kind == PawnKind::Guard && p.alive).min_by(|a, b| a.position.distance(me).total_cmp(&b.position.distance(me))).map(|p| p.id).context("a guard on Ext")?;
        m.alert_npc(gid);
        gid
    };
    let t = use_door(&mut g, &maps, &door_in, 1, &mut tick)?;
    let ids: Vec<u32> = originals.iter().filter(|o| o.props.travel_tags.iter().any(|x| x.eq_ignore_ascii_case(&door_in.dest_tag))).map(|o| o.spawn.id).collect();
    let present: Vec<&dishonored_sim::state::PlayerState> = g.world().players.iter().filter(|p| ids.contains(&p.id) && p.alive).collect();
    let hunting = match &g {
        Game::Mission(m) => m.crowd.brains.iter().filter(|b| ids.contains(&b.id) && b.target == Some(AGENT)).count(),
        _ => 0,
    };
    rep.check(S, "pursued through the door: the arrival tag's DisTravelSpawner guards follow, hunting the player", t.map.eq_ignore_ascii_case(INT) && present.len() == ids.len() && !ids.is_empty() && hunting > 0, format!("guard #{hunter} hunting on Ext; on {}: {}/{} pursuers present, {hunting} targeting the agent", t.map, present.len(), ids.len()));
    Ok(())
}
