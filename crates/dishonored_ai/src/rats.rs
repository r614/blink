//! Rat swarm brains (`DisRatSwarm` with `DisTweaks_RatSwarm`): the swarms of the Boyle sewers
//! and the Devouring Swarm power. See `research/dishonored/AI.md` ("Rat swarms").
//!
//! Every rat pawn of the simulation (`PawnKind::Rat`, spawned with
//! `dishonored_sim::rats::spawn_rat`) gets a brain; `rats` is the number of living rats of its
//! swarm, so the swarm-level rules below hold for each member. A brain acts through
//! `PlayerInput` like every NPC: it runs to its target and holds `PRIMARY` within the bite reach,
//! and the simulation's attack groups (`dishonored_sim::rats`, 0x00C891E0) apply the delay, the
//! group damage and the slowdown. Devoured corpses are reported as [`RatEvent`]s.
//!
//! From the data (`DisTweaks_RatSwarm`) and the swarm/group state enums
//! (`EDisRatSwarmState`, `EDisRatGroupState`): a swarm detects pawns within
//! `m_fPawnDetectionRadius` (`_WhenBusy` while attacking or eating), players only after
//! `m_fPlayerDetectionDelayAfterSpawn`; it is aggressive with at least
//! `m_InitialRatCountToBeAggressive` rats (the Devouring Swarm always); an attack group needs
//! `m_MinRatCountPerAttackGroup` rats; once at the target (`m_fDestinationRadius`) it waits
//! `m_fInitialDelayBeforeDealingDamage`, then every `m_fDamageTimeInterval` deals
//! `clamp(m_fDamagePerBite * rats, m_fMinDamage, m_fMaxDamage)`; a target beyond
//! `m_fPawnEscapeRadius` or unreachable for `m_fPawnUnreachableDelay` is dropped; a corpse is
//! eaten with `m_EatRequiredRatCount` rats (`m_fEatStartupDelayAfterKill`,
//! `m_fEatStartupDuration`, `m_fEatPerLimbDuration` per limb); a non-aggressive swarm moves
//! `m_fIdleAwayFromPawnMoveDist` away from a pawn that stays within
//! `m_fIdleAwayFromPawnTriggerDist` for `m_fIdleAwayFromPawnTriggerDuration`; a swarm too small
//! to attack flees `m_fFleeAttemptMinDistance`-`MaxDistance` unless `m_bNeverFleeSystemicaly`.
//! The native swarm tick (0x00C65380) runs the first of Hidden, Flee, Attack, Move, Idle whose
//! condition holds (exit / enter on a change), then that state's update; the Attack state keeps
//! or drops its groups' targets, then chases, bites (0x00C891E0) or eats (0x00C6D760) per group
//! (research/dishonored/AI.md "Rat swarms", U-260).

use dishonored_ir::ai::RatSwarmConfig;
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::pawn_states::context as npc_moves;
use dishonored_sim::state::{PlayerId, PlayerState, WorldState};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Limbs eaten from a corpse (`DisSeveredLimbInfo` of the NPC: both lower legs and arms, the neck
/// and the chest; U-261).
pub const LIMBS: u32 = dishonored_sim::rats::CORPSE_LIMBS;

/// `EDisRatSwarmState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SwarmState {
    Hidden,
    Flee,
    Attack,
    Move,
    #[default]
    Idle,
}

/// What a swarm did this tick, for the simulation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RatEvent {
    /// The swarm starts eating a corpse.
    EatStart { swarm: PlayerId, corpse: PlayerId },
    /// The corpse is eaten: no body remains.
    Devoured { swarm: PlayerId, corpse: PlayerId },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RatSwarm {
    /// The swarm's pawn in the simulation.
    pub id: PlayerId,
    pub cfg: RatSwarmConfig,
    pub state: SwarmState,
    /// Living rats.
    pub rats: u32,
    pub target: Option<PlayerId>,
    /// Devouring Swarm caster: never a target.
    pub owner: Option<PlayerId>,
    pub since_spawn: f32,
    /// Corpse being eaten and the eating clock.
    pub eating: Option<PlayerId>,
    pub eat_time: f32,
    pub eaten: Vec<PlayerId>,
    /// `m_fCannotReachDuration`.
    pub unreachable: f32,
    pub last_pos: Vec3,
    /// Pawn lingering near an idle swarm (`m_fIdleAwayFromPawnTimer`) and where it moves to.
    pub away_timer: f32,
    pub goal: Option<Vec3>,
    pub flee_retry: f32,
    pub rng: u64,
    /// The context the swarm pawn runs this tick (`dishonored_sim::pawn_states::context`:
    /// attacking, eating; 0 none), its request to the pawn.
    #[serde(skip)]
    pub context: u8,
}

