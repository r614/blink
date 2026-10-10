//! Presentation rules without Bevy: which effects, sounds and animation reactions the
//! simulation's events ask for. The runtime (`effects`, `audio`, `characters`) applies them to
//! its Bevy world; `tools/oracle` compiles this same file (`#[path]`) to record them headless, so
//! the oracle checks the game's own decisions. Keep it free of `crate::` paths and of Bevy: only
//! glam and the game crates. The particle simulation of an effect instance lives in
//! `dishonored_fx` (re-exported here, so the runtime's `present_rules::` paths still resolve).

pub use dishonored_fx::*;

use dishonored_audio::contact::ContactSystem;
use dishonored_audio::cues;
use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::{PawnKind, PlayerId, PlayerState, ProjectileState};
use dishonored_world::{CollisionWorld, WorldQuery};
use glam::{Mat4, Vec3};
use std::collections::HashMap;

/// Muzzle flash of the player's first-person pistol (`Twk_Inv_PistolCorvo`'s
/// `DisTweaks_FirePistol.m_pMuzzleFlashFX`, at the pistol's `Spitfire` socket).
pub const PLAYER_MUZZLE: &str = "Vfx_Weapon.Effects.pistol.Ps_PlayerPistol_StdMuzzle_01";
/// Muzzle flash of a pistol seen in third person (the pawn-specific `DisTweaks_NPCFireGun`).
pub const PAWN_MUZZLE: &str = "Vfx_Weapon.Effects.Ps_PistolMuzzle";
/// Shadow Kill ash (`Twk_Pawn_DefaultNPC.m_ShadowKillFXs`): one system per listed bone,
/// `m_fShadowKillDelay` 0.1 s after the kill.
pub const SHADOW_KILL_PS: &str = "Vfx_GamePlay.ShadowKill.ps_SK_Leg_Arms";
pub const SHADOW_KILL_BONES: &[&str] = &[
    "upper_leg_L_jnt",
    "lower_leg_L_jnt",
    "upper_leg_R_jnt",
    "lower_leg_R_jnt",
    "upper_arm_L_jnt",
    "lower_arm_L_jnt",
    "upper_arm_R_jnt",
    "lower_arm_R_jnt",
    "spine_1_jnt",
    "head_jnt",
];
pub const SHADOW_KILL_DELAY: f32 = 0.1;

/// ParticleSystems the gameplay effects use (decoded with every level, from DishonoredGame):
/// the tweaks' effects and every contact-system particle (`particle_material::GAMEPLAY_PARTICLES`).
pub const GAMEPLAY_PARTICLES: &[&str] = dishonored_assets::particle_material::GAMEPLAY_PARTICLES;

/// Blink's targeting indicators (`Twk_Blink`: `m_pGroundPS` on a floor, `m_pLowFallPS` /
/// `m_pHighFallPS` in the air, `m_pMantlePS` on a ledge). Their emitters loop forever with
/// `bKillOnDeactivate`: the power keeps one at the target while aiming and kills it on release.
pub const BLINK_GROUND_PS: &str = "Vfx_GamePlay.Blink.Blink_Ground_01";
pub const BLINK_FALL_PS: &str = "Vfx_GamePlay.Blink.Blink_Fall_01";
pub const BLINK_MANTLE_PS: &str = "Vfx_GamePlay.Blink.Blink_Mantle_01";
pub const CROSSBOW_MUZZLE: &str = "Vfx_Weapon.Effects.Ps_CrossBowMuzzle";
/// Possession: the hand cast, and the burst on a human / rat host.
pub const POSSESSION_CAST_PS: &str = "Vfx_GamePlay.Possession.Possession_HandCast_01";
pub const POSSESSION_HUMAN_PS: &str = "Vfx_GamePlay.Possession.PossesTarget_Human_01";
pub const POSSESSION_RAT_PS: &str = "Vfx_GamePlay.Possession.PossesTarget_Rats_01";
/// Devouring Swarm: the hand cast and the summoning (levels 1 / 2).
pub const SWARM_CAST_PS: &str = "Vfx_GamePlay.RatSwarm.Ps_RSwarm_HandCast_01";
/// `Twk_Powers_Possess.m_HandCastParticleAttachSocket` / `Twk_DevouringSwarm.m_HandFXSocketName`.
pub const HAND_CAST_SOCKET: &str = "LeftHand_PowerCastA";
pub const SWARM_SUMMON_PS: &str = "Vfx_GamePlay.RatSwarm.RSwarm_Summoning_lvl1";
pub const SWARM_SUMMON2_PS: &str = "Vfx_GamePlay.RatSwarm.RSwarm_Summoning_lvl2";
/// Grenade explosion and Spring Razor burst (`Startup`).
pub const GRENADE_EXPLOSION_PS: &str = "Vfx_Weapon.Effects.Grenade.ps_grenade";
pub const SPRING_RAZOR_PS: &str = "Vfx_Weapon.Effects.SpringRazor.Ps_SpringRazor";

