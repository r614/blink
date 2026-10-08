//! The combat behaviours of the special NPC classes (research/dishonored/systems/ai.md F6): the
//! behaviour in the Combat slot of the pawn's brain tweak ([`NpcClass`]). Each runs in place of
//! the elite guard's combat when the brain's `CombatBegin` places it.
//!
//! - **`DisBehaviorCombatWolfhound`**: chase at the run loop, walk within `m_fWalkDistance`,
//!   bite with the bite's contexts under ManageAttacks pacing (flurries, stamina, aggression).
//!   An unreachable target is held at long distance (0x00b13a80 / 0x00b14010).
//! - **`DisBehaviorWeep`**: follow the target (walk), run once fewer than
//!   `m_iSimultaneousRunnerAllowed` weepers run (0x00afbe30) and grab
//!   (`DisItemContext_WeeperGrab`); weep standing after a grab; cough a plague cloud every
//!   `m_fCoughingCooldown`; give up when the target stays beyond `m_fHomeRadius` for
//!   `m_LostTimer` (0x00af6bd0).
//! - **`DisBehaviorMusicalCombat`**: MaintainDistance closes to the protection tune's
//!   `m_fIdealContextRange` and never backs away (0x00b2e600); each refresh tries the bash, else
//!   the combat tune when the target is seen (0x00b13a20); the tune stops after
//!   `m_fStopWhenNotSeeingTimer` unseen or `m_fStopWhenOutOfRangeTimer` out of range (0x00c2b680)
//!   and waits its `m_ContextCooldown`.
//! - **`DisBehaviorTallBoyCombat`**: FindShootingPosition until the bow can fire (0x00b475e0);
//!   ManageAttacks picks the first context of the bow that can be used: the bow, else the leg
//!   stomp (0x00b470d0); the shot waits for the aim alignment (`StateNPCMasterTallboyAim`) and hits
//!   with the accuracy of its range (0x00b732e0), aimed at the ground under the target with lead.

use super::*;
use dishonored_ir::ai::{ManageAttacksConfig, SpecialAttack};

/// The weeper runner registration (`inhibit` entry code; the original's runner count 0x00afbe30).
const WEEPER_RUNNER: u8 = 255;
/// The distance a weeper that stands after a grab resumes the chase beyond (2 x the follow
/// sub-state's arrival threshold, 0x00aff9e0).
const WEEPER_RESUME_FACTOR: f32 = 2.0;

/// Weeper sub-states (`DisBehaviorWeep`).
pub mod weep {
    pub const FOLLOW: u8 = 0;
    pub const GRAB: u8 = 1;
    pub const STAND: u8 = 2;
}

/// State of the special combat behaviours (one at a time, per the brain's class).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpecialCombat {
    /// The behaviour started (its timers rolled).
    pub started: bool,
    /// Sub-state of the running combat behaviour (class-specific).
    pub sub: u8,
    /// Sub-state timer.
    pub timer: f32,
    /// Per-context cooldowns (the class's attacks, in config order).
    pub cooldowns: Vec<f32>,
    /// Overseer: the combat tune plays; its effect clock and shutdown timers (-1 off).
    pub playing: bool,
    pub effect_timer: f32,
    pub out_of_sight: f32,
    pub out_of_range: f32,
    /// Cooldown of the tune (Overseer) or the bow (Tallboy).
    pub ranged_cd: f32,
    /// Tallboy: aim alignment time; FindShootingPosition destination, progress timer and the
    /// position progress is measured from.
    pub aim_timer: f32,
    pub fsp_goal: Option<Vec3>,
    pub fsp_timer: f32,
    pub fsp_from: Vec3,
    /// Weeper: running (the grab variation), the cough cooldown, the plague cloud left and its
    /// period, the lost timer and its limit.
    pub running: bool,
    pub cough_cd: f32,
    pub plague_left: f32,
    pub plague_tick: f32,
    pub lost: f32,
    pub lost_limit: f32,
}

/// What the combat behaviour knows of its target this tick.
pub(crate) struct Target<'a> {
    pub t: &'a PlayerState,
    pub sighted: bool,
    pub visible: bool,
    pub goal: Vec3,
    pub d: f32,
    pub dz: f32,
}

