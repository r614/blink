//! An NPC's teleport spell (`DisItemContext_NPCTeleportSpell`, the whalers' and Daud's hands;
//! research/dishonored/systems/teleport_spell.md), decoded from `Dishonored.exe`.
//!
//! - **Start** (0x00C30B50): the reappearance timer is the custom delay when set (the Kismet's
//!   `m_fReappearanceDelay` >= 0), else a roll in the tweak's `m_TeleportDelay`. The destination is
//!   the given location (`EDisTeleportIntention_Exact` with a non-pawn target, the Kismet's
//!   `DisSeqAct_NPCDoTeleportSpell`). Then the disappearance starts (state Disappearing, the fade
//!   timer from `m_fDisappearanceFadeOutTime`).
//! - **Tick** (vslot 77, 0x00C350F0):
//!   - Disappearing: the fade timer runs down; at 0 the pawn is fully hidden (its attached
//!     actors hidden, 0x00C2BC80) and the state is FullyHidden.
//!   - FullyHidden: the reappearance timer runs down; below 0 the pawn moves to the destination
//!     (0x00C30E10: `FindSpot` there, its success the spell's `m_bTeleportSuccess`; an exact
//!     teleport takes the destination actor's rotation) and the state is Tesseract.
//!   - Tesseract: the reappearance starts (0x00C2BD80: the effects, the fade timer from
//!     `m_fReappearanceFadeInTime`), state Reappearing.
//!   - Reappearing: the fade timer runs down; at 0 the context ends (0x00C12930 -> 0x00C39B80 ->
//!     0x00C35280: collision restored, the pawn faces its new rotation, `DisSeqEvent_TeleportSpell`
//!     output 1) and reports Completed when the teleport succeeded, else Interrupted.
//! - **Abort** (the Kismet's Abort input, 0x00B5DD40 -> 0x00B4ACF0(2)): Interrupted; the context
//!   ends running its remaining steps at once (0x00C39B80 falls through the states).
//!
//! The pawn is hidden (not seen, not hit) while FullyHidden and Tesseract, and does nothing else
//! while the spell runs. Not ported: the spot search's collision fit (the destination is taken as
//! it is: the shipped destinations are placed target points), the combat and escape intentions
//! (the AI's use of the spell), the fade's material parameter and the effects.

use crate::state::PlayerState;
use dishonored_ir::ai::TeleportSpellConfig;
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// `EDisTeleportSpellState`.
pub mod stage {
    pub const NONE: u8 = 0;
    pub const DISAPPEARING: u8 = 1;
    pub const FULLY_HIDDEN: u8 = 2;
    pub const TESSERACT: u8 = 3;
    pub const REAPPEARING: u8 = 4;
    pub const DONE: u8 = 5;
}

/// The spell's runtime state (`m_TeleportSpellState`, `m_fFadeTimer`, `m_fReappearanceTimer`,
/// `m_TargetLocation`, `m_bTeleportSuccess`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpellState {
    pub stage: u8,
    pub fade: f32,
    pub reappear: f32,
    pub dest: Vec3,
    /// The yaw the pawn takes at the destination (an exact teleport's destination rotation).
    pub yaw: Option<f32>,
    pub success: bool,
    /// The tweak's fade times (`m_fDisappearanceFadeOutTime`, `m_fReappearanceFadeInTime`).
    pub fade_out: f32,
    pub fade_in: f32,
    /// Done: 1 Completed, 2 Interrupted (`EDisSeqAct_NPCDoTeleportSpell` outputs), for the driver
    /// to read and clear.
    pub result: u8,
}

/// Start an exact teleport to `dest` (0x00B5DD40 -> 0x00C1C4E0 -> 0x00C30B50). `delay` is the
/// custom delay (negative: roll the tweak's).
pub fn start_exact(p: &mut PlayerState, cfg: &TeleportSpellConfig, dest: Vec3, yaw: Option<f32>, delay: f32) {
    let reappear = if delay >= 0.0 { delay } else { cfg.delay[0] + p.roll() * (cfg.delay[1] - cfg.delay[0]) };
    p.spell = SpellState { stage: stage::DISAPPEARING, fade: cfg.fade_out, reappear, dest, yaw, success: false, fade_out: cfg.fade_out, fade_in: cfg.fade_in, result: 0 };
    p.velocity = Vec3::ZERO;
}

/// Whether the spell runs (from the disappearance to the end of the reappearance).
pub fn active(p: &PlayerState) -> bool {
    matches!(p.spell.stage, stage::DISAPPEARING..=stage::REAPPEARING)
}