/// Projectile trails (`Twk_Proj_*` trail templates): pistol tracer, bolt trail, grenade trail.
pub const PISTOL_TRACER_PS: &str = "Vfx_Weapon.Effects.Trails.Ps_Pistol_StdTracer_01";
pub const BOLT_TRAIL_PS: &str = "Vfx_Weapon.Effects.Trails.StdBolt_Trail";
pub const GRENADE_TRAIL_PS: &str = "Vfx_Weapon.Effects.Grenade.Ps_Grenade_Trail";
/// `Twk_Proj_Arrow_Flare.m_pTrailEffect`.
pub const FLARE_TRAIL_PS: &str = "Vfx_Weapon.Effects.FlareBolts.Flare_Bolt_Trail";
/// Dark Vision's NPC vision cones (`Twk_DarkVision` / pawn `m_VisionConePatrolTemplate`).
pub const DV_CONE_PS: &str = "Vfx_GamePlay.DarkVision.Ps_DVision_VCone_Blue_01";

/// A transform whose X axis points along `dir` (UE3 effects face +X), at `at`.
pub fn facing(at: Vec3, dir: Vec3) -> Mat4 {
    let x = if dir.length_squared() > 1e-6 { dir.normalize() } else { Vec3::X };
    let z = if x.z.abs() > 0.99 { Vec3::X } else { Vec3::Z };
    let y = z.cross(x).normalize();
    let z = x.cross(y);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), at.extend(1.0))
}

/// The effects' random stream (an LCG; particles' start values and lifetimes).
pub fn next_rand(state: &mut u64) -> f32 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*state >> 40) as f32) / (1u64 << 24) as f32
}

/// A one-shot instance is finished when every emitter is done (or past 10 s) and empty; an
/// emitter that loops forever (`EmitterLoops` 0, e.g. the Blink indicators) is deactivated after
/// [`ONE_SHOT_LIMIT`], its particles killed (`bKillOnDeactivate`), so no spawned effect outlives
/// its use.
pub fn one_shot_finished(sims: &[EmitterSim]) -> bool {
    sims.iter().all(|s| ((s.done || s.time > 10.0) && s.particles.is_empty()) || s.time > ONE_SHOT_LIMIT)
}

/// The contact type of the nearest surface around a point (the collision triangle's physical
/// material; impact particles and sounds).
pub fn surface_near(world: &CollisionWorld, p: Vec3) -> String {
    let mut best: Option<(f32, u32)> = None;
    for d in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        if let Some(hit) = world.raycast(p - d * 12.0, d, 36.0, dishonored_world::channel::ALL) {
            if best.is_none_or(|b| hit.distance < b.0) {
                best = Some((hit.distance, hit.tri));
            }
        }
    }
    best.and_then(|b| world.surface_of(b.1)).unwrap_or("DisContactType_Environment").to_string()
}

/// The outward normal of the surface at a point where something struck along `dir` (the ray
/// along it, its normal turned against it); without a direction, of the nearest surface around
/// the point.
pub fn normal_at(world: &CollisionWorld, p: Vec3, dir: Vec3) -> Option<Vec3> {
    let dir = dir.normalize_or_zero();
    if dir != Vec3::ZERO {
        if let Some(h) = world.raycast(p - dir * 8.0, dir, 16.0, dishonored_world::channel::ALL) {
            return Some(if h.normal.dot(dir) > 0.0 { -h.normal } else { h.normal });
        }
    }
    let mut best: Option<(f32, Vec3)> = None;
    for d in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        if let Some(h) = world.raycast(p - d * 12.0, d, 36.0, dishonored_world::channel::ALL) {
            if best.is_none_or(|b| h.distance < b.0) {
                best = Some((h.distance, if h.normal.dot(d) > 0.0 { -h.normal } else { h.normal }));
            }
        }
    }
    best.map(|b| b.1)
}

/// The decal a contact (`striker` x the surface `struck`) leaves, if its `DisContactSub_Decal`
/// fires ([`dishonored_audio::contact::ContactDecal::fires`]): one of `m_Materials` (the original
/// picks `rand() % Num`; here by the point, so the same hit looks the same), `m_MaxWidth` x
/// `m_MaxHeight` uu, living `m_fLifetime`. `speed`: the contact's speed, if it has one.
pub fn contact_decal(s: &FxScene, striker: &str, struck: &str, at: Vec3, dir: Vec3, speed: Option<f32>) -> Option<FxRequest> {
    let cd = s.contact?.decal(striker, struck).filter(|d| d.fires(speed))?;
    let normal = normal_at(s.world?, at, dir)?;
    let pick = ((at.x * 7.31 + at.y * 3.17 + at.z * 1.91).abs() as usize) % cd.materials.len();
    Some(FxRequest::Decal { material: cd.materials[pick].clone(), at, normal, dir, width: cd.max_width, height: cd.max_height, lifetime: cd.lifetime })
}

/// Contact type of the surface below a point, and its distance.
pub fn surface_below(world: &CollisionWorld, pos: Vec3, depth: f32) -> Option<(String, f32)> {
    let hit = world.raycast(pos, -Vec3::Z, depth, dishonored_world::channel::ALL)?;
    Some((world.surface_of(hit.tri).unwrap_or("DisContactType_Environment").to_string(), hit.distance))
}

