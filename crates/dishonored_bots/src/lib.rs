//! Test bots. They only see what a client sees (player states) and only act through
//! `PlayerInput`, so they exercise exactly the same paths as humans: prediction,
//! server validation, combat resolution.
//!
//! Free-for-all bots hunt: every client receives every player's replicated position (what a
//! human reads off the scoreboard and minimap), so a bot paths on the map's original navmesh
//! toward the nearest opponent it can reach, fights the ones it has a clear line to, and roams
//! the navmesh when nobody is reachable. Every move goes through [`hazard::safe_ahead`] so bots
//! keep off drops, deep water and pain volumes; a bot that ends up swimming heads for the
//! nearest navmesh shore.

use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::movement::{forward_vector, right_vector};
use dishonored_sim::state::{MoveMode, PawnKind, PlayerId, PlayerState};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;

pub mod hazard;
pub mod nav;
pub use nav::NavGraph;

/// Eye above the pawn centre (what the bot aims from).
const EYE: f32 = 64.0;
/// A target further than this is hunted by position, not fought.
const SIGHT: f32 = 4000.0;
/// Ticks a target stays marked unreachable after a failed path.
const UNREACHABLE_TICKS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotStyle {
    /// Close in and use the sword; Blink to close distance.
    Duelist,
    /// Keep mid range and shoot (pistol/crossbow).
    Gunner,
    /// Wander only (movement soak).
    Wanderer,
}

#[derive(Debug, Clone)]
pub struct Bot {
    pub id: PlayerId,
    pub style: BotStyle,
    rng: u64,
    tick: u64,
    wander_yaw: f32,
    last_pos: Vec3,
    stuck_ticks: u32,
    blink_hold: u32,
    fire_cooldown: u32,
    /// The opponent being fought (clear line of fire).
    pub target: Option<PlayerId>,
    /// The opponent being hunted by its replicated position.
    pub hunt: Option<PlayerId>,
    follow: nav::Follower,
    route: nav::Route,
    unreachable: Vec<(PlayerId, u64)>,
    roam: Option<Vec3>,
    sidestep: u32,
    side_yaw: f32,
    water_exit: Option<Vec3>,
    /// Consecutive ticks the hazard check refused the wanted move.
    refused: u32,
    /// Contracts: ticks spent close to the target, flight from a charging guard.
    close_ticks: u32,
    flee_ticks: u32,
    flee_from: Vec3,
}

