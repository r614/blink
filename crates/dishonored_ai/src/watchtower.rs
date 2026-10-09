//! Watch towers (`DisWatchTower`, research/dishonored/systems/watchtowers.md): the update
//! 0x00A3C640 called from the tick (vslot 103, 0x00A3F890).
//!
//! - States (`EDisWatchTowerState`): Explore (1), Attack (2), Deactivated (3). The Kismet turns a
//!   tower on and off (`DisSeqAct_WatchTower`) and a rewired panel reverses its polarity
//!   (`m_bPolarityReversed`: `m_HackedDispositions` replace its faction's relations and it sees
//!   through `m_pHackedVisionTweak`).
//! - Every tick the tower and its light turn towards their goal (0x00A30E40) at
//!   `m_fStateMaxTowerRotSpeedDeg` / `m_fStateMaxLightRotSpeedDeg` times `m_fSpeedPercentBonus + 1`,
//!   the light's pitch kept between `m_fStateLightLowestRotDeg` below and
//!   `m_fStateLightHighestRotDeg` above the horizon; the first aim snaps to the goal
//!   (`m_bYawAndPitchSet`). The bonus grows in Explore by `m_fStateExploreSpeedIncrease x dt` (at
//!   most 100 a tick, the sum is not capped) and is 0 on entering or leaving Explore (0x00A31280).
//!   The goal angles are taken from the light (offset from the tower's axis), the fixed point of
//!   the original's closed form (`m_fTowerToLightXDist` / `YDist` with an `asin` correction), and
//!   the vision component
//!   (`DisCpntType_VisionWatchtower`, the AI's vision cones from the light socket; in attack the
//!   field of view is scaled by `m_fAttackFOVMultiplier`) reports the best seen actor and its
//!   suspicion (0x00A21CC0; 3: a live enemy).
//! - Explore: the light follows `m_pExploreTracked` (an `InterpActor` its matinee sweeps); a live
//!   enemy in view switches to Attack (0x00A31280(2)) with `m_fStateAttackInitialDelay` before the
//!   first shot.
//! - Attack with the enemy in view: the tower aims at it; the warning sound plays once a volley
//!   when the time to fire drops under `m_fSoundAttackWarningTime`; at zero it fires its
//!   projectile from the arrow socket (0x00A29B30) and waits `m_fVolleyDelayBetweenShots`, or
//!   after the volley's last shot `m_fDelayBetweenVolleys` (`m_VolleyShots` per volley). The shot
//!   aims at a Tallboy's ranged-aim point (an `IsA DisTallboyNPCPawn` target, the class of the
//!   static class struct 0x0133A080: hit region 2, 0x00B68010; Tallboys are not tower targets
//!   here) and at any other target's `Location + VRand() x m_fFireArrowMaxSpread` (a point on the
//!   sphere, 0x00402240), through the projectile tweak's ballistic solve with lead (0x00C3F2A0
//!   mode 0, the point given: no floor trace, the lead keeps its height).
//! - Attack with the enemy lost: the frantic sweep, a sine of `m_fFranticSpeed` cycles a second and
//!   `m_fFranticRandomRange` amplitude about the last seen location, perpendicular to the line from
//!   the tower; after `m_fStateAttackTimeout` back to Explore.
//! - Corpses in view fire the Kismet's neutral / ally / enemy corpse outputs once each.
//!
//! Not ported: the explore animation when no tracked actor is set (the tower holds its yaw); the
//! ally tracking (0x00A3C370: in Explore the light follows `m_AllysPlayerLocation`, the player
//! location known to an NPC of `m_SupportedSquads` at `m_AllyAttentionLevelToTrack` or above,
//! which also zeroes the bonus); the reversed tower's battery drain (0x00A2A140: every
//! `m_fReversedBatteryChargeTime` s while `m_bPolarityReversed` with a receptacle attached, the
//! receptacle's 0x00C85170(2)); a hacked tower's projectile instigated by the player (0x00A29B30
//! sets it from `m_bPolarityReversed`); the light blinding and rat repulsion. Presentation only:
//! the attack light blend (0x00A29D10), the gun and light bone controls (0x00A29F30, 0x00A29910)
//! and the rotation sound RTPCs.

