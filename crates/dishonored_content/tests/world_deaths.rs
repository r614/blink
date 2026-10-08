//! Where pawns could leave Boyle Ext's playable space, against the original level data (skips
//! without the install). See research/dishonored/PLAYER.md "Level collision".
use dishonored_content::{DEFAULT_MAP, load_match};
use dishonored_sim::state::{MoveMode, WorldState};
use dishonored_sim::{spawn_player, step_world};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;

/// The level transition doors are closed for good: Boyle's front door (`DishonoredUsableObject_6`,
/// courtyard to foyer) fills the doorway the courtyard wall leaves open. Without its bodies a
/// pawn walked through onto a ledge and fell into the sea.
#[test]
fn boyle_front_door_blocks_the_doorway() {
    if dishonored_assets::Content::locate().is_err() {
        return;
    }
    let mc = load_match(DEFAULT_MAP).unwrap();
    let w = &mc.collision;
    // All four transition doors to the interior (balcony, courtyard, garden, sewer) collide.
    for d in ["DishonoredUsableObject_5", "DishonoredUsableObject_6", "DishonoredUsableObject_7", "DishonoredUsableObject_8"] {
        let src = w.sources.iter().position(|s| s.ends_with(&format!(":{d}"))).unwrap_or_else(|| panic!("{d} has no collision"));
        let n = w.tris.iter().filter(|t| t.source as usize == src).count();
        println!("{d}: {n} triangles");
        assert!(n >= 12 && n % 12 == 0, "{d}: {n}");
    }
    for y in [1350.0, 1431.0, 1500.0] {
        let o = Vec3::new(1300.0, y, 80.0);
        let h = w.raycast(o, Vec3::X, 2000.0, channel::PAWN).expect("doorway blocked");
        let src = &w.sources[w.tris[h.tri as usize].source as usize];
        assert!(src.ends_with("DishonoredUsableObject_6"), "{src} at {}", o.x + h.distance);
        assert!((o.x + h.distance - 1510.0).abs() < 20.0, "{}", o.x + h.distance);
    }
    // A pawn pushing east from the front-door PlayerStart stays in the courtyard.
    let mut st = WorldState::default();
    spawn_player(&mut st, 1, Vec3::new(1355.0, 1445.0, 98.0), 0.0, &mc.sim);
    let input = dishonored_sim::input::PlayerInput { forward: 127, yaw: 0.0, ..Default::default() };
    for _ in 0..300 {
        step_world(&mut st, &[(1, input)], w, &mc.sim);
    }
    let p = st.player(1).unwrap();
    assert!(p.position.x < 1500.0 && p.position.z > 0.0 && p.mode == MoveMode::Walking, "{:?} {:?}", p.position, p.mode);
}

/// Two PlayerStarts stand on `BlockingVolume`s (their `Base`): the garden start on
/// `BlockingVolume_5` and the river grate start on `BlockingVolume_2`. A pawn spawned there
/// stands instead of falling through.
#[test]
fn playerstarts_stand_on_their_blocking_volumes() {
    if dishonored_assets::Content::locate().is_err() {
        return;
    }
    let mc = load_match(DEFAULT_MAP).unwrap();
    let w = &mc.collision;
    assert_eq!(w.sources.iter().filter(|s| s.contains(":BlockingVolume_")).count(), 8);
    for (start, volume) in [(Vec3::new(-2663.0, -1879.0, 98.0), "BlockingVolume_5"), (Vec3::new(-921.0, 1160.0, -1835.0), "BlockingVolume_2")] {
        let h = w.raycast(start, -Vec3::Z, 400.0, channel::PAWN).expect("floor under the start");
        let src = &w.sources[w.tris[h.tri as usize].source as usize];
        // The volume's top, or a floor in the same place (the river grate's sewer floor at
        // z -1935 coincides with `BlockingVolume_2`'s top).
        let top = w.tris.iter().filter(|t| w.sources[t.source as usize].ends_with(volume)).map(|t| t.a.z.max(t.b.z).max(t.c.z)).fold(f32::MIN, f32::max);
        assert!(src.ends_with(volume) || (h.point.z - top).abs() < 1.0, "{start}: {src} at {}, volume top {top}", h.point.z);
        let mut st = WorldState::default();
        spawn_player(&mut st, 1, start, 0.0, &mc.sim);
        for _ in 0..120 {
            step_world(&mut st, &[], w, &mc.sim);
        }
        let p = st.player(1).unwrap();
        assert!((p.position.z - start.z).abs() < 30.0 && p.mode == MoveMode::Walking, "{start}: {:?} {:?}", p.position, p.mode);
    }
}

