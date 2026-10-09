//! The NPC head: the look-at component's cursor (`ArkCpntType_Lookat`, tick 0x00955B40) and
//! the vision eye built from it (`ArkComponentVisionNPC` eye update 0x00C676A0). See
//! stealth.md section 13.
//!
//! * Look-at request: the attention process asks the look-at component to follow its focus
//!   when the behaviour level is HeadTrack and the focus is an enemy (0x00B25890: head weight
//!   1, torso weight `m_fHeadTrackTorsoRatio`, the move-speed flag; speed 0.5 from the
//!   request builder 0x00954B30). Any other level drops it (0x00CA9FE0).
//! * Cursor: a `DynInterpFVector2D` (yaw, pitch; radians) toward the target angles of the
//!   look-at mode (0x00953050), integrated by 0x0094E240; the head and torso weights are two
//!   1-D `DynInterp`s (0x0094DFF0). Constants from the constructor 0x00955D60.
//! * Node (`ArkAnimNodeLookAt`): the aim is split between the torso and head grids
//!   (0x009176F0); the blended aim (+0x1C0) is what the vision reads.
//! * Eye: the `camera_jnt` position (baked per archetype, `AiConfig::head_poses`). Axis: with
//!   a request, the pawn's rotation turned by the blended aim (0x0094D760); without one, the
//!   pawn's rotation; within `m_fVision_BoneNullzone_Deg` of the pawn's rotation, the pawn's
//!   rotation. Pullback and angle leeway are 0 in every shipped vision tweak.

use dishonored_ir::ai::{AiConfig, HeadLoop, HeadPose};
use dishonored_ir::npc_anim::LookatMode;
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::f32::consts::{FRAC_PI_2, PI};

/// The constants of a `DynInterp` (constructor 0x00955D60).
#[derive(Debug, Clone, Copy)]
pub struct InterpConsts {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub max_speed: [f32; 2],
    /// `m_fMaxStrength`, `m_fFriction`, `m_fDampingDistance`.
    pub strength: f32,
    pub friction: f32,
    pub damping: f32,
}

/// The look-at cursor: yaw within +-pi, pitch within +-pi/2, max speed 100, strength 100,
/// friction 7, damping distance pi/2.
pub const CURSOR: InterpConsts = InterpConsts { min: [-PI, -FRAC_PI_2], max: [PI, FRAC_PI_2], max_speed: [100.0, 100.0], strength: 100.0, friction: 7.0, damping: FRAC_PI_2 };
/// The head / torso weights (`m_fUseHeadRange` / `m_fUseTorsoRange` sources): 0..1, max speed
/// 50, strength 50, friction 3, damping distance 0.5.
pub const WEIGHT: InterpConsts = InterpConsts { min: [0.0, 0.0], max: [1.0, 1.0], max_speed: [50.0, 50.0], strength: 50.0, friction: 3.0, damping: 0.5 };

/// The acceleration scale of a speed factor: `pow(sf + 0.5, 4)` (x87 `_CIpow` with the base in
/// ST(1)); 1 at the requests' 0.5.
pub fn speed_scale(sf: f32) -> f32 {
    (sf + 0.5).powf(4.0)
}

/// `DynInterpFVector2D::Update` (0x0094E240).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Interp2 {
    pub cur: [f32; 2],
    pub speed: [f32; 2],
    pub target: [f32; 2],
}

impl Interp2 {
    pub fn update(&mut self, dt: f32, sf: f32, c: &InterpConsts) {
        let t = self.target;
        let d = Vec2::new(t[0] - self.cur[0], t[1] - self.cur[1]);
        let dist = d.length();
        let spd = Vec2::from(self.speed).length();
        if dist < 1e-4 && spd < 1e-4 {
            self.cur = t;
            self.speed = [0.0; 2];
            return;
        }
        let dir = if dist > 1e-8 { d / dist } else { Vec2::X };
        // Friction, stronger inside the damping distance.
        let mut f = Vec2::new(-self.speed[0], -self.speed[1]) * dt * c.friction;
        if dist < c.damping {
            f *= (c.damping - dist) * 0.5 + 1.0;
        }
        self.speed[0] += f.x;
        self.speed[1] += f.y;
        // Pull: full strength, scaled down near the target from rest, or capped so the speed
        // reached would not pass the remaining distance.
        let mut accel = c.strength;
        if spd <= 1e-4 {
            if dist < c.damping {
                accel = ((dist / c.damping) * 0.9 + 0.1) * accel;
            }
        } else {
            let a2 = (accel / spd) * dist;
            if a2 < accel {
                accel = a2;
            }
        }
        let p = speed_scale(sf) * dt;
        self.speed[0] += p * dir.x * accel;
        self.speed[1] += p * dir.y * accel;
        for k in 0..2 {
            self.speed[k] = self.speed[k].clamp(-c.max_speed[k], c.max_speed[k]);
        }
        for k in 0..2 {
            let old = self.cur[k];
            let new = old + self.speed[k] * dt;
            self.cur[k] = new;
            // Passing (or reaching) the target snaps to it.
            if (old - t[k]) * (new - t[k]) <= 1e-4 || ((new - t[k]).abs() < 1e-4 && new.abs() < 1e-4) {
                self.speed[k] = 0.0;
                self.cur[k] = t[k];
            }
        }
        for k in 0..2 {
            self.cur[k] = if self.cur[k] < c.min[k] { c.min[k] } else if c.max[k] <= self.cur[k] { c.max[k] } else { self.cur[k] };
        }
    }
}

