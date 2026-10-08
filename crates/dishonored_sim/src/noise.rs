//! AI noises the player and the world make (`DisAINoiseInfo`, dispatched by 0x00C5A760). The
//! radii and contexts come from the data (`AiConfig::noise`, research/dishonored/systems/
//! stealth.md section 5): the contact system's `DisContactSub_AINoise` objects for footfalls,
//! landings, sword contacts, bullet impacts and bodies hitting the floor, and the pistol fire
//! tweak. Nothing here is heard yet: the AI listeners take the `SimEvent::Sound`s of a step.

use crate::events::{SimEvent, SoundCategory};
use crate::movement::half_height;
use crate::state::{MoveMode, PawnKind, PlayerId, PlayerState};
use dishonored_ir::ai::{AiNoise, FootfallClip};
use dishonored_ir::gameplay::GameplayConfig;
use glam::Vec3;

/// Push the noise `n` of a contact at `speed` (nothing for a Silent noise or below the
/// noise's minimum speed: 0x00BD6C20).
pub fn emit(events: &mut Vec<SimEvent>, n: &AiNoise, position: Vec3, category: SoundCategory, source: PlayerId, speed: f32) {
    if n.emits(speed) {
        events.push(SimEvent::Sound { position, radius: n.radius, category, context: n.context, source });
    }
}

/// The footfall striker's noise of the player (`DishonoredPlayerPawn` vslot 398, 0x00AAB7D0):
/// not sprinting (or crouched): sneaking -> `Footfall_Sneak_Player`, `Attribute_SilentRunning`
/// -> `Footfall_Run_Player_Silent`, else the body tweak's `Footfall_Run_Player`; sprinting:
/// `Attribute_SilentSprinting` -> silent, else `Footfall_Sprint_Player`.
pub fn footfall_noise(p: &PlayerState, cfg: &GameplayConfig) -> (AiNoise, SoundCategory) {
    let t = &cfg.ai.noise;
    let attr = |name: &str| crate::powers::attribute(p, cfg, name, 0.0) != 0.0;
    if !p.sprinting || p.crouched {
        if p.crouched {
            (t.footfall_sneak, SoundCategory::Footstep)
        } else if attr("Attribute_SilentRunning") {
            (t.footfall_silent, SoundCategory::Footstep)
        } else {
            (t.footfall_run, SoundCategory::Footstep)
        }
    } else if attr("Attribute_SilentSprinting") {
        (t.footfall_silent, SoundCategory::Sprint)
    } else {
        (t.footfall_sprint, SoundCategory::Sprint)
    }
}

/// The locomotion loop the player's footfalls come from: `Sword_Ready_Sprint` while sprinting,
/// `_Walk` while walking (`Pawn.bIsWalking`), else `_Run`. Sneaking plays the sneak loops,
/// whose footfalls are silent (`Footfall_Sneak_Player`), so it has none here.
fn locomotion_clip<'a>(p: &PlayerState, cfg: &'a GameplayConfig) -> Option<&'a FootfallClip> {
    let t = &cfg.ai.noise;
    if p.crouched {
        None
    } else if p.sprinting {
        Some(&t.sprint_clip)
    } else if p.walking {
        Some(&t.walk_clip)
    } else {
        Some(&t.run_clip)
    }
}

/// The player's footfalls this tick: the locomotion loop advances while the pawn moves on the
/// ground (its phase is kept across loops, as a sync group keeps the relative position), and
/// each `DishonoredNotify_Footfall` it passes is a footfall contact under the pawn
/// (0x00A8D5F0 -> 0x00B66ED0 -> 0x00B66A80) at the pawn's speed, with the striker's noise.
/// NPC footfalls are Silent in the table, so only players run this.
///
/// The loop plays at its own rate (`RateScale` 1); the locomotion tree's speed scaling of the
/// rate was not decoded (stealth.md F1).
pub fn step_footfalls(p: &mut PlayerState, cfg: &GameplayConfig, dt: f32, events: &mut Vec<SimEvent>) {
    if p.kind != PawnKind::Human || !p.alive || p.mode != MoveMode::Walking || p.slide.active {
        return;
    }
    let speed2d = p.velocity.truncate().length();
    let Some(clip) = locomotion_clip(p, cfg) else { return };
    if speed2d <= 0.0 || clip.length <= 0.0 {
        return;
    }
    let from = p.footfall_phase;
    let to = from + dt / clip.length;
    p.footfall_phase = to.fract();
    let passed = clip.footfalls.iter().filter(|&&f| {
        let f = f / clip.length;
        (f > from && f <= to) || (to >= 1.0 && f + 1.0 > from && f + 1.0 <= to)
    });
    let n = passed.count();
    if n == 0 {
        return;
    }
    let (noise, category) = footfall_noise(p, cfg);
    let feet = p.position - Vec3::Z * half_height(p, &cfg.movement);
    for _ in 0..n {
        emit(events, &noise, feet, category, p.id, p.velocity.length());
    }
}