/// An effect the events ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum FxRequest {
    /// A one-shot system at a point, its X axis along `dir`.
    Effect { path: String, at: Vec3, dir: Vec3 },
    /// A cast effect on the local player's left hand (`LeftHand_PowerCastA`); `at` / `dir` are
    /// the fallback placement when the rig has no such socket.
    HandCast { path: String, at: Vec3, dir: Vec3 },
    /// A beam one-shot (`PEB2STM_UserSet` target) from `from` to `to`.
    Beam { path: &'static str, from: Vec3, to: Vec3 },
    /// A gameplay decal (`DisContactSub_Decal`, [`contact_decal`]): the material's object path,
    /// the centre and surface normal (Dishonored world), the direction it was struck along (the
    /// decal's texture axis follows it), its size (uu) and lifetime (s).
    Decal { material: String, at: Vec3, normal: Vec3, dir: Vec3, width: f32, height: f32, lifetime: f32 },
    /// The trail of a projectile that ended within its launch tick (never in a snapshot, so
    /// `attached_want` cannot start it): started at the launch and deactivated at once, as the
    /// projectile's trail component is when the projectile explodes (`DeactivateSystem`); it ends
    /// once its particles (if any) die.
    EndedTrail { path: &'static str, xf: Mat4 },
}

/// The trail a projectile kind carries (`Twk_Proj_*` `m_pTrailEffect`): `StdBolt_Trail` for
/// bolts and sleep darts, `Flare_Bolt_Trail` for the incendiary bolt, `Ps_Grenade_Trail` for
/// grenades; bullets have the tracer beam instead (None).
pub fn trail_of(kind: dishonored_sim::state::ProjectileKind) -> Option<&'static str> {
    use dishonored_sim::state::ProjectileKind as K;
    match kind {
        K::Bullet => None,
        K::Bolt | K::SleepDart => Some(BOLT_TRAIL_PS),
        K::IncendiaryBolt => Some(FLARE_TRAIL_PS),
        K::Grenade | K::StickyGrenade => Some(GRENADE_TRAIL_PS),
        // `TallboyExplosiveArrow_twk.m_pTrailEffect` (`Vfx_Weapon.Effects.WhaleOil.ps_TB_Trail`) is not
        // in the presented effect set yet.
        K::TallboyArrow => None,
        // The trap's explosive arrow carries the same `ps_TB_Trail` (`TrapExplosiveArrow_twk`), also
        // not presented yet; the Overseers' Back launcher's plain arrow (`Twk_Proj_Arrow`) the
        // standard bolt trail.
        K::TrapArrow => None,
        K::TrapBolt => Some(BOLT_TRAIL_PS),
    }
}

/// What the gameplay effects remember between frames.
#[derive(Debug, Clone, Default)]
pub struct FxEventState {
    /// Effects due later: (time, system, position, direction).
    pub delayed: Vec<(f32, String, Vec3, Vec3)>,
    /// Projectile id -> fired as a crossbow bolt (impact striker).
    pub bolts: HashMap<u32, bool>,
    /// Spring Razor traps placed: trap id -> position (their trigger effect).
    pub traps: HashMap<u32, Vec3>,
    /// Pistol shots in flight: projectile id -> muzzle point (the tracer's start).
    pub shots: HashMap<u32, Vec3>,
    /// Projectiles in flight: id -> launch direction (the decal of its impact follows it).
    pub dirs: HashMap<u32, Vec3>,
}

/// What the gameplay effects see of the frame.
pub struct FxScene<'a> {
    /// Every pawn (the local one included).
    pub players: &'a [PlayerState],
    pub me: Option<PlayerId>,
    /// Pistol `Spitfire` sockets of the animated pawns: (position, firing direction).
    pub muzzle: &'a HashMap<u32, (Vec3, Vec3)>,
    /// `SHADOW_KILL_BONES` positions of the animated pawns.
    pub bones: &'a HashMap<u32, Vec<Vec3>>,
    pub contact: Option<&'a ContactSystem>,
    pub world: Option<&'a CollisionWorld>,
    pub now: f32,
    /// The projectiles of the snapshot (a fired one missing from it ended in its launch tick).
    pub projectiles: &'a [ProjectileState],
}

