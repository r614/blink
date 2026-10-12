//! The simulation's animation notifies (`SimEvent::AnimNotify`, fired by `dishonored_sim::anim`
//! for every notify a layer's sequence crosses in `[t0, t1)`) resolved through the config's
//! sequence table (`GameplayConfig::sequences`: each notify's class, time, duration and the
//! notify object's properties over its class defaults) and handed to the presentation, stamped
//! with the tick the simulation fired them in. Nothing scans clip windows on a render clock:
//! - sounds: `AnimNotify_AkEvent` posts its event on the pawn (`FUN_00716bd0`),
//!   `DishonoredNotify_Footfall` a footfall under it (`FUN_00b66ed0`), through
//!   `audio::AnimSounds`;
//! - the local player's camera lens effects (`AnimNotify_CameraEffect`, `lens_effects`);
//! - what needs the pawn's drawn mesh, from [`AnimNotifies::events`]: particles
//!   (`AnimNotify_PlayParticleEffect`) on its sockets, its items dropped (`DisNotify_DropItem`),
//!   its limbs severed (`DishonoredNotify_SeverLimb`) and the ragdoll hand-over
//!   (`DishonoredNotify_Ragdoll`) in `characters.rs`; the first-person rig's particles in
//!   `viewmodel.rs`.
//!
//! A notify with a duration (`DisNotify_Trails`) is a window the simulation keeps open on the
//! layer (`dishonored_sim::anim::PawnAnim::window_open`): the trails read it there.

use crate::session::{Session, SessionView};
use bevy::prelude::*;
use dishonored_ir::gameplay::NotifyInfo;
use dishonored_sim::events::SimEvent;
use dishonored_sim::state::PlayerId;
use std::collections::HashMap;

/// A `DishonoredNotify_SeverLimb` (0x00A966D0, the notify's values over its class defaults:
/// blood, no hiding, ragdoll, impulse 300): the pawn's limb at `bone` is cut off (0x00B6CC10,
/// `crate::severed_limbs`).
#[derive(Clone, Debug, PartialEq)]
pub struct SeverNotify {
    pub bone: String,
    /// `m_bUseBlood`: the limb's particle effects play.
    pub blood: bool,
    /// `m_bHideLimb`: the limb only disappears (no severed piece).
    pub hide: bool,
    /// `m_bGoToRagdoll`.
    pub ragdoll: bool,
    /// `m_ImpulseMode`: 0 none, 1 / 2 / 3 the severed bone's X / Y / Z axis.
    pub mode: u8,
    /// `m_fImpulseStrength`.
    pub strength: f32,
}

/// A `DisNotify_DropItem` (0x00A8D400): a dead pawn (health below 1, pawn slot 348 0x00B49480)
/// lets go of its items.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropNotify {
    /// `m_WhichItem` (`DisWhichEquippedItemToDrop`): 0 every equipped item, 1 every attached
    /// item, 2 the item of `m_ByUsage`.
    pub which: u8,
    /// `m_ByUsage` (`EDisEquipUsage`) is not the secondary usage (the off hand).
    pub primary: bool,
    /// `m_DropVelocity`, in the pawn mesh's frame (turned by its LocalToWorld).
    pub velocity: glam::Vec3,
}

/// What a notify asks of the presentation, by its class.
#[derive(Clone, Debug, PartialEq)]
pub enum Notify {
    /// `AnimNotify_AkEvent`: the AkEvent's name.
    Sound { event: String },
    /// `DishonoredNotify_Footfall`: `m_iFootIndex`.
    Footfall { foot: i32 },
    /// `AnimNotify_PlayParticleEffect`: the system, its socket (else bone) and `bAttach`.
    Particle { template: String, point: String, attach: bool },
    DropItem(DropNotify),
    SeverLimb(SeverNotify),
    /// `DishonoredNotify_Ragdoll`: the body goes to physics.
    Ragdoll,
    /// `AnimNotify_CameraEffect`: the `CameraLensEffect` class.
    CameraEffect { class: String },
    /// A class only the simulation handles, or none consumes yet.
    Other,
}

/// A name, enum or object property of a notify record ("None" and unset are absent).
fn name<'a>(n: &'a NotifyInfo, key: &str) -> Option<&'a str> {
    n.names.get(key).map(String::as_str).filter(|v| !v.is_empty() && *v != "None")
}

/// The object name of an object property (the record keeps the object's path).
fn object_name<'a>(n: &'a NotifyInfo, key: &str) -> Option<&'a str> {
    name(n, key).map(|p| p.rsplit('.').next().unwrap_or(p))
}

/// A bool property (payload 0 / 1), `default` when unset.
fn flag(n: &NotifyInfo, key: &str, default: bool) -> bool {
    n.payload.get(key).map_or(default, |v| *v != 0.0)
}

