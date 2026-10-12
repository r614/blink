//! `ArkAnimNodeLookAt` (vtable 0x0109E198; ANIMATION.md section 9): two 3x3 grids of
//! single-frame additive sequences (`Anims[]`: nine head, then nine torso, in the order LU, LC,
//! LD, CU, CC, CD, RU, RC, RD), weighted from the aim of each part against the look-at mode's
//! angle ranges (`ArkComponentLookatConfig.m_LookAtModes`), and the blink timing of the same
//! config.
//!
//! The grid weighting is the runtime's (`dishonored_runtime::npc_anim::animate`, the
//! `LookatBlender` block): bilinear between the four cells around the normalised aim. The
//! original's weighting inside a grid was not decompiled (U-281, APPROXIMATION); the split of
//! the AI's look-at cursor into the torso and head aims (0x009176F0) is `dishonored_ai::head`.

use dishonored_ir::npc_anim::LookatMode;
use serde::{Deserialize, Serialize};

/// The nine cell weights of a grid for an aim (degrees: yaw right, pitch up) against the
/// grid's ranges (right, left, up, down; left and down negative).
pub fn grid_weights(aim: [f32; 2], range: [f32; 4]) -> [f32; 9] {
    let (ax, ay) = (aim[0], aim[1]);
    let x = if ax >= 0.0 { ax / range[0].max(1.0) } else { ax / range[1].min(-1.0) * -1.0 };
    let y = if ay >= 0.0 { ay / range[2].max(1.0) } else { ay / range[3].min(-1.0) * -1.0 };
    let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
    // Columns left / centre / right, rows up / centre / down; bilinear between the four cells
    // around (x, y).
    let col = [(-x).max(0.0), 1.0 - x.abs(), x.max(0.0)];
    let row = [y.max(0.0), 1.0 - y.abs(), (-y).max(0.0)];
    let mut w = [0.0; 9];
    for c in 0..3 {
        for r in 0..3 {
            w[c * 3 + r] = col[c] * row[r];
        }
    }
    w
}

/// The eighteen weights of the node (head grid, then torso grid) for a mode and the two aims.
pub fn node_weights(mode: &LookatMode, head: [f32; 2], torso: [f32; 2]) -> [f32; 18] {
    let mut w = [0.0; 18];
    w[..9].copy_from_slice(&grid_weights(head, mode.head_range));
    w[9..].copy_from_slice(&grid_weights(torso, mode.torso_range));
    w
}

/// The blink roll of the look-at component: a blink every `[MinDurationBetweenBlink,
/// MaxDurationBetweenBlink]` seconds at a play rate in `[MinBlinkSpeed, MaxBlinkSpeed]`
/// (`ArkComponentLookatConfig` class defaults 0.5-6 s, 1-2). The roll (0x00952EC0 from the look-at
/// tick 0x00955B40): the countdown (component +0x1D0) falls by `dt` every enabled tick, blinking
/// or not; below 0 it is rolled again in the interval and the `Blink` sequence restarts at a rate
/// rolled in its range (interval first). The countdown is never initialised natively, so the
/// first enabled tick blinks (LIKELY). The original rolls the CRT's global `rand()`; the port rolls
/// a seeded xorshift per pawn, so a run is deterministic but not the original's sequence of
/// blinks. Part of the instance's `tree::TreeState` (the countdown and the generator's state
/// travel with it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlinkTimer {
    pub interval: [f32; 2],
    pub rate: [f32; 2],
    left: f32,
    rng: u32,
}

impl BlinkTimer {
    pub fn new(interval: [f32; 2], rate: [f32; 2], seed: u32) -> Self {
        BlinkTimer { interval, rate, left: 0.0, rng: seed.wrapping_mul(2654435761) | 1 }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    fn roll(&mut self, range: [f32; 2]) -> f32 {
        range[0] + self.rand() * (range[1] - range[0])
    }

    /// Count down (blinking or not, 0x00952EC0); returns the play rate of a blink that starts
    /// this tick.
    pub fn tick(&mut self, dt: f32, _blinking: bool) -> Option<f32> {
        self.left -= dt;
        if self.left >= 0.0 {
            return None;
        }
        self.left = self.roll(self.interval);
        Some(self.roll(self.rate))
    }
}
