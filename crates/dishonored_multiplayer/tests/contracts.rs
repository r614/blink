//! Contracts rules on a synthetic world (no retail data needed).
use dishonored_ai::CrowdConfig;
use dishonored_bots::{Bot, BotStyle};
use dishonored_multiplayer::MatchPhase;
use dishonored_multiplayer::contracts::{ContractsRules, Proximity, validate_cycle};
use dishonored_multiplayer::game::{CrowdSource, Game, GameSetup};
use dishonored_sim::SimConfig;
use dishonored_sim::input::{PlayerInput, buttons};
use dishonored_sim::state::PawnKind;
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn floor() -> CollisionWorld {
    let (a, b, c, d) = (Vec3::new(-8000., -8000., 0.), Vec3::new(8000., -8000., 0.), Vec3::new(8000., 8000., 0.), Vec3::new(-8000., 8000., 0.));
    let tris = [(a, b, c), (a, c, d)].iter().map(|&(x, y, z)| Tri { a: x, b: y, c: z, n: Vec3::Z, channels: channel::ALL, source: 0 }).collect();
    CollisionWorld::from_triangles(tris, vec!["floor".into()])
}

fn spawns() -> Vec<(Vec3, f32)> {
    (0..12).map(|i| (Vec3::new((i % 4) as f32 * 1500.0 - 2250.0, (i / 4) as f32 * 1500.0 - 1500.0, 100.0), (i * 30) as f32)).collect()
}

fn contracts_of(g: &Game) -> &dishonored_multiplayer::contracts::ContractsMatch {
    match g {
        Game::Contracts(m) => m,
        _ => panic!(),
    }
}

#[test]
fn cycle_is_valid_for_all_sizes_and_churn() {
    let w = floor();
    let sim = cfg();
    for n in 2..=8u32 {
        let setup = GameSetup::contracts(ContractsRules { warmup_time: 0.0, ..Default::default() }, spawns(), None, &w);
        let mut g = setup.build(&sim);
        for id in 1..=n {
            g.add_player(id, &format!("p{id}"), 0, &w, &sim);
        }
        validate_cycle(&contracts_of(&g).contracts).unwrap_or_else(|e| panic!("n={n}: {e}"));
        // Leave + rejoin churn.
        g.remove_player(1);
        validate_cycle(&contracts_of(&g).contracts).unwrap_or_else(|e| panic!("n={n} after leave: {e}"));
        g.add_player(1, "p1", 0, &w, &sim);
        validate_cycle(&contracts_of(&g).contracts).unwrap_or_else(|e| panic!("n={n} after rejoin: {e}"));
    }
}

#[test]
fn tracking_is_approximate_only() {
    let w = floor();
    let sim = cfg();
    let mut g = GameSetup::contracts(ContractsRules { warmup_time: 0.0, reacquire_delay: 0.0, ..Default::default() }, spawns(), None, &w).build(&sim);
    for id in 1..=3 {
        g.add_player(id, "p", 0, &w, &sim);
    }
    for _ in 0..30 {
        g.step(&[], &w, &sim);
    }
    let ti = g.target_info(1).unwrap();
    assert!(ti.target.is_some());
    assert!(ti.bearing_octant.is_some_and(|o| o < 8));
    assert!(matches!(ti.proximity, Some(Proximity::Far | Proximity::Medium | Proximity::Near | Proximity::VeryNear)));
    // The struct has no coordinate fields at all (compile-time guarantee) — nothing else to leak.
}

