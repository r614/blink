//! Headless scenario runner: the game's own match, built and stepped the way the runtime's
//! offline session does (`dishonored_content::mission_setup` -> `GameSetup::build` ->
//! `Game::add_player` -> `Game::step` at 60 Hz), with the map from the game's loader
//! (`dishonored_content::MapContents::load` -> `load_match`). The scenario's setup becomes a
//! mission definition (NPCs, loadout, story flags, original spawners, original Kismet), so
//! a passing scenario exercises the same code paths as the game. The step's events then go
//! through the runtime's presentation handlers (`present.rs`).
//!
//! Recorded every tick: the pawn, vitals, camera, powers and left hand, projectiles, world
//! counters, every named NPC (position, health, AI state, attention on the player, stance),
//! plus ordered events: simulation events, Kismet activations, effects, sounds, animation
//! inputs and projectile spawns / ends. Enumerated channels carry their legend in the unit
//! (`enum:Idle,Patrol,...`).

use crate::present::{FxLibrary, Presenter};
use anyhow::{Context, Result, bail};
use dishonored_content::MapContents;
use dishonored_mission::{Behavior, Loadout, MarkerDef, MissionDef, NpcDef, NpcKind, ObjectiveDef, OriginalKismet, Place, TaskDef, TaskState};
use dishonored_multiplayer::game::Game;
use dishonored_multiplayer::mission::MissionMatch;
use dishonored_oracle::scenario::{Button, Scenario};
use dishonored_oracle::trace::{Trace, TraceEvent};
use dishonored_sim::input::{LeftHand, PlayerInput, actions, buttons};
use dishonored_sim::state::{PlayerId, power};
use dishonored_world::channel;
use glam::Vec3;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

const ME: PlayerId = 1;
/// Channels sampled from the player (pawn, vitals, camera, left hand, powers, inventory).
const PLAYER_CHANNELS: usize = 49;

/// What a run produced besides the trace.
pub struct RunOut {
    pub trace: Trace,
    /// Setup notes: what the port could not set up as specified (never silent).
    pub notes: Vec<String>,
    pub seconds: f64,
}

fn button_bits(b: &[Button]) -> u16 {
    b.iter()
        .map(|b| match b {
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
        })
        .fold(0, |a, b| a | b)
}

/// A `LeftHand` by its name (case-insensitive).
pub fn left_hand(name: &str) -> Option<LeftHand> {
    (0..=13u8).map(LeftHand::from_u8).find(|h| format!("{h:?}").eq_ignore_ascii_case(name)).or_else(|| match name.to_ascii_lowercase().as_str() {
        "empty" | "none" => Some(LeftHand::Empty),
        "possess" => Some(LeftHand::Possession),
        "swarm" => Some(LeftHand::DevouringSwarm),
        _ => None,
    })
}

/// The scenario's items and powers as the game's loadout (`apply_loadout`, the original's
/// `OnAddPower` / `OnAddInventoryItem` handlers).
fn loadout(s: &Scenario, notes: &mut Vec<String>) -> Option<Loadout> {
    let st = &s.setup;
    if st.items.is_empty() && st.powers.is_empty() {
        return None;
    }
    let mut l = Loadout::default();
    for (name, level) in &st.powers {
        if power::NAMES.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            l.powers.push((name.clone(), *level));
        } else {
            notes.push(format!("unknown power `{name}` (one of {:?})", power::NAMES));
        }
    }
    for it in &st.items {
        let (what, n) = match it.split_once(':') {
            Some((a, b)) => (a, b.parse::<u32>().ok()),
            None => (it.as_str(), None),
        };
        match what.to_ascii_lowercase().as_str() {
            "fullammo" => l.full_ammo = true,
            "allpowers" => l.all_powers = true,
            "reset" => l.reset = true,
            "healthelixir" => l.elixirs.0 = n.unwrap_or(1) as u8,
            "manaelixir" => l.elixirs.1 = n.unwrap_or(1) as u8,
            "coins" => l.coins = n.unwrap_or(0),
            "runes" => l.runes = n.unwrap_or(0) as u16,
            _ if what.to_ascii_lowercase().starts_with("upgrade") => l.upgrades.push(it.split_once(':').map(|x| x.1.to_string()).unwrap_or_default()),
            // Ammo types are given after the loadout (counts), see `give_ammo`.
            _ if ammo_type(what).is_some() => {}
            _ => notes.push(format!("unknown item `{it}`")),
        }
    }
    Some(l)
}

fn ammo_type(n: &str) -> Option<dishonored_ir::items::AmmoType> {
    dishonored_ir::items::AmmoType::ALL.into_iter().find(|t| format!("{t:?}").eq_ignore_ascii_case(n))
}

