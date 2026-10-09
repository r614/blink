//! DLC06 (The Knife of Dunwall) gameplay pieces as pure models, ahead of their wiring into the
//! world (the Daud pawn config, ammo, HUD and viewmodel integration are separate work):
//!
//! - **Arc Mine** (`DisDLC06Gadget_ArcMinePlaced`, tick 0x00ce59d0, init 0x00cd7900): arms, fires
//!   instant arcs at targets touching its detection cylinder with a delay between shots, cleans up
//!   when its shots are spent; damaged, it explodes after `m_fTimeToExplodeFromDamage`.
//! - **Saw cut** (`DisDLC06ItemContext_NPCSawCut`, tick 0x00cdb660): the Butcher's saw swing damages
//!   every pawn in a cone in whole points, carrying the fraction over.
//! - **Chokedust** (`DisDLC06Explosion`, tick 0x00cfb780): a lingering cloud re-running its damage
//!   pass every `m_fDamageInterval` while it lives.

use glam::Vec3;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------------
// Arc Mine

/// `EArcMineState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ArcMineState {
    #[default]
    Arming,
    Active,
    FiringCooldown,
    Damaged,
    Exploding,
    PendingDestroy,
}

/// `DisDLC06Tweaks_ArcMinePlaced` (`DLC06_Inv_ArcMine.DLC06_ArcMine_Placed` / `DLC06_StunMine_Placed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArcMineConfig {
    /// `m_fArmingTime`, `m_NumShots`, `m_fDelayBetweenShots`, `m_fTimeToExplodeFromDamage`.
    pub arming_time: f32,
    pub num_shots: i32,
    pub delay_between_shots: f32,
    pub time_to_explode_from_damage: f32,
    /// `m_DamageToPawns`, `m_DamageToNonPawns`, `m_fRatKillingAoE`.
    pub damage_to_pawns: f32,
    pub damage_to_non_pawns: f32,
    pub rat_killing_aoe: f32,
    /// `m_bAttackNonPawns`, `m_bAshDeadPawns` (both off on the Stun Mine, which shoots
    /// `DisDLC06DamageType_ArcMineStunShot`).
    pub attack_non_pawns: bool,
    pub ash_dead_pawns: bool,
    pub stun: bool,
    /// The detection cylinder (`pDetectionCylinder`: radius, half height; Pawn channel only).
    pub detection_radius: f32,
    pub detection_half_height: f32,
}

impl Default for ArcMineConfig {
    fn default() -> Self {
        Self::lethal()
    }
}

impl ArcMineConfig {
    /// `DLC06_ArcMine_Placed`.
    pub fn lethal() -> Self {
        ArcMineConfig {
            arming_time: 2.0,
            num_shots: 2,
            delay_between_shots: 1.0,
            time_to_explode_from_damage: 2.5,
            damage_to_pawns: 50.0,
            damage_to_non_pawns: 50.0,
            rat_killing_aoe: 80.0,
            attack_non_pawns: true,
            ash_dead_pawns: true,
            stun: false,
            detection_radius: 512.0,
            detection_half_height: 256.0,
        }
    }

    /// `DLC06_StunMine_Placed`: the same timing and shots, the stun shot, no non-pawn targets.
    pub fn stun() -> Self {
        ArcMineConfig { attack_non_pawns: false, ash_dead_pawns: false, stun: true, ..Self::lethal() }
    }
}

/// What the mine asks of the world each tick.
pub trait ArcMineWorld {
    type Target: Copy;
    /// A pawn touches the detection cylinder (`Touching.Num > 0`).
    fn touching(&self) -> bool;
    /// The gate before a shot (`*(DAT_0145f614 + 0x38c) & 0x8000 == 0`; UNKNOWN, read as
    /// "gameplay not frozen").
    fn gate_open(&self) -> bool {
        true
    }
    /// `FindTarget` (0x00ce4b50): the base pawn first, else a touching visible pawn that is not
    /// friendly or the player, else (with `m_bAttackNonPawns`) a touching rat or River Krust, else a
    /// random visible rat inside the cylinder.
    fn find_target(&mut self) -> Option<Self::Target>;
    /// `Fire(target)` (0x00ce3120): the shot's trace and damage; false when blocked.
    fn fire(&mut self, target: Self::Target) -> bool;
}

