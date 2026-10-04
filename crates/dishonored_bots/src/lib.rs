//! Test bots. They only see what a client sees (player states) and only act through
//! `PlayerInput`, so they exercise exactly the same paths as humans: prediction,
//! server validation, combat resolution.

use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::state::{PlayerId, PlayerState};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;

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
    pub target: Option<PlayerId>,
}

impl Bot {
    pub fn new(id: PlayerId, style: BotStyle, seed: u64) -> Self {
        Self { id, style, rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, tick: 0, wander_yaw: (seed % 360) as f32, last_pos: Vec3::ZERO, stuck_ticks: 0, blink_hold: 0, fire_cooldown: 0, target: None }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Decide this tick's input from the bot's own state and the visible players.
    pub fn think(&mut self, me: &PlayerState, others: &[PlayerState], world: &dyn WorldQuery) -> PlayerInput {
        self.tick += 1;
        let mut input = PlayerInput { yaw: me.yaw, pitch: 0.0, left_hand: LeftHand::Blink, ..Default::default() };
        if !me.alive {
            return input;
        }
        // Stuck detection -> jump and pick a new heading.
        if me.position.distance(self.last_pos) < 1.0 {
            self.stuck_ticks += 1;
        } else {
            self.stuck_ticks = 0;
        }
        self.last_pos = me.position;
        if self.stuck_ticks > 30 {
            self.wander_yaw = self.rand() * 360.0;
            self.stuck_ticks = 0;
            input.buttons |= buttons::JUMP;
        }

        let eye = me.position + Vec3::Z * 85.0;
        let visible = |o: &PlayerState| -> bool {
            let to = o.position + Vec3::Z * 60.0 - eye;
            let d = to.length();
            d < 4000.0 && world.raycast(eye, to, d, channel::VISION).is_none_or(|h| h.distance >= d - 40.0)
        };
        let target = if self.style == BotStyle::Wanderer {
            None
        } else {
            others.iter().filter(|o| o.alive && o.id != me.id && visible(o)).min_by(|a, b| a.position.distance(me.position).total_cmp(&b.position.distance(me.position)))
        };
        self.target = target.map(|t| t.id);

        match target {
            None => {
                if self.tick % 180 == 0 {
                    self.wander_yaw += (self.rand() - 0.5) * 120.0;
                }
                input.yaw = self.wander_yaw;
                input.forward = 100;
                if self.rand() < 0.003 {
                    input.buttons |= buttons::JUMP;
                }
            }
            Some(t) => {
                let to = t.position - me.position;
                let dist = to.truncate().length();
                let aim = t.position + Vec3::Z * 50.0 - eye;
                input.yaw = to.y.atan2(to.x).to_degrees();
                input.pitch = (aim.z / aim.truncate().length().max(1.0)).atan().to_degrees();
                match self.style {
                    BotStyle::Duelist => {
                        input.left_hand = LeftHand::Blink;
                        if dist > 160.0 {
                            input.forward = 127;
                            if dist > 700.0 {
                                input.buttons |= buttons::SPRINT;
                            }
                            // Occasionally Blink in when far.
                            if dist > 600.0 && me.vitals.mana >= 20.0 && (self.blink_hold > 0 || self.rand() < 0.01) {
                                self.blink_hold += 1;
                                if self.blink_hold < 12 {
                                    input.buttons |= buttons::SECONDARY;
                                } else {
                                    self.blink_hold = 0;
                                }
                            }
                        } else {
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
                        input.forward = if dist > 1200.0 { 100 } else if dist < 400.0 { -100 } else { 0 };
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
        }
        input
    }
}
