//! Tests of the runtime's animation drawing (integration tests, so they link the normal library
//! build instead of recompiling the whole crate in test mode): the simulation's notify records
//! resolved for the presentation (`anim_notify::resolve`, as the config's sequence table records
//! them), the look-at mode and the matinee channels the tree posing sets (`npc_anim`).

use dishonored_ir::gameplay::NotifyInfo;
use dishonored_ir::npc_anim::{LookatConfig, LookatMode};
use dishonored_runtime::anim_notify::{DropNotify, Notify, SeverNotify, resolve};
use dishonored_runtime::npc_anim::{Scripted, lookat_mode, matinee_channels};

fn record(class: &str, names: &[(&str, &str)], payload: &[(&str, f32)]) -> NotifyInfo {
    NotifyInfo { class: class.into(), names: names.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), payload: payload.iter().map(|(k, v)| (k.to_string(), *v)).collect(), ..Default::default() }
}

#[test]
fn sound_and_footfall_notifies() {
    // `Sword_Ready_AttackRight_A_Small`'s whoosh: the AkEvent's object name.
    let whoosh = record("AnimNotify_AkEvent", &[("AkEvent", "Snd_Weapon.Snd_Weapon_Sword_Whooshe_ak"), ("BoneName", "handAttachment_R_jnt")], &[("PitchMultiplier", 1.1)]);
    assert_eq!(resolve(&whoosh), Notify::Sound { event: "Snd_Weapon_Sword_Whooshe_ak".into() });
    assert_eq!(resolve(&record("AnimNotify_AkEvent", &[("AkEvent", "None")], &[])), Notify::Other, "no event");
    assert_eq!(resolve(&record("DishonoredNotify_Footfall", &[], &[("m_iFootIndex", 1.0)])), Notify::Footfall { foot: 1 });
    assert_eq!(resolve(&record("DishonoredNotify_Footfall", &[], &[])), Notify::Footfall { foot: 0 });
}

#[test]
fn particle_drop_sever_ragdoll_and_lens_notifies() {
    // `Powers_Cast_BendTime`'s glow on the Mark's socket, attached.
    let glow = record("AnimNotify_PlayParticleEffect", &[("PSTemplate", "Vfx_GamePlay.Powers.Ps_Tattoo_Glow_01"), ("SocketName", "Tattoo")], &[("bAttach", 1.0)]);
    assert_eq!(resolve(&glow), Notify::Particle { template: "Vfx_GamePlay.Powers.Ps_Tattoo_Glow_01".into(), point: "Tattoo".into(), attach: true });
    // Without a socket, on the bone.
    let on_bone = record("AnimNotify_PlayParticleEffect", &[("PSTemplate", "Fx.Splash"), ("SocketName", "None"), ("BoneName", "index_1_L_jnt")], &[]);
    assert_eq!(resolve(&on_bone), Notify::Particle { template: "Fx.Splash".into(), point: "index_1_L_jnt".into(), attach: false });
    // `Generic_DeathFrontA`'s drop has no property over the class defaults: every equipped item,
    // no velocity.
    assert_eq!(resolve(&record("DisNotify_DropItem", &[], &[])), Notify::DropItem(DropNotify { which: 0, primary: true, velocity: glam::Vec3::ZERO }));
    let by_usage = record("DisNotify_DropItem", &[("m_WhichItem", "DisWhichEquippedItemToDrop_ByEquipUsage"), ("m_ByUsage", "EDisEquipUsage_Secondary")], &[("m_DropVelocity.X", 10.0), ("m_DropVelocity.Z", 50.0)]);
    assert_eq!(resolve(&by_usage), Notify::DropItem(DropNotify { which: 2, primary: false, velocity: glam::Vec3::new(10.0, 0.0, 50.0) }));
    // A sever: the class defaults where the record has none (blood, ragdoll, impulse 300).
    let chop = record("DishonoredNotify_SeverLimb", &[("m_SeverBoneName", "neck_jnt"), ("m_ImpulseMode", "SeverImpulse_SeverBoneZ")], &[("m_bHideLimb", 1.0)]);
    assert_eq!(resolve(&chop), Notify::SeverLimb(SeverNotify { bone: "neck_jnt".into(), blood: true, hide: true, ragdoll: true, mode: 3, strength: 300.0 }));
    assert_eq!(resolve(&record("DishonoredNotify_SeverLimb", &[], &[])), Notify::Other, "no bone to cut");
    assert_eq!(resolve(&record("DishonoredNotify_Ragdoll", &[], &[])), Notify::Ragdoll);
    // `Sword_Ready_Assassination_Front_Master`'s blood on the lens: the class's object name.
    let lens = record("AnimNotify_CameraEffect", &[("CameraLensEffect", "DishonoredGame.DisEmitterCameraLensEffect_blood_medium")], &[]);
    assert_eq!(resolve(&lens), Notify::CameraEffect { class: "DisEmitterCameraLensEffect_blood_medium".into() });
    // The simulation's own classes ask nothing of the presentation.
    assert_eq!(resolve(&record("DishonoredNotify_AttackZone", &[], &[("m_fDamageZoneMaxTime", 0.05)])), Notify::Other);
}

#[test]
fn lookat_mode_follows_the_locomotion_stance_and_gait() {
    let mode = |n: &str| LookatMode { name: n.into(), ..Default::default() };
    let cfg = LookatConfig { modes: vec![mode("EmptyIdle"), mode("SwordIdle"), mode("SwordWalk"), mode("SearchRun")], ..Default::default() };
    let name = |m: &str, speed: f32| lookat_mode(&cfg, m, speed).map(|x| x.name.clone()).unwrap();
    assert_eq!(name("SwordPatrol", 0.0), "SwordIdle");
    assert_eq!(name("Sword", 100.0), "SwordWalk");
    assert_eq!(name("Aim", 10.0), "SwordIdle");
    assert_eq!(name("SearchSword", 400.0), "SearchRun");
    // A mode the config does not have: its first.
    assert_eq!(name("PanicEmpty", 400.0), "EmptyIdle");
}

#[test]
fn matinee_tracks_are_the_stack_channels() {
    let s = |n: &str, w: f32| Scripted { name: n.into(), time: 1.5, weight: w, ..Default::default() };
    let tracks: Vec<Scripted> = (0..7).map(|k| s(&format!("Gesture{k}"), 0.5)).collect();
    let ch = matinee_channels(&tracks);
    assert_eq!(ch.len(), 6, "the node's six channels");
    for (k, c) in ch.iter().enumerate() {
        assert_eq!((c.stack.as_str(), c.channel, c.sequence.as_str(), c.time, c.weight), ("MatineeStackNode", k, format!("Gesture{k}").as_str(), 1.5, 0.5));
    }
}
