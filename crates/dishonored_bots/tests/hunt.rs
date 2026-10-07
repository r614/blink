//! Test bots on synthetic worlds: they hunt opponents they cannot see and keep off drops and
//! deep water.
use dishonored_bots::{Bot, BotStyle, hazard};
use dishonored_sim::SimConfig;
use dishonored_sim::state::{MoveMode, WorldState};
use dishonored_sim::{spawn_player, step_world};
use dishonored_world::{CollisionWorld, Tri, WaterVolume, channel};
use glam::Vec3;

fn cfg() -> SimConfig {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
    SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn quad(tris: &mut Vec<Tri>, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
    for (x, y, z) in [(a, b, c), (a, c, d)] {
        let n = (y - x).cross(z - x).normalize();
        tris.push(Tri { a: x, b: y, c: z, n, channels: channel::ALL, source: 0 });
    }
}

/// A floor at z 0 over x in [-3000, 0] (a ledge at x = 0, a 2000 uu drop into a water volume
/// below), and a wall at x = -1500 from y -500 to 3000 between the two halves of the floor.
fn ledge_over_water() -> CollisionWorld {
    let mut tris = Vec::new();
    let v = Vec3::new;
    quad(&mut tris, v(-3000., -3000., 0.), v(0., -3000., 0.), v(0., 3000., 0.), v(-3000., 3000., 0.));
    quad(&mut tris, v(-1500., -500., 0.), v(-1500., -500., 400.), v(-1500., 3000., 400.), v(-1500., 3000., 0.));
    let mut w = CollisionWorld::from_triangles(tris, vec!["fixture".into()]);
    w.nav = Some(std::sync::Arc::new(dishonored_world::nav::NavWorld::from_data(&ledge_nav())));
    let planes = vec![[1., 0., 0., 4000.], [-1., 0., 0., -10.], [0., 1., 0., 3000.], [0., -1., 0., 3000.], [0., 0., 1., -2000.], [0., 0., -1., 4000.]];
    w.water = Some(std::sync::Arc::new(vec![WaterVolume { name: "sea".into(), hulls: vec![planes], min: v(10., -3000., -4000.), max: v(4000., 3000., -2000.), fluid_friction: 2.4, terminal_velocity: 4000.0, shallow: false }]));
    w
}

/// The floor's navmesh: west of the wall (A), the strip south of the wall's end (B) and the
/// part east of the wall (C); A and C meet only through B.
fn ledge_nav() -> dishonored_ir::nav::NavData {
    use dishonored_ir::nav::{NavData, NavEdge, NavEdgeClass, NavEdgeType, NavMesh, NavPoly, NavPylon};
    let v = |x: f32, y: f32| Vec3::new(x, y, 0.0);
    let verts = vec![v(-3000., -3000.), v(-1500., -3000.), v(-1500., -500.), v(-1500., 3000.), v(-3000., 3000.), v(0., -3000.), v(0., -500.), v(0., 3000.)];
    let poly = |vs: Vec<u16>, edges: Vec<u16>, min: Vec3, max: Vec3| NavPoly { center: (min + max) * 0.5, verts: vs, edges, normal: Vec3::Z, min, max, height: 200.0 };
    let edge = |v0, v1, p0, p1| NavEdge { class: NavEdgeClass::Base, kind: NavEdgeType::Normal, v0, v1, poly0: p0, poly1: p1, length: 1000.0, effective_length: 1000.0, center: Vec3::ZERO, path_object: None };
    let m = NavMesh {
        source: "test".into(),
        version: (28, 28),
        verts,
        polys: vec![
            poly(vec![0, 1, 2, 3, 4], vec![0], v(-3000., -3000.), v(-1500., 3000.)),
            poly(vec![1, 5, 6, 2], vec![0, 1], v(-1500., -3000.), v(0., -500.)),
            poly(vec![2, 6, 7, 3], vec![1], v(-1500., -500.), v(0., 3000.)),
        ],
        edges: vec![edge(1, 2, 0, 1), edge(2, 6, 1, 2)],
        min: v(-3000., -3000.),
        max: v(0., 3000.),
    };
    NavData { pylons: vec![NavPylon { name: "p".into(), package: "t".into(), object_ref: 1, mesh: m, ..Default::default() }], path_objects: Vec::new() }
}

#[test]
fn hazard_probes() {
    let w = ledge_over_water();
    let at = Vec3::new(-100.0, 0.0, 90.0);
    assert!(hazard::safe_ahead(&w, at, -Vec3::X, 70.0), "back onto the floor");
    assert!(!hazard::safe_ahead(&w, at, Vec3::X, 150.0), "off the ledge");
    assert!(hazard::safe_ahead(&w, Vec3::new(-1450.0, 0.0, 90.0), -Vec3::X, 70.0), "into a wall is harmless");
    assert!(hazard::deep_water(&w, Vec3::new(500.0, 0.0, -2100.0)));
    assert!(!hazard::deep_water(&w, at));
}

/// A bot walks around a wall toward an opponent it cannot see, and does not follow it off a
/// ledge into the sea.
#[test]
fn bot_hunts_unseen_opponent_and_keeps_off_the_ledge() {
    let w = ledge_over_water();
    let sim = cfg();
    let mut st = WorldState::default();
    // The opponent stands behind the wall; later it waits at the ledge's edge.
    spawn_player(&mut st, 1, Vec3::new(-2500.0, 1500.0, 92.0), 0.0, &sim);
    spawn_player(&mut st, 2, Vec3::new(-800.0, 1500.0, 92.0), 0.0, &sim);
    let mut bot = Bot::new(1, BotStyle::Duelist, 7);
    let start = st.player(1).unwrap().position;
    let mut seen_hunt = false;
    for t in 0..(60 * 20) {
        if t == 60 * 8 {
            // The opponent moves off the edge into the air beyond the ledge (out of reach).
            let o = st.players.iter_mut().find(|p| p.id == 2).unwrap();
            o.position = Vec3::new(250.0, 0.0, 92.0);
        }
        let snap = st.players.clone();
        let me = snap.iter().find(|p| p.id == 1).unwrap();
        let input = bot.think(me, &snap, &w);
        seen_hunt |= bot.hunt == Some(2) && bot.target.is_none();
        // The opponent stands still (and floats where it was put).
        step_world(&mut st, &[(1, input)], &w, &sim);
        if let Some(o) = st.players.iter_mut().find(|p| p.id == 2) {
            o.velocity = Vec3::ZERO;
            if t >= 60 * 8 {
                o.position = Vec3::new(250.0, 0.0, 92.0);
                o.mode = MoveMode::Walking;
            }
        }
        let p = st.player(1).unwrap();
        assert!(p.alive && p.position.x < 0.0 && p.mode != MoveMode::Swimming, "t={t}: {:?} {:?}", p.position, p.mode);
    }
    assert!(seen_hunt, "hunted the unseen opponent");
    let p = st.player(1).unwrap();
    assert!(p.position.distance(start) > 1500.0, "went after the opponent: {:?}", p.position);
    assert!(p.position.x > -400.0, "waits at the ledge: {:?}", p.position);
}