impl FxEventState {
    /// Gameplay effects from simulation events, with the systems and sockets the original's
    /// tweaks name (U-165): the pistol muzzle flash at the `Spitfire` socket
    /// (`m_MuzzleSocketName`; first-person `Ps_PlayerPistol_StdMuzzle_01`, third-person
    /// `Ps_PistolMuzzle`), the crossbow muzzle, Possession, Devouring Swarm, grenade and Spring
    /// Razor systems, Windblast's per-level `m_EffectTemplate` along the view, Shadow Kill ash
    /// on the `m_ShadowKillFXs` bones, impacts and blood from the contact system's particle
    /// sub-intersections, sword sparks on blocks and parries, the pistol tracer beam.
    pub fn requests(&mut self, events: &[SimEvent], s: &FxScene) -> Vec<FxRequest> {
        let mut out = Vec::new();
        let now = s.now;
        // Delayed effects (Shadow Kill ash) that are due.
        let due: Vec<(f32, String, Vec3, Vec3)> = self.delayed.iter().filter(|d| d.0 <= now).cloned().collect();
        self.delayed.retain(|d| d.0 > now);
        for (_, path, at, dir) in due {
            out.push(FxRequest::Effect { path, at, dir });
        }
        let find = |id: u32| s.players.iter().find(|p| p.id == id).copied();
        let fwd = |yaw: f32| Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
        let look = |p: &PlayerState| {
            let (y, pi) = (p.yaw.to_radians(), p.pitch.to_radians());
            Vec3::new(y.cos() * pi.cos(), y.sin() * pi.cos(), pi.sin())
        };
        let surface = |p: Vec3| -> String { s.world.map(|w| surface_near(w, p)).unwrap_or_else(|| "DisContactType_Environment".into()) };
        for ev in events {
            let (path, at, dir): (String, Vec3, Vec3) = match ev {
                SimEvent::SwordBlocked { defender, .. } | SimEvent::SwordParried { defender, .. } => {
                    let Some(p) = find(*defender) else { continue };
                    ("Vfx_Weapon.Effects.Ps_SwordSparks_Parry".into(), p.position + Vec3::Z * 40.0 + fwd(p.yaw) * 40.0, fwd(p.yaw))
                }
                SimEvent::Fired { id, bolt, projectile, kind, origin, velocity } => {
                    self.bolts.insert(*projectile, *bolt);
                    self.dirs.insert(*projectile, velocity.normalize_or_zero());
                    // A projectile already gone (it hit within its launch tick): its trail starts
                    // and stops here; live ones get theirs from `attached_want`.
                    if let Some(path) = trail_of(*kind).filter(|_| !s.projectiles.iter().any(|q| q.id == *projectile)) {
                        out.push(FxRequest::EndedTrail { path, xf: facing(*origin, *velocity) });
                    }
                    // A launcher's shot leaves its tripod, not a pawn's hand: no muzzle effect (its
                    // `id` is the sequence's instigator, if any).
                    if kind.is_launcher_shot() {
                        continue;
                    }
                    let Some(p) = find(*id) else { continue };
                    if *bolt {
                        // The crossbow's muzzle (`Twk_Inv_Crossbow`'s fire tweak), in front of the eye.
                        (CROSSBOW_MUZZLE.into(), p.position + Vec3::Z * 60.0 + look(&p) * 50.0, look(&p))
                    } else {
                        // Grenade throws also report `Fired` (a projectile without a bolt): no muzzle;
                        // nor the Tallboy's bow (its own effects are not presented yet).
                        if matches!(p.left.item, LeftHand::Grenade | LeftHand::StickyGrenade) || *kind == dishonored_sim::state::ProjectileKind::TallboyArrow {
                            continue;
                        }
                        let path = if Some(*id) == s.me { PLAYER_MUZZLE } else { PAWN_MUZZLE };
                        // The animated pistol's socket; without one (first person), in front of the eye.
                        let (at, dir) = s.muzzle.get(id).copied().filter(|m| m.0.distance(p.position) < 200.0).unwrap_or((p.position + Vec3::Z * 60.0 + look(&p) * 50.0, look(&p)));
                        self.shots.insert(*projectile, at);
                        (path.into(), at, dir)
                    }
                }
                // Possession: the cast at the caster's hand and the possession burst on the host
                // (`PossesTarget_Rats_01` for a rat).
                SimEvent::PossessStart { id, host } => {
                    let (Some(p), Some(h)) = (find(*id), find(*host)) else { continue };
                    let (at, dir) = (p.position + Vec3::Z * 40.0 + look(&p) * 40.0, look(&p));
                    if Some(*id) == s.me {
                        out.push(FxRequest::HandCast { path: POSSESSION_CAST_PS.into(), at, dir });
                    } else {
                        out_push(&mut out, POSSESSION_CAST_PS, at, dir);
                    }
                    let target = if h.kind == PawnKind::Rat { POSSESSION_RAT_PS } else { POSSESSION_HUMAN_PS };
                    (target.into(), h.position, Vec3::Z)
                }
                SimEvent::PossessEnd { host, .. } => {
                    let Some(h) = find(*host) else { continue };
                    let target = if h.kind == PawnKind::Rat { POSSESSION_RAT_PS } else { POSSESSION_HUMAN_PS };
                    (target.into(), h.position, Vec3::Z)
                }
                // Devouring Swarm: the hand cast and the summoning at the swarm's spot (level 2
                // has its own system).
                SimEvent::SwarmSummoned { id, position, .. } => {
                    if let Some(p) = find(*id) {
                        let (at, dir) = (p.position + Vec3::Z * 40.0 + look(&p) * 40.0, look(&p));
                        if Some(*id) == s.me {
                            out.push(FxRequest::HandCast { path: SWARM_CAST_PS.into(), at, dir });
                        } else {
                            out_push(&mut out, SWARM_CAST_PS, at, dir);
                        }
                    }
                    let lvl = find(*id).map(|p| p.inv.power_levels[dishonored_sim::state::power::DEVOURING_SWARM]).unwrap_or(1);
                    ((if lvl >= 2 { SWARM_SUMMON2_PS } else { SWARM_SUMMON_PS }).into(), *position, Vec3::Z)
                }
                // Grenade / sticky grenade explosion (`DisProjectile_Grenade` explosion template).
                SimEvent::Explosion { position, .. } => (GRENADE_EXPLOSION_PS.into(), *position, Vec3::Z),
                // Spring Razor: the trap's burst when it springs, at the trap.
                SimEvent::TrapPlaced { trap, position, .. } => {
                    self.traps.insert(*trap, *position);
                    continue;
                }
                SimEvent::TrapTriggered { trap } => {
                    let Some(at) = self.traps.remove(trap) else { continue };
                    (SPRING_RAZOR_PS.into(), at, Vec3::Z)
                }
                SimEvent::Damage { victim, cause, .. } => {
                    let Some(p) = find(*victim) else { continue };
                    let striker = match cause {
                        DamageCause::Pistol | DamageCause::PistolHeadshot => "DisContactType_Bullet",
                        DamageCause::Crossbow | DamageCause::CrossbowHeadshot | DamageCause::TrapShot => "DisContactType_Arrow",
                        DamageCause::Sword | DamageCause::SwordCounter | DamageCause::Assassination | DamageCause::DropAssassination => "DisContactType_Sword",
                        _ => continue,
                    };
                    let path = s.contact.and_then(|c| c.particle(striker, "DisContactType_Body")).unwrap_or_else(|| match cause {
                        DamageCause::Pistol | DamageCause::PistolHeadshot => "Vfx_PhysMat.Blood.Pfx_PistolBlood_Blast".into(),
                        _ => "Vfx_PhysMat.Blood.Pfx_BloodSProjectile_Default".into(),
                    });
                    (path, p.position + Vec3::Z * 50.0, -fwd(p.yaw))
                }
                SimEvent::WindblastCast { id } => {
                    let Some(p) = find(*id) else { continue };
                    let lvl = p.inv.power_levels[dishonored_sim::state::power::WINDBLAST];
                    let path = if lvl >= 2 { "Vfx_GamePlay.Windblast.Windblast_lvl2" } else { "Vfx_GamePlay.Windblast.Windblast_lvl1" };
                    (path.into(), p.position + Vec3::Z * 60.0, look(&p))
                }
                SimEvent::BodyRemoved { body, ash: true } => {
                    let pos = find(*body).map(|p| p.position).unwrap_or(Vec3::ZERO);
                    let pts = s.bones.get(body).cloned().filter(|v| !v.is_empty()).unwrap_or_else(|| vec![pos]);
                    for b in pts {
                        self.delayed.push((now + SHADOW_KILL_DELAY, SHADOW_KILL_PS.into(), b, Vec3::Z));
                    }
                    continue;
                }
                SimEvent::ProjectileImpact { projectile, point, victim } if self.shots.contains_key(projectile) => {
                    // `Twk_Proj_Bullet.m_pBulletTracerFX`: a beam (`TypeDataBeam2`, target
                    // `PEB2STM_UserSet`, one 0.1 s loop) from the muzzle to the hit, set by the shot.
                    let from = self.shots.remove(projectile).unwrap_or(*point);
                    out_beam(&mut out, PISTOL_TRACER_PS, from, *point);
                    self.dirs.remove(projectile);
                    if victim.is_some() {
                        self.bolts.remove(projectile);
                        continue;
                    }
                    let sf = surface(*point);
                    self.bolts.remove(projectile);
                    out.extend(contact_decal(s, "DisContactType_Bullet", &sf, *point, *point - from, None));
                    let path = s.contact.and_then(|c| c.particle("DisContactType_Bullet", &sf)).unwrap_or_else(|| "Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt".into());
                    (path, *point, Vec3::Z)
                }
                SimEvent::ProjectileImpact { projectile, point, victim: None } => {
                    let striker = if self.bolts.remove(projectile).unwrap_or(false) { "DisContactType_Arrow" } else { "DisContactType_Bullet" };
                    let sf = surface(*point);
                    let dir = self.dirs.remove(projectile).unwrap_or(Vec3::ZERO);
                    out.extend(contact_decal(s, striker, &sf, *point, dir, None));
                    let path = s.contact.and_then(|c| c.particle(striker, &sf)).unwrap_or_else(|| "Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt".into());
                    (path, *point, Vec3::Z)
                }
                SimEvent::ProjectileImpact { projectile, .. } => {
                    self.bolts.remove(projectile);
                    self.dirs.remove(projectile);
                    continue;
                }
                // A wall of light destroyed the projectile (no impact effects, 0x00C4F3A0): the
                // bullet's tracer still ends where it struck.
                SimEvent::WallOfLightHit { projectile, point, .. } => {
                    if let Some(from) = self.shots.remove(projectile) {
                        out_beam(&mut out, PISTOL_TRACER_PS, from, *point);
                    }
                    self.bolts.remove(projectile);
                    self.dirs.remove(projectile);
                    continue;
                }
                _ => continue,
            };
            out_push(&mut out, &path, at, dir);
        }
        out
    }
}