/// What a tick did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcMineEvent {
    None,
    Armed,
    Fired { shots_left: i32 },
    /// The damaged fuse ran out (0x00cdc010): the explosion goes off at the mine.
    Exploded,
    /// The shots are spent (0x00cdbf90: the destruction effect, then destroyed).
    CleanedUp,
    /// An unfired mine detached by force gives its pickup back (0x00cdc0e0).
    Pickup,
}

/// A placed Arc Mine's runtime state (`m_fArmingCooldownTimer`, `m_fFiringCooldownTimer`,
/// `m_fExplosionTimer`, `m_nShotsRemaining`, `m_nStartingShots`, `m_State`, `m_bBasedOnTarget`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ArcMine {
    pub state: ArcMineState,
    pub arming: f32,
    pub firing_cooldown: f32,
    pub explosion: f32,
    pub shots_remaining: i32,
    pub starting_shots: i32,
    pub based_on_target: bool,
}

impl ArcMine {
    /// Init (0x00cd7900): armed after `m_fArmingTime`; the player's own mines add
    /// `(int)Attribute_ArcMineUpgradedShots` when positive.
    pub fn new(cfg: &ArcMineConfig, bonus_shots: f32) -> Self {
        let starting = cfg.num_shots + (bonus_shots as i32).max(0);
        ArcMine { state: ArcMineState::Arming, arming: cfg.arming_time, starting_shots: starting, shots_remaining: starting, ..Default::default() }
    }

    /// The tick (0x00ce59d0).
    pub fn step<W: ArcMineWorld>(&mut self, cfg: &ArcMineConfig, dt: f32, world: &mut W) -> ArcMineEvent {
        match self.state {
            ArcMineState::Arming => {
                self.arming -= dt;
                if self.arming <= 0.0 {
                    self.state = ArcMineState::Active;
                    return ArcMineEvent::Armed;
                }
            }
            ArcMineState::Active => {
                if world.gate_open() && (world.touching() || self.based_on_target) {
                    if let Some(t) = world.find_target() {
                        if world.fire(t) {
                            self.shots_remaining -= 1;
                            self.firing_cooldown = cfg.delay_between_shots;
                            if self.shots_remaining > 0 {
                                self.state = ArcMineState::FiringCooldown;
                                return ArcMineEvent::Fired { shots_left: self.shots_remaining };
                            }
                            self.state = ArcMineState::PendingDestroy;
                            return ArcMineEvent::CleanedUp;
                        }
                    }
                }
            }
            ArcMineState::FiringCooldown => {
                self.firing_cooldown -= dt;
                if self.firing_cooldown <= 0.0 {
                    self.state = ArcMineState::Active;
                }
            }
            ArcMineState::Damaged => {
                self.explosion -= dt;
                if self.explosion <= 0.0 {
                    self.state = ArcMineState::PendingDestroy;
                    return ArcMineEvent::Exploded;
                }
            }
            ArcMineState::Exploding | ArcMineState::PendingDestroy => {}
        }
        ArcMineEvent::None
    }

    /// Damage that breaks the mine (0x00cd7ee0): the damaged fuse starts.
    pub fn damaged(&mut self, cfg: &ArcMineConfig) {
        if matches!(self.state, ArcMineState::Damaged | ArcMineState::Exploding | ArcMineState::PendingDestroy) {
            return;
        }
        self.state = ArcMineState::Damaged;
        self.explosion = cfg.time_to_explode_from_damage;
    }