use crate::Social;
use dishonored_ir::ai::Relationship;
use dishonored_ir::interactables::{Interactables, WatchTowerDef};
use dishonored_sim::events::SimEvent;
use dishonored_sim::launchers::LaunchersState;
use dishonored_sim::state::{PlayerId, WorldState};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;
use serde::{Deserialize, Serialize};

pub const EXPLORE: u8 = 1;
pub const ATTACK: u8 = 2;
pub const DEACTIVATED: u8 = 3;

/// One tower's state (the actor's `m_eCurrentState`, `m_fTowerYaw`, `m_fLightPitch`, ...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TowerState {
    pub state: u8,
    /// World yaw of the tower and pitch of the light (degrees; pitch positive up).
    pub yaw: f32,
    pub pitch: f32,
    pub target: Option<PlayerId>,
    /// `m_LastTargetLocation`.
    pub last_target: Vec3,
    /// `m_fTimeToFire`, `m_nVolleyShotsRemaining`, `m_bWarningPlayedThisVolley`.
    pub time_to_fire: f32,
    pub shots_left: i32,
    pub warned: bool,
    /// `m_fSuspectTimeWithoutActors` (+0x2bc): time since the enemy was lost in attack.
    pub lost: f32,
    /// `m_fSpeedPercentBonus` (+0x2c8).
    pub speed_bonus: f32,
    /// `m_bYawAndPitchSet`: the first aim snaps.
    pub aimed: bool,
    /// `m_bPolarityReversed`.
    pub hacked: bool,
    /// A Kismet `WatchTowerShootAtTarget` target (`m_pScriptedTarget`).
    pub scripted: Option<PlayerId>,
    /// Corpses already reported (`m_pAlreadyNotedCorpses`).
    pub noted: Vec<PlayerId>,
    pub shots_fired: u32,
}

/// The towers of the active map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchTowersState {
    pub towers: Vec<TowerState>,
}

/// What a step reports to the Kismet (`DisSeqEvent_WatchTower` outputs) and the presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TowerEvent {
    Activated(usize),
    Deactivated(usize),
    NeutralCorpseSeen(usize),
    AllyCorpseSeen(usize),
    EnemyCorpseSeen(usize),
    Hacked(usize),
    /// The attack warning sound (`m_pSoundAttackWarning`) and a shot.
    Warning(usize),
    Shot(usize),
}

impl WatchTowersState {
    pub fn new(defs: &Interactables) -> Self {
        WatchTowersState {
            towers: defs
                .watchtowers
                .iter()
                .map(|w| TowerState { state: if w.active { EXPLORE } else { DEACTIVATED }, yaw: w.yaw_deg, shots_left: w.volley_shots.max(1), ..Default::default() })
                .collect(),
        }
    }

    fn sync(&mut self, defs: &Interactables) {
        if self.towers.len() != defs.watchtowers.len() {
            *self = WatchTowersState::new(defs);
        }
    }

    pub fn find(defs: &Interactables, name: &str) -> Option<usize> {
        defs.watchtowers.iter().position(|w| w.name.eq_ignore_ascii_case(name))
    }

