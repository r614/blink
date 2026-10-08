//! What the special NPC classes do to other pawns (research/dishonored/systems/ai.md F6). The
//! brains (`dishonored_ai::special`) decide; the item context in use rides in
//! `PlayerInput::npc_move` and a Secondary press marks the context's effect tick:
//!
//! - **Combat tune** (`DisItemContext_NPCTune_Combat`, its player component `DisCpntType_TuneCombat`):
//!   a pawn in the music box's range `[m_fMinContextRange, m_fMaxContextRange)` within
//!   `m_fAllowedAngle` of the Overseer's facing, with a clear trace (0x00c6bfc0), is pushed away by
//!   `lerp(m_fRepulseForceRange, 1 - d / max)` per second (0x00c75270) and, at each effect tick
//!   (`m_fMusicEffectPeriod`), takes `period x m_iMusicEffectDamage` Musical damage
//!   (0x00c66ef0), reduced by `Attribute_MusicDamageReduction` (0x00b5ccd0).
//! - **Powers inhibited** (0x00bd7390): within the protection tune's
//!   `m_fMusicEffectRadius_InhibitPowers` of an Overseer playing a tune, powers cannot be used.
//! - **Plague cloud** (0x00c86ea0): at each tick of a weeper's cough, every pawn but plague
//!   carriers within `m_fPlagueRadius` (3D), in the front half-space (`m_fPlagueDmgAngle` 90) with a
//!   clear trace takes `m_iPlagueDmgPerTick`.
//! - **Tallboy arrow** (`DisItemContext_NPCFireBow`): launched along the brain's aim at
//!   `m_fProjectileSpeed`; it explodes at its first impact (0x00c550d0).

use crate::events::{DamageCause, SimEvent};
use crate::input::{LeftHand, PlayerInput, buttons, npc_moves};
use crate::movement::{eye_position, forward_vector, view_vector};
use crate::state::{PawnKind, PlayerId, PlayerState, ProjectileKind, WorldState};
use crate::vitals::apply_damage;
use dishonored_ir::ai::MusicalOverseerConfig;
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;

/// The left-hand items that are powers (`DisItemContext_UsePower`).
pub fn is_power(item: LeftHand) -> bool {
    matches!(item, LeftHand::Blink | LeftHand::Windblast | LeftHand::DarkVision | LeftHand::Possession | LeftHand::BendTime | LeftHand::DevouringSwarm)
}

fn acting(p: &PlayerState) -> bool {
    p.kind != PawnKind::Human && p.alive && !p.incapacitated()
}

/// 0x00bd7390: a music-box Overseer playing a tune has `p` within the protection tune's
/// inhibit-powers radius.
pub fn powers_inhibited(state: &WorldState, ins: &[PlayerInput], p: &PlayerState, g: &GameplayConfig) -> bool {
    let r = g.ai.special.overseer.inhibit_powers_radius;
    r > 0.0 && state.players.iter().zip(ins).any(|(o, i)| o.id != p.id && acting(o) && i.npc_move == npc_moves::OVERSEER_TUNE && o.position.distance(p.position) <= r)
}

/// The combat tune's target test (0x00c6bfc0 / 0x00c66dd0): range from the Overseer, facing cone
/// (2D), and a clear trace to the pawn. Returns the distance.
fn tune_reaches(o: &PlayerState, v: &PlayerState, t: &MusicalOverseerConfig, world: &dyn WorldQuery, g: &GameplayConfig) -> Option<f32> {
    let from = eye_position(o, &g.movement);
    let d = from.distance(v.position);
    if d < t.tune.min_range || d >= t.tune.max_range {
        return None;
    }
    let to = (v.position - o.position).truncate().normalize_or_zero();
    if forward_vector(o.yaw).truncate().dot(to) <= t.tune.angle.to_radians().cos() {
        return None;
    }
    let seg = v.position - from;
    let len = seg.length();
    if world.raycast(from, seg, len, channel::PROJECTILE).is_some_and(|h| h.distance < len - g.movement.collision_radius) {
        return None;
    }
    Some(d)
}