fn rnd(s: &mut u64) -> f32 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 40) as f32 / (1u64 << 24) as f32
}

impl RatSwarm {
    pub fn new(id: PlayerId, cfg: RatSwarmConfig, rats: u32, owner: Option<PlayerId>, seed: u64) -> Self {
        RatSwarm {
            id,
            cfg,
            state: SwarmState::Idle,
            rats,
            target: None,
            owner,
            since_spawn: 0.0,
            eating: None,
            eat_time: 0.0,
            eaten: Vec::new(),
            unreachable: 0.0,
            last_pos: Vec3::ZERO,
            away_timer: 0.0,
            goal: None,
            flee_retry: 0.0,
            rng: seed | 1,
            context: 0,
        }
    }

    pub fn aggressive(&self) -> bool {
        self.cfg.swarm_type == 3 || self.rats as i32 >= self.cfg.initial_rat_count_to_be_aggressive
    }

    fn sees(me: Vec3, p: Vec3, world: &dyn WorldQuery) -> bool {
        let from = me + Vec3::Z * 10.0;
        let to = p - from;
        let l = to.length();
        l < 1.0 || !world.raycast(from, to / l, l, channel::VISION).is_some_and(|h| h.distance < l - 40.0)
    }

    /// One tick: the swarm pawn's input and the bites / meals to apply.
    pub fn think(&mut self, me: &PlayerState, ws: &WorldState, world: &dyn WorldQuery, dt: f32) -> (PlayerInput, Vec<RatEvent>) {
        let mut ev = Vec::new();
        self.since_spawn += dt;
        self.context = npc_moves::NONE;
        let mut i = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        if self.rats == 0 || !me.alive {
            self.state = SwarmState::Hidden;
            return (i, ev);
        }
        let c = self.cfg.clone();
        let busy = self.state == SwarmState::Attack || self.eating.is_some();
        let radius = if busy { c.pawn_detection_radius_when_busy } else { c.pawn_detection_radius };
        // Drop a target that escaped, died (then eat it) or stayed unreachable.
        if let Some(t) = self.target {
            let tp = ws.player(t);
            let moved = me.position.distance(self.last_pos) > 1.0;
            match tp {
                Some(p) if !p.alive => {
                    self.target = None;
                    if self.rats as i32 >= c.eat_required_rat_count && !p.no_body && !self.eaten.contains(&t) {
                        self.eating = Some(t);
                        self.eat_time = 0.0;
                    }
                }
                Some(p) if p.position.distance(me.position) > c.pawn_escape_radius => self.target = None,
                None => self.target = None,
                Some(p) => {
                    let far = p.position.distance(me.position) > c.destination_radius;
                    self.unreachable = if far && !moved { self.unreachable + dt } else { 0.0 };
                    if self.unreachable >= c.pawn_unreachable_delay {
                        self.target = None;
                        self.unreachable = 0.0;
                    }
                }
            }
        }
        self.last_pos = me.position;
        // Detection.
        let can_attack = self.rats as i32 >= c.min_rat_count_per_attack_group;
        if self.target.is_none() && self.aggressive() && can_attack {
            let pick = ws
                .players
                .iter()
                .filter(|p| p.alive && p.id != me.id && p.kind != dishonored_sim::state::PawnKind::Rat && Some(p.id) != self.owner && (p.kind != dishonored_sim::state::PawnKind::Human || self.since_spawn >= c.player_detection_delay_after_spawn))
                .filter(|p| p.position.distance(me.position) <= radius && Self::sees(me.position, p.position, world))
                .min_by(|a, b| a.position.distance(me.position).total_cmp(&b.position.distance(me.position)).then(a.id.cmp(&b.id)));
            if let Some(p) = pick {
                self.target = Some(p.id);
                self.eating = None;
            }
        }
        // Attack.
        if let Some(t) = self.target.and_then(|t| ws.player(t)) {
            self.state = SwarmState::Attack;
            let d = t.position.distance(me.position);
            i.yaw = yaw(me.position, t.position);
            self.context = npc_moves::RAT_ATTACK;
            // Close in to the bite reach, then hold the attack: the simulation's attack groups
            // (`dishonored_sim::rats`, 0x00C891E0) apply the initial delay, the group damage and
            // the slowdown.
            let reach = dishonored_sim::rats::attack_distance(t);
            if d > reach * 0.8 {
                i.forward = 127;
                i.buttons |= buttons::SPRINT;
            }
            if d <= reach {
                i.buttons |= buttons::PRIMARY;
            }
            return (i, ev);
        }
        // Eating.
        if let Some(cid) = self.eating {
            let Some(body) = ws.player(cid).filter(|p| !p.alive && !p.no_body) else {
                self.eating = None;
                return (i, ev);
            };
            self.state = SwarmState::Move;
            i.yaw = yaw(me.position, body.position);
            if body.position.distance(me.position) > c.destination_radius {
                i.forward = 127;
                return (i, ev);
            }
            self.context = npc_moves::RAT_EAT;
            let before = self.eat_time;
            self.eat_time += dt;
            let start = c.eat_startup_delay_after_kill;
            if before < start && self.eat_time >= start {
                ev.push(RatEvent::EatStart { swarm: self.id, corpse: cid });
            }
            // The eating clock starts at -m_fEatStartupDelayAfterKill for a target the swarm was
            // attacking; then the startup, one interval per limb and a last one (0x00C6D760).
            if self.eat_time >= start + dishonored_sim::rats::eat_duration(true, LIMBS, c.eat_startup_duration, c.eat_per_limb_duration) {
                ev.push(RatEvent::Devoured { swarm: self.id, corpse: cid });
                self.eaten.push(cid);
                self.eating = None;
            }
            return (i, ev);
        }
        // Too few rats to attack: flee away from pawns.
        if !can_attack && !c.never_flee_systemically {
            if let Some(p) = ws.players.iter().filter(|p| p.alive && p.id != me.id && Some(p.id) != self.owner && p.position.distance(me.position) <= c.pawn_detection_radius).min_by(|a, b| a.position.distance(me.position).total_cmp(&b.position.distance(me.position))) {
                self.flee_retry -= dt;
                if self.goal.is_none() || self.flee_retry <= 0.0 {
                    let away = (me.position - p.position).truncate().normalize_or(glam::Vec2::X);
                    let dist = c.flee_attempt_distance[0] + (c.flee_attempt_distance[1] - c.flee_attempt_distance[0]) * rnd(&mut self.rng);
                    self.goal = Some(me.position + Vec3::new(away.x, away.y, 0.0) * dist);
                    self.flee_retry = c.flee_retry_delay;
                }
                self.state = SwarmState::Flee;
            }
        } else if !self.aggressive() {
            // Idle swarm moving away from a lingering pawn.
            let near = ws.players.iter().filter(|p| p.alive && p.id != me.id && p.position.distance(me.position) <= c.idle_away_from_pawn_trigger_dist).min_by(|a, b| a.position.distance(me.position).total_cmp(&b.position.distance(me.position)));
            match near {
                Some(p) => {
                    self.away_timer += dt;
                    if self.away_timer >= c.idle_away_from_pawn_trigger_duration && self.goal.is_none() {
                        let away = (me.position - p.position).truncate().normalize_or(glam::Vec2::X);
                        self.goal = Some(me.position + Vec3::new(away.x, away.y, 0.0) * c.idle_away_from_pawn_move_dist);
                        self.state = SwarmState::Move;
                    }
                }
                None => self.away_timer = 0.0,
            }
        }
        if let Some(g) = self.goal {
            if g.truncate().distance(me.position.truncate()) <= c.destination_radius {
                self.goal = None;
                self.state = SwarmState::Idle;
                self.away_timer = 0.0;
            } else {
                i.yaw = yaw(me.position, g);
                i.forward = 127;
                if self.state == SwarmState::Flee {
                    i.buttons |= buttons::SPRINT;
                }
                return (i, ev);
            }
        }
        if self.state != SwarmState::Flee {
            self.state = SwarmState::Idle;
        }
        (i, ev)
    }
}