/// The scenario as a mission definition.
fn mission(s: &Scenario, mc: &dishonored_content::MatchContent, notes: &mut Vec<String>) -> Result<(MissionDef, Vec<(String, Option<String>)>)> {
    let st = &s.setup;
    let mut def = MissionDef {
        title: format!("oracle {}", s.name),
        map: s.map.clone(),
        objectives: vec![ObjectiveDef { id: "run".into(), description: "oracle scenario".into(), hidden: true, tasks: vec![TaskDef { id: "run".into(), description: "run".into(), initial: TaskState::Active, hidden: true, optional: false, target: None }] }],
        ..Default::default()
    };
    def.settings.respawn = false;
    // A mission needs an end; never reached in a run.
    def.rules.push(dishonored_mission::RuleDef { id: None, on: dishonored_mission::Trigger::Timer(1.0e9), when: Vec::new(), then: vec![dishonored_mission::Action::Complete("end".into())], once: true, disabled: false });
    for (f, on) in &st.story_flags {
        if *on {
            def.flags_set.push(f.clone());
        } else {
            def.flags.push(f.clone());
        }
    }
    if st.original_npcs {
        def.original_npcs.push(s.map.clone());
    }
    if st.kismet {
        let start = st.kismet_start.clone().context("setup.kismet needs setup.kismet_start (the global sequence's remote event that starts the level)")?;
        def.original_kismet = Some(OriginalKismet { start_event: start, seed: st.seed });
    }
    def.loadout = loadout(s, notes);
    // NPCs: (trace name, original spawner name when it is the level's own pawn).
    let mut names = Vec::new();
    for (i, n) in st.npcs.iter().enumerate() {
        let name = n.id.clone().or_else(|| n.spawner.clone()).unwrap_or_else(|| format!("npc{i}"));
        if st.original_npcs && n.spawner.is_some() && n.position.is_none() {
            // The populated level's own pawn.
            names.push((name, n.spawner.clone()));
            continue;
        }
        let spawner = n.spawner.as_ref().and_then(|sp| mc.spawners.iter().find(|x| x.name.eq_ignore_ascii_case(sp)));
        if let Some(sp) = &n.spawner {
            let k = mc.spawners.iter().filter(|x| x.name.eq_ignore_ascii_case(sp)).count();
            if k > 1 {
                notes.push(format!("spawner `{sp}` is ambiguous on {} ({k} spawners of that name in the level's packages): the first is used", s.map));
            }
        }
        if n.spawner.is_some() && spawner.is_none() {
            notes.push(format!("spawner `{}` not found on {}", n.spawner.as_deref().unwrap_or(""), s.map));
        }
        if n.tweak.is_some() {
            notes.push(format!("{name}: pawn tweak `{}` not applied (mission NPCs take the role archetype; U-113 health rule)", n.tweak.as_deref().unwrap_or("")));
        }
        let pos = n.position.map(Vec3::from).or(spawner.map(|x| x.position)).context(format!("NPC {name}: no position or spawner"))?;
        let kind = match n.kind.as_deref().map(|k| k.to_ascii_lowercase()) {
            Some(k) if k == "civilian" => NpcKind::Civilian,
            Some(k) if k != "guard" => bail!("NPC {name}: kind `{k}` (Guard or Civilian)"),
            _ if spawner.is_some_and(|x| !x.guard) => NpcKind::Civilian,
            _ => NpcKind::Guard,
        };
        let state = n.state.as_deref().unwrap_or("Idle").to_ascii_lowercase();
        let behavior = match state.as_str() {
            "patrol" => {
                let mut route = Vec::new();
                for (k, p) in n.route.iter().enumerate() {
                    let id = format!("{name}_route{k}");
                    def.markers.push(MarkerDef { id: id.clone(), at: Place::At(p[0], p[1], p[2]), radius: 100.0, usable: None });
                    route.push(id);
                }
                Behavior::Patrol(route)
            }
            "wander" => Behavior::Wander,
            "idle" | "passive" | "alerted" | "combat" => Behavior::Idle,
            other => bail!("NPC {name}: state `{other}` (Idle, Patrol, Wander, Alerted, Passive)"),
        };
        def.npcs.push(NpcDef {
            id: name.clone(),
            kind,
            archetype: None,
            at: Place::At(pos.x, pos.y, pos.z),
            spawn_at_start: true,
            behavior,
            name: Some(name.clone()),
            talk: None,
            talk_conversation: None,
            speaker: None,
            faction: match n.faction.as_deref() {
                Some("") | Some("None") | Some("none") => None,
                Some(f) => Some(f.to_string()),
                // A spawner-placed NPC takes the spawner's faction like the original
                // (`DisSpawnerInfo`): its `m_pFactionTweakOverride`, else its pawn tweak's
                // `m_pFactionTweak` (Boyle's party guard, spawner 46: a neutral guard).
                None if spawner.is_some_and(|sp| !sp.props.faction.is_empty() || mc.ai_level.tweak_factions.contains_key(&sp.tweak)) => {
                    let sp = spawner.unwrap();
                    Some(if sp.props.faction.is_empty() { mc.ai_level.tweak_factions[&sp.tweak].clone() } else { sp.props.faction.clone() })
                }
                None if kind == NpcKind::Guard && state != "passive" => {
                    if let Some(sp) = spawner {
                        notes.push(format!("{name}: spawner `{}` has no faction (tweak `{}` not resolved): Faction_Guard_Default", sp.name, sp.tweak));
                    }
                    Some("DisFaction_Defaults.Faction_Guard_Default".into())
                }
                None => None,
            },
            passive: state == "passive",
        });
        names.push((name, None));
    }
    Ok((def, names))
}