/// `DynInterp` (float) `Update` (0x0094DFF0), with an optional target override.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Interp1 {
    pub cur: f32,
    pub speed: f32,
    pub target: f32,
}

impl Interp1 {
    pub fn update(&mut self, dt: f32, sf: f32, target: Option<f32>, c: &InterpConsts) {
        let t = target.unwrap_or(self.target);
        let d = t - self.cur;
        let dist = d.abs();
        let spd = self.speed.abs();
        if dist < 1e-4 && spd < 1e-4 {
            self.cur = t;
            self.speed = 0.0;
            return;
        }
        let dir = if dist > 1e-8 { d / dist } else { 1.0 };
        let mut f = -(c.friction * self.speed * dt);
        if dist < c.damping {
            f *= (c.damping - dist) * 0.5 + 1.0;
        }
        let v0 = self.speed + f;
        let mut accel = c.strength;
        if spd <= 1e-4 {
            if dist < c.damping {
                accel = ((dist / c.damping) * 0.9 + 0.1) * accel;
            }
        } else {
            let a2 = (accel / spd) * dist;
            if a2 < accel {
                accel = a2;
            }
        }
        let mut v = (speed_scale(sf) * accel * dir * dt + v0).clamp(-c.max_speed[0], c.max_speed[0]);
        // Moving away from the target stops.
        if v * d < 0.0 {
            v = 0.0;
        }
        self.speed = v;
        let old = self.cur;
        let new = old + v * dt;
        self.cur = new;
        if (old - t) * (new - t) <= 1e-4 || ((new - t).abs() < 1e-4 && new.abs() < 1e-4) {
            self.speed = 0.0;
            self.cur = t;
        }
        self.cur = if self.cur < c.min[0] { c.min[0] } else if c.max[0] <= self.cur { c.max[0] } else { self.cur };
    }
}

/// A look-at request (`FUN_009539F0` entry): what to follow and with which weights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookRequest {
    /// The followed actor's eyes view point (0x00952BD0 with no offset: its controller's
    /// `GetActorEyesViewPoint`).
    pub target: Vec3,
    pub head_weight: f32,
    pub torso_weight: f32,
    /// Reduce the torso weight with the pawn's speed (`m_fTorsoInfluenceMaxMoveSpeed`).
    pub torso_speed_reduction: bool,
    /// The request's speed (the cursor's and the weights' `m_fSpeedFactor`).
    pub speed: f32,
}

/// The vision eye: position and the eye-space axes (x forward, y right, z up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eye {
    pub pos: Vec3,
    pub fwd: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

impl Eye {
    /// An eye looking along `fwd` without roll.
    pub fn looking(pos: Vec3, fwd: Vec3) -> Eye {
        let fwd = fwd.try_normalize().unwrap_or(Vec3::X);
        let right = Vec3::Z.cross(fwd).try_normalize().unwrap_or(Vec3::Y);
        let up = fwd.cross(right);
        Eye { pos, fwd, right, up }
    }

    /// Eye-space offset of a world point.
    pub fn local(&self, p: Vec3) -> Vec3 {
        let d = p - self.pos;
        Vec3::new(d.dot(self.fwd), d.dot(self.right), d.dot(self.up))
    }
}

/// Unit vector of a UE3 rotator (yaw and pitch in radians).
fn rot_vector(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin())
}