/// The special contexts' effects this tick (after every pawn moved). Tallboy arrows are pushed to
/// `spawns` as (owner, kind, position, velocity).
pub fn step(state: &mut WorldState, ins: &[PlayerInput], world: &dyn WorldQuery, g: &GameplayConfig, dt: f32, spawns: &mut Vec<(PlayerId, ProjectileKind, Vec3, Vec3)>, events: &mut Vec<SimEvent>) {
    let sp = &g.ai.special;
    for j in 0..state.players.len().min(ins.len()) {
        let o = state.players[j];
        let input = ins[j];
        if !acting(&o) {
            continue;
        }
        let pulse = input.buttons & buttons::SECONDARY != 0;
        match input.npc_move {
            npc_moves::OVERSEER_TUNE => {
                let t = &sp.overseer;
                for vi in 0..state.players.len() {
                    let v = state.players[vi];
                    if v.kind != PawnKind::Human || !v.alive || crate::powers::is_hidden(&v) {
                        continue;
                    }
                    let Some(d) = tune_reaches(&o, &v, t, world, g) else { continue };
                    // Repulse (0x00c75270): stronger near the Overseer.
                    let k = (1.0 - d / t.tune.max_range.max(1.0)).clamp(0.0, 1.0);
                    let force = t.repulse[0] + (t.repulse[1] - t.repulse[0]) * k;
                    let away = (v.position - o.position).truncate().normalize_or_zero();
                    let vm = &mut state.players[vi];
                    vm.velocity += Vec3::new(away.x, away.y, 0.0) * force * dt;
                    if pulse {
                        let reduction = crate::powers::attribute(vm, g, "Attribute_MusicDamageReduction", t.player_music_reduction);
                        let dmg = (t.tune_period * t.tune_damage * (1.0 - reduction)).round();
                        if dmg > 0.0 {
                            apply_damage(vm, dmg, Some(o.id), DamageCause::Musical, events, &g.vitals);
                        }
                    }
                }
            }
            npc_moves::WEEPER_COUGH if pulse => {
                let w = &sp.weeper;
                let fwd = forward_vector(o.yaw).truncate();
                let from = eye_position(&o, &g.movement);
                for vi in 0..state.players.len() {
                    let v = state.players[vi];
                    if v.id == o.id || !v.alive || v.plague || crate::powers::is_hidden(&v) || v.carried_by.is_some() {
                        continue;
                    }
                    if v.position.distance(o.position) > w.plague_radius {
                        continue;
                    }
                    let to = (v.position - o.position).truncate().normalize_or_zero();
                    if fwd.dot(to) <= w.plague_angle.to_radians().cos() {
                        continue;
                    }
                    let seg = v.position - from;
                    let len = seg.length();
                    if world.raycast(from, seg, len, channel::PROJECTILE).is_some_and(|h| h.distance < len - g.movement.collision_radius) {
                        continue;
                    }
                    apply_damage(&mut state.players[vi], w.plague_damage, Some(o.id), DamageCause::Plague, events, &g.vitals);
                }
            }
            npc_moves::TALLBOY_FIRE_BOW if pulse => {
                let tb = &sp.tallboy;
                let dir = view_vector(input.yaw, input.pitch);
                spawns.push((o.id, ProjectileKind::TallboyArrow, eye_position(&o, &g.movement), dir * tb.arrow_speed));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::ai::SpecialAttack;
    use dishonored_world::{CollisionWorld, Tri};

    fn cfg() -> GameplayConfig {
        let mut g = crate::SimConfig::from_json(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json")).unwrap()).unwrap().gameplay;
        // `Twk_Weapon_MusicBox_Default` / `DisTweaks_Tune_Protection_0` / `Pwn_WeeperMiddle_MTall_1`.
        let o = &mut g.ai.special.overseer;
        o.tune = SpecialAttack { kind: npc_moves::OVERSEER_TUNE, min_range: 150.0, max_range: 2000.0, angle: 45.0, ..Default::default() };
        o.tune_period = 0.3;
        o.tune_damage = 90.0;
        o.repulse = [1000.0, 3000.0];
        o.inhibit_powers_radius = 1500.0;
        o.player_music_reduction = 0.93;
        g
    }

    fn floor() -> CollisionWorld {
        let s = 20000.0;
        let (a, b, c, d) = (Vec3::new(-s, -s, 0.0), Vec3::new(s, -s, 0.0), Vec3::new(s, s, 0.0), Vec3::new(-s, s, 0.0));
        let tri = |x: Vec3, y: Vec3, z: Vec3| Tri { a: x, b: y, c: z, n: Vec3::Z, channels: channel::ALL, source: 0 };
        CollisionWorld::from_triangles(vec![tri(a, b, c), tri(a, c, d)], vec!["floor".into()])
    }

    fn world(npc: PawnKind, at: f32) -> WorldState {
        let mut ws = WorldState::default();
        let mut o = PlayerState::new(1000, Vec3::new(0.0, 0.0, 90.0), 0.0);
        o.kind = npc;
        let mut pl = PlayerState::new(1, Vec3::new(at, 0.0, 90.0), 180.0);
        pl.vitals.health = 100.0;
        ws.players = vec![pl, o];
        ws
    }

    fn damage(ev: &[SimEvent], cause: DamageCause) -> Vec<f32> {
        ev.iter().filter_map(|e| match e {
            SimEvent::Damage { amount, cause: c, .. } if *c == cause => Some(*amount),
            _ => None,
        }).collect()
    }

    /// The combat tune's effect tick: round(0.3 x 90 x (1 - 0.93)) = 2 Musical damage within the
    /// cone and range (0x00c66ef0, 0x00b5ccd0), a push away from the Overseer, nothing beyond
    /// 2000 uu; powers inhibited within 1500 uu while it plays (0x00bd7390).
    #[test]
    fn combat_tune_damages_pushes_and_inhibits() {
        let g = cfg();
        let w = floor();
        let tune = PlayerInput { npc_move: npc_moves::OVERSEER_TUNE, buttons: buttons::SECONDARY, ..Default::default() };
        let ins = [PlayerInput::default(), tune];
        let mut ws = world(PawnKind::Guard, 1000.0);
        let mut ev = Vec::new();
        step(&mut ws, &ins, &w, &g, 1.0 / 60.0, &mut Vec::new(), &mut ev);
        assert_eq!(damage(&ev, DamageCause::Musical), vec![2.0]);
        assert!(ws.players[0].velocity.x > 0.0, "pushed away");
        assert!(powers_inhibited(&ws, &ins, &ws.players[0], &g));
        // Beyond the inhibit radius, and beyond the tune's range.
        let far = world(PawnKind::Guard, 1600.0);
        assert!(!powers_inhibited(&far, &ins, &far.players[0], &g));
        let mut out = world(PawnKind::Guard, 2100.0);
        let mut ev = Vec::new();
        step(&mut out, &ins, &w, &g, 1.0 / 60.0, &mut Vec::new(), &mut ev);
        assert!(damage(&ev, DamageCause::Musical).is_empty());
        // Not playing: no inhibition.
        assert!(!powers_inhibited(&ws, &[PlayerInput::default(), PlayerInput::default()], &ws.players[0], &g));
    }

    /// A weeper's plague tick: `m_iPlagueDmgPerTick` within the radius in front, not to another
    /// plague carrier, not behind (0x00c86ea0).
    #[test]
    fn plague_cloud_spares_carriers_and_the_back() {
        let g = cfg();
        let w = floor();
        let cough = PlayerInput { npc_move: npc_moves::WEEPER_COUGH, buttons: buttons::SECONDARY, ..Default::default() };
        let mut ws = world(PawnKind::Guard, 300.0);
        let mut other = PlayerState::new(1001, Vec3::new(200.0, 50.0, 90.0), 0.0);
        other.kind = PawnKind::Guard;
        other.plague = true;
        ws.players.push(other);
        let mut ev = Vec::new();
        step(&mut ws, &[PlayerInput::default(), cough, PlayerInput::default()], &w, &g, 1.0 / 60.0, &mut Vec::new(), &mut ev);
        assert_eq!(damage(&ev, DamageCause::Plague), vec![g.ai.special.weeper.plague_damage]);
        let mut behind = world(PawnKind::Guard, -300.0);
        let mut ev = Vec::new();
        step(&mut behind, &[PlayerInput::default(), cough], &w, &g, 1.0 / 60.0, &mut Vec::new(), &mut ev);
        assert!(damage(&ev, DamageCause::Plague).is_empty());
    }
}