/// Angle (degrees) between the pawn's facing and the direction to `p`, in the plane.
fn off_facing(me: &PlayerState, p: Vec3) -> f32 {
    let to = (p - me.position).truncate().normalize_or_zero();
    let f = glam::Vec2::new(me.yaw.to_radians().cos(), me.yaw.to_radians().sin());
    f.dot(to).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Launch pitch (degrees, up positive) that carries a projectile at `speed` under `gravity`
/// (negative, uu/s^2) over horizontal distance `x` to height `y` (the low arc); 45 degrees when
/// out of reach.
pub fn ballistic_pitch(speed: f32, gravity: f32, x: f32, y: f32) -> f32 {
    let g = -gravity;
    if g <= 1e-3 || x <= 1e-3 {
        return (y / x.max(1e-3)).atan().to_degrees();
    }
    let v2 = speed * speed;
    let disc = v2 * v2 - g * (g * x * x + 2.0 * y * v2);
    if disc < 0.0 {
        return 45.0;
    }
    ((v2 - disc.sqrt()) / (g * x)).atan().to_degrees()
}

impl Brain {
    /// The special class's combat behaviour (`None`: the elite guard's runs).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn special_combat(&mut self, me: &PlayerState, tg: &Target, ws: &WorldState, world: &dyn WorldQuery, nav: Option<&NavWorld>, ai: &AiConfig, inhibit: &mut Vec<(PlayerId, u8, f32)>, dt: f32) -> Option<PlayerInput> {
        match self.class {
            NpcClass::Wolfhound => Some(self.combat_hound(me, tg, nav, ai, inhibit, dt)),
            NpcClass::Weeper => Some(self.combat_weep(me, tg, nav, ai, inhibit, dt)),
            NpcClass::MusicalOverseer => Some(self.combat_musical(me, tg, nav, ai, inhibit, dt)),
            NpcClass::Tallboy => Some(self.combat_tallboy(me, tg, ws, world, nav, ai, inhibit, dt)),
            _ => None,
        }
    }

    /// ManageAttacks when the behaviour starts (0x00b3dc10) and each tick: the aggression roll and
    /// regen, the flurry / stamina pacing (0x00b47880), the cooldowns.
    fn manage_tick(&mut self, m: &ManageAttacksConfig, n: usize, dt: f32) {
        if !self.special.started {
            self.special.started = true;
            self.flurry = combat::roll_i(m.attacks_per_flurry, rng_next(&mut self.rng));
            self.stamina_timer = combat::roll(m.stamina_recovery, rng_next(&mut self.rng));
            self.aggression = rng_next(&mut self.rng);
        }
        self.special.cooldowns.resize(n, 0.0);
        for c in self.special.cooldowns.iter_mut() {
            *c = (*c - dt).max(0.0);
        }
        self.aggression = (self.aggression + m.aggression_regen_rate * dt).clamp(0.0, 1.0);
        if self.flurry < 1 && self.attack == 0 {
            if self.stamina_timer < 0.0 {
                self.flurry = combat::roll_i(m.attacks_per_flurry, rng_next(&mut self.rng));
                self.stamina_timer = combat::roll(m.stamina_recovery, rng_next(&mut self.rng));
            } else {
                self.stamina_timer -= dt;
            }
        }
    }

    /// The first attack that can be used now: off cooldown, not inhibited on the target, in range,
    /// height and facing angle (`DisItemContext` CanDo).
    fn pick_special(&self, attacks: &[SpecialAttack], me: &PlayerState, tg: &Target, inhibit: &[(PlayerId, u8, f32)]) -> Option<usize> {
        let ang = off_facing(me, tg.t.position);
        (0..attacks.len()).find(|&k| {
            let a = &attacks[k];
            self.special.cooldowns.get(k).is_none_or(|c| *c <= 0.0) && !inhibit.iter().any(|x| x.0 == tg.t.id && x.1 == a.kind) && tg.d >= a.min_range && tg.d <= a.max_range && tg.dz.abs() <= a.max_height && ang <= a.angle
        })
    }

    /// Start an attack context: its cooldown (scaled by aggression, 0x00b3deb0), the group
    /// inhibition, the flurry count; the swing travels to the sword's reach first.
    fn start_special(&mut self, a: &SpecialAttack, k: usize, m: &ManageAttacksConfig, tg: &Target, inhibit: &mut Vec<(PlayerId, u8, f32)>, ai: &AiConfig, mut i: PlayerInput) -> PlayerInput {
        let scale = (1.0 - self.aggression) * m.cooldown_factor_at_min_aggression + 1.0;
        self.special.cooldowns[k] = combat::roll(a.cooldown, rng_next(&mut self.rng)) * scale;
        if a.inhibition > 0.0 {
            inhibit.push((tg.t.id, a.kind, a.inhibition));
        }
        self.flurry -= 1;
        self.barks.push(("DDH_COMBAT_ATTACK".into(), Some(tg.t.id)));
        self.attack = a.kind;
        self.attack_timer = ATTACK_WINDOW;
        i.npc_move = a.kind;
        if tg.d <= SWORD_REACH {
            i.buttons |= buttons::PRIMARY;
            self.attack = 0;
        } else {
            i.forward = 127;
            i.buttons |= buttons::SPRINT;
            // The attack's travel at the pawn's run loop (its root motion is not read).
            i.npc_speed = self.loco_speed(ai, true);
            if a.kind == npc_moves::WH_JUMP_ATTACK {
                i.buttons |= buttons::JUMP;
            }
        }
        i
    }

    /// `DisBehaviorCombatWolfhound`.
    fn combat_hound(&mut self, me: &PlayerState, tg: &Target, nav: Option<&NavWorld>, ai: &AiConfig, inhibit: &mut Vec<(PlayerId, u8, f32)>, dt: f32) -> PlayerInput {
        let h = &ai.special.wolfhound;
        self.manage_tick(&h.manage, h.attacks.len(), dt);
        let face = yaw_to(me.position, tg.goal);
        let i = PlayerInput { yaw: face, left_hand: LeftHand::Empty, ..Default::default() };
        if let Some(out) = self.attack_in_progress(me, tg.d, tg.sighted, ai, dt, i) {
            return out;
        }
        // Unreachable: CombatLongDistance (hold, bark; the bark context is not ported).
        if nav.is_some() && self.follow.failed {
            self.follow.clear();
            return i;
        }
        if tg.sighted && self.flurry > 0 && me.sword.phase == SwordPhase::Idle {
            if let Some(k) = self.pick_special(&h.attacks, me, tg, inhibit) {
                let a = h.attacks[k].clone();
                return self.start_special(&a, k, &h.manage, tg, inhibit, ai, i);
            }
        }
        // MeleeChase / Engage: to the attack pattern's ideal range (the medium bite's), walking
        // within `m_fWalkDistance`.
        let bite = h.attacks.iter().find(|a| a.kind == npc_moves::WH_BITE);
        let ideal = bite.map_or(200.0, |a| a.ideal_range.max(1.0));
        if tg.sighted && bite.is_some_and(|b| tg.d < b.min_range) {
            // Inside the bite's range: back to the pattern position at its ideal range.
            let mut b = i;
            b.forward = -127;
            b.npc_speed = self.loco_speed(ai, false);
            return b;
        }
        if !tg.sighted || tg.d > ideal {
            let run = tg.d > h.walk_distance || !tg.sighted;
            let mut m = self.move_to(me, nav, tg.goal, run, ai);
            if tg.sighted && tg.d < 400.0 {
                m.yaw = face;
            }
            return m;
        }
        i
    }

    /// `DisBehaviorWeep`.
    fn combat_weep(&mut self, me: &PlayerState, tg: &Target, nav: Option<&NavWorld>, ai: &AiConfig, inhibit: &mut Vec<(PlayerId, u8, f32)>, dt: f32) -> PlayerInput {
        let w = &ai.special.weeper;
        if !self.special.started {
            // OnStart (0x00afe030): the Weep variation, follow at walk, the cough timer.
            self.special.started = true;
            self.special.sub = weep::FOLLOW;
            self.special.cough_cd = combat::roll(w.coughing_cooldown, rng_next(&mut self.rng));
            self.special.lost_limit = combat::roll(w.lost_timer, rng_next(&mut self.rng));
            self.special.timer = w.time_before_running;
        }
        let face = yaw_to(me.position, tg.goal);
        let mut i = PlayerInput { yaw: face, left_hand: LeftHand::Empty, ..Default::default() };
        // The cough: a plague cloud of `m_fPlagueDmgLength`, a damage tick every
        // `m_fPlagueDmgePeriod` from its start (0x00c70e10 / 0x00c86ea0), the cooldown again after.
        let mut pulse = false;
        if self.special.plague_left > 0.0 {
            self.special.plague_tick -= dt;
            self.special.plague_left -= dt;
            if self.special.plague_tick <= 0.0 {
                self.special.plague_tick += w.plague_period;
                pulse = true;
            }
            if self.special.plague_left <= 0.0 {
                self.special.cough_cd = combat::roll(w.coughing_cooldown, rng_next(&mut self.rng));
            }
        } else {
            self.special.cough_cd -= dt;
            if self.special.cough_cd <= 0.0 {
                self.special.plague_left = combat::roll(w.plague_length, rng_next(&mut self.rng));
                self.special.plague_tick = w.plague_period;
            }
        }
        let cough = |mut i: PlayerInput| {
            if pulse {
                i.npc_move = npc_moves::WEEPER_COUGH;
                i.buttons |= buttons::SECONDARY;
            }
            i
        };
        // The leash (0x00af6bd0): the target beyond `m_fHomeRadius` for the lost timer gives up.
        if tg.t.position.distance(self.home) > w.home_radius {
            self.special.lost += dt;
            if self.special.lost >= self.special.lost_limit {
                self.target = None;
                self.complete(slot::COMBAT);
                return cough(i);
            }
        } else {
            self.special.lost = 0.0;
        }
        match self.special.sub {
            weep::GRAB => {
                i.npc_move = npc_moves::WEEPER_GRAB;
                self.special.timer -= dt;
                if self.special.timer <= 0.0 {
                    // After the grab (0x00afbe20): weep standing.
                    self.special.sub = weep::STAND;
                    self.special.timer = combat::roll(w.stand_still, rng_next(&mut self.rng));
                }
                cough(i)
            }
            weep::STAND => {
                self.special.timer -= dt;
                if self.special.timer <= 0.0 {
                    if tg.d > w.follow_threshold * WEEPER_RESUME_FACTOR {
                        // Resume the chase at walk (0x00aff9e0).
                        self.special.sub = weep::FOLLOW;
                        self.special.running = false;
                        self.special.timer = w.time_before_running;
                    } else {
                        self.special.timer = combat::roll(w.stand_still, rng_next(&mut self.rng));
                    }
                }
                cough(i)
            }
            _ => {
                // The runner limiter (0x00afbe30): at most `m_iSimultaneousRunnerAllowed` weepers
                // run; one that may run after `m_TimeBeforeRunning` switches to the grab variation.
                if !self.special.running {
                    self.special.timer -= dt;
                    let runners = inhibit.iter().filter(|x| x.1 == WEEPER_RUNNER && x.0 != self.id).count();
                    if self.special.timer <= 0.0 && (runners as u32) < w.runners_allowed {
                        self.special.running = true;
                    }
                }
                if self.special.running {
                    inhibit.retain(|x| !(x.1 == WEEPER_RUNNER && x.0 == self.id));
                    inhibit.push((self.id, WEEPER_RUNNER, 2.0 * dt));
                    // The grab (`DisItemContext_WeeperGrab`): range, feet height difference, angle,
                    // and the combat manager's inhibition on the target.
                    let feet = (tg.t.position.z - me.position.z).abs();
                    if tg.sighted && tg.d <= w.grab_range && feet <= w.grab_height && off_facing(me, tg.t.position) <= w.grab_angle && !inhibit.iter().any(|x| x.0 == tg.t.id && x.1 == npc_moves::WEEPER_GRAB) {
                        inhibit.push((tg.t.id, npc_moves::WEEPER_GRAB, w.grab_inhibition));
                        self.barks.push(("DDH_COMBAT_ATTACK".into(), Some(tg.t.id)));
                        self.special.sub = weep::GRAB;
                        self.special.timer = w.grab_duration;
                        i.npc_move = npc_moves::WEEPER_GRAB;
                        return cough(i);
                    }
                }
                if tg.d <= w.follow_threshold && tg.sighted {
                    self.special.sub = weep::STAND;
                    self.special.timer = combat::roll(w.stand_still, rng_next(&mut self.rng));
                    return cough(i);
                }
                let run = self.special.running;
                cough(self.move_to(me, nav, tg.goal, run, ai))
            }
        }
    }

    /// `DisBehaviorMusicalCombat`.
    fn combat_musical(&mut self, me: &PlayerState, tg: &Target, nav: Option<&NavWorld>, ai: &AiConfig, inhibit: &mut Vec<(PlayerId, u8, f32)>, dt: f32) -> PlayerInput {
        let o = &ai.special.overseer;
        let manage = ManageAttacksConfig { attacks_per_flurry: [i32::MAX / 2; 2], stamina_recovery: [0.0; 2], aggression_regen_rate: 0.0, aggression_delta_per_damage: 0.0, cooldown_factor_at_min_aggression: 0.0 };
        self.manage_tick(&manage, o.attacks.len(), dt);
        let face = yaw_to(me.position, tg.goal);
        let mut i = PlayerInput { yaw: face, left_hand: LeftHand::Empty, ..Default::default() };
        if let Some(out) = self.attack_in_progress(me, tg.d, tg.sighted, ai, dt, i) {
            return out;
        }
        // The bash first (0x00b13990), also while the tune plays (0x00b13a50).
        if me.sword.phase == SwordPhase::Idle {
            if let Some(k) = self.pick_special(&o.attacks, me, tg, inhibit) {
                self.special.playing = false;
                let a = o.attacks[k].clone();
                return self.start_special(&a, k, &manage, tg, inhibit, ai, i);
            }
        }
        let seen = tg.sighted && tg.visible;
        let in_range = tg.d >= o.tune.min_range && tg.d < o.tune.max_range && off_facing(me, tg.t.position) <= o.tune.angle;
        if self.special.playing {
            // The shutdown timers (0x00c2b680): first frame out sets them, then they run down.
            let run = |t: &mut f32, ok: bool, set: f32| {
                if ok {
                    *t = -1.0;
                    false
                } else if (*t + 1.0).abs() < 0.01 {
                    *t = set;
                    false
                } else {
                    *t = (*t - dt).max(0.0);
                    *t <= 1e-8
                }
            };
            let a = run(&mut self.special.out_of_sight, seen, o.stop_not_seeing);
            let b = run(&mut self.special.out_of_range, in_range, o.stop_out_of_range);
            if a || b {
                self.special.playing = false;
                self.special.ranged_cd = combat::roll(o.tune.cooldown, rng_next(&mut self.rng));
            } else {
                // The tune's effect clock (0x00c6a240, `m_fMusicEffectPeriod`).
                i.npc_move = npc_moves::OVERSEER_TUNE;
                self.special.effect_timer -= dt;
                if self.special.effect_timer <= 0.0 {
                    self.special.effect_timer += o.tune_period.max(dt);
                    i.buttons |= buttons::SECONDARY;
                }
                return i;
            }
        }
        self.special.ranged_cd -= dt;
        if self.special.ranged_cd <= 0.0 && seen && in_range {
            self.special.playing = true;
            self.special.effect_timer = o.tune_period;
            self.special.out_of_sight = -1.0;
            self.special.out_of_range = -1.0;
            self.barks.push(("DDH_COMBAT_ATTACK".into(), Some(tg.t.id)));
            i.npc_move = npc_moves::OVERSEER_TUNE;
            return i;
        }
        // MaintainDistance (0x00b2e600): close in at Jog to the ideal distance, stop inside it;
        // an unseen target is looked for at its proxy.
        if !tg.sighted || tg.d >= o.ideal_distance {
            return self.move_to(me, nav, tg.goal, true, ai);
        }
        i
    }

    /// `DisBehaviorTallBoyCombat`.
    #[allow(clippy::too_many_arguments)]
    fn combat_tallboy(&mut self, me: &PlayerState, tg: &Target, ws: &WorldState, world: &dyn WorldQuery, nav: Option<&NavWorld>, ai: &AiConfig, inhibit: &mut Vec<(PlayerId, u8, f32)>, dt: f32) -> PlayerInput {
        let tb = &ai.special.tallboy;
        self.manage_tick(&tb.manage, tb.attacks.len(), dt);
        self.special.ranged_cd -= dt;
        let face = yaw_to(me.position, tg.goal);
        let i = PlayerInput { yaw: face, left_hand: LeftHand::Empty, ..Default::default() };
        if let Some(out) = self.attack_in_progress(me, tg.d, tg.sighted, ai, dt, i) {
            return out;
        }
        let eye = me.position + Vec3::Z * EYE;
        let line_clear = |to: Vec3| {
            let seg = to - eye;
            let len = seg.length();
            !world.raycast(eye, seg, len, channel::PROJECTILE).is_some_and(|h| h.distance < len - 40.0)
        };
        // The bow's CanDo (0x00c25240): range, facing angle, no ally of this pawn within
        // `m_fAlliesHoldFireRadius` of the target; a clear line (the shooting position's test).
        let b = &tb.bow;
        let allies_near = self.allies.iter().filter_map(|a| ws.player(*a)).any(|a| a.alive && a.id != self.id && a.position.distance(tg.t.position) <= tb.allies_hold_fire_radius);
        let bow_range = tg.d >= b.min_range && tg.d <= b.max_range;
        let can_shoot = tg.sighted && bow_range && !allies_near && line_clear(tg.t.position);
        if can_shoot && self.special.ranged_cd <= 0.0 {
            self.special.fsp_goal = None;
            let mut out = i;
            out.npc_move = npc_moves::TALLBOY_FIRE_BOW;
            // The aim state: aligned within `m_fAimAlignYawDegrees` for `m_fAimAlignTimeBeforeShooting`.
            if off_facing(me, tg.t.position) <= tb.aim_align_deg.min(b.angle) {
                self.special.aim_timer += dt;
            } else {
                self.special.aim_timer = 0.0;
            }
            let (yaw, pitch) = self.tallboy_aim(me, tg, world, tb);
            out.yaw = yaw;
            out.pitch = pitch;
            if self.special.aim_timer >= tb.aim_time {
                out.buttons |= buttons::SECONDARY;
                self.special.aim_timer = 0.0;
                let scale = (1.0 - self.aggression) * tb.manage.cooldown_factor_at_min_aggression + 1.0;
                self.special.ranged_cd = combat::roll(b.cooldown, rng_next(&mut self.rng)) * scale;
                self.barks.push(("DDH_COMBAT_RANGED_BEGIN".into(), Some(tg.t.id)));
            }
            return out;
        }
        self.special.aim_timer = 0.0;
        // The leg stomp, when the bow cannot be used (it comes after the bow in the slot order).
        if tg.sighted && !(can_shoot) && self.flurry > 0 && me.sword.phase == SwordPhase::Idle {
            if let Some(k) = self.pick_special(&tb.attacks, me, tg, inhibit) {
                let a = tb.attacks[k].clone();
                return self.start_special(&a, k, &tb.manage, tg, inhibit, ai, i);
            }
        }
        if can_shoot {
            // Waiting out the bow's cooldown where it can shoot.
            return i;
        }
        // FindShootingPosition (0x00b46960): the point at the bow's ideal range on the line from
        // the target, on the navmesh, with a clear line to the target, at walk; given up after
        // `m_fTimeBeforeGivingUp` without progress.
        if self.special.fsp_goal.is_none() {
            let away = (me.position - tg.t.position).truncate().normalize_or(glam::Vec2::X);
            let p = tg.t.position + Vec3::new(away.x, away.y, 0.0) * b.ideal_range.max(b.min_range);
            let cand = match nav {
                Some(n) => n.closest_point(p, 600.0).map(|(_, q)| q + Vec3::Z * 90.0),
                None => Some(p),
            };
            let ok = cand.filter(|c| {
                let from = *c + Vec3::Z * EYE;
                let seg = tg.t.position - from;
                let len = seg.length();
                c.truncate().distance(me.position.truncate()) > tb.fsp_avoidance && !world.raycast(from, seg, len, channel::PROJECTILE).is_some_and(|h| h.distance < len - 40.0)
            });
            self.special.fsp_goal = ok.or(Some(tg.goal));
            self.special.fsp_timer = tb.fsp_give_up;
            self.special.fsp_from = me.position;
        }
        let goal = self.special.fsp_goal.unwrap_or(tg.goal);
        if me.position.distance(self.special.fsp_from) > 30.0 {
            self.special.fsp_from = me.position;
            self.special.fsp_timer = tb.fsp_give_up;
        } else {
            self.special.fsp_timer -= dt;
        }
        if self.special.fsp_timer <= 0.0 || Self::reached(me, goal, 120.0) || self.follow.failed {
            // PathFailed / arrived: a new search next refresh.
            self.special.fsp_goal = None;
            self.follow.clear();
            return i;
        }
        let mut m = self.move_to(me, nav, goal, false, ai);
        if tg.sighted {
            m.yaw = face;
        }
        m
    }

    /// The bow's launch direction (0x00c2fab0): a hit (chance from `Attribute_MaxAccuracy` at the
    /// bow's min range to `Attribute_MinAccuracy` at its max range, 0x00b732e0) aims at the ground
    /// under the target with its velocity's lead; a miss aims off the target within
    /// `m_fCheatAngleDeviation`. The pitch solves the arrow's arc.
    fn tallboy_aim(&mut self, me: &PlayerState, tg: &Target, world: &dyn WorldQuery, tb: &dishonored_ir::ai::TallboyConfig) -> (f32, f32) {
        let b = &tb.bow;
        let eye = me.position + Vec3::Z * EYE;
        let span = (b.max_range - b.min_range).max(1.0);
        let k = ((tg.d - b.min_range) / span).clamp(0.0, 1.0);
        let chance = tb.accuracy[0] + (tb.accuracy[1] - tb.accuracy[0]) * k;
        let hit = rng_next(&mut self.rng) <= chance;
        // `m_TargetRegion` Ground: 300 uu down from the target.
        let ground = world.raycast(tg.t.position, -Vec3::Z, 300.0, channel::PAWN).map_or(tg.t.position, |h| tg.t.position - Vec3::Z * h.distance);
        let gravity = tb.gravity_z * tb.arrow_gravity_mult;
        let solve = |p: Vec3| {
            let x = (p - eye).truncate().length();
            (yaw_to(eye, p), ballistic_pitch(tb.arrow_speed, gravity, x, p.z - eye.z))
        };
        let mut aim = ground;
        // The lead: the target's horizontal velocity over the flight time, solved again.
        let (_, p0) = solve(aim);
        let flight = (aim - eye).truncate().length() / (tb.arrow_speed * p0.to_radians().cos()).max(1.0);
        aim += Vec3::new(tg.t.velocity.x, tg.t.velocity.y, 0.0) * flight;
        let (mut yaw, pitch) = solve(aim);
        if !hit {
            yaw += (rng_next(&mut self.rng) * 2.0 - 1.0) * tb.miss_angle;
        }
        (yaw, pitch)
    }

    /// A melee move in progress (travel to the sword's reach, then one swing).
    pub(crate) fn attack_in_progress(&mut self, me: &PlayerState, d: f32, sighted: bool, ai: &AiConfig, dt: f32, mut i: PlayerInput) -> Option<PlayerInput> {
        if self.attack == 0 || matches!(self.attack, npc_moves::SIDE_STEP | npc_moves::BACK_STEP) {
            return None;
        }
        self.attack_timer -= dt;
        let kind = self.attack;
        i.npc_move = kind;
        let close = d <= SWORD_REACH;
        if close && me.sword.phase == SwordPhase::Idle {
            i.buttons |= buttons::PRIMARY;
            if kind == npc_moves::ATTACK_STEP_BACK {
                i.forward = -127;
            }
            self.attack = 0;
            return Some(i);
        }
        if self.attack_timer <= 0.0 || !sighted {
            self.attack = 0;
        } else if !close && combat::lunges(kind) {
            i.forward = 127;
            i.buttons |= buttons::SPRINT;
            i.npc_speed = self.loco_speed(ai, true);
            if matches!(kind, npc_moves::JUMP_ATTACK | npc_moves::WH_JUMP_ATTACK) && self.attack_timer > ATTACK_WINDOW - dt * 1.5 {
                i.buttons |= buttons::JUMP;
            }
        }
        Some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_world::{CollisionWorld, Tri};

    fn floor() -> CollisionWorld {
        let s = 20000.0;
        let (a, b, c, d) = (Vec3::new(-s, -s, 0.0), Vec3::new(s, -s, 0.0), Vec3::new(s, s, 0.0), Vec3::new(-s, s, 0.0));
        let tri = |x: Vec3, y: Vec3, z: Vec3| Tri { a: x, b: y, c: z, n: Vec3::Z, channels: channel::ALL, source: 0 };
        CollisionWorld::from_triangles(vec![tri(a, b, c), tri(a, c, d)], vec!["floor".into()])
    }

    fn pawns(d: f32) -> (PlayerState, PlayerState) {
        let mut me = PlayerState::new(NPC_ID_BASE, Vec3::new(0.0, 0.0, 90.0), 0.0);
        me.kind = PawnKind::Guard;
        (me, PlayerState::new(1, Vec3::new(d, 0.0, 90.0), 180.0))
    }

    /// `DisBehaviorMusicalCombat`: the tune starts on a seen target in range, ticks its effect
    /// every `m_fMusicEffectPeriod`, and stops `m_fStopWhenNotSeeingTimer` after the target is
    /// lost (0x00b13920, 0x00c6a240, 0x00c2b680).
    #[test]
    fn musical_combat_plays_and_stops_the_tune() {
        let mut ai = AiConfig::default();
        let o = &mut ai.special.overseer;
        o.tune = SpecialAttack { kind: npc_moves::OVERSEER_TUNE, min_range: 150.0, max_range: 2000.0, angle: 45.0, cooldown: [2.0, 5.0], ..Default::default() };
        o.tune_period = 0.3;
        o.stop_not_seeing = 5.0;
        o.stop_out_of_range = 1.0;
        o.ideal_distance = 1000.0;
        let w = floor();
        let ws = WorldState::default();
        let (me, t) = pawns(1200.0);
        let mut b = Brain::new(me.id, PawnKind::Guard, me.position, 1);
        b.class = NpcClass::MusicalOverseer;
        let dt = 1.0 / 60.0;
        let mut pulses = 0;
        for _ in 0..60 {
            let tg = Target { t: &t, sighted: true, visible: true, goal: t.position, d: 1200.0, dz: 0.0 };
            let i = b.special_combat(&me, &tg, &ws, &w, None, &ai, &mut Vec::new(), dt).unwrap();
            assert_eq!(i.npc_move, npc_moves::OVERSEER_TUNE);
            assert_eq!(i.forward, 0, "the tune is played standing");
            pulses += (i.buttons & buttons::SECONDARY != 0) as u32;
        }
        assert_eq!(pulses, 3, "one effect tick per 0.3 s");
        // Lost from sight: the tune holds 5 s, then stops and waits its cooldown.
        let mut stopped = None;
        for k in 0..400 {
            let tg = Target { t: &t, sighted: false, visible: false, goal: t.position, d: 1200.0, dz: 0.0 };
            let i = b.special_combat(&me, &tg, &ws, &w, None, &ai, &mut Vec::new(), dt).unwrap();
            if i.npc_move != npc_moves::OVERSEER_TUNE {
                stopped = Some(k);
                break;
            }
        }
        let k = stopped.expect("the tune stops");
        assert!((299..=302).contains(&k), "stopped after {k} ticks");
        assert!(b.special.ranged_cd >= 2.0 && b.special.ranged_cd <= 5.0);
    }

    /// `DisBehaviorWeep`: after the cough cooldown, a plague cloud of `m_fPlagueDmgLength` ticks
    /// every `m_fPlagueDmgePeriod` (0x00c70e10, 0x00c86ea0).
    #[test]
    fn weeper_coughs_plague_ticks() {
        let mut ai = AiConfig::default();
        let wc = &mut ai.special.weeper;
        wc.coughing_cooldown = [2.0, 2.0];
        wc.plague_length = [2.0, 2.0];
        wc.plague_period = 0.5;
        let w = floor();
        let ws = WorldState::default();
        let (me, t) = pawns(60.0);
        let mut b = Brain::new(me.id, PawnKind::Guard, me.position, 1);
        b.class = NpcClass::Weeper;
        let dt = 1.0 / 60.0;
        let mut ticks: Vec<i32> = Vec::new();
        for k in 0..300 {
            let tg = Target { t: &t, sighted: true, visible: true, goal: t.position, d: 60.0, dz: 0.0 };
            let i = b.special_combat(&me, &tg, &ws, &w, None, &ai, &mut Vec::new(), dt).unwrap();
            if i.npc_move == npc_moves::WEEPER_COUGH && i.buttons & buttons::SECONDARY != 0 {
                ticks.push(k);
            }
        }
        assert_eq!(ticks.len(), 4, "2 s cloud, 0.5 s period: {ticks:?}");
        assert!(ticks.windows(2).all(|p| (p[1] - p[0]).abs_diff(30) <= 1));
    }

    #[test]
    fn ballistic_pitch_reaches_the_target() {
        // Flat ground, 1200 uu, 2000 uu/s under half of -1500 gravity.
        let g = -750.0;
        let p = ballistic_pitch(2000.0, g, 1200.0, 0.0);
        let (s, c) = p.to_radians().sin_cos();
        let t = 1200.0 / (2000.0 * c);
        let y = 2000.0 * s * t + 0.5 * g * t * t;
        assert!(y.abs() < 1.0, "pitch {p} lands at {y}");
        // No gravity: straight at the target.
        assert!((ballistic_pitch(2000.0, 0.0, 1000.0, 1000.0) - 45.0).abs() < 1e-3);
    }
}