/// The boundary of an aim polygon (`FUN_0090C4C0` over the 9 grid points of
/// `FUN_0090A140`): the rectangle of the mode's ranges `[right, left, up, down]` (degrees,
/// left and down negative), hit along `dir`. Radians.
pub fn reach(range: [f32; 4], dir: Vec2) -> Vec2 {
    if dir == Vec2::ZERO {
        return Vec2::ZERO;
    }
    let (r, l, u, d) = (range[0].to_radians(), range[1].to_radians(), range[2].to_radians(), range[3].to_radians());
    // The ring 1..8 around the centre: (R,0) (R,U) (0,U) (L,U) (L,0) (L,D) (0,D) (R,D).
    let ring = [Vec2::new(r, 0.0), Vec2::new(r, u), Vec2::new(0.0, u), Vec2::new(l, u), Vec2::new(l, 0.0), Vec2::new(l, d), Vec2::new(0.0, d), Vec2::new(r, d)];
    for i in 0..8 {
        let (a, b) = (ring[i], ring[(i + 1) % 8]);
        if 0.0 < dir.y * a.x - a.y * dir.x && b.x * dir.y - b.y * dir.x <= 0.0 {
            let den = (a.y - b.y) * dir.x - (a.x - b.x) * dir.y;
            let mut t = b.y * a.x - a.y * b.x;
            if den != 0.0 {
                t /= den;
            }
            return Vec2::new(-(dir.x * t), -(dir.y * t));
        }
    }
    Vec2::ZERO
}

/// `ArkAnimNodeLookAt` 0x009176F0: the aim (radians) split between the torso and the head
/// grids by their reaches; returns the blended aim (+0x1C0).
pub fn blended_aim(mode: &LookatMode, aim: Vec2, use_head: f32, use_torso: f32, dead_zone: f32) -> Vec2 {
    let (head, torso) = split_aim(mode, aim, use_head, use_torso, dead_zone);
    head + torso
}

/// The node's split of the aim (radians) into the head grid's and the torso grid's parts
/// (0x009176F0); their sum is [`blended_aim`]. The animation poses each grid by its part.
pub fn split_aim(mode: &LookatMode, aim: Vec2, use_head: f32, use_torso: f32, dead_zone: f32) -> (Vec2, Vec2) {
    let hr = reach(mode.head_range, aim) * use_head;
    let tr = reach(mode.torso_range, aim) * use_torso;
    let (lh, lt) = (hr.length(), tr.length());
    let inv = if lt + lh <= 1e-4 { 0.0 } else { 1.0 / (lt + lh) };
    let start = mode.start_torso * (inv * lh);
    let f_torso = inv * lt;
    let end = (1.0 - f_torso) * mode.end_head + f_torso;
    let r = aim.length() * inv;
    let mut torso = Vec2::ZERO;
    if start <= r {
        if end <= r || r <= start {
            torso = aim - hr;
            if lt < torso.length() {
                torso = tr;
            }
        } else {
            let mut s = (r - start) / (end - start);
            if s < 0.5 {
                s = (3.0 - s * 2.0) * s * s;
            }
            torso = ((tr + hr) * end - hr) * s;
        }
    }
    torso = torso * dead_zone + tr * ((1.0 - dead_zone) * r);
    let mut head = aim - torso;
    if lh + 1e-4 < head.length() {
        head = hr;
        torso = aim - hr;
        if lt < torso.length() {
            torso = tr;
        }
    }
    (head, torso)
}

/// The NPC's head state (look-at component plus node).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Head {
    /// The cursor (+0x9C) and the head / torso weights (+0xDC / +0x104).
    pub cursor: Interp2,
    pub use_head: Interp1,
    pub use_torso: Interp1,
    /// The current request's speed (+0xD8; 0.5 without one, 0x0094E8A0).
    pub speed: f32,
    pub torso_speed_reduction: bool,
    pub requested: bool,
    /// The turn-around delay (+0x250), the last raw target yaw (+0x23C) and the last target
    /// offset from the cursor (+0x240).
    pub delay: f32,
    pub last_yaw: f32,
    pub last_delta: [f32; 2],
    /// The node's blended aim (radians, +0x1C0).
    pub aim: [f32; 2],
    /// Loop time of the baked head pose.
    pub loop_time: f32,
    /// The eye of the last step (world): position, forward, right and up.
    pub eye: Option<([f32; 3], [f32; 3], [f32; 3], [f32; 3])>,
    /// The ambient idle playing (`DisAISubProcessAmbientAnims`).
    #[serde(default)]
    pub ambient: Option<Ambient>,
    /// `m_fAmbientAnimTimer` (+0x68): counts down while the NPC is idle and still; the next
    /// ambient idle starts when it passes below zero. A new NPC starts with 0.
    #[serde(default)]
    pub amb_timer: f32,
    /// The CRT `rand()` state of the variant pick (0: not drawn yet, the stream starts at the
    /// seed 1 of an unseeded CRT).
    #[serde(default)]
    pub amb_rng: u32,
}

/// The ambient idle sequence playing and how long it has played. While it plays the look-at is
/// disabled (`DisNotify_NPCDisableLookAt`, whole length): the vision eye is the animated bone as is
/// (0x00C676A0), and look-at requests are ignored.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Ambient {
    pub seq: String,
    pub age: f32,
}