/// Legend-indexed enum channel values.
#[derive(Default)]
struct Legends(BTreeMap<String, Vec<String>>);

/// Fixed legends (declaration order) of the enumerated channels, so `TimeAt("pawn.mode", 5)`
/// means Swimming in every trace. Keyed by the channel's last segment.
const SEEDS: &[(&str, &[&str])] = &[
    ("mode", &["Walking", "Falling", "Mantling", "Blinking", "Dead", "Swimming", "Climbing"]),
    ("phase", &["Idle", "Aiming", "Equipping", "Recovering", "Reloading"]),
    ("item", &["Blink", "Windblast", "DarkVision", "Pistol", "Crossbow", "Empty", "Possession", "BendTime", "DevouringSwarm", "SleepDart", "IncendiaryBolt", "Grenade", "StickyGrenade", "SpringRazor"]),
    ("possess", &["Inactive", "Targeting", "PrePossess", "Active", "Cooldown"]),
    ("state", &["Idle", "Wander", "WalkToPoint", "GroupIdle", "React", "Flee", "Patrol", "Investigate", "Alert", "Combat", "Notice", "Search", "Cower", "RingAlarm"]),
    ("condition", &["Normal", "BeingChoked", "Unconscious", "Possessed"]),
    ("stance", &["Relaxed", "Patrol", "Search", "Combat", "Panic", "Cower", "Notice"]),
];

impl Legends {
    fn idx(&mut self, chan: &str, v: impl std::fmt::Debug) -> f64 {
        let s = format!("{v:?}");
        let s = s.split([' ', '{', '(']).next().unwrap_or("").to_string();
        let l = self.0.entry(chan.to_string()).or_insert_with(|| {
            let last = chan.rsplit('.').next().unwrap_or("");
            SEEDS.iter().find(|x| x.0 == last).map(|x| x.1.iter().map(|s| s.to_string()).collect()).unwrap_or_default()
        });
        match l.iter().position(|x| *x == s) {
            Some(i) => i as f64,
            None => {
                l.push(s);
                (l.len() - 1) as f64
            }
        }
    }
}

/// `Name { a: 1, b: Some(2) }` -> (`Name`, `a=1 b=2`), ids as trace names.
fn event_detail(e: &impl std::fmt::Debug, name_of: &dyn Fn(u32) -> String) -> (String, String) {
    let s = format!("{e:?}");
    let (head, rest) = match s.find(['{', '(']) {
        Some(k) => (s[..k].trim().to_string(), s[k + 1..].trim_end_matches(['}', ')']).trim().to_string()),
        None => (s.trim().to_string(), String::new()),
    };
    // Split at top-level commas.
    let mut parts = Vec::new();
    let (mut depth, mut cur) = (0, String::new());
    for c in rest.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    let ids = ["id", "victim", "attacker", "killer", "defender", "host", "by", "carrier", "body", "owner", "source", "rat", "target"];
    let fields: Vec<String> = parts
        .iter()
        .filter_map(|p| {
            let (k, v) = p.split_once(':')?;
            let (k, mut v) = (k.trim(), v.trim().to_string());
            if let Some(inner) = v.strip_prefix("Some(").and_then(|x| x.strip_suffix(')')) {
                v = inner.to_string();
            }
            if ids.contains(&k) {
                if let Ok(n) = v.parse::<u32>() {
                    v = name_of(n);
                }
            }
            let v = v.replace("Vec3(", "").replace(')', "").replace(", ", ",").replace(' ', "_");
            Some(format!("{k}={v}"))
        })
        .collect();
    (head, fields.join(" "))
}

fn mm(g: &Game) -> &MissionMatch {
    match g {
        Game::Mission(m) => m,
        _ => unreachable!("the runner builds mission games"),
    }
}

fn mm_mut(g: &mut Game) -> &mut MissionMatch {
    match g {
        Game::Mission(m) => m,
        _ => unreachable!("the runner builds mission games"),
    }
}