#[test]
fn silent_assassination_outscores_loud_ranged_kill() {
    let w = floor();
    let sim = cfg();
    let run = |assassinate: bool| -> i32 {
        let mut g = GameSetup::contracts(ContractsRules { warmup_time: 0.0, ..Default::default() }, vec![(Vec3::new(0., 0., 100.), 0.0), (Vec3::new(110., 0., 100.), 0.0)], None, &w).build(&sim);
        g.add_player(1, "hunter", 0, &w, &sim);
        g.add_player(2, "target", 0, &w, &sim);
        for _ in 0..10 {
            g.step(&[], &w, &sim);
        }
        // Place: hunter right behind the target, both facing +X.
        {
            let wm = g.world_mut();
            wm.player_mut(1).unwrap().position = Vec3::new(0.0, 0.0, 88.5);
            wm.player_mut(2).unwrap().position = Vec3::new(if assassinate { 110.0 } else { 900.0 }, 0.0, 88.5);
            wm.player_mut(1).unwrap().yaw = 0.0;
            wm.player_mut(2).unwrap().yaw = 0.0;
        }
        for _ in 0..30 {
            g.step(&[(1, PlayerInput { yaw: 0.0, ..Default::default() }), (2, PlayerInput { yaw: 0.0, ..Default::default() })], &w, &sim);
        }
        if assassinate {
            g.step(&[(1, PlayerInput { yaw: 0.0, buttons: buttons::PRIMARY, ..Default::default() }), (2, PlayerInput { yaw: 0.0, ..Default::default() })], &w, &sim);
        } else {
            for k in 0..900 {
                let b = if k % 50 == 0 { buttons::SECONDARY } else { 0 };
                g.step(&[(1, PlayerInput { yaw: 0.0, pitch: -3.0, buttons: b, left_hand: dishonored_sim::input::LeftHand::Pistol, ..Default::default() }), (2, PlayerInput { yaw: 0.0, ..Default::default() })], &w, &sim);
            }
        }
        for _ in 0..10 {
            g.step(&[], &w, &sim);
        }
        g.view().scores.iter().find(|s| s.id == 1).unwrap().score
    };
    let silent = run(true);
    let loud = run(false);
    println!("silent {silent} loud {loud}");
    assert!(silent >= 600, "silent assassination score {silent}");
    assert!(loud > 0 && loud < silent, "loud kill {loud} vs silent {silent}");
}

#[test]
fn full_match_with_crowd_completes_without_deadlock() {
    let w = floor();
    let sim = cfg();
    let pts: Vec<(Vec3, f32)> = (0..40).map(|i| (Vec3::new((i % 8) as f32 * 600.0 - 2100.0, (i / 8) as f32 * 600.0 - 1200.0, 100.0), 0.0)).collect();
    let crowd = CrowdConfig { civilians: 20, guards: 3, center: Vec3::ZERO, radius: 5000.0, seed: 7, civilian_archetypes: vec![0, 2, 3], guard_archetypes: vec![1] };
    let rules = ContractsRules { warmup_time: 1.0, time_limit: 240.0, crowd: Some(crowd), ..Default::default() };
    let guard_spawns = vec![(Vec3::new(0., 0., 100.), 0.0)];
    let setup = GameSetup::contracts(rules, spawns(), Some(CrowdSource { points: &pts, guard_spawns: &guard_spawns }), &w);
    let mut g = setup.build(&sim);
    let mut bots: Vec<Bot> = (1..=8).map(|i| Bot::new(i, BotStyle::Duelist, 40 + i as u64)).collect();
    for b in &bots {
        g.add_player(b.id, &format!("bot{}", b.id), 0, &w, &sim);
    }
    let npcs = g.world().players.iter().filter(|p| p.kind != PawnKind::Human).count();
    assert_eq!(npcs, 23);
    let (mut contract_kills, mut deaths) = (0, 0);
    for tick in 0..(60 * 260) {
        let snap = g.world().players.clone();
        let inputs: Vec<_> = bots.iter_mut().filter_map(|b| {
            let me = snap.iter().find(|p| p.id == b.id)?;
            let t = g.target_info(b.id).and_then(|t| t.target);
            Some((b.id, b.think_contract(me, &snap, &w, t)))
        }).collect();
        let out = g.step(&inputs, &w, &sim);
        deaths += out.events.iter().filter(|e| matches!(e, dishonored_sim::events::SimEvent::Death { .. })).count();
        if g.view().phase == MatchPhase::Playing {
            if let Err(e) = validate_cycle(&contracts_of(&g).contracts) {
                panic!("tick {tick}: {e}");
            }
        }
        contract_kills = g.view().scores.iter().map(|s| s.kills).sum::<i32>().max(contract_kills);
    }
    println!("deaths {deaths} contract kills {contract_kills} phase {:?} scores {:?}", g.view().phase, g.view().scores.iter().map(|s| (s.id, s.score, s.kills)).collect::<Vec<_>>());
    assert!(contract_kills >= 4, "contract kills {contract_kills}");
    assert!(g.view().phase == MatchPhase::Ended || g.view().match_number > 0, "match should end on the time limit");
    assert!(g.view().winner.is_some() || g.view().match_number > 0);
}