    /// A forced detach (0x00cdc0e0): an unfired mine gives its pickup back; a damaged one explodes;
    /// any other cleans up.
    pub fn forced_detach(&mut self) -> ArcMineEvent {
        if self.shots_remaining == self.starting_shots && self.state != ArcMineState::Damaged {
            self.state = ArcMineState::PendingDestroy;
            return ArcMineEvent::Pickup;
        }
        let was_damaged = self.state == ArcMineState::Damaged;
        self.state = ArcMineState::PendingDestroy;
        if was_damaged { ArcMineEvent::Exploded } else { ArcMineEvent::CleanedUp }
    }
}

/// A shot's damage to a pawn (0x00ce0f20): `m_DamageToPawns`, except at the pawn the mine is
/// based on, which takes every remaining shot at once (the shots left drop to 1, the shot itself
/// spending the last). Returns (damage, the shots remaining before the shot's decrement).
pub fn arc_mine_damage(cfg: &ArcMineConfig, is_owner_base: bool, shots_remaining: i32) -> (f32, i32) {
    if is_owner_base {
        (cfg.damage_to_pawns * shots_remaining as f32, 1)
    } else {
        (cfg.damage_to_pawns, shots_remaining)
    }
}

// ---------------------------------------------------------------------------------------------
// Saw cut

/// `DisDLC06Tweaks_NPCSawCut` (`DLC06_Inv_SawButcher.pNPCSpecific.DisDLC06Tweaks_NPCSawCut_2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SawCutConfig {
    /// `m_fSawDamageRadius`, `m_SawDamagePerSecond[difficulty]`,
    /// `m_MaxSawDamageApplicationsPerSecond`, `m_fSawDamageAngleDeg`, `m_fSawDamageOriginWidth`,
    /// `m_bIgnoreIncapacitatedPawns`.
    pub radius: f32,
    pub damage_per_second: [f32; 4],
    pub max_applications_per_second: f32,
    pub angle_deg: f32,
    pub origin_width: f32,
    pub ignore_incapacitated: bool,
}

impl Default for SawCutConfig {
    fn default() -> Self {
        SawCutConfig { radius: 200.0, damage_per_second: [5.0, 10.0, 20.0, 40.0], max_applications_per_second: 20.0, angle_deg: 40.0, origin_width: 85.0, ignore_incapacitated: true }
    }
}

/// A victim candidate of the saw: an id, its position and whether it is incapacitated (the
/// faction and `m_DamageDisabledStates` tests are the caller's filter).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SawVictim<T> {
    pub id: T,
    pub position: Vec3,
    pub incapacitated: bool,
}

/// One saw hit: the victim, the damage, the hit location (`pawn - dir x 50`) and direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SawHit<T> {
    pub id: T,
    pub damage: f32,
    pub hit: Vec3,
    pub dir: Vec3,
}

/// The saw swing's accumulator (`m_fAccumulatedSawDamage`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct SawCut {
    pub accumulated: f32,
}

