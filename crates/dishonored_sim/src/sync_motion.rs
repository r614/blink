//! The root motion of the player's paired moves (assassinations, synced short fatalities and
//! finishers): their masters' states use `m_RootMotionMode` `RMM_Translate` with
//! `RMRM_RotateActor` (`DisTweaks_PlayerPawn_FullBodyAnimation`), so the master clip's root bone
//! translation moves the pawn through its walking physics, and the clip's root stays on the pawn
//! (`GameplayConfig::root_motion`). The victim's slave chain (`RMM_Accel`) plays where the victim
//! stood, its root's travel part of the choreography, turned to the move's relation (the linked
//! action's `m_bDoNotRotateSlave` is off; the presentation's `viewmodel::BodyFrames`). No
//! placement of either pawn was found (U-969).
//!
//! The motion is the master's playhead on the pawn's full-body layer (`crate::anim::root_delta`:
//! the root-motion keys between the tick's step start and end, in the frame of the pawn's yaw at
//! the move's start), applied at the pose barrier: after every pawn's physics, states and
//! animation advance, before the melee sweep (tick_order.md section 5: `RMM_Translate` moves the
//! owner at once, `0x00746280`).

use crate::SimConfig;
use crate::movement::MoveCtx;
use crate::state::{PawnKind, WorldState};
use dishonored_world::WorldQuery;

/// Move every pawn in a paired move by its master's root motion of this tick's advance.
pub fn step(state: &mut WorldState, world: &dyn WorldQuery, cfg: &SimConfig) {
    for i in 0..state.players.len() {
        step_one(state, i, world, cfg);
    }
}

/// [`step`] for one pawn.
pub fn step_one(state: &mut WorldState, i: usize, world: &dyn WorldQuery, cfg: &SimConfig) {
    let p = state.players[i];
    if p.kind != PawnKind::Human || !p.alive {
        return;
    }
    let Some(delta) = state.anim.get(&p.id).and_then(|a| crate::anim::root_delta(a, &p, cfg)) else { return };
    let mvc = crate::powers::effective_movement(&p, &cfg.gameplay);
    let ctx = MoveCtx { world, cfg: &mvc, dt: crate::DT * p.time_scale.max(1e-3) };
    crate::movement::walk_move(&mut state.players[i], delta, &ctx);
}
