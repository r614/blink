//! Pain-causing physics volumes (Engine `PhysicsVolume`): a `VolumeTimer` spawned at level
//! start pops every `PainInterval` (default 1 s) and calls `CausePainTo` on every damageable
//! actor touching the volume: `TakeDamage(int(DamagePerSec x PainInterval), DamageType)`.
//! With `bEntryPain` (default true) a pawn entering the volume takes the same pain at once
//! (UE3 `AActor::SetZone`). The zone velocity is applied by the movement (`movement.rs`).

use crate::SimConfig;
use crate::events::{DamageCause, SimEvent};
use crate::movement::{DT, capsule};
use crate::state::WorldState;
use crate::vitals;
use dishonored_world::WorldQuery;

/// The sim's cause for a volume's `DamageType` class.
fn cause(damage_type: &str) -> DamageCause {
    if damage_type.eq_ignore_ascii_case("DisDamageType_SpringRazor") { DamageCause::SpringRazor } else { DamageCause::World }
}

pub fn step_pain(state: &mut WorldState, world: &dyn WorldQuery, cfg: &SimConfig, events: &mut Vec<SimEvent>) {
    let volumes = world.physics_volumes();
    if !volumes.iter().any(|v| v.pain.is_some()) {
        return;
    }
    // Level time before and after this step (the timers start with the level).
    let (t0, t1) = (state.tick as f64 * DT as f64, (state.tick + 1) as f64 * DT as f64);
    let mv = &cfg.gameplay.movement;
    for p in state.players.iter_mut() {
        if !p.alive {
            p.pain_volumes = 0;
            continue;
        }
        let c = capsule(p, mv);
        let mut touching = 0u64;
        for (i, v) in volumes.iter().enumerate().take(64) {
            let Some(pain) = &v.pain else { continue };
            if !v.touches(c.center, c.radius, c.half_height) {
                continue;
            }
            touching |= 1 << i;
            let interval = pain.interval.max(1e-3) as f64;
            let popped = (t1 / interval).floor() > (t0 / interval).floor();
            let entered = pain.entry && p.pain_volumes & (1 << i) == 0;
            if (popped || entered) && pain.damage_per_sec > 0.0 {
                let amount = (pain.damage_per_sec * pain.interval).trunc();
                vitals::apply_damage(p, amount, None, cause(&pain.damage_type), events, &cfg.gameplay.vitals);
                if !p.alive {
                    break;
                }
            }
        }
        p.pain_volumes = touching;
    }
}
