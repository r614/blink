//! The root motion of the player's paired moves (assassinations, synced short fatalities and
//! finishers): their masters' states use `m_RootMotionMode` `RMM_Translate` with
//! `RMRM_RotateActor` (`DisTweaks_PlayerPawn_FullBodyAnimation`), so the master clip's root bone
//! translation moves the pawn through its walking physics, and the clip's root stays on the pawn
//! (`GameplayConfig::root_motion`). The victim's slave chain (`RMM_Accel`) plays where the victim
//! stood, its root's travel part of the choreography, turned to the move's relation (the linked
//! action's `m_bDoNotRotateSlave` is off; the presentation's `viewmodel::BodyFrames`). No
//! placement of either pawn was found (U-969).
//!
//! The motion runs along the master's yaw at the move's start (`SwordState::root_yaw`), from the
//! clip time it last reached (`SwordState::root_t`) to the sword timer.

use crate::fatality::FATALITY_MOVE;
use crate::movement::{MoveCtx, forward_vector, right_vector};
use crate::state::{PawnKind, PlayerState, SwordPhase, WorldState};
use dishonored_ir::gameplay::{ASSASSINATION_MASTERS, GameplayConfig, RootMotion};
use dishonored_world::WorldQuery;

/// The master clip of the paired move `p` plays, if it moves the pawn.
pub fn master<'a>(p: &PlayerState, g: &'a GameplayConfig) -> Option<&'a RootMotion> {
    if p.sword.phase != SwordPhase::Assassinating {
        return None;
    }
    let name = match p.sword.assassination_move {
        m if (m as usize) < ASSASSINATION_MASTERS.len() => ASSASSINATION_MASTERS[m as usize],
        FATALITY_MOVE => {
            let run = p.sword.fatality?;
            let clip = g.fatality.moves.get(run.action as usize)?.get(run.variant as usize)?;
            if clip.upper_body {
                return None;
            }
            clip.master.as_str()
        }
        _ => return None,
    };
    g.root_motion.get(name)
}

/// Move every pawn in a paired move by its master's root motion since the last tick.
pub fn step(state: &mut WorldState, world: &dyn WorldQuery, g: &GameplayConfig) {
    for p in state.players.iter_mut() {
        if p.kind != PawnKind::Human || !p.alive {
            continue;
        }
        let Some(rm) = master(p, g) else { continue };
        let (t0, t1) = (p.sword.root_t, p.sword.timer);
        if t1 <= t0 {
            continue;
        }
        let (a, b) = (rm.at(t0), rm.at(t1));
        let delta = forward_vector(p.sword.root_yaw) * (b[0] - a[0]) + right_vector(p.sword.root_yaw) * (b[1] - a[1]);
        let mvc = crate::powers::effective_movement(p, g);
        let ctx = MoveCtx { world, cfg: &mvc, dt: crate::DT * p.time_scale.max(1e-3) };
        crate::movement::walk_move(p, delta, &ctx);
        p.sword.root_t = t1;
    }
}