fn out_push(out: &mut Vec<FxRequest>, path: &str, at: Vec3, dir: Vec3) {
    out.push(FxRequest::Effect { path: path.to_string(), at, dir });
}

fn out_beam(out: &mut Vec<FxRequest>, path: &'static str, from: Vec3, to: Vec3) {
    out.push(FxRequest::Beam { path, from, to });
}

/// Blink's targeting indicator (`Twk_Blink`, EFFECTS.md "Gameplay effects"): while the local
/// player aims Blink (`left.blink_target`), one instance of `m_pGroundPS` stands on the floor
/// under the target, `m_pLowFallPS` floats at a target in the air, `m_pMantlePS` marks a ledge
/// the arrival would mantle; it follows the target every frame and is killed when aiming ends
/// (release, cancel, travel), as its emitters (`bKillOnDeactivate`, endless loops) are
/// deactivated by the power. The (system, point) to show, if any.
pub fn blink_indicator_want(me: Option<&PlayerState>, world: &CollisionWorld, mv: &dishonored_ir::gameplay::MovementConfig) -> Option<(&'static str, Vec3)> {
    let me = me?;
    let t = me.left.blink_target?;
    let hh = mv.collision_half_height;
    // Mantle: the Blink edge finder (`MantleEdgeFinder_Blink`, the search the arrival runs,
    // `movement::try_blink_mantle`) finds a ledge from the target, facing the aim.
    let mut probe = *me;
    probe.position = t;
    probe.velocity = Vec3::ZERO;
    probe.mode = dishonored_sim::state::MoveMode::Falling;
    let ctx = dishonored_sim::movement::MoveCtx { world, cfg: mv, dt: dishonored_sim::movement::DT };
    if dishonored_sim::movement::try_blink_mantle(&mut probe, &ctx, &mut Vec::new()) {
        return Some((BLINK_MANTLE_PS, t - Vec3::Z * hh));
    }
    // The target is raised to CollisionHeight above the floor below it: a floor within that
    // (plus a margin) means the pawn lands standing (Ground); otherwise it falls (Fall).
    let floor = world.raycast(t, -Vec3::Z, hh + 24.0, dishonored_world::channel::PAWN).map(|h| t - Vec3::Z * h.distance);
    Some(match floor {
        Some(f) => (BLINK_GROUND_PS, f),
        None => (BLINK_FALL_PS, t),
    })
}