/// The landing contact of the player (0x00AB53A0): faster than `m_fFall_Stun_Vel` it is the
/// stun landing (`JumpLand_Big`), else `JumpLand_Stealth` for a fall begun sneaking
/// (`m_bIsStealthFall`) or `JumpLand`; the contact is under the pawn at its velocity.
pub fn landing(p: &PlayerState, impact_speed: f32, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    if p.kind != PawnKind::Human {
        return;
    }
    let t = &cfg.ai.noise;
    let n = if impact_speed > t.fall_stun_speed {
        t.land_big
    } else if p.stealth_fall {
        t.land_stealth
    } else {
        t.land
    };
    let feet = p.position - Vec3::Z * half_height(p, &cfg.movement);
    let speed = Vec3::new(p.velocity.x, p.velocity.y, -impact_speed).length();
    emit(events, &n, feet, SoundCategory::Land, p.id, speed);
}

/// Track the fall state's entry (`StatePlayerMasterFalling` enter, 0x00A759F0): falling and no
/// longer in the jump state; `m_bIsStealthFall` is whether the pawn sneaks at that moment.
pub fn track_fall(before: &PlayerState, p: &mut PlayerState) {
    let falling = |q: &PlayerState| q.mode == MoveMode::Falling && !q.jump.active;
    if falling(p) && !falling(before) {
        p.stealth_fall = p.crouched;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::ai::NoiseContext;

    fn cfg() -> GameplayConfig {
        crate::SimConfig::from_json(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json")).unwrap()).unwrap().gameplay
    }

    fn sounds(ev: &[SimEvent]) -> Vec<(f32, NoiseContext)> {
        ev.iter().filter_map(|e| if let SimEvent::Sound { radius, context, .. } = e { Some((*radius, *context)) } else { None }).collect()
    }

    /// Running for one second passes the run loop's two footfalls about once: 600 uu
    /// Footfall noises; sprinting gives 1200 uu Anomaly; sneaking none (contact table).
    #[test]
    fn footfalls_by_gait() {
        let c = cfg();
        let run = |sprint: bool, crouch: bool| {
            let mut p = PlayerState::new(1, Vec3::ZERO, 0.0);
            p.mode = MoveMode::Walking;
            p.velocity = Vec3::new(400.0, 0.0, 0.0);
            p.sprinting = sprint;
            p.crouched = crouch;
            let mut ev = Vec::new();
            for _ in 0..60 {
                step_footfalls(&mut p, &c, 1.0 / 60.0, &mut ev);
            }
            sounds(&ev)
        };
        let r = run(false, false);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|s| *s == (c.ai.noise.footfall_run.radius, c.ai.noise.footfall_run.context)));
        assert_eq!(c.ai.noise.footfall_run.radius, 600.0);
        let s = run(true, false);
        assert!(s.len() >= 3);
        assert!(s.iter().all(|x| *x == (1200.0, NoiseContext::Anomaly)));
        assert!(run(false, true).is_empty());
    }

    /// Landings: 400 uu, silent after a sneaking fall, the stun landing 800 uu.
    #[test]
    fn landings() {
        let c = cfg();
        let mut p = PlayerState::new(1, Vec3::ZERO, 0.0);
        let mut ev = Vec::new();
        landing(&p, 500.0, &c, &mut ev);
        p.stealth_fall = true;
        landing(&p, 500.0, &c, &mut ev);
        landing(&p, c.ai.noise.fall_stun_speed + 1.0, &c, &mut ev);
        assert_eq!(sounds(&ev), vec![(400.0, NoiseContext::Anomaly), (800.0, NoiseContext::Anomaly)]);
    }
}