/// The ambient node's weight ramps 0 -> 1 in 0.2 s (measured on `stealth/detect_600`: +1/12 per
/// tick from the tick it starts), blending the idle loop's pose into the ambient sequence's.
pub const AMBIENT_BLEND: f32 = 0.2;

/// What the brain tells the head about the ambient idle this tick.
#[derive(Debug, Clone, Copy, Default)]
pub struct AmbientIn<'a> {
    /// The NPC is on routine, not moving: `DisAISubProcessAmbientAnims` runs its timer.
    pub eligible: bool,
    /// The scenario's pinned sequence (`Empty_Idle_03`), else the pick is random.
    pub pin: Option<&'a str>,
}

/// The MSVC CRT `rand()` (LCG 214013 / 2531011, bits 16..30).
fn crt_rand(state: &mut u32) -> u32 {
    if *state == 0 {
        *state = 1;
    }
    *state = state.wrapping_mul(214013).wrapping_add(2531011);
    (*state >> 16) & 0x7fff
}

/// The baked loop of a sequence name for an archetype.
pub fn find_loop<'a>(poses: &'a [HeadPose], archetype: u8, seq: &str) -> Option<&'a HeadLoop> {
    let pose = poses.get(archetype as usize).filter(|p| !p.loops.is_empty()).or_else(|| poses.iter().find(|p| !p.loops.is_empty()))?;
    pose.loops.iter().find(|l| l.sequence.eq_ignore_ascii_case(seq))
}

/// The baked `camera_jnt` (pawn frame) for an archetype, stance modifier and speed index.
pub fn bone(poses: &[HeadPose], archetype: u8, modifier: &str, speed: u8, t: f32) -> Option<[f32; 6]> {
    let pose = poses.get(archetype as usize).filter(|p| !p.loops.is_empty()).or_else(|| poses.iter().find(|p| !p.loops.is_empty()))?;
    pose.find(modifier, speed)?.at(t)
}

/// The look-at mode of a stance and gait (`{Empty|Sword|Search}{Idle|Walk|Run}`, as the
/// runtime's look-at blender picks it).
pub fn mode<'a>(ai: &'a AiConfig, modifier: &str, speed: f32) -> Option<&'a LookatMode> {
    let stance = if modifier.starts_with("Search") { "Search" } else if modifier.starts_with("Sword") || modifier == "Aim" { "Sword" } else { "Empty" };
    let gait = if speed > 250.0 { "Run" } else if speed > 20.0 { "Walk" } else { "Idle" };
    let name = format!("{stance}{gait}");
    ai.lookat.modes.iter().find(|m| m.name.eq_ignore_ascii_case(&name)).or_else(|| ai.lookat.modes.first())
}

impl Head {
    /// The pawn-frame bone, moved to the world by the pawn's position and yaw.
    fn bone_world(pos: Vec3, yaw_deg: f32, b: [f32; 6]) -> (Vec3, Vec3) {
        let (s, c) = yaw_deg.to_radians().sin_cos();
        let rot = |v: Vec3| Vec3::new(v.x * c - v.y * s, v.x * s + v.y * c, v.z);
        (pos + rot(Vec3::new(b[0], b[1], b[2])), rot(Vec3::new(b[3], b[4], b[5])))
    }