/// Effects that follow a live object, kept alive while it exists: projectile trails along the
/// flight, and Dark Vision's vision cones on the revealed NPCs while the power is on. Each:
/// ((kind 0 trail / 1 cone, key), system, transform). A trail whose projectile is gone stops
/// spawning and fades out; a cone goes with the power.
pub fn attached_want(me: Option<&PlayerState>, others: &[PlayerState], projectiles: &[ProjectileState], reveals: &[PlayerId]) -> Vec<((u8, u32), &'static str, Mat4)> {
    let mut want: Vec<((u8, u32), &'static str, Mat4)> = Vec::new();
    for p in projectiles {
        // The pistol tracer is a beam from the muzzle to the hit (a `Beam` request on impact).
        let Some(path) = trail_of(p.kind) else { continue };
        want.push(((0, p.id), path, facing(p.position, p.velocity)));
    }
    if me.is_some_and(|me| me.vitals.dark_vision_remaining > 0.0) {
        for o in others.iter().filter(|o| o.alive && reveals.contains(&o.id) && matches!(o.kind, PawnKind::Guard | PawnKind::Civilian)) {
            let dir = Vec3::new(o.yaw.to_radians().cos(), o.yaw.to_radians().sin(), 0.0);
            want.push(((1, o.id), DV_CONE_PS, facing(o.position + Vec3::Z * 60.0, dir)));
        }
    }
    want
}

/// The same sound for the same pawn within this many seconds is one sound: the first- and
/// third-person rigs of a pawn both report a clip's AkEvent notifies (not in step: their clips
/// start on different frames), and a shot is also posted from `SimEvent::Fired`. The pistol
/// played its shot twice when two of them were more than 0.05 s apart (the old window). Shorter
/// than the fastest refire (`Pistol_Fire` exit 0.30 s).
pub const SAME_SOUND_WINDOW: f32 = 0.2;

/// Where a sound is posted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SoundAt {
    /// On a pawn's game object.
    Pawn(PlayerId),
    /// On a temporary object at an impact point.
    Impact(Vec3),
}

/// What the event sounds remember and read.
#[derive(Debug, Clone, Default)]
pub struct SoundEventState {
    /// Projectile id -> fired as a crossbow bolt.
    pub projectiles: HashMap<u32, bool>,
    /// Last post of an event per pawn (`SAME_SOUND_WINDOW`), shared with the animation notifies.
    pub recent: HashMap<(PlayerId, String), f32>,
    /// Pawns whose animation reported sound notifies recently (their swing sounds come from it).
    pub notify_driven: HashMap<PlayerId, f32>,
}

