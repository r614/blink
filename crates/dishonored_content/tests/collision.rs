//! The map's collision against facts measured in the original with the engine hook
//! (research/dishonored/COLLISION.md). Skipped without the retail install.

use dishonored_content::load_match;
use dishonored_world::{PawnBox, WorldQuery, channel};
use glam::Vec3;

fn ext() -> Option<dishonored_content::MatchContent> {
    let c = dishonored_assets::Content::locate().ok()?;
    if !c.has_package("L_Boyle_Ext_P") {
        return None;
    }
    load_match("L_Boyle_Ext_P").ok()
}

/// Hidden collision proxies (`Env_Blockout.collisions.Collision_model`, a mesh the components
/// inherit from their archetype) block the player's movement but not projectiles or sight.
#[test]
fn collision_proxies_block_movement_only() {
    let Some(mc) = ext() else { return };
    let w = &mc.collision;
    let proxies = w.sources.iter().filter(|s| s.starts_with("Env_Blockout.collisions.Collision_model")).count();
    eprintln!("{proxies} collision proxies");
    assert!(proxies >= 100, "{proxies}");
    // A proxy near the guard house: a horizontal line from inside it hits where it starts for
    // the player's movement (measured: time 0), and passes for a projectile.
    let start = Vec3::new(-1703.0, -4005.0, -88.0);
    for (si, s) in w.sources.iter().enumerate().filter(|s| s.1.starts_with("Env_Blockout.collisions.Collision_model")) {
        let (lo, hi) = w.tris.iter().filter(|t| t.source as usize == si).fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), t| (a.min(t.a).min(t.b).min(t.c), b.max(t.a).max(t.b).max(t.c)));
        if (lo + hi).truncate().distance(start.truncate() * 2.0) < 2000.0 {
            eprintln!("{s}: {lo:.0?}..{hi:.0?}");
        }
    }
    let h = w.raycast(start, Vec3::X, 600.0, channel::PAWN).expect("player blocked");
    assert!(h.distance < 0.5, "{}", h.distance);
    assert!(w.raycast(start, Vec3::X, 600.0, channel::PROJECTILE).is_none_or(|h| h.distance > 1.0));
}

/// Static meshes the designers set to gameplay traces only (`m_CollisionTraceTypes` without
/// the movement bits) do not stop the player's box, and the garden start stands on its
/// blocking volume.
#[test]
fn trace_types_split_movement_and_gameplay() {
    let Some(mc) = ext() else { return };
    let w = &mc.collision;
    let b = PawnBox::new(Vec3::new(-2714.0, -1879.0, 150.0), 35.0, 87.5);
    let h = w.sweep_box(&b, Vec3::new(0.0, 0.0, -300.0), channel::PAWN).expect("floor");
    let z = 150.0 - 300.0 * h.fraction;
    // Measured in the original: the box rests at z 86.5 (the volume's top at -1, minus the
    // 4 uu... the line check's back-off is not part of the port's sweeps).
    assert!((z - 86.5).abs() < 2.0, "{z}");
}