    /// One tick of the look-at component (0x00953FB0 then 0x00953050) and the node's blend,
    /// for a pawn at `pos` facing `yaw_deg` and moving at `speed`; then the vision eye.
    #[allow(clippy::too_many_arguments)]
    pub fn step(&mut self, ai: &AiConfig, archetype: u8, modifier: &str, pos: Vec3, yaw_deg: f32, speed: f32, request: Option<LookRequest>, amb: AmbientIn, dt: f32) -> Eye {
        self.loop_time += dt;
        let gait = if speed > 20.0 { 1 } else { 0 };
        // The ambient idle (DisAISubProcessAmbientAnims tick 0x00B39620).
        self.ambient_tick(ai, archetype, modifier, amb, dt);
        let idle = ai.head_poses.get(archetype as usize).filter(|p| !p.loops.is_empty()).or_else(|| ai.head_poses.iter().find(|p| !p.loops.is_empty())).and_then(|p| p.find(modifier, gait));
        let b = idle.and_then(|l| l.at(self.loop_time));
        // The bone's axes in the pawn frame (forward, right, up): the idle loop's, blended into
        // the ambient sequence's while one plays.
        let mut axes: Option<[Vec3; 3]> = idle.and_then(|l| Some([Vec3::from_slice(&b?[3..6]), Vec3::from_slice(&l.axes_at(self.loop_time)?[0..3]), Vec3::from_slice(&l.axes_at(self.loop_time)?[3..6])]));
        let mut local = b.map(|b| Vec3::from_slice(&b[0..3]));
        if let Some(a) = &self.ambient {
            if let Some(l) = find_loop(&ai.head_poses, archetype, &a.seq) {
                let age = a.age.max(0.0);
                if let (Some(ab), Some(ax)) = (l.at(age), l.axes_at(age)) {
                    let w = (age / AMBIENT_BLEND).clamp(0.0, 1.0);
                    let (af, ar, au) = (Vec3::from_slice(&ab[3..6]), Vec3::from_slice(&ax[0..3]), Vec3::from_slice(&ax[3..6]));
                    local = Some(match local {
                        Some(p) => p.lerp(Vec3::from_slice(&ab[0..3]), w),
                        None => Vec3::from_slice(&ab[0..3]),
                    });
                    axes = Some(match axes {
                        Some([f, r, u]) => [f.lerp(af, w).normalize_or_zero(), r.lerp(ar, w).normalize_or_zero(), u.lerp(au, w).normalize_or_zero()],
                        None => [af, ar, au],
                    });
                }
            }
        }
        let (s, c) = yaw_deg.to_radians().sin_cos();
        let rot = |v: Vec3| Vec3::new(v.x * c - v.y * s, v.x * s + v.y * c, v.z);
        let origin = local.map(|p| pos + rot(p)).unwrap_or(pos + Vec3::Z * 70.0);
        let bone_axes = if self.ambient.is_some() { axes.map(|[f, r, u]| [rot(f), rot(r), rot(u)]) } else { None };
        // The request (0x0094E5E0 / 0x0094E690 copy its speed and weights; without one,
        // 0x0094E8A0 sets the speeds to 0.5 and keeps the weights).
        match request {
            Some(r) => {
                self.speed = r.speed;
                self.use_head.target = r.head_weight;
                self.use_torso.target = r.torso_weight;
                self.torso_speed_reduction = r.torso_speed_reduction;
            }
            None => self.speed = 0.5,
        }
        self.requested = request.is_some();
        // The target direction (+0x14C): the followed point from the bone, else the pawn's
        // forward; then the yaw / pitch against the pawn's axes.
        let fwd = rot_vector(yaw_deg.to_radians(), 0.0);
        let dir = request.map(|r| r.target - origin).and_then(|d| d.try_normalize()).unwrap_or(fwd);
        let right = Vec3::Z.cross(fwd);
        let yaw = dir.dot(right).atan2(dir.dot(fwd));
        let pitch = FRAC_PI_2 - dir.dot(Vec3::Z).clamp(-1.0, 1.0).acos();
        // 0x00953050: the weights. The torso weight's target falls with the pawn's speed for a
        // flagged request.
        let sf = self.speed;
        self.use_head.update(dt, sf, None, &WEIGHT);
        let mut factor = 1.0;
        if self.torso_speed_reduction {
            let max = ai.lookat.torso_influence_max_move_speed;
            factor = if speed < max { 1.0 - speed / max } else { 0.0 };
        }
        let tt = self.use_torso.target * factor;
        self.use_torso.update(dt, sf, Some(tt), &WEIGHT);
        let (uh, ut) = (self.use_head.cur.clamp(0.0, 1.0), self.use_torso.cur.clamp(0.0, 1.0));
        let m = mode(ai, modifier, speed);
        if uh > 0.0 || ut > 0.0 {
            if let Some(m) = m {
                let delta = [yaw - self.cursor.cur[0], pitch - self.cursor.cur[1]];
                let prev = Vec2::from(self.last_delta) - Vec2::from(delta);
                if prev.length() <= 4.712389 {
                    if PI <= Vec2::from(delta).length() {
                        self.delay = (self.delay - dt).max(0.0);
                    } else {
                        self.delay = 0.0;
                    }
                } else {
                    self.delay = (1.0 - sf).max(0.0);
                }
                self.last_delta = delta;
                if self.delay <= 1e-4 {
                    let infl = (self.use_head.cur + self.use_torso.cur).clamp(0.0, 1.0);
                    let mut t = Vec2::new(infl * m.head_horizontal_pct * yaw * 0.01, infl * m.head_vertical_pct * pitch * 0.01);
                    if let Some(c) = m.clamp {
                        t.x = t.x.min(c[0].to_radians()).max(c[1].to_radians());
                        t.y = t.y.min(c[2].to_radians()).max(c[3].to_radians());
                    }
                    // The node's reach along the target (0x0090C5B0).
                    let r = reach(m.head_range, t) * uh + reach(m.torso_range, t) * ut;
                    if r.x.abs() < t.x.abs() || r.y.abs() < t.y.abs() {
                        t = r;
                    }
                    // A target swinging across the back keeps its side.
                    let keep = self.last_yaw * yaw < 0.0 && self.last_yaw.abs() > 2.8274333 && yaw.abs() > 2.8274333;
                    if !keep {
                        self.last_yaw = yaw;
                        self.cursor.target[0] = t.x;
                    }
                    self.cursor.target[1] = t.y;
                }
                self.cursor.update(dt, sf, &CURSOR);
                let aim = Vec2::new(self.cursor.cur[0].clamp(-PI, PI), self.cursor.cur[1].clamp(-FRAC_PI_2, FRAC_PI_2));
                // The torso dead zone weight (+0x254) rests at 1.
                self.aim = blended_aim(m, aim, uh, ut, 1.0).into();
            }
        } else {
            self.aim = [0.0; 2];
        }
        let eye = self.vision(ai, origin, yaw_deg, bone_axes);
        self.eye = Some((eye.pos.into(), eye.fwd.into(), eye.right.into(), eye.up.into()));
        eye
    }