    /// `DisSeqAct_WatchTower` (`EDisWatchTowerKismetAction`): 0 turn off, 1 on, 2 toggle, 5 switch
    /// polarity. Returns the event to fire.
    pub fn kismet_action(&mut self, defs: &Interactables, i: usize, action: u8) -> Option<TowerEvent> {
        self.sync(defs);
        let t = self.towers.get_mut(i)?;
        let on = match action {
            0 => false,
            1 => true,
            2 => t.state == DEACTIVATED,
            5 => {
                t.hacked = !t.hacked;
                t.target = None;
                return Some(TowerEvent::Hacked(i));
            }
            _ => return None,
        };
        if on && t.state == DEACTIVATED {
            t.state = EXPLORE;
            t.speed_bonus = 0.0;
            Some(TowerEvent::Activated(i))
        } else if !on && t.state != DEACTIVATED {
            t.state = DEACTIVATED;
            t.speed_bonus = 0.0;
            t.target = None;
            Some(TowerEvent::Deactivated(i))
        } else {
            None
        }
    }

    /// `DisSeqAct_WatchTowerShootAtTarget` (`m_pTargetActor` -> `m_pScriptedTarget`).
    pub fn shoot_at(&mut self, defs: &Interactables, i: usize, target: PlayerId) {
        self.sync(defs);
        if let Some(t) = self.towers.get_mut(i) {
            t.scripted = Some(target);
        }
    }