impl SawCut {
    /// The tick while the swing is active (0x00cdb660): the damage per second at the difficulty
    /// accumulates; once at least `max(dps / max_applications, 1)` it is dealt in whole points to
    /// every pawn within the radius inside the cone (the apex pushed back by
    /// `(origin_width / 2) / tan(angle / 2)` along `forward`), and the whole points leave the
    /// accumulator.
    pub fn step<T: Copy>(&mut self, cfg: &SawCutConfig, dt: f32, difficulty: usize, origin: Vec3, forward: Vec3, victims: &[SawVictim<T>]) -> Vec<SawHit<T>> {
        let dps = cfg.damage_per_second[difficulty.min(3)];
        let per_app_min = (dps / cfg.max_applications_per_second.max(1e-6)).max(1.0);
        let acc = dps * dt + self.accumulated;
        let mut out = Vec::new();
        if acc < per_app_min {
            self.accumulated = acc;
            return out;
        }
        let dmg = acc.trunc();
        let half = (cfg.angle_deg * 0.5).to_radians();
        let fwd = forward.normalize_or_zero();
        let apex = origin - fwd * ((cfg.origin_width * 0.5) / half.tan().max(1e-6));
        for v in victims {
            if cfg.ignore_incapacitated && v.incapacitated {
                continue;
            }
            if v.position.distance(origin) > cfg.radius {
                continue;
            }
            let dir = (v.position - apex).normalize_or_zero();
            if dir.dot(fwd) < half.cos() {
                continue;
            }
            out.push(SawHit { id: v.id, damage: dmg, hit: v.position - dir * 50.0, dir });
        }
        self.accumulated = acc - dmg;
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Chokedust

/// `DisDLC06Tweaks_Explosion` (`DLC06_Twk_Inv_GrenadeDaud.DLC06_Explosion_Chokedust`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChokeExplosionConfig {
    /// `m_fExplosionDuration` (the actor's `LifeSpan`), `m_fDamageInterval`.
    pub duration: f32,
    pub interval: f32,
    /// `max(m_fDamageRadius, m_fPlayerDamageRadius)`, `m_RadialDamage`.
    pub radius: f32,
    pub damage: f32,
    /// `m_fRatRepulsionRadius`, `m_fRatKillRadius`.
    pub rat_repulsion_radius: f32,
    pub rat_kill_radius: f32,
}

impl Default for ChokeExplosionConfig {
    fn default() -> Self {
        ChokeExplosionConfig { duration: 3.0, interval: 0.2, radius: 300.0, damage: 1.0, rat_repulsion_radius: 1000.0, rat_kill_radius: 200.0 }
    }
}

/// A Chokedust cloud (`m_fTimeUntilNextDamage`, the life left, `m_fCurRadiusFactor`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChokeExplosion {
    pub next_damage: f32,
    pub life: f32,
    pub radius_factor: f32,
}

impl ChokeExplosion {
    /// `Explode(source, radiusFactor)` (0x00d04450).
    pub fn new(cfg: &ChokeExplosionConfig, radius_factor: f32) -> Self {
        ChokeExplosion { next_damage: cfg.interval, life: cfg.duration, radius_factor }
    }

    /// The damage radius of a pass (0x00cf4d50).
    pub fn radius(&self, cfg: &ChokeExplosionConfig) -> f32 {
        cfg.radius * self.radius_factor
    }

    /// The tick (0x00cfb780): while it lives, a damage pass each time the interval runs out.
    /// Returns the number of passes this tick (0 or 1 at shipped rates).
    pub fn step(&mut self, cfg: &ChokeExplosionConfig, dt: f32) -> u32 {
        let mut passes = 0;
        if self.life <= 0.0 {
            return 0;
        }
        self.life -= dt;
        self.next_damage -= dt;
        while self.next_damage <= 0.0 {
            self.next_damage += cfg.interval.max(1e-3);
            passes += 1;
        }
        passes
    }

    pub fn alive(&self) -> bool {
        self.life > 0.0
    }
}