/// Free-for-all spawns: of Boyle Ext's five PlayerStarts, the scripted Samuel drop-off is off
/// the navmesh (in the canal under the visual water, outside the sea's water volume: a pawn
/// left there walks off the canal floor to `KillZ`), and the roof patio and river grate starts
/// stand on navmesh islands of their own. The garden and front-door starts share the estate
/// grounds.
#[test]
fn ffa_spawns_are_the_estate_starts() {
    if dishonored_assets::Content::locate().is_err() {
        return;
    }
    let mc = load_match(DEFAULT_MAP).unwrap();
    assert_eq!(mc.spawns.len(), 5);
    let s = dishonored_content::ffa_spawns(&mc);
    let mut xy: Vec<(i32, i32)> = s.iter().map(|p| (p.position.x.round() as i32, p.position.y.round() as i32)).collect();
    xy.sort();
    assert_eq!(xy, vec![(-2663, -1879), (1355, 1445)]);
}

/// AI pawns go by their original NPC names (the pawn tweak's localized interactable `m_Name`),
/// never by id: the built-in party guests and guards, and a tweak-resolved look.
#[test]
fn npc_names_come_from_the_pawn_tweaks() {
    if dishonored_assets::Content::locate().is_err() {
        return;
    }
    use dishonored_sim::state::PawnKind;
    assert_eq!(dishonored_content::pawn_name(PawnKind::Civilian, 0, &[]).as_deref(), Some("Party Guest"));
    assert_eq!(dishonored_content::pawn_name(PawnKind::Guard, 1, &[]).as_deref(), Some("Watch Officer"));
    assert_eq!(dishonored_content::pawn_name(PawnKind::Civilian, 3, &[]).as_deref(), Some("Party Guest"));
    let looks = vec![
        dishonored_multiplayer::Look { tweak: "Twk_Pawn_Boyle_Party.Twk_Pawn_LordBrisby".into(), ..Default::default() },
        dishonored_multiplayer::Look { tweak: "Twk_Pawn_Boyle_Party.Twk_Pawn_Boyle_Aristocrat1".into(), ..Default::default() },
    ];
    let base = dishonored_multiplayer::mission::APPEARANCE_BASE;
    assert_eq!(dishonored_content::pawn_name(PawnKind::Civilian, base, &looks).as_deref(), Some("Lord Brisby"));
    // A tweak without a name of its own falls back to its role's party tweak.
    assert_eq!(dishonored_content::pawn_name(PawnKind::Civilian, base + 1, &looks).as_deref(), Some("Party Guest"));
}

/// Contracts spawns: like free-for-all, never on the scripted canal drop-off or the navmesh
/// islands (roof patio, river grate), where a player can never reach anyone; every spawn is
/// walkable to the estate starts.
#[test]
fn contracts_spawns_stay_in_the_walkable_estate() {
    if dishonored_assets::Content::locate().is_err() {
        return;
    }
    use dishonored_multiplayer::contracts::ContractsRules;
    use dishonored_multiplayer::game::{CrowdSource, GameSetup, crowd_center, default_crowd};
    let mc = load_match(DEFAULT_MAP).unwrap();
    let pts: Vec<(Vec3, f32)> = mc.spawns.iter().map(|s| (s.position, s.yaw)).collect();
    let rules = ContractsRules { warmup_time: 0.0, crowd: Some(default_crowd(crowd_center(&pts))), ..Default::default() };
    let src = CrowdSource { points: &mc.ai_points, guard_spawns: &mc.guard_spawns, flee_points: &mc.flee_points, routes: &mc.routes, ai: Some((&mc.ai_level, &mc.spawners)) };
    let mut g = GameSetup::contracts(rules, pts, Some(src), &mc.collision).build(&mc.sim);
    let nav = mc.collision.nav().unwrap();
    let garden = dishonored_content::ffa_spawns(&mc)[0].position;
    for id in 1..=8 {
        g.add_player(id, &format!("p{id}"), 0, &mc.collision, &mc.sim);
        let p = g.world().player(id).unwrap().position;
        assert!(nav.find_path(p, garden, &Default::default()).is_some(), "player {id} spawned at {p:?}, cut off from the estate");
    }
}