    /// Step every tower (`dt` seconds). `tracked` gives an explore-tracked actor's location;
    /// `gravity_z` is the world's `GravityZ` (the shot's ballistic solve).
    #[allow(clippy::too_many_arguments)]
    pub fn step(&mut self, defs: &Interactables, state: &mut WorldState, launchers: &mut LaunchersState, soc: &Social, world: &dyn WorldQuery, tracked: &dyn Fn(&str) -> Option<Vec3>, dt: f32, gravity_z: f32, out: &mut Vec<TowerEvent>, events: &mut Vec<SimEvent>) {
        self.sync(defs);
        for (i, def) in defs.watchtowers.iter().enumerate() {
            let t = &mut self.towers[i];
            if t.state == DEACTIVATED || dt <= 1e-4 {
                continue;
            }
            let eye = light_at(def, t.yaw);
            let vision = if t.hacked { &def.hacked_vision } else { &def.vision };
            let fov_mult = if t.state == ATTACK { if t.hacked { def.hacked_attack_fov_mult } else { def.attack_fov_mult } } else { 1.0 };
            let (inner, outer) = scaled_cones(vision, fov_mult);
            let faction = soc.factions.as_ref().and_then(|f| f.index(&def.faction));
            // The vision component: the best live enemy in either cone with a clear line, nearest
            // first; corpses in view are noted.
            let mut best: Option<(PlayerId, f32, Vec3)> = None;
            for p in &state.players {
                if dishonored_sim::powers::is_hidden(p) || p.carried_by.is_some() {
                    continue;
                }
                let at = p.position + Vec3::Z * 30.0;
                let to = at - eye;
                let d = to.length();
                if d < 1.0 || d > inner.0.max(outer.0) {
                    continue;
                }
                let local = to_eye_space(to / d, t.yaw, t.pitch);
                let seen = (d <= inner.0 && inner.1.contains(local)) || (d <= outer.0 && outer.1.contains(local));
                if !seen || world.raycast(eye, to, d - 20.0, channel::VISION).is_some() {
                    continue;
                }
                let rel = relation(def, t.hacked, faction, p.id, soc);
                if !p.alive {
                    if !t.noted.contains(&p.id) {
                        t.noted.push(p.id);
                        out.push(match rel {
                            Relationship::Ally => TowerEvent::AllyCorpseSeen(i),
                            Relationship::Enemy => TowerEvent::EnemyCorpseSeen(i),
                            Relationship::Neutral => TowerEvent::NeutralCorpseSeen(i),
                        });
                    }
                    continue;
                }
                if rel == Relationship::Enemy && best.is_none_or(|b| d < b.1) {
                    best = Some((p.id, d, at));
                }
            }
            if let Some(s) = t.scripted {
                if let Some(p) = state.player(s).filter(|p| p.alive) {
                    best = Some((s, p.position.distance(eye), p.position + Vec3::Z * 30.0));
                }
            }
            let goal: Option<Vec3>;
            match t.state {
                EXPLORE => {
                    t.speed_bonus += (def.explore_speed_increase * dt).min(100.0);
                    goal = tracked(&def.explore_tracked);
                    if let Some((id, _, at)) = best {
                        t.state = ATTACK;
                        t.speed_bonus = 0.0;
                        t.target = Some(id);
                        t.last_target = at;
                        t.time_to_fire = def.attack_initial_delay;
                        t.shots_left = def.volley_shots.max(1);
                        t.warned = false;
                        t.lost = 0.0;
                    }
                }
                _ => match best {
                    Some((id, _, at)) => {
                        t.target = Some(id);
                        t.last_target = at;
                        t.lost = 0.0;
                        goal = Some(at);
                        if !t.warned && t.time_to_fire <= def.warning_time {
                            t.warned = true;
                            out.push(TowerEvent::Warning(i));
                        }
                        t.time_to_fire -= dt;
                        if t.time_to_fire <= 0.0 {
                            let muzzle = muzzle_at(def, t.yaw);
                            let (point, vel) = match state.player(id) {
                                Some(p) => (p.position + vrand(i, t.shots_fired) * def.fire_spread, p.velocity),
                                None => (at, Vec3::ZERO),
                            };
                            let g = -gravity_z * def.projectile.gravity_mult;
                            let dir = dishonored_sim::launchers::aim(0, muzzle, point, vel, false, def.projectile.speed, g);
                            launchers.fire_tower(defs, i, muzzle, dir * def.projectile.speed, state, events);
                            t.shots_fired = t.shots_fired.wrapping_add(1);
                            out.push(TowerEvent::Shot(i));
                            t.shots_left -= 1;
                            if t.shots_left < 1 {
                                t.time_to_fire = def.delay_between_volleys;
                                t.shots_left = def.volley_shots.max(1);
                                t.warned = false;
                            } else {
                                t.time_to_fire = def.volley_delay;
                            }
                        }
                    }
                    None => {
                        // The frantic sweep about the last seen location.
                        let flat = Vec3::new(t.last_target.x - def.position.x, t.last_target.y - def.position.y, 0.0).normalize_or_zero();
                        let perp = Vec3::new(-flat.y, flat.x, 0.0);
                        let s = (def.frantic_speed * t.lost * std::f32::consts::TAU).sin() * def.frantic_range;
                        goal = Some(t.last_target + perp * s);
                        t.lost += dt;
                        if t.lost >= def.attack_timeout {
                            t.state = EXPLORE;
                            t.speed_bonus = 0.0;
                            t.target = None;
                            t.scripted = None;
                        }
                    }
                },
            }
            if let Some(g) = goal {
                let to = g - eye;
                let want_yaw = to.y.atan2(to.x).to_degrees();
                let want_pitch = to.z.atan2(to.truncate().length()).to_degrees().clamp(-def.light_lowest_deg, def.light_highest_deg);
                if t.aimed {
                    let mult = t.speed_bonus + 1.0;
                    t.yaw = turn(t.yaw, want_yaw, def.tower_rot_speed_deg * mult * dt);
                    t.pitch = turn(t.pitch, want_pitch, def.light_rot_speed_deg * mult * dt);
                } else {
                    t.aimed = true;
                    t.yaw = want_yaw;
                    t.pitch = want_pitch;
                }
            }
        }
    }
}

/// The tower's relationship towards a pawn: its faction's, or once hacked `m_HackedDispositions`
/// for the listed factions.
fn relation(def: &WatchTowerDef, hacked: bool, faction: Option<u16>, who: PlayerId, soc: &Social) -> Relationship {
    if hacked {
        if let (Some(t), Some(&theirs)) = (soc.factions.as_ref(), soc.pawn_faction.get(&who)) {
            for (f, r) in &def.hacked_dispositions {
                if t.index(f).is_some_and(|fi| t.isa(theirs, fi)) {
                    return match r.as_str() {
                        "Relationship_Enemy" => Relationship::Enemy,
                        "Relationship_Ally" => Relationship::Ally,
                        _ => Relationship::Neutral,
                    };
                }
            }
        }
    }
    match faction {
        Some(f) => soc.faction_relation(f, who),
        None => Relationship::Neutral,
    }
}

