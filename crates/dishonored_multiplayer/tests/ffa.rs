//! Free-for-all scoring on a synthetic world (no retail data needed).
use dishonored_multiplayer::MatchPhase;
use dishonored_multiplayer::ffa::{FfaMatch, FfaRules, SpawnPoint};
use dishonored_sim::SimConfig;
use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_world::{CollisionWorld, Tri, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A floor from x -1000 to 0: walking east off its edge falls to `KillZ`.
fn ledge() -> CollisionWorld {
    let (a, b, c, d) = (Vec3::new(-1000., -1000., 0.), Vec3::new(0., -1000., 0.), Vec3::new(0., 1000., 0.), Vec3::new(-1000., 1000., 0.));
    let tris = [(a, b, c), (a, c, d)].iter().map(|&(x, y, z)| Tri { a: x, b: y, c: z, n: Vec3::Z, channels: channel::ALL, source: 0 }).collect();
    CollisionWorld::from_triangles(tris, vec!["ledge".into()])
}

/// A world death (falling out of the world) counts a death and changes no score: Dishonored's
/// engine has no `GameInfo.ScoreKill`, so nothing takes the point stock UE3 takes.
#[test]
fn world_death_costs_no_point() {
    let w = ledge();
    let sim = cfg();
    let spawns = vec![SpawnPoint { position: Vec3::new(-100.0, 0.0, 100.0), yaw: 0.0 }, SpawnPoint { position: Vec3::new(-900.0, 0.0, 100.0), yaw: 0.0 }];
    let mut m = FfaMatch::new(FfaRules { warmup_time: 0.0, min_players: 1, respawn_delay: 1000.0, ..Default::default() }, spawns);
    m.add_player(1, "a", &sim);
    m.add_player(2, "b", &sim);
    let mut deaths = Vec::new();
    for _ in 0..(60 * 30) {
        let walk = dishonored_sim::input::PlayerInput { forward: 127, yaw: 0.0, ..Default::default() };
        let id = m.world.players.iter().find(|p| p.position.x > -500.0 && p.alive).map(|p| p.id);
        let inputs: Vec<_> = id.into_iter().map(|id| (id, walk)).collect();
        let out = m.step(&inputs, &w, &sim);
        deaths.extend(out.events.iter().filter_map(|e| match e {
            SimEvent::Death { victim, killer, cause } => Some((*victim, *killer, *cause)),
            _ => None,
        }));
        if !deaths.is_empty() {
            break;
        }
    }
    assert_eq!(m.view.phase, MatchPhase::Playing);
    assert_eq!(deaths.len(), 1, "{deaths:?}");
    let (victim, killer, cause) = deaths[0];
    assert_eq!((killer, cause), (None, DamageCause::World));
    let s = m.view.scores.iter().find(|s| s.id == victim).unwrap();
    assert_eq!((s.score, s.deaths, s.kills), (0, 1, 0), "{s:?}");
    assert!(m.view.scores.iter().all(|s| s.score == 0));
}