impl Bot {
    pub fn new(id: PlayerId, style: BotStyle, seed: u64) -> Self {
        Self {
            id,
            style,
            rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            tick: 0,
            wander_yaw: (seed % 360) as f32,
            last_pos: Vec3::ZERO,
            stuck_ticks: 0,
            blink_hold: 0,
            fire_cooldown: 0,
            target: None,
            hunt: None,
            follow: nav::Follower::default(),
            route: nav::Route::default(),
            unreachable: Vec::new(),
            roam: None,
            sidestep: 0,
            side_yaw: 0.0,
            water_exit: None,
            refused: 0,
            close_ticks: 0,
            flee_ticks: 0,
            flee_from: Vec3::ZERO,
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Where the bot is heading (debugging): its path's goal.
    pub fn goal(&self) -> Option<Vec3> {
        self.route.goal
    }

    fn reset_plan(&mut self) {
        self.route.clear();
        self.follow.clear();
        self.roam = None;
        self.sidestep = 0;
        self.stuck_ticks = 0;
        self.water_exit = None;
        self.refused = 0;
        self.blink_hold = 0;
    }

    fn reachable(&self, id: PlayerId) -> bool {
        !self.unreachable.iter().any(|(u, until)| *u == id && *until > self.tick)
    }

    /// Decide this tick's input from the bot's own state and the players it knows about.
    pub fn think(&mut self, me: &PlayerState, others: &[PlayerState], world: &dyn WorldQuery) -> PlayerInput {
        self.tick += 1;
        let mut input = PlayerInput { yaw: me.yaw, pitch: 0.0, left_hand: LeftHand::Blink, ..Default::default() };
        if !me.alive {
            self.reset_plan();
            self.target = None;
            return input;
        }
        if me.mode == MoveMode::Swimming {
            return self.swim_out(me, world, input);
        }
        self.water_exit = None;
        self.unreachable.retain(|(_, until)| *until > self.tick);

        let eye = me.position + Vec3::Z * EYE;
        let clear = |o: &PlayerState| -> bool {
            let to = o.position + Vec3::Z * 40.0 - eye;
            let d = to.length();
            d < SIGHT && world.raycast(eye, to, d, channel::VISION | channel::PROJECTILE).is_none_or(|h| h.distance >= d - 40.0)
        };
        let foes: Vec<&PlayerState> = if self.style == BotStyle::Wanderer { Vec::new() } else { others.iter().filter(|o| o.alive && o.id != me.id && o.kind == PawnKind::Human).collect() };
        let dist = |o: &PlayerState| o.position.distance(me.position);
        let visible = foes.iter().copied().filter(|o| clear(o)).min_by(|a, b| dist(a).total_cmp(&dist(b)));
        self.target = visible.map(|t| t.id);
        // Hunt the nearest reachable opponent; keep the current one unless another is much nearer.
        let nearest = foes.iter().copied().filter(|o| self.reachable(o.id)).min_by(|a, b| dist(a).total_cmp(&dist(b)));
        let current = self.hunt.and_then(|h| foes.iter().copied().find(|o| o.id == h && self.reachable(h)));
        let hunt = match (current, nearest) {
            (Some(c), Some(n)) if dist(n) < dist(c) * 0.7 => Some(n),
            (Some(c), _) => Some(c),
            (None, n) => n,
        };
        if hunt.map(|h| h.id) != self.hunt {
            self.route.force_replan();
        }
        self.hunt = hunt.map(|h| h.id);

        match (visible, hunt) {
            (Some(t), _) => self.fight(me, t, world, &mut input),
            (None, Some(h)) => {
                let goal = h.position;
                if !self.go_to(me, goal, world, &mut input) {
                    self.unreachable.push((h.id, self.tick + UNREACHABLE_TICKS));
                    self.hunt = None;
                    self.roam_on(me, world, &mut input);
                }
            }
            (None, None) => self.roam_on(me, world, &mut input),
        }
        self.keep_safe(me, world, &mut input, true);
        self.unstick(me, world, &mut input);
        self.keep_safe(me, world, &mut input, false);
        input
    }

    /// Steer along the navmesh toward `goal`; false when there is no route.
    fn go_to(&mut self, me: &PlayerState, goal: Vec3, world: &dyn WorldQuery, input: &mut PlayerInput) -> bool {
        let nav = world.nav();
        let steer = self.route.steer(world, me.position, goal);
        if nav.is_some() && self.route.failed {
            return false;
        }
        let to = steer - me.position;
        input.yaw = to.y.atan2(to.x).to_degrees();
        input.pitch = 0.0;
        input.forward = 127;
        let far = (goal - me.position).truncate().length();
        if far > 900.0 {
            input.buttons |= buttons::SPRINT;
        }
        true
    }

    /// Wander to random navmesh points (nobody reachable, or the wanderer style).
    fn roam_on(&mut self, me: &PlayerState, world: &dyn WorldQuery, input: &mut PlayerInput) {
        let Some(nav) = world.nav().filter(|n| !n.polys.is_empty()) else {
            if self.tick % 180 == 0 {
                self.wander_yaw += (self.rand() - 0.5) * 120.0;
            }
            input.yaw = self.wander_yaw;
            input.forward = 100;
            return;
        };
        let reached = self.roam.is_some_and(|r| (r - me.position).truncate().length() < 150.0);
        if self.roam.is_none() || reached || self.tick % 900 == 0 {
            let i = ((self.rand() * nav.polys.len() as f32) as usize).min(nav.polys.len() - 1);
            self.roam = Some(nav.polys[i].center + Vec3::Z * hazard::CENTRE);
            self.route.force_replan();
        }
        let goal = self.roam.unwrap();
        if !self.go_to(me, goal, world, input) {
            self.roam = None;
            input.forward = 0;
        } else {
            input.buttons &= !buttons::SPRINT;
        }
    }

    /// Combat against a target in the clear.
    fn fight(&mut self, me: &PlayerState, t: &PlayerState, world: &dyn WorldQuery, input: &mut PlayerInput) {
        let eye = me.position + Vec3::Z * EYE;
        let to = t.position - me.position;
        let dist = to.truncate().length();
        let level = to.z.abs() < 120.0;
        let aim = t.position + Vec3::Z * 40.0 - eye;
        let face = |input: &mut PlayerInput| {
            input.yaw = to.y.atan2(to.x).to_degrees();
            input.pitch = (aim.z / aim.truncate().length().max(1.0)).atan().to_degrees();
        };
        match self.style {
            BotStyle::Duelist => {
                input.left_hand = LeftHand::Blink;
                if dist > 160.0 {
                    // Straight in when close and level, else along the navmesh.
                    if dist > 500.0 || !level {
                        if !self.go_to(me, t.position, world, input) {
                            face(input);
                            input.forward = 127;
                        }
                    } else {
                        face(input);
                        input.forward = 127;
                    }
                    if dist > 700.0 {
                        input.buttons |= buttons::SPRINT;
                    }
                    // Blink in when far and roughly level (the aim point is the target's floor).
                    if dist > 600.0 && dist < 1050.0 && to.z.abs() < 250.0 && me.vitals.mana >= 20.0 && (self.blink_hold > 0 || self.rand() < 0.02) {
                        face(input);
                        self.blink_hold += 1;
                        if self.blink_hold < 12 {
                            input.buttons |= buttons::SECONDARY;
                        } else {
                            self.blink_hold = 0;
                        }
                    }
                } else {
                    face(input);
                    input.right = if (self.tick / 40) % 2 == 0 { 60 } else { -60 };
                    if self.tick % 25 == 0 {
                        input.buttons |= buttons::PRIMARY;
                    } else if self.rand() < 0.15 {
                        input.buttons |= buttons::BLOCK;
                    }
                }
            }
            BotStyle::Gunner => {
                input.left_hand = if dist > 900.0 { LeftHand::Crossbow } else { LeftHand::Pistol };
                if dist > 1200.0 {
                    // Out of range: close in along the navmesh.
                    if !self.go_to(me, t.position, world, input) {
                        face(input);
                        input.forward = 100;
                    }
                    input.buttons &= !buttons::SPRINT;
                    return;
                }
                face(input);
                input.forward = if dist < 400.0 { -100 } else { 0 };
                input.right = if (self.tick / 60) % 2 == 0 { 70 } else { -70 };
                if self.fire_cooldown == 0 {
                    input.buttons |= buttons::SECONDARY;
                    self.fire_cooldown = 45 + (self.rand() * 30.0) as u32;
                } else {
                    self.fire_cooldown -= 1;
                }
                if dist < 180.0 && self.tick % 30 == 0 {
                    input.buttons |= buttons::PRIMARY;
                }
            }
            BotStyle::Wanderer => {}
        }
    }

    /// Stuck (pushing without moving): replan, then sidestep with a hop.
    fn unstick(&mut self, me: &PlayerState, _world: &dyn WorldQuery, input: &mut PlayerInput) {
        let moved = me.position.truncate().distance(self.last_pos.truncate());
        self.last_pos = me.position;
        let pushing = input.forward != 0 || input.right != 0;
        if pushing && moved < 0.5 && me.mode == MoveMode::Walking {
            self.stuck_ticks += 1;
        } else if moved > 2.0 {
            self.stuck_ticks = 0;
        }
        if self.stuck_ticks > 45 {
            self.stuck_ticks = 0;
            self.route.force_replan();
            self.sidestep = 25;
            self.side_yaw = input.yaw + if self.rand() < 0.5 { 90.0 } else { -90.0 } + (self.rand() - 0.5) * 60.0;
            input.buttons |= buttons::JUMP;
        }
        if self.sidestep > 0 {
            self.sidestep -= 1;
            input.yaw = self.side_yaw;
            input.forward = 127;
            input.right = 0;
            input.buttons &= !(buttons::PRIMARY | buttons::SECONDARY);
        }
    }

    /// Refuse moves onto unsafe ground (on foot and in the air): try the other strafe side,
    /// else stand. `count`: this is the tick's first check (the give-up counter runs once).
    fn keep_safe(&mut self, me: &PlayerState, world: &dyn WorldQuery, input: &mut PlayerInput, count: bool) {
        if !matches!(me.mode, MoveMode::Walking | MoveMode::Falling) {
            return;
        }
        let dir_of = |i: &PlayerInput| forward_vector(i.yaw) * i.forward as f32 + right_vector(i.yaw) * i.right as f32;
        let dir = dir_of(input);
        if dir.length_squared() < 1.0 || hazard::safe_ahead(world, me.position, dir, 70.0) {
            if count {
                self.refused = 0;
            }
            return;
        }
        if input.right != 0 {
            let mut flipped = *input;
            flipped.right = -input.right;
            if hazard::safe_ahead(world, me.position, dir_of(&flipped), 70.0) {
                *input = flipped;
                return;
            }
        }
        input.forward = 0;
        input.right = 0;
        input.buttons &= !(buttons::SPRINT | buttons::JUMP);
        // Blink is aimed at the target's floor: keep it; anything else waits.
        self.sidestep = 0;
        if !count {
            return;
        }
        self.refused += 1;
        if self.refused > 60 {
            // The route keeps leading off safe ground: give up on this goal for a while.
            self.refused = 0;
            if let Some(h) = self.hunt.take() {
                self.unreachable.push((h, self.tick + UNREACHABLE_TICKS));
            }
            self.roam = None;
            self.route.force_replan();
        }
    }

    /// In deep water: swim for the nearest navmesh shore at the surface, never out past the
    /// edge of the water volume.
    fn swim_out(&mut self, me: &PlayerState, world: &dyn WorldQuery, mut input: PlayerInput) -> PlayerInput {
        self.route.clear();
        let Some(water) = world.water_at(me.position) else { return input };
        if self.water_exit.is_none() || self.tick % 240 == 0 {
            self.water_exit = Self::find_shore(world, me.position, water.surface_z);
        }
        let head_under = me.position.z + EYE < water.surface_z;
        if head_under {
            input.buttons |= buttons::JUMP;
        }
        let Some(exit) = self.water_exit else { return input };
        let to = exit - me.position;
        input.yaw = to.y.atan2(to.x).to_degrees();
        input.pitch = 0.0;
        input.forward = 127;
        // Never swim out of the volume over nothing (the sea ends; past it is `KillZ`).
        let ahead = me.position + forward_vector(input.yaw) * 150.0;
        if world.water_at(ahead).is_none() && hazard::floor_below(world, ahead, hazard::MAX_DROP).is_none() {
            input.forward = 0;
            self.water_exit = None;
        }
        input
    }

    /// The nearest navmesh point a swimmer can climb out onto: a poly at about the water
    /// surface (mantle height) and not itself under water.
    fn find_shore(world: &dyn WorldQuery, from: Vec3, surface: f32) -> Option<Vec3> {
        let nav = world.nav()?;
        let mut best: Option<(f32, Vec3)> = None;
        for p in &nav.polys {
            let c = p.center;
            if c.z < surface - 60.0 || c.z > surface + 140.0 || hazard::deep_water(world, c + Vec3::Z * hazard::CENTRE) {
                continue;
            }
            let d = (c - from).truncate().length();
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, c + Vec3::Z * hazard::CENTRE));
            }
        }
        best.map(|b| b.1)
    }
}

