//! Weapons dropped by dying pawns against the canonical GOG build (skipped without the install):
//! the pickup each third-person weapon's item tweak drops, and where the item's Drop puts its
//! static mesh (0x00C0FB30: the item mesh's transform turned -90 degrees in yaw).

use dishonored_assets::Content;
use dishonored_assets::characters::{load_model, load_static_model};
use dishonored_assets::pickups::dropped_pickup;
use dishonored_assets::util::rotator_matrix;
use glam::Vec3;

fn content() -> Option<Content> {
    Content::locate().ok()
}

fn bounds(points: impl Iterator<Item = Vec3>) -> (Vec3, Vec3) {
    points.fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(p), hi.max(p)))
}

#[test]
fn every_weapon_drops_its_pickup() {
    let Some(c) = content() else { return };
    let script = vec!["L_Boyle_Ext_Script".to_string()];
    for (item, mesh) in [
        ("Twk_Inv_EliteGuard.Twk_Inv_SwordEliteGuard", "Wpn_EliteSword.Wpn_EliteSword_world"),
        ("Twk_Inv_PlayerSpecific.Twk_Inv_SwordCorvo", "Wpn_PlySwords.Wpn_PlySword01Closed_world"),
        ("Twk_Inv_PlayerSpecific.Twk_Inv_PistolCorvo", "Wpn_EliteGun.EliteGun_world"),
        ("Twk_Inv_PlayerSpecific.Twk_Inv_CrossbowCorvo", "Crossbows.crossbow_01_world"),
    ] {
        let d = dropped_pickup(&c, &script, item).unwrap_or_else(|e| panic!("{item}: {e:#}"));
        assert!(d.mesh.1.eq_ignore_ascii_case(mesh), "{item}: {:?}", d.mesh);
        // `DisTweaks_InventoryItem` class default; no weapon overrides it.
        assert_eq!(d.chance, 1.0, "{item}");
    }
}

/// The swords' swing trails: the AnimTrail of their blade extents, on the blade's corner sockets,
/// and the attack animations' trail windows.
#[test]
fn sword_swings_leave_trails() {
    let Some(c) = content() else { return };
    let script = vec!["L_Boyle_Ext_Script".to_string()];
    let db = dishonored_assets::tweaks::TweakDb::load_with(&c, &script).unwrap();
    for item in ["Twk_Inv_EliteGuard.Twk_Inv_SwordEliteGuard", "Twk_Inv_PlayerSpecific.Twk_Inv_SwordCorvo"] {
        let t = dishonored_assets::melee_trail::melee_trail_in(&db, item).unwrap().unwrap_or_else(|| panic!("{item}: no trail"));
        assert_eq!(t.template, "Vfx_Weapon.Effects.Trails.Sword_Trail", "{item}");
        assert_eq!(t.sockets, ["BladeExtent_BL".to_string(), "BladeExtent_UR".to_string()], "{item}");
    }
    let w = dishonored_assets::characters::trail_notifies(&c, "L_Boyle_Ext_Script", "Npc_EliteGuard_Sword_as.Npc_EliteGuard_Sword_as").unwrap();
    assert_eq!(w.len(), 18, "{w:?}");
    assert!(w.iter().all(|(_, t, d)| *t >= 0.0 && *d > 0.1), "{w:?}");
}

/// The body burnt to ash: `Twk_Pawn_DefaultNPC`'s sequence and `Shadow_kill_inst`'s mask.
#[test]
fn ash_bodies_fade_then_burn_away() {
    let Some(c) = content() else { return };
    let d = dishonored_assets::shadow_kill::ash_body_def(&c).unwrap();
    assert_eq!((d.delay, d.fade_to_black, d.fiery), (0.1, 0.2, 2.0));
    assert_eq!((d.tiling, d.edge_size), (2.0, 25.0));
    assert!(d.mask.1.eq_ignore_ascii_case("Vfx_Materials.Textures.Water_foam"), "{:?}", d.mask);
    // Nearly whole at the start, mostly gone at the end.
    let (start, end) = (dishonored_assets::shadow_kill::fiery_cutoff(1.0, d.edge_size), dishonored_assets::shadow_kill::fiery_cutoff(0.0, d.edge_size));
    assert!(start < 0.1 && end > 0.9, "{start} {end}");
}

/// The dropped sword's static mesh, placed by the Drop's turn, lies where the held sword's
/// skeletal mesh was (its box under the item component's Origin / RotOrigin).
#[test]
fn the_dropped_sword_starts_where_the_held_one_was() {
    let Some(c) = content() else { return };
    let script = vec!["L_Boyle_Ext_Script".to_string()];
    let d = dropped_pickup(&c, &script, "Twk_Inv_EliteGuard.Twk_Inv_SwordEliteGuard").unwrap();
    let held = load_model(&c, "Wpn_EliteSword.Wpn_EliteSword", &[("Startup", "Wpn_EliteSword.Wpn_EliteSword")], None, 64).unwrap();
    let part = &held.parts[0];
    let (hl, hh) = bounds(part.positions.iter().map(|p| part.mesh_to_pawn.transform_point3(Vec3::from(*p))));
    let world = load_static_model(&c, &d.mesh.0, &d.mesh.1, 64).unwrap();
    let turn = rotator_matrix(0, -16384, 0);
    let (wl, wh) = bounds(world.parts[0].positions.iter().map(|p| turn.transform_point3(Vec3::from(*p))));
    assert!((hl - wl).abs().max_element() < 1.0 && (hh - wh).abs().max_element() < 1.0, "held {hl} {hh}, dropped {wl} {wh}");
}