/// Play a scenario on the port and record its trace.
pub fn run(s: &Scenario, maps: &MapContents, fx: Arc<FxLibrary>) -> Result<RunOut> {
    let t0 = std::time::Instant::now();
    if s.tick_hz != 60 {
        bail!("{}: the port steps at a fixed 60 Hz (tick_hz {})", s.name, s.tick_hz);
    }
    let mc = maps.get(&s.map).with_context(|| format!("map {} not loaded", s.map))?.clone();
    let mut notes = Vec::new();
    let (def, npc_names) = mission(s, &mc, &mut notes)?;
    maps.set.set_active(&mc.map_name);
    let setup = dishonored_content::mission_setup(maps, def).map_err(|e| anyhow::anyhow!("mission setup: {e}"))?;
    let cfg = &mc.sim;
    let mut game = setup.build(cfg);
    game.add_player(ME, "oracle", 0, &maps.set, cfg);

    // The player: placed, turned, vitals and ammo as specified.
    let ps = s.setup.player.clone().unwrap_or_default();
    let mv = &cfg.gameplay.movement;
    let mut pos = Vec3::from(ps.position);
    if ps.snap_to_floor {
        // As the hook places the original's pawn (`probe_launch` `Session::place`): the
        // engine's line check (flags 0x20DF, `TRACE_Blocking` of the pawn's moves) from 100 uu
        // above to 1900 below, centre at the reported floor + CollisionHeight + 1. A
        // non-accurate line check reports its hit backed off (`engine_check`), so both sides
        // start from the same point.
        let (a, b) = (pos + Vec3::Z * 100.0, pos - Vec3::Z * 1900.0);
        if let Some(h) = dishonored_world::engine_check(&mc.collision, &[], a, b, Vec3::ZERO, channel::PAWN, 0x20df) {
            pos = a + (b - a) * h.time + Vec3::Z * (mv.collision_half_height + 1.0);
        }
    }
    {
        let m = mm_mut(&mut game);
        if let Some(p) = m.world.player_mut(ME) {
            if s.setup.player.is_some() {
                p.position = pos;
                p.velocity = Vec3::ZERO;
                // The hook's `Session::place` writes Location and Velocity only: the original's
                // pawn keeps walking physics (no fall at tick 1).
                if ps.snap_to_floor {
                    p.mode = dishonored_sim::state::MoveMode::Walking;
                }
            }
            p.yaw = ps.yaw;
            p.pitch = ps.pitch;
            if let Some(h) = ps.health {
                p.vitals.health = h;
            }
            if let Some(mn) = ps.mana {
                p.vitals.mana = mn;
                p.vitals.mana_regen_cap = mn;
            }
            for it in &s.setup.items {
                let (what, n) = match it.split_once(':') {
                    Some((a, b)) => (a, b.parse::<i32>().ok()),
                    None => (it.as_str(), None),
                };
                if let Some(t) = ammo_type(what) {
                    let cap = dishonored_sim::inventory::capacity(p, t, &cfg.gameplay);
                    dishonored_sim::inventory::add_ammo(p, t, n.unwrap_or(cap as i32).max(0) as _, &cfg.gameplay);
                }
            }
        }
        if s.setup.player.is_some() {
            // `PlayerState::right_hand`: the original's pawn starts with nothing in the right
            // hand; the hook gives it the sword (`lab give Twk_Inv_SwordCorvo`) when the spec asks.
            if let Some(p) = m.world.player_mut(ME) {
                p.right_hand = if ps.sword_drawn { dishonored_sim::state::RightHandItem::Sword } else { dishonored_sim::state::RightHandItem::Empty };
            }
        }
        if s.setup.keep_alive {
            notes.push("keep_alive: not applied on the port (no minimum scripted health): deaths happen".into());
        }
    }
    if !s.setup.equip.is_empty() {
        notes.push(format!("equip {:?}: use setup.left_hand on the port", s.setup.equip));
    }

    // Names: the player, the scenario's NPCs, the mission NPCs and original spawners.
    let mut by_id: HashMap<PlayerId, String> = HashMap::new();
    by_id.insert(ME, "player".into());
    let mut tracked: Vec<(String, PlayerId)> = Vec::new();
    {
        let m = mm(&game);
        for (name, sp) in &m.setup.npcs {
            by_id.insert(sp.id, name.clone());
        }
        for o in &m.setup.original {
            by_id.entry(o.spawn.id).or_insert_with(|| o.name.clone());
        }
        for (name, spawner) in &npc_names {
            let id = match spawner {
                Some(sp) => m.setup.original.iter().find(|o| o.name.eq_ignore_ascii_case(sp)).map(|o| o.spawn.id),
                None => m.setup.npcs.iter().find(|n| &n.0 == name).map(|n| n.1.id),
            };
            match id {
                Some(id) => {
                    by_id.insert(id, name.clone());
                    tracked.push((name.clone(), id));
                }
                None => notes.push(format!("NPC {name} not found in the match")),
            }
        }
    }
    // NPC facing and alerted states.
    for (i, n) in s.setup.npcs.iter().enumerate() {
        let Some((_, id)) = tracked.iter().find(|t| t.0 == npc_names[i].0).cloned() else { continue };
        let m = mm_mut(&mut game);
        if n.position.is_some() || n.spawner.is_some() {
            if let Some(p) = m.world.player_mut(id) {
                // A mission NPC placed `At(x, y, z)` spawns with yaw 0 (the place carries no
                // rotation); a pawn without input keeps its `last_input` (step_world), so both
                // take the scenario's rotation, as `spawn_npc` does for a placed rotation.
                p.yaw = n.yaw;
                p.last_input.yaw = n.yaw;
            }
        }
        if matches!(n.state.as_deref().map(|x| x.to_ascii_lowercase()).as_deref(), Some("alerted" | "combat")) {
            m.alert_npc(id);
        }
    }
    let name_of = |id: u32| by_id.get(&id).cloned().unwrap_or_else(|| format!("#{id}"));

    // Channels.
    let mut chans: Vec<(String, &str)> = Vec::new();
    for (c, u) in [
        ("pawn.x", "uu"), ("pawn.y", "uu"), ("pawn.z", "uu"), ("pawn.vx", "uu/s"), ("pawn.vy", "uu/s"), ("pawn.vz", "uu/s"), ("pawn.speed2d", "uu/s"), ("pawn.crouched", "bool"), ("pawn.mode", "enum"), ("pawn.alive", "bool"), ("pawn.under", "bool"), ("pawn.air_used", "s"),
        ("pawn.health", "pt"), ("pawn.mana", "pt"), ("pawn.health_max", "pt"),
        ("cam.x", "uu"), ("cam.y", "uu"), ("cam.z", "uu"), ("cam.yaw", "deg"), ("cam.pitch", "deg"), ("cam.lean", "1"), ("cam.lean_deg", "deg"), ("cam.roll", "deg"),
        ("left.item", "enum"), ("left.phase", "enum"), ("left.timer", "s"), ("left.pistol_loaded", "n"), ("left.pistol_reserve", "n"), ("left.bolts_loaded", "n"), ("left.bolts_reserve", "n"), ("left.dispersion", "px720"),
        ("left.blink_target", "bool"), ("left.blink_tx", "uu"), ("left.blink_ty", "uu"), ("left.blink_tz", "uu"),
        ("power.time_scale", "1"), ("power.world_dilation", "1"), ("power.dark_vision", "s"), ("power.possess", "enum"), ("power.action_lock", "s"),
        ("inv.grenades", "n"), ("inv.sticky", "n"), ("inv.razors", "n"), ("inv.sleep_darts", "n"), ("inv.incendiary", "n"), ("inv.elixir_health", "n"), ("inv.elixir_mana", "n"), ("inv.coins", "n"), ("inv.runes", "n"),
        ("world.projectiles", "n"), ("world.windblasts", "n"), ("world.traps", "n"), ("world.swarms", "n"), ("world.rats", "n"), ("world.swarm_rats", "n"), ("world.time_bends", "n"),
        ("darkness.score", "n"), ("darkness.level", "n"), ("darkness.corpses_found", "n"), ("darkness.high", "bool"),
        ("proj.x", "uu"), ("proj.y", "uu"), ("proj.z", "uu"), ("proj.speed", "uu/s"),
        ("fx.live", "n"), ("fx.particles", "n"),
    ] {
        chans.push((c.to_string(), u));
    }
    for (name, _) in &tracked {
        for (c, u) in [("x", "uu"), ("y", "uu"), ("z", "uu"), ("yaw", "deg"), ("health", "pt"), ("alive", "bool"), ("condition", "enum"), ("state", "enum"), ("stance", "enum"), ("attn", "1"), ("attn_level", "n"), ("attn_noise", "1"), ("attn_noise_level", "n"), ("los", "1"), ("sighted", "bool"), ("suspicion", "n"), ("time_scale", "1"), ("speed2d", "uu/s")] {
            chans.push((format!("npc.{name}.{c}"), u));
        }
    }
    let chan_refs: Vec<(&str, &str)> = chans.iter().map(|(c, u)| (c.as_str(), *u)).collect();
    let mut trace = Trace::new(&s.name, "port", s.tick_hz, &chan_refs);
    let mut leg = Legends::default();
    let mut present = Presenter::new(fx, mc.clone(), ME);

    let sample = |g: &Game, leg: &mut Legends, present: &Presenter| -> Vec<f64> {
        let m = mm(g);
        let w = &m.world;
        let mut v = Vec::with_capacity(chans.len());
        let b = |x: bool| x as u8 as f64;
        match w.player(ME) {
            Some(p) => {
                let eye = dishonored_sim::movement::eye_position(p, mv);
                let hmax = dishonored_sim::powers::health_max(p, &cfg.gameplay) as f64;
                let am = |t: dishonored_ir::items::AmmoType| dishonored_sim::inventory::ammo(p, t) as f64;
                use dishonored_ir::items::AmmoType as A;
                v.extend([p.position.x as f64, p.position.y as f64, p.position.z as f64, p.velocity.x as f64, p.velocity.y as f64, p.velocity.z as f64, p.velocity.truncate().length() as f64, b(p.crouched), leg.idx("pawn.mode", p.mode), b(p.alive), b(p.water.under), p.water.air_used as f64]);
                v.extend([p.vitals.health as f64, p.vitals.mana as f64, hmax]);
                v.extend([eye.x as f64, eye.y as f64, eye.z as f64, p.yaw as f64, p.pitch as f64, p.lean as f64, dishonored_sim::movement::lean_angle(p, mv) as f64, dishonored_sim::movement::lean_roll(p, mv) as f64]);
                let bt = p.left.blink_target;
                v.extend([leg.idx("left.item", p.left.item), leg.idx("left.phase", p.left.phase), p.left.timer as f64, p.left.pistol_loaded as f64, p.left.pistol_reserve as f64, p.left.bolts_loaded as f64, p.left.bolts_reserve as f64, p.left.dispersion as f64]);
                v.extend([b(bt.is_some()), bt.map_or(f64::NAN, |t| t.x as f64), bt.map_or(f64::NAN, |t| t.y as f64), bt.map_or(f64::NAN, |t| t.z as f64)]);
                // The world rate in effect (every bend's ramped world dilation and every Blink layer).
                let wd = dishonored_sim::powers::world_time_scale(w, &cfg.gameplay, false);
                v.extend([p.time_scale as f64, wd as f64, p.vitals.dark_vision_remaining as f64, leg.idx("power.possess", p.possess.stage), p.action_lock as f64]);
                v.extend([am(A::Grenade), am(A::StickyGrenade), am(A::SpringRazor), am(A::SleepDart), am(A::IncendiaryBolt), p.inv.elixirs[0] as f64, p.inv.elixirs[1] as f64, p.inv.coins as f64, p.inv.runes as f64]);
            }
            None => v.extend(std::iter::repeat_n(f64::NAN, PLAYER_CHANNELS)),
        }
        let rats = w.players.iter().filter(|p| p.kind == dishonored_sim::state::PawnKind::Rat && p.alive).count();
        v.extend([w.projectiles.len() as f64, w.windblasts.len() as f64, w.traps.len() as f64, w.swarms.len() as f64, rats as f64, w.swarms.iter().map(|s| (s.rats as usize + w.players.iter().filter(|p| p.alive && p.swarm == Some(s.id)).count()) as f64).sum::<f64>(), w.time_bends.len() as f64]);
        // Chaos: the darkness manager's score and level (0x00C487D0), corpses found so far.
        let dl = m.darkness.level(&cfg.gameplay.items.darkness);
        let high = m.view.mission.as_ref().is_some_and(|mv| mv.high_chaos);
        v.extend([m.darkness.score as f64, dl as f64, m.corpses_discovered.len() as f64, b(high)]);
        match w.projectiles.iter().min_by_key(|p| p.id) {
            Some(pr) => v.extend([pr.position.x as f64, pr.position.y as f64, pr.position.z as f64, pr.velocity.length() as f64]),
            None => v.extend([f64::NAN; 4]),
        }
        v.extend([present.live_effects() as f64, present.live_particles() as f64]);
        let poses = dishonored_multiplayer::npc_poses(&m.crowd.brains);
        for (name, id) in &tracked {
            let p = w.player(*id);
            let brain = m.crowd.brains.iter().find(|b| b.id == *id);
            let attn = brain.and_then(|b| b.attn.iter().find(|a| a.who == ME));
            match p {
                Some(p) => v.extend([p.position.x as f64, p.position.y as f64, p.position.z as f64, p.yaw as f64, p.vitals.health as f64, b(p.alive), leg.idx(&format!("npc.{name}.condition"), p.condition)]),
                None => v.extend([f64::NAN; 7]),
            }
            v.push(brain.map_or(f64::NAN, |b| leg.idx(&format!("npc.{name}.state"), b.state)));
            v.push(poses.iter().find(|x| x.0 == *id).map_or(f64::NAN, |x| leg.idx(&format!("npc.{name}.stance"), x.1)));
            let noise = brain.and_then(|b| b.attn.iter().find(|a| a.who == dishonored_ai::NOISE));
            v.extend([attn.map_or(0.0, |a| a.value as f64), attn.map_or(0.0, |a| a.level as f64), noise.map_or(0.0, |a| a.value as f64), noise.map_or(0.0, |a| a.level as f64), attn.map_or(f64::NAN, |a| a.los_norm as f64), b(attn.is_some_and(|a| a.sighted)), brain.map_or(f64::NAN, |b| b.suspicion_level() as f64)]);
            v.extend([p.map_or(f64::NAN, |p| p.time_scale as f64), p.map_or(f64::NAN, |p| p.velocity.truncate().length() as f64)]);
        }
        v
    };

    let timeline = s.timeline();
    let base_hand = s.setup.left_hand.as_deref().map(|h| left_hand(h).with_context(|| format!("left_hand `{h}`"))).transpose()?;
    let mut tick: u64 = 0;
    let neutral = PlayerInput { yaw: ps.yaw, pitch: ps.pitch, left_hand: base_hand.unwrap_or_default(), ..Default::default() };
    for _ in 0..s.setup.settle {
        tick += 1;
        let out = game.step(&[(ME, PlayerInput { tick, ..neutral })], &maps.set, cfg);
        let w = &mm(&game).world;
        present.frame(&out.events, w, dishonored_sim::left_hand::dark_vision_reveals(w, ME, &cfg.tuning), 1.0 / 60.0);
    }
    let first = sample(&game, &mut leg, &present);
    trace.push(0, first);
    let mut seen_proj: BTreeMap<u32, (String, f32)> = BTreeMap::new();
    let mut kismet_seen = mm(&game).kismet.as_ref().map(|k| k.st.history.len()).unwrap_or(0);
    for (t, ti) in timeline.iter().enumerate() {
        let tk = t as u32 + 1;
        for c in &ti.console {
            let r = console(&mut game, c, &by_id, cfg);
            trace.events.push(TraceEvent { tick: tk, kind: "Console".into(), object: "player".into(), name: c.split_whitespace().next().unwrap_or("").to_string(), detail: match r {
                Ok(d) => format!("cmd={} {d}", c.replace(' ', "_")),
                Err(e) => format!("unsupported cmd={} why={}", c.replace(' ', "_"), e.to_string().replace(' ', "_")),
            } });
        }
        let hand = match &ti.hand {
            Some(h) => left_hand(h).with_context(|| format!("Hand(`{h}`)"))?,
            None => neutral.left_hand,
        };
        let action = match ti.drink.as_deref().map(|d| d.to_ascii_lowercase()) {
            Some(d) if d.contains("health") => actions::DRINK_HEALTH,
            Some(d) if d.contains("mana") => actions::DRINK_MANA,
            Some(d) => bail!("Drink(`{d}`): HealthElixir or ManaElixir"),
            None => actions::NONE,
        };
        tick += 1;
        // The shooter's view and dispersion before the step (a shot leaves at these).
        let before = mm(&game).world.player(ME).map(|p| (p.yaw, p.pitch, p.left.dispersion));
        let input = PlayerInput { tick, forward: ti.forward, right: ti.right, yaw: ti.yaw, pitch: ti.pitch, buttons: button_bits(&ti.held), left_hand: hand, action, npc_move: 0, npc_speed: 0.0 };
        let out = game.step(&[(ME, input)], &maps.set, cfg);
        // Simulation events.
        for e in &out.events {
            let (name, detail) = event_detail(e, &name_of);
            let object = detail.split_whitespace().next().and_then(|f| f.split_once('=')).map(|x| x.1.to_string()).unwrap_or_default();
            trace.events.push(TraceEvent { tick: tk, kind: "Sim".into(), object, name, detail });
        }
        // Kismet activations.
        if let Some(k) = mm(&game).kismet.as_ref() {
            for h in k.st.history.iter().skip(kismet_seen) {
                let name = h.split_whitespace().next().unwrap_or("").to_string();
                trace.events.push(TraceEvent { tick: tk, kind: "Kismet".into(), object: String::new(), name, detail: h.clone() });
            }
            kismet_seen = k.st.history.len();
        }
        // Projectiles.
        let w = &mm(&game).world;
        for p in &w.projectiles {
            if !seen_proj.contains_key(&p.id) {
                seen_proj.insert(p.id, (format!("{:?}", p.kind), 0.0));
                // Player shots: the offset from the view in 720-line pixels (as 0x00AA2B10 draws it,
                // `left_hand::offset_direction` inverted) and its ratio to the dispersion.
                let spread = match before.filter(|_| p.owner == ME) {
                    Some((yaw, pitch, disp)) => {
                        let f = dishonored_sim::movement::view_vector(yaw, pitch);
                        let v = p.velocity.normalize_or_zero();
                        let k = 2.0 * (dishonored_sim::left_hand::DISPERSION_REF_HFOV_DEG.to_radians() * 0.5).tan() / (720.0 * dishonored_sim::left_hand::DISPERSION_REF_ASPECT);
                        let off = (v / v.dot(f).max(1e-6) - f).length() / k;
                        format!(" off_px={off:.3} disp={disp:.3} rd={:.4}", if disp > 0.0 { off / disp } else { f32::NAN })
                    }
                    None => String::new(),
                };
                trace.events.push(TraceEvent { tick: tk, kind: "Projectile".into(), object: name_of(p.owner), name: format!("{:?}", p.kind), detail: format!("id={} speed={:.1} dmg={:.1}{spread}", p.id, p.velocity.length(), p.damage) });
            }
            if let Some(e) = seen_proj.get_mut(&p.id) {
                e.1 = p.age;
            }
        }
        let gone: Vec<u32> = seen_proj.keys().copied().filter(|id| !w.projectiles.iter().any(|p| p.id == *id)).collect();
        for id in gone {
            let (kind, age) = seen_proj.remove(&id).unwrap_or_default();
            trace.events.push(TraceEvent { tick: tk, kind: "ProjectileEnd".into(), object: String::new(), name: kind, detail: format!("id={id} age={age:.3}") });
        }
        // Presentation: effects, sounds, animation inputs.
        for pe in present.frame(&out.events, w, dishonored_sim::left_hand::dark_vision_reveals(w, ME, &cfg.tuning), 1.0 / 60.0) {
            trace.events.push(TraceEvent { tick: tk, kind: pe.kind.into(), object: if pe.object == 0 { String::new() } else { name_of(pe.object) }, name: pe.name, detail: pe.detail });
        }
        let v = sample(&game, &mut leg, &present);
        trace.push(tk, v);
    }
    for (chan, names) in leg.0 {
        if let Some(i) = trace.channel(&chan) {
            trace.channels[i].unit = format!("enum:{}", names.join(","));
        }
    }
    Ok(RunOut { trace, notes, seconds: t0.elapsed().as_secs_f64() })
}