/// Hidden while fully faded out (not seen, not hit).
pub fn hidden(p: &PlayerState) -> bool {
    matches!(p.spell.stage, stage::FULLY_HIDDEN | stage::TESSERACT)
}

/// How visible the pawn is (the fade's material parameter): 1 out of the spell.
pub fn visibility(p: &PlayerState) -> f32 {
    let s = &p.spell;
    match s.stage {
        stage::DISAPPEARING if s.fade_out > 0.0 => (s.fade / s.fade_out).clamp(0.0, 1.0),
        stage::FULLY_HIDDEN | stage::TESSERACT => 0.0,
        stage::REAPPEARING if s.fade_in > 0.0 => 1.0 - (s.fade / s.fade_in).clamp(0.0, 1.0),
        _ => 1.0,
    }
}

/// The move to the destination (0x00C30E10).
fn arrive(p: &mut PlayerState) {
    p.position = p.spell.dest + Vec3::Z * 5.0;
    p.velocity = Vec3::ZERO;
    if let Some(y) = p.spell.yaw {
        p.yaw = y;
    }
    p.spell.success = true;
}

/// The end of the context (0x00C35280): Done with the result.
fn finish(p: &mut PlayerState) {
    p.spell.stage = stage::DONE;
    p.spell.result = if p.spell.success { 1 } else { 2 };
}

/// One tick (0x00C350F0). Returns true when the spell ended this tick.
pub fn step(p: &mut PlayerState, dt: f32) -> bool {
    let s = &mut p.spell;
    match s.stage {
        stage::DISAPPEARING => {
            s.fade -= dt;
            if s.fade <= 0.0 {
                s.stage = stage::FULLY_HIDDEN;
            }
        }
        stage::FULLY_HIDDEN => {
            s.reappear -= dt;
            if s.reappear < 0.0 {
                arrive(p);
                p.spell.stage = stage::TESSERACT;
            }
        }
        stage::TESSERACT => {
            s.fade = s.fade_in;
            s.stage = stage::REAPPEARING;
        }
        stage::REAPPEARING => {
            s.fade -= dt;
            if s.fade <= 0.0 {
                finish(p);
                return true;
            }
        }
        _ => {}
    }
    false
}

/// End the spell now (0x00C39B80, an abort or the pawn's death): the remaining steps run at
/// once (a pawn still hidden arrives), and the result is Interrupted.
pub fn abort(p: &mut PlayerState) {
    if !active(p) {
        return;
    }
    if matches!(p.spell.stage, stage::DISAPPEARING | stage::FULLY_HIDDEN) {
        arrive(p);
    }
    p.spell.stage = stage::DONE;
    p.spell.result = 2;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whaler() -> TeleportSpellConfig {
        TeleportSpellConfig { fade_out: 0.45, fade_in: 0.3, delay: [0.0, 0.0], distance: [300.0, 800.0], ..Default::default() }
    }

    #[test]
    fn an_exact_teleport_fades_out_moves_and_fades_in() {
        let mut p = PlayerState::new(1000, Vec3::ZERO, 0.0);
        p.alive = true;
        start_exact(&mut p, &whaler(), Vec3::new(500.0, 0.0, 100.0), Some(90.0), -1.0);
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        let mut hidden_seen = false;
        while !step(&mut p, dt) {
            t += dt;
            hidden_seen |= hidden(&p);
            assert!(t < 2.0, "the spell ends");
        }
        t += dt;
        assert!(hidden_seen);
        assert!((t - (0.45 + 0.3 + 2.0 * dt)).abs() < 2.5 * dt, "fade out, a tick hidden, the tesseract tick, fade in: {t}");
        assert_eq!((p.spell.stage, p.spell.result), (stage::DONE, 1));
        assert_eq!((p.position, p.yaw), (Vec3::new(500.0, 0.0, 105.0), 90.0));
        assert_eq!(visibility(&p), 1.0);
    }

    #[test]
    fn an_abort_lands_the_pawn_and_interrupts() {
        let mut p = PlayerState::new(1000, Vec3::ZERO, 0.0);
        p.alive = true;
        start_exact(&mut p, &whaler(), Vec3::new(500.0, 0.0, 0.0), None, 2.0);
        for _ in 0..40 {
            step(&mut p, 1.0 / 60.0);
        }
        assert!(hidden(&p), "the custom delay keeps it hidden");
        abort(&mut p);
        assert_eq!((p.spell.stage, p.spell.result), (stage::DONE, 2));
        assert_eq!(p.position, Vec3::new(500.0, 0.0, 5.0));
    }
}