/// Contracts bot tuning (test tooling, not game rules).
mod contract {
    /// A guard closer than this with a clear line and the bot in front of it sees a strike.
    pub const WITNESS_RANGE: f32 = 2200.0;
    /// Half the frontal arc (degrees) in which a guard is counted as watching.
    pub const WITNESS_HALF_ARC: f32 = 70.0;
    /// Back assassination reach and arc the bot waits for (inside the sim's 0.6 x 275 uu and
    /// 60 degrees, with margin for the target moving during the swing start).
    pub const BACK_REACH: f32 = 150.0;
    pub const BACK_HALF_ARC: f32 = 50.0;
    /// Seconds (ticks) close to the target without a back opening before a frontal attack.
    pub const OPENING_TICKS: u32 = 480;
    /// A guard running at the bot (uu/s, toward it) this close starts a flight.
    pub const CHARGE_SPEED: f32 = 280.0;
    pub const CHARGE_RANGE: f32 = 1100.0;
    pub const FLEE_TICKS: u32 = 240;
}

impl Bot {
    /// Is a guard watching `me` (in its frontal arc, close, with a clear line)?
    fn watched(me: &PlayerState, others: &[PlayerState], world: &dyn WorldQuery) -> bool {
        others.iter().filter(|g| g.alive && g.kind == PawnKind::Guard).any(|g| {
            let to = me.position - g.position;
            let d = to.length();
            if d > contract::WITNESS_RANGE || d < 1.0 {
                return false;
            }
            let ang = forward_vector(g.yaw).truncate().normalize_or_zero().dot(to.truncate().normalize_or_zero()).clamp(-1.0, 1.0).acos().to_degrees();
            if ang > contract::WITNESS_HALF_ARC && d > 200.0 {
                return false;
            }
            let eye = g.position + Vec3::Z * EYE;
            let seg = me.position + Vec3::Z * 40.0 - eye;
            let l = seg.length();
            world.raycast(eye, seg / l, l, channel::VISION).is_none_or(|h| h.distance >= l - 40.0)
        })
    }