    /// `DisAISubProcessAmbientAnims` tick (0x00B39620): while the NPC is idle and still the
    /// timer counts down; below zero a variant of the group is picked (`rand() * n`, 0x00401940
    /// at 0x00B39728), played, and the timer rolled again in `[min, max]` of the tweak. A
    /// sequence that ends clears the ambient state (the look-at is enabled again). Not
    /// eligible (moving, alerted, off routine): the animation is dropped.
    fn ambient_tick(&mut self, ai: &AiConfig, archetype: u8, modifier: &str, amb: AmbientIn, dt: f32) {
        if !amb.eligible {
            self.ambient = None;
            return;
        }
        if let Some(a) = &mut self.ambient {
            a.age += dt;
            let dur = find_loop(&ai.head_poses, archetype, &a.seq).map(|l| l.duration).unwrap_or(0.0);
            if a.age >= dur {
                self.ambient = None;
            }
        }
        self.amb_timer -= dt;
        if self.amb_timer < 0.0 && self.ambient.is_none() {
            let prefix = if modifier.starts_with("Sword") { "Sword_Patrol_Idle_" } else { "Empty_Idle_" };
            let seq = match amb.pin {
                Some(p) => p.to_string(),
                None => {
                    // The variants numbered from 01 while the sequence exists.
                    let n = (1..).take_while(|k| find_loop(&ai.head_poses, archetype, &format!("{prefix}{k:02}")).is_some()).count() as u32;
                    if n == 0 {
                        return;
                    }
                    let i = (crt_rand(&mut self.amb_rng) as f32 * n as f32 * 3.0517578e-5) as u32;
                    format!("{prefix}{:02}", i.min(n - 1) + 1)
                }
            };
            if find_loop(&ai.head_poses, archetype, &seq).is_some() {
                self.ambient = Some(Ambient { seq, age: 0.0 });
                let [lo, hi] = ai.ambient_anim_freq;
                self.amb_timer = crt_rand(&mut self.amb_rng) as f32 * (hi - lo) * 3.051851e-5 + lo;
            }
        }
    }

    /// The node's head and torso parts of the current aim (radians; [`split_aim`] of the cursor
    /// with the head / torso weights, as [`Head::step`] blends them), for the animation's
    /// look-at grids. Zero when both weights are zero.
    pub fn parts(&self, ai: &AiConfig, modifier: &str, speed: f32) -> (Vec2, Vec2) {
        let (uh, ut) = (self.use_head.cur.clamp(0.0, 1.0), self.use_torso.cur.clamp(0.0, 1.0));
        if uh <= 0.0 && ut <= 0.0 {
            return (Vec2::ZERO, Vec2::ZERO);
        }
        let Some(m) = mode(ai, modifier, speed) else { return (Vec2::ZERO, Vec2::ZERO) };
        let aim = Vec2::new(self.cursor.cur[0].clamp(-PI, PI), self.cursor.cur[1].clamp(-FRAC_PI_2, FRAC_PI_2));
        split_aim(m, aim, uh, ut, 1.0)
    }

    /// The vision eye (0x00C676A0): the bone position; with a request the pawn's rotation
    /// turned by the blended aim, else the pawn's rotation; the null zone snaps an axis within
    /// `m_fVision_BoneNullzone_Deg` of the pawn's rotation to it.
    pub fn vision(&self, ai: &AiConfig, origin: Vec3, yaw_deg: f32, bone: Option<[Vec3; 3]>) -> Eye {
        let pawn = rot_vector(yaw_deg.to_radians(), 0.0);
        // Look-at disabled (an ambient idle plays): the bone matrix as is, requests ignored.
        let eye = match bone {
            Some([fwd, right, up]) => Eye { pos: origin, fwd, right, up },
            None => Eye::looking(origin, if self.requested { rot_vector(yaw_deg.to_radians() + self.aim[0], self.aim[1]) } else { pawn }),
        };
        if eye.fwd.dot(pawn).clamp(-1.0, 1.0).acos() <= ai.vision.bone_nullzone_deg.to_radians() {
            return Eye::looking(origin, pawn);
        }
        eye
    }