fn rotate_about(def: &WatchTowerDef, p: Vec3, yaw: f32) -> Vec3 {
    let a = (yaw - def.yaw_deg).to_radians();
    let (s, c) = a.sin_cos();
    let d = p - def.position;
    def.position + Vec3::new(d.x * c - d.y * s, d.x * s + d.y * c, d.z)
}

pub fn light_at(def: &WatchTowerDef, yaw: f32) -> Vec3 {
    rotate_about(def, def.light, yaw)
}

pub fn muzzle_at(def: &WatchTowerDef, yaw: f32) -> Vec3 {
    rotate_about(def, def.muzzle, yaw)
}

pub fn aim_dir(yaw: f32, pitch: f32) -> Vec3 {
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), p.sin())
}

/// A world unit direction in the light's frame (x along the aim, y left, z up).
fn to_eye_space(d: Vec3, yaw: f32, pitch: f32) -> Vec3 {
    let fwd = aim_dir(yaw, pitch);
    let left = Vec3::new(-yaw.to_radians().sin(), yaw.to_radians().cos(), 0.0);
    let up = fwd.cross(left).normalize_or_zero();
    Vec3::new(d.dot(fwd), d.dot(left), d.dot(up))
}

fn scaled_cones(v: &dishonored_ir::ai::VisionConfig, mult: f32) -> ((f32, crate::Cone), (f32, crate::Cone)) {
    let mut s = v.clone();
    s.inner_fov_deg *= mult;
    s.outer_fov_deg *= mult;
    let (i, o) = crate::cones(&s);
    ((s.inner_range, i), (s.outer_range, o))
}

/// Turn `from` towards `to` (degrees) by at most `step`, the short way round for the yaw.
fn turn(from: f32, to: f32, step: f32) -> f32 {
    let d = (to - from + 540.0).rem_euclid(360.0) - 180.0;
    from + d.clamp(-step, step)
}

/// `VRand()` (0x00402240): a unit vector from a point drawn in the cube [-1, 1]^3 until it lies in
/// the unit ball, normalised. The draws are a hash of the tower and the shot (deterministic).
fn vrand(tower: usize, shot: u32) -> Vec3 {
    let mut h = (tower as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (shot as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    let mut next = || {
        h = h.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = h;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
    };
    loop {
        let v = Vec3::new(next(), next(), next());
        let l2 = v.length_squared();
        if l2 <= 1.0 && l2 > 1e-8 {
            return v / l2.sqrt();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_takes_the_short_way_and_is_limited() {
        assert_eq!(turn(10.0, 30.0, 5.0), 15.0);
        assert_eq!(turn(170.0, -170.0, 5.0), 175.0);
        assert_eq!(turn(0.0, 2.0, 5.0), 2.0);
    }

    #[test]
    fn aim_axis_maps_to_eye_space_forward() {
        let d = aim_dir(30.0, -10.0);
        let e = to_eye_space(d, 30.0, -10.0);
        assert!((e.x - 1.0).abs() < 1e-4 && e.y.abs() < 1e-4 && e.z.abs() < 1e-4, "{e:?}");
    }

    #[test]
    fn vrand_is_a_repeatable_unit_vector() {
        let mut sum = Vec3::ZERO;
        for s in 0..400 {
            let v = vrand(1, s);
            assert!((v.length() - 1.0).abs() < 1e-4, "{v:?}");
            assert_eq!(v, vrand(1, s));
            sum += v;
        }
        // Spread over the sphere, not bunched on one side.
        assert!(sum.length() / 400.0 < 0.15, "{sum:?}");
    }
}