fn yaw(from: Vec3, to: Vec3) -> f32 {
    let d = to - from;
    d.y.atan2(d.x).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_sim::state::PawnKind;
    use dishonored_world::CollisionWorld;

    fn world() -> CollisionWorld {
        CollisionWorld::from_triangles(vec![], vec![])
    }

    fn pawn(id: PlayerId, kind: PawnKind, pos: Vec3) -> PlayerState {
        let mut p = PlayerState::new(id, pos, 0.0);
        p.kind = kind;
        p
    }

    #[test]
    fn swarm_waits_detection_delay_then_attacks_in_reach() {
        let w = world();
        let me = pawn(5000, PawnKind::Rat, Vec3::ZERO);
        let player = pawn(1, PawnKind::Human, Vec3::new(400.0, 0.0, 0.0));
        let mut ws = WorldState::default();
        ws.players = vec![me.clone(), player];
        let mut s = RatSwarm::new(me.id, RatSwarmConfig::default(), 20, None, 1);
        let dt = 0.1;
        // Players are ignored for `m_fPlayerDetectionDelayAfterSpawn` (2 s).
        for _ in 0..19 {
            s.think(&me, &ws, &w, dt);
        }
        assert_eq!(s.target, None);
        let (i, _) = s.think(&me, &ws, &w, dt);
        assert_eq!(s.target, Some(1));
        // Out of reach (200 uu from a standing player): run at it, no attack yet.
        assert!(i.forward > 0 && i.buttons & buttons::PRIMARY == 0 && s.context == npc_moves::RAT_ATTACK);
        ws.players[1].position = Vec3::new(150.0, 0.0, 0.0);
        let (i, _) = s.think(&me, &ws, &w, dt);
        assert!(i.buttons & buttons::PRIMARY != 0);
    }

    #[test]
    fn small_swarm_is_not_aggressive_and_eats_corpses_with_enough_rats() {
        let w = world();
        let me = pawn(5000, PawnKind::Civilian, Vec3::ZERO);
        let player = pawn(1, PawnKind::Human, Vec3::new(100.0, 0.0, 0.0));
        let mut ws = WorldState::default();
        ws.players = vec![me.clone(), player];
        let mut s = RatSwarm::new(me.id, RatSwarmConfig::default(), 6, None, 1);
        s.since_spawn = 10.0;
        s.think(&me, &ws, &w, 0.1);
        assert_eq!(s.target, None);
        assert!(!s.aggressive());
        // A Devouring Swarm is always aggressive, but never toward its caster.
        let mut d = RatSwarm::new(me.id, RatSwarmConfig { swarm_type: 3, ..RatSwarmConfig::default() }, 10, Some(1), 1);
        d.since_spawn = 10.0;
        d.think(&me, &ws, &w, 0.1);
        assert_eq!(d.target, None);
        // Eating: a dead target is eaten (startup 1 + 2 s, then 5 limbs of 1.85 s).
        let mut e = RatSwarm::new(me.id, RatSwarmConfig::default(), 20, None, 1);
        e.target = Some(1);
        ws.players[1].alive = false;
        let mut evs = Vec::new();
        for _ in 0..200 {
            evs.extend(e.think(&me, &ws, &w, 0.1).1);
        }
        assert!(evs.iter().any(|x| matches!(x, RatEvent::EatStart { corpse: 1, .. })));
        assert!(evs.iter().any(|x| matches!(x, RatEvent::Devoured { corpse: 1, .. })));
    }
}