/// The damage pass's filter (0x00cfb840): an actor of `only` types (any when empty) that is not of
/// `exclude` types; `exact` compares the class itself instead of its ancestry.
pub fn choke_affects(class_chain: &[&str], only: &[&str], exclude: &[&str], exact: bool) -> bool {
    let is = |list: &[&str]| -> bool {
        if exact {
            class_chain.first().is_some_and(|c| list.iter().any(|x| x.eq_ignore_ascii_case(c)))
        } else {
            class_chain.iter().any(|c| list.iter().any(|x| x.eq_ignore_ascii_case(c)))
        }
    };
    (only.is_empty() || is(only)) && !is(exclude)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct W {
        touching: bool,
        fires: u32,
    }

    impl ArcMineWorld for W {
        type Target = u32;
        fn touching(&self) -> bool {
            self.touching
        }
        fn find_target(&mut self) -> Option<u32> {
            self.touching.then_some(7)
        }
        fn fire(&mut self, _t: u32) -> bool {
            self.fires += 1;
            true
        }
    }

    #[test]
    fn the_arc_mine_arms_fires_twice_then_cleans_up() {
        let cfg = ArcMineConfig::lethal();
        let mut m = ArcMine::new(&cfg, 0.0);
        let mut w = W { touching: true, fires: 0 };
        let dt = 0.1;
        let mut events = Vec::new();
        for k in 0..60 {
            let e = m.step(&cfg, dt, &mut w);
            if e != ArcMineEvent::None {
                events.push((k, e));
            }
        }
        // Armed at 2.0 s, the first shot at once, the second 1.0 s later, then spent.
        assert_eq!(events[0].1, ArcMineEvent::Armed);
        assert!((19..=20).contains(&events[0].0), "{events:?}");
        assert_eq!(events[1].1, ArcMineEvent::Fired { shots_left: 1 });
        assert_eq!(events[2].1, ArcMineEvent::CleanedUp);
        assert!((events[2].0 - events[1].0) >= 10, "{events:?}");
        assert_eq!(w.fires, 2);
        // The player's upgraded mines carry more shots.
        assert_eq!(ArcMine::new(&cfg, 1.0).starting_shots, 3);
    }

    #[test]
    fn a_damaged_mine_explodes_and_an_unfired_detach_gives_the_pickup() {
        let cfg = ArcMineConfig::lethal();
        let mut m = ArcMine::new(&cfg, 0.0);
        m.damaged(&cfg);
        let mut w = W { touching: false, fires: 0 };
        let mut t = 0.0;
        let mut exploded = None;
        for _ in 0..40 {
            t += 0.1;
            if m.step(&cfg, 0.1, &mut w) == ArcMineEvent::Exploded {
                exploded = Some(t);
                break;
            }
        }
        assert!(exploded.is_some_and(|t| (t - 2.5f32).abs() < 0.11), "{exploded:?}");
        assert_eq!(ArcMine::new(&cfg, 0.0).forced_detach(), ArcMineEvent::Pickup);
        assert_eq!(arc_mine_damage(&cfg, true, 2), (100.0, 1));
        assert_eq!(arc_mine_damage(&cfg, false, 2), (50.0, 2));
    }

    #[test]
    fn the_saw_deals_whole_points_in_its_cone() {
        let cfg = SawCutConfig::default();
        let mut s = SawCut::default();
        let ahead = SawVictim { id: 1, position: Vec3::new(100.0, 0.0, 0.0), incapacitated: false };
        // Behind the apex (42.5 / tan 20 deg = 117 back from the origin).
        let behind = SawVictim { id: 2, position: Vec3::new(-150.0, 0.0, 0.0), incapacitated: false };
        let off = SawVictim { id: 3, position: Vec3::new(100.0 * 60f32.to_radians().cos(), 100.0 * 60f32.to_radians().sin(), 0.0), incapacitated: false };
        let mut total = 0.0;
        for _ in 0..60 {
            for h in s.step(&cfg, 1.0 / 60.0, 0, Vec3::ZERO, Vec3::X, &[ahead, behind, off]) {
                assert_eq!(h.id, 1, "only the pawn ahead");
                assert_eq!(h.damage.fract(), 0.0);
                total += h.damage;
            }
        }
        assert!((total - 5.0f32).abs() < 1.0 + 1e-3, "{total}");
    }

    #[test]
    fn chokedust_damages_fifteen_times_over_its_life() {
        let cfg = ChokeExplosionConfig::default();
        let mut c = ChokeExplosion::new(&cfg, 1.0);
        let mut passes = 0;
        for _ in 0..(60 * 4) {
            passes += c.step(&cfg, 1.0 / 60.0);
        }
        assert_eq!(passes, 15);
        assert!(!c.alive());
        assert!(choke_affects(&["DishonoredNPCPawn", "DishonoredPawn"], &["DishonoredPawn"], &["DishonoredPlayerPawn"], false));
        assert!(!choke_affects(&["DishonoredPlayerPawn", "DishonoredPawn"], &["DishonoredPawn"], &["DishonoredPlayerPawn"], false));
    }
}