/// What a notify record asks of the presentation.
pub fn resolve(n: &NotifyInfo) -> Notify {
    match n.class.as_str() {
        "AnimNotify_AkEvent" => match object_name(n, "AkEvent") {
            Some(e) => Notify::Sound { event: e.to_string() },
            None => Notify::Other,
        },
        "DishonoredNotify_Footfall" => Notify::Footfall { foot: n.payload.get("m_iFootIndex").copied().unwrap_or(0.0) as i32 },
        // The system plays on the socket (else the bone) of the mesh, attached or where it was.
        "AnimNotify_PlayParticleEffect" => match name(n, "PSTemplate") {
            Some(t) => Notify::Particle { template: t.to_string(), point: name(n, "SocketName").or_else(|| name(n, "BoneName")).unwrap_or_default().to_string(), attach: flag(n, "bAttach", false) },
            None => Notify::Other,
        },
        "DisNotify_DropItem" => {
            let which = match name(n, "m_WhichItem") {
                Some("DisWhichEquippedItemToDrop_All_Attached") => 1,
                Some("DisWhichEquippedItemToDrop_ByEquipUsage") => 2,
                _ => 0,
            };
            let v = |a: &str| n.payload.get(&format!("m_DropVelocity.{a}")).copied().unwrap_or(0.0);
            Notify::DropItem(DropNotify { which, primary: name(n, "m_ByUsage") != Some("EDisEquipUsage_Secondary"), velocity: glam::Vec3::new(v("X"), v("Y"), v("Z")) })
        }
        "DishonoredNotify_SeverLimb" => {
            let Some(bone) = name(n, "m_SeverBoneName") else { return Notify::Other };
            let mode = match name(n, "m_ImpulseMode") {
                Some(e) if e.ends_with("SeverBoneX") => 1,
                Some(e) if e.ends_with("SeverBoneY") => 2,
                Some(e) if e.ends_with("SeverBoneZ") => 3,
                _ => 0,
            };
            let strength = n.payload.get("m_fImpulseStrength").copied().unwrap_or(300.0);
            Notify::SeverLimb(SeverNotify { bone: bone.to_string(), blood: flag(n, "m_bUseBlood", true), hide: flag(n, "m_bHideLimb", false), ragdoll: flag(n, "m_bGoToRagdoll", true), mode, strength })
        }
        "DishonoredNotify_Ragdoll" => Notify::Ragdoll,
        "AnimNotify_CameraEffect" => match object_name(n, "CameraLensEffect") {
            Some(c) => Notify::CameraEffect { class: c.to_string() },
            None => Notify::Other,
        },
        _ => Notify::Other,
    }
}

/// This frame's notifies, resolved, for the systems that place them on the drawn meshes.
#[derive(Resource, Default)]
pub struct AnimNotifies {
    /// (tick fired, pawn, what it asks).
    pub events: Vec<(u64, PlayerId, Notify)>,
    /// The pawn whose notifies the first-person rig placed this frame (the local player while
    /// its rig is drawn): `characters.rs` leaves its particles alone.
    pub first_person: Option<PlayerId>,
    /// Lens effect classes resolved (`dishonored_assets::characters::lens_effect_class`).
    lens: HashMap<String, Option<dishonored_ir::LensEffectDef>>,
    content: Option<Option<dishonored_assets::Content>>,
}

impl AnimNotifies {
    /// The notifies of one pawn this frame.
    pub fn of(&self, pawn: PlayerId) -> impl Iterator<Item = &Notify> {
        self.events.iter().filter(move |e| e.1 == pawn).map(|e| &e.2)
    }
}

/// Take this frame's simulation events (`SessionView::anim_events`): resolve the animation
/// notifies, post their sounds and the local player's lens effects, keep the rest for the drawn
/// meshes; the bodies the simulation removed go to their burn (`ash_bodies`).
#[allow(clippy::too_many_arguments)]
pub fn dispatch(
    mut view: ResMut<SessionView>,
    session: Res<Session>,
    time: Res<Time>,
    mut notes: ResMut<AnimNotifies>,
    mut sounds: ResMut<crate::audio::AnimSounds>,
    mut lens: Option<ResMut<crate::lens_effects::LensEffects>>,
    mut ash: Option<ResMut<crate::ash_bodies::AshBodies>>,
) {
    let events = std::mem::take(&mut view.anim_events);
    notes.events.clear();
    notes.first_person = None;
    let me = session.local_id();
    let table = session.content.sim.anim.clone();
    for (tick, e) in events {
        match e {
            SimEvent::BodyRemoved { body, ash: burnt } => {
                if let Some(a) = ash.as_mut() {
                    a.removed.insert(body, (burnt, time.elapsed_secs_f64()));
                }
            }
            SimEvent::AnimNotify { pawn, sequence, notify, .. } => {
                let Some(info) = table.info(sequence).and_then(|s| s.notifies.get(notify as usize)) else { continue };
                let n = resolve(info);
                match &n {
                    Notify::Sound { event } => sounds.post(pawn, event),
                    Notify::Footfall { foot } => sounds.foot_down(pawn, *foot),
                    Notify::CameraEffect { class } if Some(pawn) == me => {
                        let notes = &mut *notes;
                        let content = notes.content.get_or_insert_with(|| dishonored_assets::Content::locate().ok());
                        let def = match (notes.lens.get(class), content.as_ref()) {
                            (Some(d), _) => d.clone(),
                            (None, Some(c)) => {
                                let d = dishonored_assets::characters::lens_effect_class(c, class);
                                if d.is_none() {
                                    warn!("animation lens effect class {class} not found");
                                }
                                notes.lens.insert(class.clone(), d.clone());
                                d
                            }
                            (None, None) => None,
                        };
                        if let (Some(d), Some(l)) = (def, lens.as_mut()) {
                            l.local.push(d);
                        }
                    }
                    _ => {}
                }
                notes.events.push((tick, pawn, n));
            }
            _ => {}
        }
    }
}
