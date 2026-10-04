//! NPC crowd and guard behaviour (Phase II S0, S8).
//!
//! Brains observe the authoritative `WorldState` plus gameplay events and act **only**
//! through `PlayerInput` on ordinary pawns, so NPCs move exactly like players can move.
//! That is what makes behavioural blending possible. Everything is deterministic (seeded
//! per-brain RNG, no wall clock), so server, replays and tests agree.
//!
//! This is not a reproduction of Dishonored's campaign AI (non-goal). It implements the
//! behaviour vocabulary the brief needs: civilians Idle / Wander / WalkToPoint /
//! GroupIdle / React / Flee, guards Idle / Patrol / Investigate / Alert / Combat.

use dishonored_sim::events::{SimEvent, SoundCategory};
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::state::{PawnKind, PlayerId, PlayerState, SwordPhase, WorldState};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;
use serde::{Deserialize, Serialize};

pub const NPC_ID_BASE: PlayerId = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrainState {
    Idle,
    Wander,
    WalkToPoint,
    GroupIdle,
    React,
    Flee,
    Patrol,
    Investigate,
    Alert,
    Combat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Brain {
    pub id: PlayerId,
    pub kind: PawnKind,
    pub state: BrainState,
    pub rng: u64,
    pub timer: f32,
    pub goal: Option<Vec3>,
    pub home: Vec3,
    pub route: Vec<Vec3>,
    pub route_i: usize,
    pub threat_pos: Option<Vec3>,
    pub target: Option<PlayerId>,
    /// Social spot index for GroupIdle.
    pub spot: Option<usize>,
    pub last_pos: Vec3,
    pub stuck: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrowdConfig {
    pub civilians: usize,
    pub guards: usize,
    /// Population centre/radius (uu) used to select points and spawns.
    pub center: Vec3,
    pub radius: f32,
    pub seed: u64,
    /// Visual archetype pools.
    pub civilian_archetypes: Vec<u8>,
    pub guard_archetypes: Vec<u8>,
}

/// Static layout shared by all brains (original Dishonored AI points near the centre).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CrowdLayout {
    pub points: Vec<Vec3>,
    pub social_spots: Vec<Vec3>,
    pub guard_posts: Vec<(Vec3, f32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NpcSpawn {
    pub id: PlayerId,
    pub kind: PawnKind,
    pub archetype: u8,
    pub position: Vec3,
    pub yaw: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Crowd {
    pub brains: Vec<Brain>,
    pub layout: CrowdLayout,
    pub spawns: Vec<NpcSpawn>,
    /// Seconds until a dead NPC respawns at its spawn.
    pub respawn: Vec<(PlayerId, f32)>,
}

fn rng_next(s: &mut u64) -> f32 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 40) as f32 / (1u64 << 24) as f32
}

/// Walk speed matching a player holding WALK (200 uu/s x sword factor), so careful
/// players are indistinguishable from strolling NPCs.
fn yaw_to(from: Vec3, to: Vec3) -> f32 {
    let d = to - from;
    d.y.atan2(d.x).to_degrees()
}

impl Crowd {
    /// Build a crowd from original placement data.
    /// `points`: (position, yaw) of nav/target points; `guard_spawns`: original guard
    /// spawners; `floor` validates positions against collision.
    pub fn populate(cfg: &CrowdConfig, points: &[(Vec3, f32)], guard_spawns: &[(Vec3, f32)], world: &dyn WorldQuery) -> Crowd {
        let mut rng = cfg.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let near: Vec<(Vec3, f32)> = points.iter().copied().filter(|(p, _)| p.distance(cfg.center) <= cfg.radius).filter_map(|(p, y)| snap_floor(world, p).map(|q| (q, y))).collect();
        let mut layout = CrowdLayout { points: near.iter().map(|p| p.0).collect(), ..Default::default() };
        // Social spots: every third point (deterministic) where groups gather.
        layout.social_spots = layout.points.iter().copied().step_by(3).collect();
        layout.guard_posts = guard_spawns.iter().copied().filter(|(p, _)| p.distance(cfg.center) <= cfg.radius * 1.5).filter_map(|(p, y)| snap_floor(world, p).map(|q| (q, y))).collect();
        let mut brains = Vec::new();
        let mut spawns = Vec::new();
        let mut id = NPC_ID_BASE;
        for i in 0..cfg.civilians {
            if layout.points.is_empty() {
                break;
            }
            let base = layout.points[(rng_next(&mut rng) * layout.points.len() as f32) as usize % layout.points.len()];
            let off = Vec3::new(rng_next(&mut rng) - 0.5, rng_next(&mut rng) - 0.5, 0.0) * 160.0;
            let pos = snap_floor(world, base + off).unwrap_or(base);
            let yaw = rng_next(&mut rng) * 360.0;
            let arch = cfg.civilian_archetypes[i % cfg.civilian_archetypes.len().max(1)];
            spawns.push(NpcSpawn { id, kind: PawnKind::Civilian, archetype: arch, position: pos, yaw });
            brains.push(Brain::new(id, PawnKind::Civilian, pos, cfg.seed ^ (id as u64 * 7919)));
            id += 1;
        }
        for i in 0..cfg.guards {
            let (pos, yaw) = if !layout.guard_posts.is_empty() {
                layout.guard_posts[i % layout.guard_posts.len()]
            } else if !layout.points.is_empty() {
                (layout.points[i % layout.points.len()], 0.0)
            } else {
                break;
            };
            let arch = cfg.guard_archetypes[i % cfg.guard_archetypes.len().max(1)];
            spawns.push(NpcSpawn { id, kind: PawnKind::Guard, archetype: arch, position: pos, yaw });
            let mut b = Brain::new(id, PawnKind::Guard, pos, cfg.seed ^ (id as u64 * 104729));
            // Patrol route: nearest points around the post, visited in a loop.
            let mut pts: Vec<Vec3> = layout.points.clone();
            pts.sort_by(|a, c| a.distance(pos).total_cmp(&c.distance(pos)));
            b.route = pts.into_iter().take(4).collect();
            b.state = if b.route.len() >= 2 { BrainState::Patrol } else { BrainState::Idle };
            brains.push(b);
            id += 1;
        }
        Crowd { brains, layout, spawns, respawn: Vec::new() }
    }

    /// Produce this tick's inputs for every NPC. `events` are the previous tick's events.
    pub fn think(&mut self, world_state: &WorldState, events: &[SimEvent], world: &dyn WorldQuery, dt: f32) -> Vec<(PlayerId, PlayerInput)> {
        let layout = &self.layout;
        let mut out = Vec::with_capacity(self.brains.len());
        for b in self.brains.iter_mut() {
            let Some(me) = world_state.player(b.id) else { continue };
            if !me.alive {
                continue;
            }
            b.perceive(me, world_state, events, world);
            out.push((b.id, b.act(me, world_state, layout, world, dt)));
        }
        out
    }

    /// Respawn bookkeeping: returns NPCs to respawn now.
    pub fn tick_respawns(&mut self, events: &[SimEvent], dt: f32) -> Vec<NpcSpawn> {
        for e in events {
            if let SimEvent::Death { victim, .. } = e {
                if self.brains.iter().any(|b| b.id == *victim) && !self.respawn.iter().any(|(i, _)| i == victim) {
                    self.respawn.push((*victim, 20.0));
                }
            }
        }
        let mut due = Vec::new();
        for (id, t) in self.respawn.iter_mut() {
            *t -= dt;
            if *t <= 0.0 {
                if let Some(s) = self.spawns.iter().find(|s| s.id == *id) {
                    due.push(s.clone());
                }
            }
        }
        self.respawn.retain(|(_, t)| *t > 0.0);
        for s in &due {
            if let Some(b) = self.brains.iter_mut().find(|b| b.id == s.id) {
                *b = Brain { route: b.route.clone(), ..Brain::new(s.id, s.kind, s.position, b.rng) };
                if b.kind == PawnKind::Guard && b.route.len() >= 2 {
                    b.state = BrainState::Patrol;
                }
            }
        }
        due
    }
}

fn snap_floor(world: &dyn WorldQuery, p: Vec3) -> Option<Vec3> {
    let h = world.raycast(p + Vec3::Z * 120.0, -Vec3::Z, 400.0, channel::PAWN)?;
    (h.normal.z > 0.7).then(|| h.point + Vec3::Z * 90.0)
}

/// Is the pawn doing something a guard treats as criminal?
fn criminal(p: &PlayerState) -> bool {
    matches!(p.sword.phase, SwordPhase::Startup | SwordPhase::Active | SwordPhase::Assassinating) || p.left.phase == dishonored_sim::state::LeftPhase::Recovering
}

impl Brain {
    pub fn new(id: PlayerId, kind: PawnKind, home: Vec3, seed: u64) -> Self {
        Brain { id, kind, state: BrainState::Idle, rng: seed | 1, timer: 0.0, goal: None, home, route: Vec::new(), route_i: 0, threat_pos: None, target: None, spot: None, last_pos: home, stuck: 0.0 }
    }

    fn sees(&self, me: &PlayerState, other: &PlayerState, world: &dyn WorldQuery, range: f32, fov_deg: f32) -> bool {
        let eye = me.position + Vec3::Z * 85.0;
        let to = other.position + Vec3::Z * 40.0 - eye;
        let d = to.length();
        if d > range || d < 1.0 {
            return false;
        }
        let fwd = dishonored_sim::movement::forward_vector(me.yaw);
        if fwd.dot(to / d) < (fov_deg * 0.5).to_radians().cos() {
            return false;
        }
        world.raycast(eye, to, d, channel::VISION).is_none_or(|h| h.distance >= d - 40.0)
    }

    fn perceive(&mut self, me: &PlayerState, ws: &WorldState, events: &[SimEvent], world: &dyn WorldQuery) {
        // Hearing: gameplay sound events (never raw audio).
        for e in events {
            if let SimEvent::Sound { position, radius, category, source } = e {
                if *source == self.id || position.distance(me.position) > *radius {
                    continue;
                }
                let violent = matches!(category, SoundCategory::Pistol | SoundCategory::SwordImpact | SoundCategory::SwordBlock | SoundCategory::Windblast | SoundCategory::BodyFall | SoundCategory::Crossbow);
                match self.kind {
                    PawnKind::Guard if violent || *category == SoundCategory::Blink => {
                        if !matches!(self.state, BrainState::Combat) {
                            self.state = BrainState::Investigate;
                            self.threat_pos = Some(*position);
                            self.timer = 0.0;
                        }
                    }
                    PawnKind::Civilian if violent => {
                        self.state = BrainState::Flee;
                        self.threat_pos = Some(*position);
                        self.timer = 0.0;
                    }
                    _ => {}
                }
            }
            if let SimEvent::Death { victim, killer: Some(k), .. } = e {
                // Witnessed murder.
                if let (Some(v), Some(kp)) = (ws.player(*victim), ws.player(*k)) {
                    if self.sees(me, v, world, 1500.0, 140.0) || self.sees(me, kp, world, 1500.0, 140.0) {
                        match self.kind {
                            PawnKind::Guard => {
                                self.state = BrainState::Combat;
                                self.target = Some(*k);
                                self.timer = 0.0;
                            }
                            PawnKind::Civilian => {
                                self.state = BrainState::Flee;
                                self.threat_pos = Some(kp.position);
                                self.timer = 0.0;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        // Guards: visible criminal activity by humans -> Combat.
        if self.kind == PawnKind::Guard && self.state != BrainState::Combat {
            for p in ws.players.iter().filter(|p| p.alive && p.kind == PawnKind::Human) {
                if criminal(p) && self.sees(me, p, world, 1400.0, 120.0) {
                    self.state = BrainState::Combat;
                    self.target = Some(p.id);
                    self.timer = 0.0;
                    break;
                }
            }
        }
    }

    fn walk_to(&mut self, me: &PlayerState, goal: Vec3, hurry: bool) -> PlayerInput {
        let mut i = PlayerInput { yaw: yaw_to(me.position, goal), forward: 127, left_hand: LeftHand::Empty, ..Default::default() };
        if !hurry {
            i.buttons |= buttons::WALK;
        } else {
            i.buttons |= buttons::SPRINT;
        }
        i
    }

    fn act(&mut self, me: &PlayerState, ws: &WorldState, layout: &CrowdLayout, _world: &dyn WorldQuery, dt: f32) -> PlayerInput {
        self.timer += dt;
        // Stuck detection.
        if me.position.distance(self.last_pos) < 0.5 && self.goal.is_some() {
            self.stuck += dt;
        } else {
            self.stuck = 0.0;
        }
        self.last_pos = me.position;
        if self.stuck > 1.5 {
            self.stuck = 0.0;
            self.goal = None;
            if matches!(self.state, BrainState::WalkToPoint | BrainState::Wander) {
                self.state = BrainState::Idle;
                self.timer = 0.0;
            }
        }
        let idle = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        let pick_point = |rng: &mut u64| -> Option<Vec3> {
            if layout.points.is_empty() {
                return None;
            }
            let i = (rng_next(rng) * layout.points.len() as f32) as usize % layout.points.len();
            Some(layout.points[i])
        };
        match self.state {
            BrainState::Idle => {
                if self.timer > 3.0 + rng_next(&mut self.rng) * 6.0 {
                    self.timer = 0.0;
                    let r = rng_next(&mut self.rng);
                    if self.kind == PawnKind::Guard && self.route.len() >= 2 {
                        self.state = BrainState::Patrol;
                    } else if r < 0.45 && !layout.social_spots.is_empty() {
                        let s = (rng_next(&mut self.rng) * layout.social_spots.len() as f32) as usize % layout.social_spots.len();
                        self.spot = Some(s);
                        let ang = rng_next(&mut self.rng) * std::f32::consts::TAU;
                        self.goal = Some(layout.social_spots[s] + Vec3::new(ang.cos(), ang.sin(), 0.0) * 90.0);
                        self.state = BrainState::WalkToPoint;
                    } else {
                        self.goal = pick_point(&mut self.rng);
                        self.state = BrainState::Wander;
                    }
                }
                idle
            }
            BrainState::Wander | BrainState::WalkToPoint => match self.goal {
                Some(g) if g.truncate().distance(me.position.truncate()) > 60.0 => self.walk_to(me, g, false),
                _ => {
                    self.goal = None;
                    self.timer = 0.0;
                    self.state = if self.spot.is_some() { BrainState::GroupIdle } else { BrainState::Idle };
                    idle
                }
            },
            BrainState::GroupIdle => {
                // Face the group centre and chat; leave after a while.
                let yaw = self.spot.map(|s| yaw_to(me.position, layout.social_spots[s])).unwrap_or(me.yaw);
                if self.timer > 10.0 + rng_next(&mut self.rng) * 15.0 {
                    self.spot = None;
                    self.state = BrainState::Idle;
                    self.timer = 0.0;
                }
                PlayerInput { yaw, left_hand: LeftHand::Empty, ..Default::default() }
            }
            BrainState::React => {
                if self.timer > 1.0 {
                    self.state = BrainState::Flee;
                }
                idle
            }
            BrainState::Flee => {
                // Run away from the threat; recover after 8 s (keeps panic local).
                let from = self.threat_pos.unwrap_or(me.position);
                let away = (me.position - from).truncate().normalize_or_zero();
                if self.timer > 8.0 || away == glam::Vec2::ZERO {
                    self.state = BrainState::Idle;
                    self.timer = 0.0;
                    self.threat_pos = None;
                    return idle;
                }
                let goal = me.position + Vec3::new(away.x, away.y, 0.0) * 500.0;
                self.walk_to(me, goal, true)
            }
            BrainState::Patrol => {
                if self.route.is_empty() {
                    self.state = BrainState::Idle;
                    return idle;
                }
                let g = self.route[self.route_i % self.route.len()];
                self.goal = Some(g);
                if g.truncate().distance(me.position.truncate()) < 70.0 {
                    self.route_i = (self.route_i + 1) % self.route.len();
                    self.state = BrainState::Alert; // brief look-around at each post
                    self.timer = 0.0;
                    return idle;
                }
                self.walk_to(me, g, false)
            }
            BrainState::Alert => {
                if self.timer > 2.5 {
                    self.state = BrainState::Patrol;
                    self.timer = 0.0;
                }
                PlayerInput { yaw: me.yaw + (self.timer * 40.0).sin() * 1.5, left_hand: LeftHand::Empty, ..Default::default() }
            }
            BrainState::Investigate => {
                let Some(t) = self.threat_pos else {
                    self.state = BrainState::Patrol;
                    return idle;
                };
                if t.truncate().distance(me.position.truncate()) < 120.0 || self.timer > 12.0 {
                    self.state = BrainState::Alert;
                    self.threat_pos = None;
                    self.timer = 0.0;
                    return idle;
                }
                self.walk_to(me, t, true)
            }
            BrainState::Combat => {
                let Some(t) = self.target.and_then(|id| ws.player(id)).filter(|p| p.alive) else {
                    self.state = BrainState::Alert;
                    self.target = None;
                    self.timer = 0.0;
                    return idle;
                };
                if self.timer > 30.0 || t.position.distance(me.position) > 3000.0 {
                    self.state = BrainState::Investigate;
                    self.threat_pos = Some(t.position);
                    self.target = None;
                    self.timer = 0.0;
                    return idle;
                }
                let d = t.position.distance(me.position);
                let mut i = PlayerInput { yaw: yaw_to(me.position, t.position), left_hand: LeftHand::Empty, ..Default::default() };
                if d > 150.0 {
                    i.forward = 127;
                    i.buttons |= buttons::SPRINT;
                } else if ((self.timer * 60.0) as u32) % 45 == 0 {
                    i.buttons |= buttons::PRIMARY;
                } else if t.sword.phase == SwordPhase::Startup {
                    i.buttons |= buttons::BLOCK;
                }
                i
            }
        }
    }
}