/// The port's console hooks for `Action::Console`.
fn console(game: &mut Game, cmd: &str, by_id: &HashMap<PlayerId, String>, cfg: &dishonored_sim::SimConfig) -> Result<String> {
    let w: Vec<&str> = cmd.split_whitespace().collect();
    let f = |i: usize| -> Result<f32> { w.get(i).context("missing argument")?.parse::<f32>().context("number") };
    let npc = |name: &str| by_id.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(id, _)| *id).context("unknown NPC");
    let m = mm_mut(game);
    match w.first().map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("teleport") => {
            let p = m.world.player_mut(ME).context("no player")?;
            p.position = Vec3::new(f(1)?, f(2)?, f(3)?);
            p.velocity = Vec3::ZERO;
            if w.len() > 4 {
                p.yaw = f(4)?;
            }
        }
        Some("health") => m.world.player_mut(ME).context("no player")?.vitals.health = f(1)?,
        Some("mana") => m.world.player_mut(ME).context("no player")?.vitals.mana = f(1)?,
        Some("power") => {
            let i = power::NAMES.iter().position(|n| n.eq_ignore_ascii_case(w.get(1).copied().unwrap_or(""))).context("power name")?;
            // SetPowerLevel (0x00ACE270): a raised HealthMax raises the health with it.
            let level = f(2)? as u8;
            dishonored_sim::powers::set_power_level(m.world.player_mut(ME).context("no player")?, i, level, &cfg.gameplay);
        }
        Some("give") => {
            let t = ammo_type(w.get(1).copied().unwrap_or("")).context("ammo type")?;
            let p = m.world.player_mut(ME).context("no player")?;
            dishonored_sim::inventory::add_ammo(p, t, f(2)? as _, &cfg.gameplay);
        }
        Some("alert") => {
            let id = npc(w.get(1).copied().unwrap_or(""))?;
            m.alert_npc(id);
        }
        // The cheat `adddarkness N` (DishonoredCheatManager slot 300, 0x00B753A0): the darkness
        // manager's Add (0x00C41B50).
        Some("adddarkness") => {
            m.darkness.add(f(1)? as i32);
            m.sync_darkness(cfg);
            return Ok(format!("score={} level={}", m.darkness.score, m.state.darkness_level));
        }
        Some("calm") => {
            let id = npc(w.get(1).copied().unwrap_or(""))?;
            if let Some(b) = m.crowd.brains.iter_mut().find(|b| b.id == id) {
                b.calm();
                b.state = dishonored_ai::BrainState::Idle;
            }
        }
        // Save the match (save::encode), load it back (save::restore) and continue from the copy.
        Some("saveload") => {
            use dishonored_multiplayer::mission::save;
            let blocked = save::save_blocked(m);
            let h0 = save::state_hash(m);
            let bytes = save::encode(m, 0, "oracle")?;
            let (_, restored) = save::restore(&bytes, m.setup.clone())?;
            *m = restored;
            let h1 = save::state_hash(m);
            return Ok(format!("bytes={} same_state={} allowed={} blocked={}", bytes.len(), h0 == h1, blocked.is_none(), blocked.unwrap_or("-").replace(' ', "_")));
        }
        Some("cansave") => {
            let blocked = dishonored_multiplayer::mission::save::save_blocked(m);
            return Ok(format!("allowed={} blocked={}", blocked.is_none(), blocked.unwrap_or("-").replace(' ', "_")));
        }
        _ => bail!("no port hook"),
    }
    let _ = cfg;
    Ok(String::new())
}