impl SoundEventState {
    /// The AkEvents one simulation event posts (cue names from `dishonored_audio::cues`, contact
    /// sounds from the contact system).
    pub fn posts(&mut self, ev: &SimEvent, players: &[PlayerState], me: Option<PlayerId>, contact: Option<&ContactSystem>, world: &CollisionWorld, now: f32) -> Vec<(String, SoundAt)> {
        let player = |id: PlayerId| players.iter().find(|p| p.id == id);
        let cue = |k: &str| cues::cue_event(k).unwrap_or("").to_string();
        let on = SoundAt::Pawn;
        let mut posts: Vec<(String, SoundAt)> = Vec::new();
        match *ev {
            SimEvent::SwordSwing { id } if !self.notify_driven.contains_key(&id) => posts.push((cue("sword_swing"), on(id))),
            SimEvent::SwordBlocked { defender, .. } => posts.push((cue("sword_clash"), on(defender))),
            SimEvent::SwordParried { defender, .. } => {
                posts.push((cue("sword_clash"), on(defender)));
                posts.push((cue("sword_parry_whoosh"), on(defender)));
            }
            // The Tallboy's bow (`Snd_AI_TBoy_Arrow_Fire*`) is not in the cue set: no pistol shot.
            SimEvent::Fired { kind: dishonored_sim::state::ProjectileKind::TallboyArrow, .. } => {}
            // A launcher's shot (`m_pSoundEvent_InAir`, the launch's own sounds) is not in the cue set.
            SimEvent::Fired { kind, .. } if kind.is_launcher_shot() => {}
            SimEvent::Fired { id, bolt, projectile, .. } => {
                self.projectiles.insert(projectile, bolt);
                // One shot, one sound: skipped when a fire animation's AkEvent notify already
                // posted it for this pawn, and recorded so a later notify of the same shot is
                // skipped.
                let fire = cue(if bolt { "crossbow_fire" } else { "pistol_fire" });
                let key = (id, fire.clone());
                if !self.recent.get(&key).is_some_and(|t| now - *t < SAME_SOUND_WINDOW) {
                    self.recent.insert(key, now);
                    posts.push((fire, on(id)));
                }
            }
            SimEvent::Reloaded { id } => posts.push((cue("pistol_reload"), on(id))),
            SimEvent::BlinkStart { id, .. } => posts.push((cue("blink_start"), on(id))),
            SimEvent::BlinkEnd { id } => posts.push((cue("blink_end"), on(id))),
            SimEvent::BlinkFizzle { id } => posts.push((cue("power_empty"), on(id))),
            SimEvent::WindblastCast { id } => posts.push((cue("windblast"), on(id))),
            SimEvent::DarkVision { id, active } if Some(id) == me => posts.push((cue(if active { "dark_vision_on" } else { "dark_vision_off" }), on(id))),
            SimEvent::Jumped { id } => posts.push((cue("jump"), on(id))),
            // `DisTweaks_Climbable.m_pClimbingSound` of both Boyle chains.
            SimEvent::ClimbSound { id, .. } => posts.push(("Snd_P_Chain_Climbing".to_string(), on(id))),
            SimEvent::Mantle { id, edge_height } => {
                let k = if edge_height < 80.0 { "mantle_low" } else if edge_height < 140.0 { "mantle_medium" } else { "mantle_high" };
                posts.push((cue(k), on(id)));
            }
            SimEvent::Landed { id, speed, .. } if speed > 250.0 => {
                if let Some(p) = player(id) {
                    // JumpLand contact: big above the hard landing speed, stealth when crouched.
                    let striker = if speed > 900.0 { "DisContactType_JumpLand_Big" } else if p.crouched { "DisContactType_JumpLand_Stealth" } else { "DisContactType_JumpLand" };
                    let surface = surface_below(world, p.position, 200.0).map(|s| s.0).unwrap_or_else(|| "DisContactType_Environment".into());
                    match contact.and_then(|c| c.sound(striker, &surface)) {
                        Some(s) => posts.push((s.event, on(id))),
                        None => posts.push((cues::landing_event("Stone", speed > 900.0), on(id))),
                    }
                }
            }
            SimEvent::Damage { victim, cause, amount, .. } => {
                let k = match cause {
                    DamageCause::Sword | DamageCause::SwordCounter => Some("sword_hit_body"),
                    DamageCause::Assassination => Some("assassinate"),
                    DamageCause::DropAssassination => Some("assassinate_drop"),
                    DamageCause::Pistol => Some("bullet_body"),
                    DamageCause::PistolHeadshot => Some("bullet_head"),
                    DamageCause::Crossbow | DamageCause::TrapShot => Some("bolt_body"),
                    DamageCause::CrossbowHeadshot => Some("bolt_head"),
                    _ => None,
                };
                if let Some(k) = k {
                    posts.push((cue(k), on(victim)));
                }
                let human = player(victim).is_none_or(|p| p.kind == PawnKind::Human);
                let vo = if !human { "npc_pain" } else if amount >= 40.0 { "pain_big" } else { "pain_low" };
                posts.push((cue(vo), on(victim)));
            }
            // Shadow Kill: `Twk_Pawn_DefaultNPC.m_pShadowKillSound` on the body.
            SimEvent::BodyRemoved { body, ash: true } => posts.push((cue("shadow_kill"), on(body))),
            SimEvent::Death { victim, .. } => {
                posts.push((cue("death_vo"), on(victim)));
                posts.push((cue("body_fall"), on(victim)));
            }
            SimEvent::ProjectileImpact { projectile, point, victim: None } => {
                let bolt = self.projectiles.remove(&projectile).unwrap_or(false);
                let surface = surface_near(world, point);
                let striker = if bolt { "DisContactType_Arrow" } else { "DisContactType_Bullet" };
                match contact.and_then(|c| c.sound(striker, &surface)) {
                    Some(s) => posts.push((s.event, SoundAt::Impact(point))),
                    None => posts.push((cues::impact_event(bolt, "Stone"), SoundAt::Impact(point))),
                }
            }
            SimEvent::ProjectileImpact { projectile, .. } | SimEvent::WallOfLightHit { projectile, .. } => {
                self.projectiles.remove(&projectile);
            }
            _ => {}
        }
        posts
    }
}