    /// A guard charging the bot (running toward it, close): the position to run from.
    fn charging_guard(me: &PlayerState, others: &[PlayerState]) -> Option<Vec3> {
        others
            .iter()
            .filter(|g| g.alive && g.kind == PawnKind::Guard)
            .filter(|g| {
                let to = (me.position - g.position).truncate();
                let d = to.length();
                d < contract::CHARGE_RANGE && d > 1.0 && g.velocity.truncate().dot(to / d) > contract::CHARGE_SPEED
            })
            .map(|g| g.position)
            .min_by(|a, b| a.distance(me.position).total_cmp(&b.distance(me.position)))
    }

    /// The back assassination is lined up: the bot stands behind `t`, close, and no other pawn
    /// would be taken instead (the sim assassinates the nearest pawn in the context's geometry).
    fn back_open(me: &PlayerState, t: &PlayerState, others: &[PlayerState]) -> bool {
        let lined = |v: &PlayerState| {
            let to_me = (me.position - v.position).truncate();
            let d = to_me.length();
            let back = -forward_vector(v.yaw).truncate().normalize_or_zero();
            d <= contract::BACK_REACH && (v.position.z - me.position.z).abs() < 60.0 && back.dot(to_me / d.max(1.0)).clamp(-1.0, 1.0).acos().to_degrees() <= contract::BACK_HALF_ARC
        };
        if !lined(t) {
            return false;
        }
        let dt = t.position.distance(me.position);
        !others.iter().any(|o| o.id != t.id && o.id != me.id && o.alive && o.position.distance(me.position) < dt + 20.0 && o.position.distance(me.position) < 220.0)
    }

