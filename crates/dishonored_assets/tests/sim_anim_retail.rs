//! The simulation's sequence table and NPC profile against the canonical GOG build (skipped
//! without the install): lengths, rate scales and notifies as cooked, the notify classes'
//! defaults under the cooked properties, the elite guard's chains and sets.

use dishonored_assets::Content;
use dishonored_assets::tweaks::TweakDb;
use std::collections::BTreeMap;

fn table() -> Option<BTreeMap<String, dishonored_ir::gameplay::SequenceInfo>> {
    let c = Content::locate().ok()?;
    let db = TweakDb::load(&c).unwrap();
    let mut prov = BTreeMap::new();
    Some(dishonored_assets::sim_anim::load_sequences(&c, &db, &BTreeMap::new(), &mut prov).unwrap())
}

#[test]
fn player_attack_zone_as_cooked() {
    let Some(t) = table() else { return };
    let s = &t["Ply_Sword_Attack_as.Sword_Ready_AttackRight_A_Small"];
    assert!((s.duration - 1.1).abs() < 1e-3 && s.rate_scale == 1.0, "{} {}", s.duration, s.rate_scale);
    let z = s.notify("DishonoredNotify_AttackZone").unwrap();
    assert!((z.time - 0.138).abs() < 1e-3 && (z.duration - 0.207).abs() < 1e-3, "{z:?}");
    assert_eq!(z.payload.get("m_fDamageZoneMaxTime").copied(), Some(0.05));
    assert_eq!(z.payload.get("m_fVersusZoneMaxTime").copied(), Some(0.07));
    assert_eq!(z.names.get("m_SwingDirection").map(String::as_str), Some("EDisSwingDirection_RightLeft"));
    assert!((s.notify_time("DishonoredNotify_AttackInterruptable").unwrap() - 0.632).abs() < 1e-3);
    // Notifies keep the cooked (time) order.
    assert!(s.notifies.windows(2).all(|w| w[0].time <= w[1].time));
}

#[test]
fn weapon_and_power_notifies() {
    let Some(t) = table() else { return };
    let fire = &t["Crossbow_as.Crossbow_Fire"];
    assert!((fire.notify_time("DishonoredNotify_FireProjectile").unwrap() - 0.0336).abs() < 1e-3);
    assert!((t["Ply_Crossbow_as.Crossbow_Fire"].exit_time() - 0.398).abs() < 1e-3);
    let cast = &t["Ply_Powers_as.Powers_Cast_BendTime"];
    assert!((cast.notify_time("DisNotify_BendTime").unwrap() - 0.140).abs() < 1e-3);
    assert!((cast.exit_time() - 1.295).abs() < 1e-3);
    // A sequence without an AnimStateExit ends with its length.
    let razor = &t["Ply_Gadgets_SpringRazor_as.Gadgets_SpringRazor_Deploy"];
    assert!(razor.notify("DishonoredNotify_AnimStateExit").is_none());
    assert!((razor.exit_time() - 0.367).abs() < 1e-3);
}

#[test]
fn elite_guard_attack_and_profile() {
    let Some(t) = table() else { return };
    let a = &t["Npc_EliteGuard_Sword_as.Sword_Attack_ShortSmallA"];
    let z = a.notify("DishonoredNotify_AttackZone").unwrap();
    assert!((z.time - 0.490).abs() < 1e-3 && (z.duration - 0.454).abs() < 1e-3, "{z:?}");
    assert!((a.notify_time("DishonoredNotify_AttackInterruptable").unwrap() - 1.020).abs() < 1e-3);
    // An AttackZone that leaves `m_fVersusZoneMaxTime` takes the class default.
    let l = &t["Npc_EliteGuard_Sword_as.Sword_Attack_Left180SmallA"].notify("DishonoredNotify_AttackZone").unwrap().payload;
    assert!(l.contains_key("m_fVersusZoneMaxTime"), "{l:?}");
    let c = Content::locate().unwrap();
    let npc = dishonored_assets::sim_anim::load_npc_anim(&c, &mut BTreeMap::new()).unwrap();
    assert_eq!(npc.defs.class, "DisNPCAnimDefinitions_EliteGuard");
    assert!(npc.sets.iter().any(|s| s == "Npc_EliteGuard_Sword_as"), "{:?}", npc.sets);
    assert_eq!(npc.item_prefix, "Sword_");
    assert!(npc.defs.chain(dishonored_ir::npc_anim::NpcAnim::AttackShort).is_some());
}