    /// The eye of the last step, or one from the pawn when the head never stepped.
    pub fn last_eye(&self, ai: &AiConfig, archetype: u8, modifier: &str, pos: Vec3, yaw_deg: f32) -> Eye {
        match self.eye {
            Some((p, f, r, u)) => Eye { pos: p.into(), fwd: f.into(), right: r.into(), up: u.into() },
            None => {
                let origin = bone(&ai.head_poses, archetype, modifier, 0, 0.0).map(|b| Self::bone_world(pos, yaw_deg, b).0).unwrap_or(pos + Vec3::Z * 70.0);
                Eye::looking(origin, rot_vector(yaw_deg.to_radians(), 0.0))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_settles_on_its_target() {
        let mut c = Interp2 { target: [0.8, -0.2], ..Default::default() };
        for _ in 0..120 {
            c.update(1.0 / 60.0, 0.5, &CURSOR);
        }
        assert!((c.cur[0] - 0.8).abs() < 1e-3 && (c.cur[1] + 0.2).abs() < 1e-3, "{:?}", c.cur);
    }

    /// A HeadTrack request 30 deg to the right turns the eye (90% of it: `EmptyIdle`'s
    /// horizontal percentage) within a second; without a request the axis is the facing.
    #[test]
    fn head_track_turns_the_eye() {
        let mut ai = AiConfig::default();
        ai.lookat.modes.push(LookatMode { name: "EmptyIdle".into(), head_range: [65.0, -65.0, 60.0, -35.0], torso_range: [60.0, -60.0, 35.0, -45.0], head_horizontal_pct: 90.0, head_vertical_pct: 100.0, end_head: 1.0, ..Default::default() });
        let mut h = Head::default();
        let target = Vec3::new(1000.0 * 30f32.to_radians().cos(), 1000.0 * 30f32.to_radians().sin(), 70.0);
        let req = LookRequest { target, head_weight: 1.0, torso_weight: 0.0, torso_speed_reduction: true, speed: 0.5 };
        let mut eye = h.step(&ai, 0, "Empty", Vec3::ZERO, 0.0, 0.0, Some(req), AmbientIn::default(), 1.0 / 30.0);
        for _ in 0..60 {
            eye = h.step(&ai, 0, "Empty", Vec3::ZERO, 0.0, 0.0, Some(req), AmbientIn::default(), 1.0 / 30.0);
        }
        let yaw = eye.fwd.y.atan2(eye.fwd.x).to_degrees();
        assert!((yaw - 27.0).abs() < 1.0, "{yaw}");
        let eye = h.step(&ai, 0, "Empty", Vec3::ZERO, 0.0, 0.0, None, AmbientIn::default(), 1.0 / 30.0);
        assert!(eye.fwd.y.abs() < 1e-5);
    }

    /// The animation's head / torso parts are the blended aim split: they sum to it, and with
    /// head weight only the torso part is zero.
    #[test]
    fn parts_sum_to_the_blended_aim() {
        let mut ai = AiConfig::default();
        ai.lookat.modes.push(LookatMode { name: "EmptyIdle".into(), head_range: [65.0, -65.0, 60.0, -35.0], torso_range: [60.0, -60.0, 35.0, -45.0], head_horizontal_pct: 90.0, head_vertical_pct: 100.0, end_head: 1.0, ..Default::default() });
        let mut h = Head::default();
        let target = Vec3::new(1000.0 * 40f32.to_radians().cos(), 1000.0 * 40f32.to_radians().sin(), 200.0);
        let req = LookRequest { target, head_weight: 1.0, torso_weight: 0.5, torso_speed_reduction: true, speed: 0.5 };
        for _ in 0..60 {
            h.step(&ai, 0, "Empty", Vec3::ZERO, 0.0, 0.0, Some(req), AmbientIn::default(), 1.0 / 30.0);
        }
        let (head, torso) = h.parts(&ai, "Empty", 0.0);
        let sum = head + torso;
        assert!((sum.x - h.aim[0]).abs() < 1e-5 && (sum.y - h.aim[1]).abs() < 1e-5, "{head:?} {torso:?} {:?}", h.aim);
        assert!(head.x > 0.0 && torso.x >= 0.0, "{head:?} {torso:?}");
        let (_, torso) = split_aim(&ai.lookat.modes[0], Vec2::new(0.5, 0.1), 1.0, 0.0, 1.0);
        assert!(torso.length() < 1e-6, "{torso:?}");
    }

    #[test]
    fn reach_hits_the_range_rectangle() {
        let r = reach([60.0, -60.0, 30.0, -40.0], Vec2::new(1.0, 0.1));
        assert!((r.x - 60f32.to_radians()).abs() < 1e-4 && (r.y - 6f32.to_radians()).abs() < 1e-4, "{r:?}");
        let r = reach([60.0, -60.0, 30.0, -40.0], Vec2::new(0.0, -1.0));
        assert!(r.x.abs() < 1e-4 && (r.y + 40f32.to_radians()).abs() < 1e-4, "{r:?}");
    }

    fn ambient_ai() -> AiConfig {
        let mut ai = AiConfig::default();
        // An idle loop and an ambient loop that pitches the head down 30 deg over one second.
        let idle = HeadLoop { modifier: "Empty".into(), speed: 0, sequence: "Empty_Idle".into(), duration: 1.0, sample_rate: 30.0, samples: vec![[0.0, 0.0, 80.0, 1.0, 0.0, 0.0]; 2], axes: vec![[0.0, 1.0, 0.0, 0.0, 0.0, 1.0]; 2] };
        let (s, c) = 30f32.to_radians().sin_cos();
        let amb = HeadLoop { modifier: "Ambient".into(), speed: 2, sequence: "Empty_Idle_01".into(), duration: 1.0, sample_rate: 30.0, samples: vec![[0.0, 0.0, 80.0, 1.0, 0.0, 0.0], [0.0, 0.0, 80.0, c, 0.0, -s]], axes: vec![[0.0, 1.0, 0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, s, 0.0, c]] };
        ai.head_poses = vec![HeadPose { archetype: "a".into(), mesh: String::new(), loops: vec![idle, amb] }];
        ai.ambient_anim_freq = [30.0, 60.0];
        ai
    }

    /// `DisAISubProcessAmbientAnims`: a new head starts the variant at its first idle tick, the
    /// look-at is disabled while it plays (the bone axis, requests ignored), and the next one is a
    /// tweak period away. A pinned sequence is played instead of the random pick.
    #[test]
    fn ambient_idle_drives_the_eye_while_it_plays() {
        let ai = ambient_ai();
        let mut h = Head::default();
        let amb = AmbientIn { eligible: true, pin: None };
        let dt = 1.0 / 60.0;
        let e0 = h.step(&ai, 0, "Empty", Vec3::ZERO, 0.0, 0.0, None, amb, dt);
        assert_eq!(h.ambient.as_ref().map(|a| a.seq.as_str()), Some("Empty_Idle_01"));
        assert!(e0.fwd.z.abs() < 1e-3, "{:?}", e0.fwd);
        // 0.7 s in: the weight is 1 and the head is 21 deg down on the bone's axis.
        let mut e = e0;
        for _ in 0..42 {
            e = h.step(&ai, 0, "Empty", Vec3::ZERO, 0.0, 0.0, None, amb, dt);
        }
        assert!(e.fwd.z < -0.3 && e.up.x > 0.2, "{e:?}");
        // The sequence ends: the look-at is enabled again, the axis the pawn's rotation.
        for _ in 0..30 {
            e = h.step(&ai, 0, "Empty", Vec3::ZERO, 0.0, 0.0, None, amb, dt);
        }
        assert!(h.ambient.is_none() && e.fwd.z.abs() < 1e-3 && h.amb_timer > 25.0, "{:?} {}", h.ambient, h.amb_timer);
        // Not eligible (moving or alerted): nothing plays and the timer waits.
        let mut still = Head::default();
        still.step(&ai, 0, "Empty", Vec3::ZERO, 0.0, 0.0, None, AmbientIn { eligible: false, pin: None }, dt);
        assert!(still.ambient.is_none() && still.amb_timer == 0.0);
        // A pin to a missing sequence plays nothing.
        let mut pinned = Head::default();
        pinned.step(&ai, 0, "Empty", Vec3::ZERO, 0.0, 0.0, None, AmbientIn { eligible: true, pin: Some("Empty_Idle_09") }, dt);
        assert!(pinned.ambient.is_none());
    }

    #[test]
    fn eye_local_matches_yaw_only_frame() {
        let e = Eye::looking(Vec3::ZERO, rot_vector(90f32.to_radians(), 0.0));
        let l = e.local(Vec3::new(0.0, 100.0, 10.0));
        assert!((l.x - 100.0).abs() < 1e-3 && l.y.abs() < 1e-3 && (l.z - 10.0).abs() < 1e-3, "{l:?}");
    }
}