    /// Nothing but the target is within sword reach in front of the bot.
    fn clear_swing(me: &PlayerState, t: &PlayerState, others: &[PlayerState]) -> bool {
        let fwd = (t.position - me.position).truncate().normalize_or_zero();
        let dt = t.position.distance(me.position);
        !others.iter().any(|o| {
            if o.id == t.id || o.id == me.id || !o.alive {
                return false;
            }
            let to = (o.position - me.position).truncate();
            let d = to.length();
            d < dt + 60.0 && d < 260.0 && fwd.dot(to / d.max(1.0)) > 0.3
        })
    }

    /// Contracts behaviour (test bot). Bots know their target's identity (humans must infer
    /// it); they exist to exercise the rules, not to be fair. They play the way the mode is
    /// meant to be played: walk among the guests, line up a silent back assassination, never
    /// strike in front of a guard (a seen attack makes every guard who sees it hostile), never
    /// swing with a bystander in the way, and run from a guard charging them. A duelist falls
    /// back to the sword and a gunner to the pistol when no back opening comes.
    pub fn think_contract(&mut self, me: &PlayerState, others: &[PlayerState], world: &dyn WorldQuery, target: Option<PlayerId>) -> PlayerInput {
        self.tick += 1;
        let mut input = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Blink, ..Default::default() };
        if !me.alive {
            self.close_ticks = 0;
            self.flee_ticks = 0;
            return input;
        }
        // Stuck: hop, sidestep and replan.
        if me.position.distance(self.last_pos) < 0.5 {
            self.stuck_ticks += 1;
        } else {
            self.stuck_ticks = 0;
        }
        self.last_pos = me.position;
        let unstick = self.stuck_ticks > 30;
        if unstick {
            self.stuck_ticks = 0;
            self.wander_yaw = self.rand() * 360.0;
            self.blink_hold = 20;
            self.route.force_replan();
        }
        if self.blink_hold > 0 {
            // Sidestep out of whatever we are wedged against.
            self.blink_hold -= 1;
            input.yaw = self.wander_yaw;
            input.forward = 127;
            if self.blink_hold == 19 {
                input.buttons |= buttons::JUMP;
            }
            self.keep_safe(me, world, &mut input, false);
            return input;
        }
        // Run from a guard charging us (along the navmesh, away from it).
        if let Some(g) = Self::charging_guard(me, others) {
            self.flee_ticks = contract::FLEE_TICKS;
            self.flee_from = g;
        }
        if self.flee_ticks > 0 {
            self.flee_ticks -= 1;
            let away = (me.position - self.flee_from).truncate().normalize_or_zero();
            let goal = me.position + away.extend(0.0) * 900.0;
            let steer = self.route.steer(world, me.position, goal);
            let to = if self.route.failed { goal - me.position } else { steer - me.position };
            input.yaw = to.y.atan2(to.x).to_degrees();
            input.forward = 127;
            input.buttons |= buttons::SPRINT;
            self.keep_safe(me, world, &mut input, false);
            return input;
        }
        let Some(t) = target.and_then(|id| others.iter().find(|o| o.id == id && o.alive)) else {
            // No contract yet: stroll like a civilian.
            self.close_ticks = 0;
            if self.tick % 240 == 0 {
                self.wander_yaw += (self.rand() - 0.5) * 140.0;
            }
            input.yaw = self.wander_yaw;
            input.forward = 100;
            input.buttons |= buttons::WALK;
            self.keep_safe(me, world, &mut input, false);
            return input;
        };
        let dist = (t.position - me.position).truncate().length();
        let face = |input: &mut PlayerInput| {
            let tt = t.position - me.position;
            input.yaw = tt.y.atan2(tt.x).to_degrees();
            input.pitch = (tt.z / tt.truncate().length().max(1.0)).atan().to_degrees().clamp(-30.0, 30.0);
        };
        let watched = dist < 400.0 && Self::watched(me, others, world);
        if dist < 400.0 {
            self.close_ticks += 1;
        } else {
            self.close_ticks = 0;
        }
        // Strike: a lined-up back assassination, or after a long wait a frontal attack; never
        // while a guard watches.
        if !watched && Self::back_open(me, t, others) {
            face(&mut input);
            input.forward = 30;
            if self.tick % 10 == 0 {
                input.buttons |= buttons::PRIMARY;
            }
            self.keep_safe(me, world, &mut input, false);
            return input;
        }
        let frontal = !watched && self.close_ticks > contract::OPENING_TICKS;
        if frontal && self.style == BotStyle::Gunner && dist < 700.0 && Self::clear_swing(me, t, &[]) {
            face(&mut input);
            input.left_hand = LeftHand::Pistol;
            if self.fire_cooldown == 0 {
                input.buttons |= buttons::SECONDARY;
                self.fire_cooldown = 50;
            } else {
                self.fire_cooldown -= 1;
            }
            self.keep_safe(me, world, &mut input, false);
            return input;
        }
        if frontal && dist < 160.0 && Self::clear_swing(me, t, others) {
            face(&mut input);
            if self.tick % 20 == 0 {
                input.buttons |= buttons::PRIMARY;
            }
            self.keep_safe(me, world, &mut input, false);
            return input;
        }
        // Approach a point behind the target; while watched, hang back at talking distance.
        let back = -forward_vector(t.yaw).truncate().normalize_or_zero().extend(0.0);
        let behind = t.position + back * if watched { 320.0 } else { 90.0 };
        let goal = if dist > 250.0 { self.route.steer(world, me.position, behind) } else { behind };
        let to = goal - me.position;
        input.yaw = to.y.atan2(to.x).to_degrees();
        if (behind - me.position).truncate().length() < 30.0 {
            face(&mut input);
        } else if dist > 1500.0 {
            input.forward = 127;
            input.buttons |= buttons::SPRINT;
        } else {
            input.forward = 127;
            input.buttons |= buttons::WALK;
        }
        self.keep_safe(me, world, &mut input, false);
        input
    }
}
