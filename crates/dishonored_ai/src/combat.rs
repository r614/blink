//! Elite guard combat moves (`DisBehaviorCombatEliteGuard`, `DisAISubProcessManageAttacks`,
//! `DisAISubStateFirePistol`). See `research/dishonored/AI.md` ("Combat moves").

use dishonored_ir::ai::{MeleeContextConfig, PistolConfig};
use dishonored_sim::input::npc_moves;
use dishonored_sim::state::PlayerId;

/// What an NPC brain is doing in a fight, for animation (`PlayerInput::npc_move`).
pub type AttackKind = u8;

/// First context of the attack pattern that may be used now: enabled in the pattern (listed),
/// off its own cooldown (`m_ContextCooldown`), not inhibited on this target by another NPC's
/// recent use of the same attack class (`DisGlobalCombatManager.m_fInhibitionTime*`), with the
/// target inside its item-context range (`m_fMinContextRange` < d <= `m_fMaxContextRange`) and
/// height difference (`m_fMaxContextHeightDifference`).
pub fn pick_context(ctxs: &[MeleeContextConfig], cooldowns: &[f32], inhibit: &[(PlayerId, u8, f32)], target: PlayerId, dist: f32, dz: f32) -> Option<usize> {
    (0..ctxs.len()).find(|&k| {
        let c = &ctxs[k];
        cooldowns.get(k).is_none_or(|t| *t <= 0.0)
            && !inhibit.iter().any(|x| x.0 == target && x.1 as usize == k)
            && dist > c.min_range
            && dist <= c.max_range
            && dz.abs() <= c.max_height
    })
}

/// Does the attack close distance as part of its animation (root motion)? The medium, long and
/// jump attacks start beyond the sword's reach and travel to the target.
pub fn lunges(kind: AttackKind) -> bool {
    matches!(kind, npc_moves::ATTACK_MEDIUM | npc_moves::ATTACK_LONG | npc_moves::JUMP_ATTACK | npc_moves::ATTACK_SHORT)
}

/// Pistol use decision of the elite combat refresh (0x00b0e220), returned as
/// (willing to start shooting, willing to continue). `gun_timer` is `m_fGunTimer`; `still` is
/// the NPC standing still (its velocity below 0.0001 on every axis); `reachable` is whether the
/// target can be reached for melee. An unreachable target is shot at as soon as it is within
/// the gun's maximum range with a clear line, without waiting for the timer.
pub fn pistol_willing(p: &PistolConfig, dist: f32, line_clear: bool, gun_timer: f32, still: bool, reachable: bool, outer_ring: bool) -> (bool, bool) {
    if !reachable {
        let w = line_clear && dist < p.max_range;
        return (w, w);
    }
    if !outer_ring {
        return (false, false);
    }
    let start = line_clear && gun_timer < 0.0 && still && dist > p.min_range && dist < p.max_range;
    let cont = line_clear && dist < p.max_range;
    (start, cont)
}

pub fn roll(lo_hi: [f32; 2], r: f32) -> f32 {
    lo_hi[0] + (lo_hi[1] - lo_hi[0]) * r
}

pub fn roll_i(lo_hi: [i32; 2], r: f32) -> i32 {
    let span = (lo_hi[1] - lo_hi[0]).max(0) + 1;
    lo_hi[0] + ((r * span as f32) as i32).min(span - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::ai::default_melee_contexts;

    #[test]
    fn contexts_follow_pattern_order_ranges_and_inhibition() {
        let c = default_melee_contexts();
        let cd = vec![0.0; c.len()];
        // Close: the short attack comes first in the pattern.
        assert_eq!(pick_context(&c, &cd, &[], 1, 90.0, 0.0), Some(0));
        // 300 uu: beyond short (235), medium (235-375) fits.
        assert_eq!(c[pick_context(&c, &cd, &[], 1, 300.0, 0.0).unwrap()].kind, npc_moves::ATTACK_MEDIUM);
        // 420 uu: the long attack (375-570).
        assert_eq!(c[pick_context(&c, &cd, &[], 1, 420.0, 0.0).unwrap()].kind, npc_moves::ATTACK_LONG);
        // 780 uu: the jump attack (730-850), but not with a 150 uu height difference.
        assert_eq!(c[pick_context(&c, &cd, &[], 1, 780.0, 0.0).unwrap()].kind, npc_moves::JUMP_ATTACK);
        assert_eq!(pick_context(&c, &cd, &[], 1, 780.0, 150.0), None);
        // 650 uu: between long and jump: nothing.
        assert_eq!(pick_context(&c, &cd, &[], 1, 650.0, 0.0), None);
        // Short on cooldown, step back next; short inhibited on this target only.
        let mut cd2 = cd.clone();
        cd2[0] = 1.0;
        assert_eq!(pick_context(&c, &cd2, &[], 1, 90.0, 0.0), Some(1));
        assert_eq!(pick_context(&c, &cd, &[(1, 0, 1.0)], 1, 90.0, 0.0), Some(1));
        assert_eq!(pick_context(&c, &cd, &[(2, 0, 1.0)], 1, 90.0, 0.0), Some(0));
    }

    #[test]
    fn pistol_start_needs_timer_stillness_and_range() {
        let p = PistolConfig { min_range: 300.0, max_range: 1700.0, ..Default::default() };
        assert_eq!(pistol_willing(&p, 800.0, true, -0.1, true, true, true), (true, true));
        assert_eq!(pistol_willing(&p, 800.0, true, 0.5, true, true, true), (false, true));
        assert_eq!(pistol_willing(&p, 800.0, true, -0.1, false, true, true), (false, true));
        assert_eq!(pistol_willing(&p, 200.0, true, -0.1, true, true, true), (false, true));
        assert_eq!(pistol_willing(&p, 1800.0, true, -0.1, true, true, true), (false, false));
        assert_eq!(pistol_willing(&p, 800.0, false, -0.1, true, true, true), (false, false));
        // Unreachable: shoot at once within the max range.
        assert_eq!(pistol_willing(&p, 200.0, true, 5.0, false, false, false), (true, true));
    }

    #[test]
    fn integer_rolls_cover_the_range() {
        assert_eq!(roll_i([2, 3], 0.0), 2);
        assert_eq!(roll_i([2, 3], 0.99), 3);
        assert_eq!(roll_i([4, 4], 0.7), 4);
    }
}