/// An animation reaction a simulation event asks of a pawn (`npc_anim`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HitKind {
    Death,
    DeathFast,
    /// Killed by a weeper's plague cloud (`DisNPCAnim_DeathPlague`).
    DeathPlague,
    /// Killed by rats (`DisNPCAnim_DeathSwarmAttack`).
    DeathSwarm,
    Melee,
    Projectile,
    Headshot,
    Windblast,
    Swing,
    Parried,
    Assassinated,
    /// A drop assassination's victim (`DisNPCAnim_AssassinationDrop`).
    AssassinatedDrop,
    /// A synced short fatality's victim: backhand or forehand, the group (A / B).
    ShortFatality { backhand: bool, group: u8 },
    /// A finisher's victim: the state's slave chain and its group (`SimEvent::Fatality`).
    Fatality { anim: dishonored_ir::npc_anim::NpcAnim, group: u8 },
    /// Knocked out by something other than a choke (sleep dart): falls like a death.
    KnockedOut,
    /// Choke victim: the choke ended (won: knocked out, lost: broke free, cancelled).
    ChokeWin,
    ChokeLose,
    ChokeCancel,
    /// A carried body was let go.
    Dropped,
}

/// Per pawn, the hits, deaths, swings and parries of this frame's simulation events, with the
/// point they come from.
pub fn collect_hits(events: &[SimEvent], all: &[&PlayerState]) -> HashMap<u32, Vec<(HitKind, Vec3)>> {
    use HitKind as H;
    use dishonored_sim::events::{DamageCause as C, SimEvent as E};
    let pos = |id: Option<u32>, fallback: u32| -> Vec3 {
        let f = all.iter().find(|p| p.id == fallback);
        match id.and_then(|i| all.iter().find(|p| p.id == i)) {
            Some(a) => a.position,
            None => f.map(|v| v.position + dishonored_sim::movement::forward_vector(v.yaw)).unwrap_or_default(),
        }
    };
    let mut out: HashMap<u32, Vec<(H, Vec3)>> = HashMap::new();
    for e in events {
        match *e {
            E::Damage { victim, attacker, cause, .. } => {
                // A sword kill the attacker plays as a paired move (`SwordState::assassination_move`
                // 8..11): the victim plays its slave.
                let synced = attacker.and_then(|a| all.iter().find(|p| p.id == a)).filter(|a| a.sword.phase == dishonored_sim::state::SwordPhase::Assassinating && a.sword.assassination_target == Some(victim) && (8..12).contains(&a.sword.assassination_move)).map(|a| a.sword.assassination_move - 8);
                let k = match cause {
                    C::Sword | C::SwordCounter if synced.is_some() => synced.map(|m| H::ShortFatality { backhand: m >= 2, group: m % 2 }),
                    C::Sword | C::SwordCounter => Some(H::Melee),
                    C::Pistol | C::Crossbow => Some(H::Projectile),
                    C::PistolHeadshot | C::CrossbowHeadshot => Some(H::Headshot),
                    C::Windblast => Some(H::Windblast),
                    _ => None,
                };
                if let Some(k) = k {
                    out.entry(victim).or_default().push((k, pos(attacker, victim)));
                }
            }
            // The death chains named after their damage (`DisNPCAnim_DeathPlague`,
            // `DisNPCAnim_DeathSwarmAttack`; LIKELY: the code choosing them was not traced, U-890).
            E::Death { victim, killer, cause } => {
                // An assassination kills outright (`vitals::kill`, no damage event): the victim
                // plays the move's slave, which its death then keeps.
                let slave = match cause {
                    C::Assassination => Some(H::Assassinated),
                    C::DropAssassination => Some(H::AssassinatedDrop),
                    _ => None,
                };
                if let Some(s) = slave {
                    out.entry(victim).or_default().push((s, pos(killer, victim)));
                }
                let k = match cause {
                    C::Plague => H::DeathPlague,
                    C::Rats | C::RatBite => H::DeathSwarm,
                    _ => H::Death,
                };
                out.entry(victim).or_default().push((k, pos(killer, victim)))
            }
            // A synced finisher: the victim plays the state's slave (the generic swing has none).
            E::Fatality { id, victim, variant, slave, .. } => {
                if let Some(anim) = dishonored_ir::npc_anim::NpcAnim::fatality_slave(slave) {
                    out.entry(victim).or_default().push((H::Fatality { anim, group: variant }, pos(Some(id), victim)));
                }
            }
            E::SwordSwing { id } => out.entry(id).or_default().push((H::Swing, Vec3::ZERO)),
            E::SwordParried { attacker, defender } => out.entry(defender).or_default().push((H::Parried, pos(Some(attacker), defender))),
            E::KnockedOut { victim, by, cause } if cause != dishonored_sim::events::KnockoutCause::Choke => out.entry(victim).or_default().push((H::KnockedOut, pos(by, victim))),
            E::ChokeEnd { attacker, victim, outcome } => {
                use dishonored_sim::events::ChokeOutcome as O;
                let k = match outcome {
                    O::Win => H::ChokeWin,
                    O::Lose => H::ChokeLose,
                    _ => H::ChokeCancel,
                };
                out.entry(victim).or_default().push((k, pos(Some(attacker), victim)));
            }
            E::BodyDropped { carrier, body } => out.entry(body).or_default().push((H::Dropped, pos(Some(carrier), body))),
            _ => {}
        }
    }
    out
}

